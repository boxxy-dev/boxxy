use crate::state::{AppInput, AppWindowInner};
use libadwaita as adw;
use libadwaita::prelude::*;
use parking_lot::Mutex;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Process-wide guard ensuring at most one `AppWindow` displays a persistent
/// Boxxy News toast at any time across multi-window sessions.
static PRESENTED_TOAST_ARTICLE_ID: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastExitReason {
    /// Default disposition when the user clicks the `×` close button on `AdwToast`.
    UserDismissed,
    /// User clicked the `"Read"` action button on the toast.
    ReadClicked,
    /// Window closed, setting toggled off, post withdrawn, or article opened from the dialog.
    ProgrammaticTeardown,
}

#[derive(Clone, Debug)]
pub struct ActiveBroadcastToast {
    pub article_id: String,
    pub toast: adw::Toast,
    pub exit_reason: Rc<Cell<ToastExitReason>>,
}

fn release_presented_slot(article_id: &str) {
    let mut slot = PRESENTED_TOAST_ARTICLE_ID.lock();
    if slot.as_deref() == Some(article_id) {
        *slot = None;
    }
}

/// Dismisses any currently visible Boxxy News toast on `inner` without marking it `'dismissed'`
/// in SQLite (leaving `'pending'` intact for future launches unless explicitly marked `'read'`).
pub fn teardown_active_toast_programmatically(inner: &mut AppWindowInner) {
    if let Some(active) = inner.active_broadcast_toast.take() {
        active.exit_reason.set(ToastExitReason::ProgrammaticTeardown);
        release_presented_slot(&active.article_id);
        active.toast.dismiss();
    }
}

/// Dismisses the active toast programmatically only if its `article_id` matches `target_id`.
pub fn teardown_toast_if_matching(inner: &mut AppWindowInner, target_id: &str) {
    let matches = inner
        .active_broadcast_toast
        .as_ref()
        .is_some_and(|t| t.article_id == target_id);
    if matches {
        teardown_active_toast_programmatically(inner);
    }
}

/// Evaluates whether this window should present (or clear) a persistent `'pending'` Boxxy News toast.
pub fn check_pending_broadcast(inner_ref: &Rc<RefCell<AppWindowInner>>) {
    let current_toast_id = {
        let Ok(mut inner) = inner_ref.try_borrow_mut() else {
            return;
        };
        if inner.force_close.get() {
            return;
        }
        if !boxxy_broadcast::is_auto_broadcast_enabled() {
            teardown_active_toast_programmatically(&mut inner);
            return;
        }
        inner
            .active_broadcast_toast
            .as_ref()
            .map(|t| t.article_id.clone())
    };

    let inner_weak = Rc::downgrade(inner_ref);
    gtk4::glib::spawn_future_local(async move {
        let pending_opt = boxxy_sys_utils::runtime()
            .spawn(async move { boxxy_broadcast::get_next_pending_article().await.ok().flatten() })
            .await
            .ok()
            .flatten();

        let Some(inner_rc) = inner_weak.upgrade() else {
            return;
        };
        let Ok(mut inner) = inner_rc.try_borrow_mut() else {
            return;
        };

        if inner.force_close.get() || !boxxy_broadcast::is_auto_broadcast_enabled() {
            teardown_active_toast_programmatically(&mut inner);
            return;
        }

        // If this window already displays a toast, verify it is still `'pending'`
        // (e.g. tear down if withdrawn remotely or read in another window).
        if let Some(existing_id) = current_toast_id {
            let still_same_pending = pending_opt
                .as_ref()
                .is_some_and(|art| art.id == existing_id);
            if !still_same_pending {
                teardown_toast_if_matching(&mut inner, &existing_id);
            } else {
                return;
            }
        }

        if inner.active_broadcast_toast.is_some() {
            return;
        }

        let Some(article) = pending_opt else {
            return;
        };

        // Defer toast presentation while any modal dialog (What's New, Welcome, Preferences,
        // Boxxy News) is open; `connect_visible_dialog_notify` re-triggers this check on close.
        if inner.window.visible_dialog().is_some() {
            return;
        }

        // Multi-window deduplication: if multiple windows exist and another window is active,
        // let the active window claim the toast; otherwise claim if unclaimed.
        if !inner.window.is_active()
            && let Some(app) = inner.window.application()
            && app.windows().iter().any(|w| w != &inner.window && w.is_active())
        {
            return;
        }

        {
            let mut slot = PRESENTED_TOAST_ARTICLE_ID.lock();
            if slot.is_some() {
                return;
            }
            *slot = Some(article.id.clone());
        }

        let toast = adw::Toast::new(&format!("Boxxy News: {}", article.title));
        toast.set_use_markup(false);
        toast.set_timeout(0);
        toast.set_button_label(Some("Read"));

        let exit_reason = Rc::new(Cell::new(ToastExitReason::UserDismissed));

        {
            let exit_click = exit_reason.clone();
            let tx_open = inner.tx.clone();
            let id_open = article.id.clone();
            toast.connect_button_clicked(move |_| {
                exit_click.set(ToastExitReason::ReadClicked);
                let _ = tx_open.send_blocking(AppInput::OpenBroadcastArticle(id_open.clone()));
            });
        }

        {
            let exit_dismiss = exit_reason.clone();
            let force_close = inner.force_close.clone();
            let inner_for_dismiss = Rc::downgrade(&inner_rc);
            let tx_next = inner.tx.clone();
            let id_dismiss = article.id.clone();
            toast.connect_dismissed(move |_| {
                release_presented_slot(&id_dismiss);

                if let Some(rc) = inner_for_dismiss.upgrade()
                    && let Ok(mut guard) = rc.try_borrow_mut()
                    && guard
                        .active_broadcast_toast
                        .as_ref()
                        .is_some_and(|t| t.article_id == id_dismiss)
                {
                    guard.active_broadcast_toast = None;
                }

                // Only record `'dismissed'` when the user explicitly clicked `×` on the toast.
                // Window closes (`force_close`) and programmatic teardowns leave `'pending'` untouched.
                if exit_dismiss.get() == ToastExitReason::UserDismissed && !force_close.get() {
                    let id_bg = id_dismiss.clone();
                    let tx_bg = tx_next.clone();
                    gtk4::glib::spawn_future_local(async move {
                        let _ = boxxy_sys_utils::runtime()
                            .spawn(async move { boxxy_broadcast::mark_article_dismissed(&id_bg).await })
                            .await;
                        let _ = tx_bg.send(AppInput::CheckPendingBroadcast).await;
                    });
                }
            });
        }

        inner.active_broadcast_toast = Some(ActiveBroadcastToast {
            article_id: article.id,
            toast: toast.clone(),
            exit_reason,
        });
        inner.toast_overlay.add_toast(toast);
    });
}
