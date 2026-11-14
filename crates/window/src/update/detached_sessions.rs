use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::state::{AppInput, AppWindowInner};

pub fn reattach_session(inner: &mut AppWindowInner, pid: u32, cwd: Option<String>) {
    let _ = super::tabs::new_tab_reattach(inner, pid, cwd);
    let tx = inner.tx.clone();
    gtk4::glib::spawn_future_local(async move {
        let _ = tx.send(AppInput::RefreshDetachedSessions).await;
        let _ = tx.send(AppInput::FocusActiveTerminal).await;
    });
}

pub fn reattach_batch(
    inner: &mut AppWindowInner,
    sessions: Vec<boxxy_terminal::DetachedSessionInfo>,
) {
    super::tabs::reattach_sessions_grouped(inner, sessions);
    let tx = inner.tx.clone();
    gtk4::glib::spawn_future_local(async move {
        let _ = tx.send(AppInput::RefreshDetachedSessions).await;
        let _ = tx.send(AppInput::FocusActiveTerminal).await;
    });
}

pub fn refresh_detached_sessions(inner: &AppWindowInner) {
    let tx = inner.tx.clone();
    gtk4::glib::spawn_future_local(async move {
        let agent = boxxy_terminal::get_agent().await;
        if let Ok(sessions) = agent.list_detached_sessions_detailed().await {
            let _ = tx
                .send(AppInput::UpdateDetachedSessionsBadge(sessions))
                .await;
        }
    });
}

pub fn update_badge(inner: &AppWindowInner, sessions: &[boxxy_terminal::DetachedSessionInfo]) {
    if sessions.is_empty() {
        inner.detached_btn.set_visible(false);
        inner.detached_btn.remove_css_class("warning");
        return;
    }

    inner
        .detached_count_lbl
        .set_label(&sessions.len().to_string());
    inner.detached_btn.set_visible(true);
    if sessions.iter().any(|s| s.waiting_for_input) {
        inner.detached_btn.add_css_class("warning");
    } else {
        inner.detached_btn.remove_css_class("warning");
    }
}

pub fn show_detached_sessions_dialog(inner: &AppWindowInner) {
    let window = inner.window.clone();
    let toast_overlay = inner.toast_overlay.clone();
    let tx = inner.tx.clone();
    gtk4::glib::spawn_future_local(async move {
        let agent = boxxy_terminal::get_agent().await;
        let sessions = match agent.list_detached_sessions_detailed().await {
            Ok(list) => list,
            Err(e) => {
                log::warn!("Failed to list detached sessions: {e}");
                Vec::new()
            }
        };

        let _ = tx
            .send(AppInput::UpdateDetachedSessionsBadge(sessions.clone()))
            .await;

        if sessions.is_empty() {
            let toast = adw::Toast::new("No detached background shells.");
            toast.set_timeout(3);
            toast_overlay.add_toast(toast);
            return;
        }

        let dialog = adw::AlertDialog::builder()
            .heading("Background Shells")
            .body("Detached terminal sessions kept alive by the Boxxy daemon.")
            .build();

        let remaining_rows = std::rc::Rc::new(std::cell::Cell::new(sessions.len()));
        let group = adw::PreferencesGroup::new();
        for s in &sessions {
            let proc_name = if s.foreground_process.is_empty() {
                "Background Process".to_string()
            } else {
                s.foreground_process.clone()
            };
            let title = if s.waiting_for_input {
                format!("{proc_name} — Waiting for input")
            } else {
                proc_name
            };
            let cwd_display = if s.cwd.is_empty() {
                "~".to_string()
            } else {
                s.cwd.clone()
            };
            let idle_display = if s.idle_secs < 60 {
                format!("{}s idle", s.idle_secs)
            } else if s.idle_secs < 3600 {
                format!("{}m idle", s.idle_secs / 60)
            } else {
                format!("{}h idle", s.idle_secs / 3600)
            };

            let mut subtitle_str = format!("{cwd_display} • {idle_display}");
            if !s.character_name.is_empty()
                && !title
                    .to_lowercase()
                    .starts_with(&s.character_name.to_lowercase())
            {
                subtitle_str.push_str(&format!(" • {}", s.character_name));
            }
            if s.pending_task_count > 0 {
                subtitle_str.push_str(&format!(
                    " • {} reminder{}",
                    s.pending_task_count,
                    if s.pending_task_count == 1 { "" } else { "s" }
                ));
            }

            let row = adw::ActionRow::builder()
                .title(&title)
                .subtitle(&subtitle_str)
                .use_markup(false)
                .build();

            let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            btn_box.set_valign(gtk4::Align::Center);

            let reattach_btn = gtk4::Button::builder()
                .label("Reattach")
                .css_classes(["suggested-action", "flat"])
                .build();
            let kill_btn = gtk4::Button::builder()
                .label("Terminate")
                .css_classes(["destructive-action", "flat"])
                .build();

            let pid = s.pid;
            let cwd_opt = (!s.cwd.is_empty()).then(|| s.cwd.clone());
            let tx_reattach = tx.clone();
            let row_weak_reattach = row.downgrade();
            let dialog_weak_reattach = dialog.downgrade();
            let rem_reattach = remaining_rows.clone();
            reattach_btn.connect_clicked(move |btn| {
                btn.set_sensitive(false);
                if let Some(r) = row_weak_reattach.upgrade() {
                    r.set_visible(false);
                }
                let left = rem_reattach.get().saturating_sub(1);
                rem_reattach.set(left);
                if left == 0
                    && let Some(dlg) = dialog_weak_reattach.upgrade()
                {
                    dlg.close();
                }
                let _ = tx_reattach
                    .send_blocking(AppInput::ReattachDetachedSession(pid, cwd_opt.clone()));
            });

            let tx_kill = tx.clone();
            let row_weak_kill = row.downgrade();
            let dialog_weak_kill = dialog.downgrade();
            let rem_kill = remaining_rows.clone();
            kill_btn.connect_clicked(move |btn| {
                btn.set_sensitive(false);
                if let Some(r) = row_weak_kill.upgrade() {
                    r.set_visible(false);
                }
                let left = rem_kill.get().saturating_sub(1);
                rem_kill.set(left);
                if left == 0
                    && let Some(dlg) = dialog_weak_kill.upgrade()
                {
                    dlg.close();
                }
                let tx_after_kill = tx_kill.clone();
                gtk4::glib::spawn_future_local(async move {
                    let agent = boxxy_terminal::get_agent().await;
                    let _ = agent.terminate_detached_session(pid).await;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let _ = tx_after_kill.send(AppInput::RefreshDetachedSessions).await;
                });
            });

            btn_box.append(&reattach_btn);
            btn_box.append(&kill_btn);
            row.add_suffix(&btn_box);
            group.add(&row);
        }

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .max_content_height(280)
            .propagate_natural_height(true)
            .child(&group)
            .build();

        let extra_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
        extra_vbox.set_width_request(440);
        extra_vbox.append(&scrolled);

        let actions_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        actions_row.set_homogeneous(true);

        let close_btn = gtk4::Button::builder()
            .label("Close")
            .height_request(38)
            .build();
        let terminate_all_btn = gtk4::Button::builder()
            .label("Terminate All")
            .css_classes(["destructive-action"])
            .height_request(38)
            .build();
        let restore_all_btn = gtk4::Button::builder()
            .label("Restore All")
            .css_classes(["suggested-action"])
            .height_request(38)
            .build();

        let chosen = std::rc::Rc::new(std::cell::Cell::new("close"));
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            close_btn.connect_clicked(move |_| {
                c.set("close");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            terminate_all_btn.connect_clicked(move |_| {
                c.set("terminate-all");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            restore_all_btn.connect_clicked(move |_| {
                c.set("restore-all");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }

        actions_row.append(&close_btn);
        actions_row.append(&terminate_all_btn);
        actions_row.append(&restore_all_btn);
        extra_vbox.append(&actions_row);

        dialog.set_extra_child(Some(&extra_vbox));

        let _ = dialog.choose_future(Some(&window)).await;
        match chosen.get() {
            "restore-all" => {
                if let Ok(remaining) = agent.list_detached_sessions_detailed().await
                    && !remaining.is_empty()
                {
                    let _ = tx.send(AppInput::ReattachDetachedBatch(remaining)).await;
                }
            }
            "terminate-all" => {
                let _ = agent.terminate_detached_session(0).await;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            _ => {}
        }
        let _ = tx.send(AppInput::RefreshDetachedSessions).await;
        let _ = tx.send(AppInput::FocusActiveTerminal).await;
    });
}
