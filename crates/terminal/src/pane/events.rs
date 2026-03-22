use super::PaneInner;
use crate::PaneOutput;
use boxxy_vte::terminal::TerminalWidget;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) fn wire_terminal_events(
    terminal: &TerminalWidget,
    inner: &Rc<RefCell<PaneInner>>,
    progress_bar: &gtk::ProgressBar,
    is_claw_active: &Rc<Cell<bool>>,
    claw_sender: &async_channel::Sender<boxxy_claw::engine::ClawMessage>,
    callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    id: String,
) {
    let pb_clone = progress_bar.clone();
    let inner_for_pb = inner.clone();
    terminal.on_progress_changed(move |state, progress| {
        let enable_pb = inner_for_pb
            .borrow()
            .current_settings
            .as_ref()
            .map(|s| s.enable_progress_bar)
            .unwrap_or(true);

        if !enable_pb {
            pb_clone.set_visible(false);
            return;
        }

        // States:
        // 0 — hide/clear the progress bar
        // 1 — normal progress (green bar)
        // 2 — error state (red)
        // 3 — indeterminate/pulsing (spinner-like)
        // 4 — warning state (yellow)

        match state {
            0 => {
                pb_clone.set_visible(false);
                pb_clone.remove_css_class("error");
                pb_clone.remove_css_class("warning");
            }
            1 | 2 | 4 => {
                pb_clone.set_visible(true);
                let frac = (progress as f64).clamp(0.0, 100.0) / 100.0;
                pb_clone.set_fraction(frac);

                pb_clone.remove_css_class("error");
                pb_clone.remove_css_class("warning");
                if state == 2 {
                    pb_clone.add_css_class("error");
                } else if state == 4 {
                    pb_clone.add_css_class("warning");
                }
            }
            3 => {
                pb_clone.set_visible(true);
                pb_clone.remove_css_class("error");
                pb_clone.remove_css_class("warning");
                pb_clone.pulse();
            }
            _ => {}
        }
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    terminal.on_title_changed(move |title| {
        cb_clone(PaneOutput::TitleChanged(id_clone.clone(), title));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    terminal.on_bell(move || {
        cb_clone(PaneOutput::BellRung(id_clone.clone()));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    terminal.on_exit(move |code| {
        cb_clone(PaneOutput::Exited(id_clone.clone(), code));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    let inner_for_cwd = inner.clone();
    terminal.on_cwd_changed(move |dir| {
        inner_for_cwd.borrow_mut().working_dir = Some(dir.clone());
        cb_clone(PaneOutput::DirectoryChanged(id_clone.clone(), dir));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    terminal.on_osc_133_a(move || {
        cb_clone(PaneOutput::Osc133A(id_clone.clone()));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    terminal.on_osc_133_b(move || {
        cb_clone(PaneOutput::Osc133B(id_clone.clone()));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    terminal.on_osc_133_c(move || {
        cb_clone(PaneOutput::Osc133C(id_clone.clone()));
    });

    let tx_osc_clone = claw_sender.clone();
    let inner_for_osc = inner.clone();
    let active_clone_for_osc = is_claw_active.clone();
    terminal.on_osc_133_d(move |exit_code| {
        if !active_clone_for_osc.get() {
            return;
        }
        if let Some(code) = exit_code {
            let tx = tx_osc_clone.clone();
            let pane = inner_for_osc.borrow().terminal.clone();
            let cwd = inner_for_osc
                .borrow()
                .working_dir
                .clone()
                .unwrap_or_default();
            gtk::glib::spawn_future_local(async move {
                if let Some(snapshot) = pane.get_text_snapshot(100, 0).await {
                    let _ = tx
                        .send(boxxy_claw::engine::ClawMessage::CommandFinished {
                            exit_code: code,
                            snapshot,
                            cwd,
                        })
                        .await;
                }
            });
        }
    });

    let tx_query_clone = claw_sender.clone();
    let inner_for_query = inner.clone();
    let active_clone_for_query = is_claw_active.clone();
    terminal.on_claw_query(move |query| {
        if !active_clone_for_query.get() {
            return;
        }
        let tx = tx_query_clone.clone();
        let pane = inner_for_query.borrow().terminal.clone();
        let cwd = inner_for_query
            .borrow()
            .working_dir
            .clone()
            .unwrap_or_default();
        gtk::glib::spawn_future_local(async move {
            if let Some(snapshot) = pane.get_text_snapshot(100, 0).await {
                let _ = tx
                    .send(boxxy_claw::engine::ClawMessage::ClawQuery {
                        query,
                        snapshot,
                        cwd,
                    })
                    .await;
            }
        });
    });

    // Auto-cancel pending proposals when the user types in the terminal
    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tx_cancel_typing = claw_sender.clone();
    key_controller.connect_key_pressed(move |_, keyval, _, state| {
        // Only trigger cancel on non-modifier keys or Enter/Backspace
        let is_modifier = state.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            || state.contains(gtk::gdk::ModifierType::ALT_MASK);
        if !is_modifier {
            let key_lower = keyval.to_lower();
            let is_printable =
                key_lower >= gtk::gdk::Key::space && key_lower <= gtk::gdk::Key::asciitilde;
            if is_printable || keyval == gtk::gdk::Key::Return || keyval == gtk::gdk::Key::BackSpace
            {
                let tx = tx_cancel_typing.clone();
                gtk::glib::spawn_future_local(async move {
                    let _ = tx
                        .send(boxxy_claw::engine::ClawMessage::CancelPending)
                        .await;
                });
            }
        }
        gtk::glib::Propagation::Proceed
    });

    use gtk4::prelude::Cast;
    use gtk4::prelude::EventControllerExt;
    use gtk4::prelude::WidgetExt;

    terminal
        .upcast_ref::<gtk::Widget>()
        .add_controller(key_controller);
}
