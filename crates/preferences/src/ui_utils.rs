#![allow(clippy::redundant_pub_crate)]

use crate::config::Settings;
use adw::prelude::*;
use boxxy_themes::ColorDot;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

pub(super) type PageFilter = Box<dyn Fn(&str) -> bool>;
pub(super) type PageSync = Box<dyn Fn(&Settings)>;
pub(super) type PageHooks = (PageSync, PageFilter);
pub(super) type ColorUpdater = Rc<dyn Fn(&str)>;
pub(super) type VoidCallbackSlot = Rc<RefCell<Option<Rc<dyn Fn()>>>>;
type PendingAction = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

/// Displays an in-dialog toast by walking up to the nearest `AdwToastOverlay`.
pub(super) fn show_toast(widget: &impl IsA<gtk::Widget>, message: &str) {
    if let Some(overlay) = widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    {
        let toast = adw::Toast::builder()
            .title(message)
            .use_markup(false)
            .timeout(3)
            .build();
        overlay.add_toast(toast);
    }
}

/// Ensures `dir` exists off the UI thread and opens it in the user's file manager
/// via `GtkFileLauncher` (portal-safe on Flatpak & Wayland).
pub(super) fn open_directory(
    widget: &impl IsA<gtk::Widget>,
    dir: PathBuf,
    toast_message: Option<&'static str>,
) {
    let widget_weak = widget.downgrade();
    glib::spawn_future_local(async move {
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            log::warn!("Failed to create directory '{}': {e}", dir.display());
        }
        let Some(w) = widget_weak.upgrade() else {
            return;
        };
        let root_win = w.root().and_downcast::<gtk::Window>();
        let file = gtk::gio::File::for_path(&dir);
        let file_fallback = file.clone();
        let launcher = gtk::FileLauncher::new(Some(&file));
        launcher.launch(
            root_win.as_ref(),
            gtk::gio::Cancellable::NONE,
            move |res| {
                if let Err(err) = res
                    && !matches!(
                        err.kind::<gtk::DialogError>(),
                        Some(gtk::DialogError::Cancelled | gtk::DialogError::Dismissed)
                    )
                {
                    let uri = file_fallback.uri();
                    let _ = gtk::gio::AppInfo::launch_default_for_uri(
                        uri.as_str(),
                        None::<&gtk::gio::AppLaunchContext>,
                    );
                }
            },
        );
        if let Some(msg) = toast_message {
            show_toast(&w, msg);
        }
    });
}

/// Opens an external HTTP/HTTPS URL via `GtkUriLauncher` (portal-safe on Flatpak & Wayland).
pub(super) fn open_uri(widget: &impl IsA<gtk::Widget>, uri: &str) {
    let root_win = widget.root().and_downcast::<gtk::Window>();
    let uri_owned = uri.to_string();
    let launcher = gtk::UriLauncher::new(uri);
    launcher.launch(
        root_win.as_ref(),
        gtk::gio::Cancellable::NONE,
        move |res| {
            if let Err(err) = res
                && !matches!(
                    err.kind::<gtk::DialogError>(),
                    Some(gtk::DialogError::Cancelled | gtk::DialogError::Dismissed)
                )
            {
                let _ = gtk::gio::AppInfo::launch_default_for_uri(
                    &uri_owned,
                    None::<&gtk::gio::AppLaunchContext>,
                );
            }
        },
    );
}

/// Coalesces rapid UI input changes (text typing or slider dragging) and guarantees
/// any pending commit is flushed immediately on `Enter`, focus-out, or widget unmap.
#[derive(Clone, Default)]
pub(super) struct Debouncer {
    source: Rc<Cell<Option<glib::SourceId>>>,
    pending: PendingAction,
}

impl Debouncer {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn schedule(&self, delay: Duration, action: impl FnOnce() + 'static) {
        if let Some(old) = self.source.take() {
            old.remove();
        }
        *self.pending.borrow_mut() = Some(Box::new(action));
        let source_cell = self.source.clone();
        let pending_cell = self.pending.clone();
        let id = glib::timeout_add_local_once(delay, move || {
            source_cell.set(None);
            let task = pending_cell.borrow_mut().take();
            if let Some(run) = task {
                run();
            }
        });
        self.source.set(Some(id));
    }

    pub(super) fn flush(&self) {
        if let Some(old) = self.source.take() {
            old.remove();
        }
        let task = self.pending.borrow_mut().take();
        if let Some(run) = task {
            run();
        }
    }

    pub(super) fn cancel(&self) {
        if let Some(old) = self.source.take() {
            old.remove();
        }
        let _ = self.pending.borrow_mut().take();
    }

    /// Flushes any pending debounced edit when `widget` loses focus or unmaps.
    pub(super) fn bind_flush_lifecycle(&self, widget: &impl IsA<gtk::Widget>) {
        let flush_unmap = self.clone();
        widget.connect_unmap(move |_| {
            flush_unmap.flush();
        });

        let focus_ctrl = gtk::EventControllerFocus::new();
        let flush_leave = self.clone();
        focus_ctrl.connect_leave(move |_| {
            flush_leave.flush();
        });
        widget.add_controller(focus_ctrl);
    }
}

/// Runs `on_map` immediately when `widget` maps and starts a single-instance repeating
/// `interval` timer that automatically cancels itself the moment `widget` unmaps.
pub(super) fn bind_mapped_poller<W, FMap, FTick>(
    widget: &W,
    interval: Duration,
    on_map: FMap,
    on_tick: FTick,
) where
    W: IsA<gtk::Widget>,
    FMap: Fn() + 'static,
    FTick: Fn() + 'static,
{
    let active_source: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    let on_tick = Rc::new(on_tick);

    let active_for_map = active_source.clone();
    widget.connect_map(move |w| {
        on_map();
        if let Some(prev) = active_for_map.take() {
            prev.remove();
        }
        let w_weak = w.downgrade();
        let active_tick = active_for_map.clone();
        let tick_fn = on_tick.clone();
        let id = glib::timeout_add_local(interval, move || {
            let Some(live) = w_weak.upgrade() else {
                active_tick.set(None);
                return glib::ControlFlow::Break;
            };
            if !live.is_mapped() {
                active_tick.set(None);
                return glib::ControlFlow::Break;
            }
            tick_fn();
            glib::ControlFlow::Continue
        });
        active_for_map.set(Some(id));
    });

    widget.connect_unmap(move |_| {
        if let Some(prev) = active_source.take() {
            prev.remove();
        }
    });
}

/// Pre-indexed preference row for allocation-free real-time search filtering.
pub(super) struct SearchRow {
    widget: gtk::Widget,
    haystack: String,
}

impl SearchRow {
    pub(super) fn new(widget: &impl IsA<gtk::Widget>, extra_keywords: &str) -> Self {
        let w: gtk::Widget = widget.clone().upcast();
        let mut combined = String::new();
        if let Some(pref_row) = w.downcast_ref::<adw::PreferencesRow>() {
            combined.push_str(&pref_row.title());
            combined.push(' ');
        }
        if let Some(action_row) = w.downcast_ref::<adw::ActionRow>()
            && let Some(sub) = action_row.subtitle()
        {
            combined.push_str(&sub);
            combined.push(' ');
        }
        combined.push_str(extra_keywords);
        Self {
            widget: w,
            haystack: combined.to_lowercase(),
        }
    }

    pub(super) fn apply(&self, query: &str) -> bool {
        let matched = query.is_empty() || self.haystack.contains(query);
        self.widget.set_visible(matched);
        matched
    }
}

/// Builds a circular clickable color swatch backed by GPU-snapshotted `boxxy_themes::ColorDot`
/// (requiring zero global `GtkCssProvider` registrations on `gdk::Display`).
pub(super) fn make_editable_color_dot<F>(
    color_hex: &str,
    tooltip: &str,
    on_color_chosen: F,
) -> (gtk::Button, ColorUpdater)
where
    F: Fn(String) + 'static,
{
    let dot_btn = gtk::Button::new();
    dot_btn.set_size_request(22, 22);
    dot_btn.set_valign(gtk::Align::Center);
    dot_btn.set_halign(gtk::Align::Center);
    dot_btn.set_focus_on_click(false);
    dot_btn.set_cursor_from_name(Some("pointer"));
    dot_btn.set_tooltip_text(Some(tooltip));
    dot_btn.add_css_class("flat");
    dot_btn.add_css_class("circular");
    dot_btn.add_css_class("color-dot-btn");

    let color_dot = ColorDot::new(color_hex, 18);
    dot_btn.set_child(Some(&color_dot));

    let current_hex = Rc::new(RefCell::new(normalize_to_hex(color_hex)));
    let apply_color: ColorUpdater = {
        let current_hex = current_hex.clone();
        Rc::new(move |raw_hex: &str| {
            let safe = normalize_to_hex(raw_hex);
            (*current_hex.borrow_mut()).clone_from(&safe);
            color_dot.set_hex(&safe);
        })
    };

    let on_color_chosen = Rc::new(on_color_chosen);
    let apply_for_click = apply_color.clone();
    dot_btn.connect_clicked(move |btn| {
        let dialog = gtk::ColorDialog::builder().with_alpha(false).build();
        let initial_rgba = gdk::RGBA::parse(&*current_hex.borrow()).ok();
        let window = btn.root().and_downcast::<gtk::Window>();
        let apply_cb = apply_for_click.clone();
        let chosen_cb = on_color_chosen.clone();
        dialog.choose_rgba(
            window.as_ref(),
            initial_rgba.as_ref(),
            gtk::gio::Cancellable::NONE,
            move |res| {
                if let Ok(rgba) = res {
                    let hex = rgba_to_hex(&rgba);
                    apply_cb(&hex);
                    chosen_cb(hex);
                }
            },
        );
    });

    (dot_btn, apply_color)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rgba_to_hex(rgba: &gdk::RGBA) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgba.red().clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.green().clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.blue().clamp(0.0, 1.0) * 255.0).round() as u8
    )
}

fn normalize_to_hex(raw: &str) -> String {
    gdk::RGBA::parse(raw).map_or_else(|_| "#808080".to_string(), |rgba| rgba_to_hex(&rgba))
}
