use super::PaneInner;
use crate::PaneOutput;
use boxxy_claw_protocol::ClawMessage;
use boxxy_vte::terminal::TerminalWidget;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) fn wire_terminal_events(
    terminal: &TerminalWidget,
    inner: &Rc<RefCell<PaneInner>>,
    progress_bar: &gtk::ProgressBar,
    is_claw_active: &Rc<Cell<bool>>,
    claw_sender: &async_channel::Sender<ClawMessage>,
    callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    id: String,
) {
    let pb_clone = progress_bar.clone();
    let inner_for_pb = Rc::downgrade(inner);
    terminal.on_progress_changed(move |state, progress| {
        let Some(inner_rc) = inner_for_pb.upgrade() else {
            return;
        };
        let enable_pb = inner_rc
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
    let inner_for_title = Rc::downgrade(inner);
    terminal.on_title_changed(move |title| {
        if let Some(inner_rc) = inner_for_title.upgrade() {
            inner_rc.borrow_mut().last_title = Some(title.clone());
        }
        cb_clone(PaneOutput::TitleChanged(id_clone.clone(), title));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    terminal.on_bell(move || {
        cb_clone(PaneOutput::BellRung(id_clone.clone()));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    let inner_for_exit = Rc::downgrade(inner);
    let active_for_exit = is_claw_active.clone();
    terminal.on_exit(move |code| {
        active_for_exit.set(false);
        if let Some(inner_rc) = inner_for_exit.upgrade() {
            let session_opt = inner_rc.borrow_mut().mark_pty_exited();
            let Some(session_arc) = session_opt else {
                return;
            };
            let pane_id = id_clone.clone();
            gtk::glib::spawn_future_local(async move {
                let sid = session_arc.lock().await.take();
                let agent = crate::get_agent().await;
                if sid.is_some() {
                    let _ = agent.release_holder(pane_id).await;
                } else {
                    let _ = agent.unregister_workspace_pane(pane_id).await;
                }
            });
        }
        cb_clone(PaneOutput::Exited(id_clone.clone(), code));
    });

    let cb_clone = callback.clone();
    let id_clone = id.clone();
    let inner_for_cwd = Rc::downgrade(inner);
    terminal.on_cwd_changed(move |dir| {
        if let Some(inner_rc) = inner_for_cwd.upgrade() {
            let mut inner_mut = inner_rc.borrow_mut();
            inner_mut.working_dir = Some(dir.clone());
            inner_mut.msg_bar.set_working_dir(Some(dir.clone()));
        }
        let pane_id = id_clone.clone();
        let cwd_for_agent = dir.clone();
        gtk::glib::spawn_future_local(async move {
            let agent = crate::get_agent().await;
            let _ = agent
                .update_workspace_pane(
                    pane_id,
                    cwd_for_agent,
                    String::new(),
                    String::new(),
                    String::new(),
                )
                .await;
        });
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
    let inner_for_osc = Rc::downgrade(inner);
    let active_clone_for_osc = is_claw_active.clone();
    let id_for_osc = id.clone();
    terminal.on_osc_133_d(move |exit_code| {
        if let Some(code) = exit_code {
            let Some(inner_rc) = inner_for_osc.upgrade() else {
                return;
            };
            let is_active = active_clone_for_osc.get();
            let tx = tx_osc_clone.clone();
            let pane_id = id_for_osc.clone();
            let pane = inner_rc.borrow().terminal.clone();
            let cwd = inner_rc
                .borrow()
                .working_dir
                .clone()
                .unwrap_or_default();
            gtk::glib::spawn_future_local(async move {
                if let Some(snapshot) = pane.get_text_snapshot(100, 0).await {
                    if is_active {
                        let _ = tx
                            .send(ClawMessage::CommandFinished {
                                exit_code: code,
                                snapshot,
                                cwd,
                            })
                            .await;
                    } else {
                        let agent = crate::get_agent().await;
                        let _ = agent
                            .notify_pane_command_finished(pane_id, code, snapshot, cwd)
                            .await;
                    }
                }
            });
        }
    });
}
