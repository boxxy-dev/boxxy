use std::cell::RefCell;
use std::rc::Rc;
use gtk4::prelude::*;
use gtk4 as gtk;
use crate::claw_popover::ClawPopover;
use crate::claw_indicator::ClawIndicator;
use crate::PaneOutput;
use super::{PaneInner, PendingDiagnosis};

pub(super) fn setup_claw(
    widget: &gtk::Overlay,
    inner: &Rc<RefCell<PaneInner>>,
    id: String,
    claw_sender: async_channel::Sender<boxxy_claw::engine::ClawMessage>,
    claw_rx: async_channel::Receiver<boxxy_claw::engine::ClawEngineEvent>,
    claw_message_list: gtk::ListBox,
    callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
) -> (ClawPopover, ClawIndicator, PendingDiagnosis) {
    let pending_proactive_diagnosis = Rc::new(RefCell::new(None::<(String, crate::ClawProposal)>));
    let pending_diag_clone = pending_proactive_diagnosis.clone();

    let pending_sidebar_buttons = Rc::new(RefCell::new(None::<gtk::Box>));

    let inner_clone_for_cmd = Rc::downgrade(inner);
    let inner_clone_for_reply = Rc::downgrade(inner);
    let tx_user_reply = claw_sender.clone();
    let tx_file_reply = claw_sender.clone();
    let tx_lazy_reply = claw_sender.clone();
    
    let claw_popover_self_ref: Rc<RefCell<Option<ClawPopover>>> = Rc::new(RefCell::new(None));
    let pending_sidebar_btns_popover_clone = pending_sidebar_buttons.clone();
    let cb_focus = callback.clone();

    let id_for_focus = id.clone();
    let claw_popover = ClawPopover::new(
        move |cmd| {
            if let Some(inner) = inner_clone_for_cmd.upgrade() {
                let mut bytes = cmd.as_bytes().to_vec();
                bytes.push(b'\r');
                inner.borrow().terminal.write_all(bytes);
            }
        },
        move |reply| {
            let tx = tx_user_reply.clone();
            let inner_opt = inner_clone_for_reply.upgrade();
            if let Some(inner) = inner_opt {
                let pane = inner.borrow().terminal.clone();
                let cwd = inner.borrow().working_dir.clone().unwrap_or_default();
                gtk::glib::spawn_future_local(async move {
                    if let Some(snapshot) = pane.get_text_snapshot(100).await {
                        let _ = tx.send(boxxy_claw::engine::ClawMessage::UserMessage {
                            
                            message: reply,
                            snapshot,
                            cwd,
                        }).await;
                    }
                });
            }
        },
        move |approved| {
            if let Some(btns) = pending_sidebar_btns_popover_clone.borrow_mut().take() {
                btns.set_visible(false);
            }
            
            let tx = tx_file_reply.clone();
            gtk::glib::spawn_future_local(async move {
                let _ = tx.send(boxxy_claw::engine::ClawMessage::FileWriteReply {
                    
                    approved,
                }).await;
            });
        },
        move |_proposal| {
            cb_focus(PaneOutput::FocusClawSidebar(id_for_focus.clone()));
        },
        {
            let tx_cancel = claw_sender.clone();
            move || {
                let tx = tx_cancel.clone();
                gtk::glib::spawn_future_local(async move {
                    let _ = tx.send(boxxy_claw::engine::ClawMessage::CancelPending).await;
                });
            }
        }
    );
    *claw_popover_self_ref.borrow_mut() = Some(claw_popover.clone());
    widget.add_overlay(claw_popover.widget());

    let popover_clone = claw_popover.clone();
    let claw_indicator = ClawIndicator::new(
        || {},
        move || {
            let tx = tx_lazy_reply.clone();
            gtk::glib::spawn_future_local(async move {
                let _ = tx.send(boxxy_claw::engine::ClawMessage::RequestLazyDiagnosis {
                    
                }).await;
            });
        },
        move || {
            if let Some((diag, proposal)) = pending_diag_clone.borrow_mut().take() {
                popover_clone.show("Boxxy-Claw", &diag, proposal);
            }
        }
    );
    widget.add_overlay(claw_indicator.widget());

    let cb_clone_events = callback.clone();
    let popover_event_clone = claw_popover.clone();
    let indicator_event_clone = claw_indicator.clone();
    let claw_list_events = claw_message_list.clone();
    let inner_for_events = inner.clone();

    gtk::glib::spawn_future_local(async move {
        while let Ok(event) = claw_rx.recv().await {
            let s = boxxy_preferences::Settings::load();
            let show_on_terminal = s.claw_terminal_suggestions;
            
            match &event {
                boxxy_claw::engine::ClawEngineEvent::AgentThinking { is_thinking, .. } => {
                    if *is_thinking && show_on_terminal {
                        indicator_event_clone.show_thinking();
                    } else {
                        indicator_event_clone.hide();
                    }
                }
                boxxy_claw::engine::ClawEngineEvent::LazyErrorIndicator { .. } => {
                    if show_on_terminal {
                        indicator_event_clone.show_lazy_error();
                    }
                }
                boxxy_claw::engine::ClawEngineEvent::DiagnosisComplete { diagnosis, .. } => {
                    boxxy_claw::ui::add_diagnosis_row(&claw_list_events, id.clone(), diagnosis);
                    indicator_event_clone.hide();
                    if show_on_terminal {
                        popover_event_clone.show(
                            "Boxxy-Claw",
                            diagnosis,
                            crate::ClawProposal::None
                        );
                    }
                }
                boxxy_claw::engine::ClawEngineEvent::InjectCommand { command, diagnosis, .. } => {
                    if !diagnosis.is_empty() {
                        boxxy_claw::ui::add_diagnosis_row(&claw_list_events, id.clone(), diagnosis);
                    }
                    
                    let tx_text_reply = claw_sender.clone();
                    let inner_for_reply = inner_for_events.clone();
                    let btns = boxxy_claw::ui::add_approval_row(&claw_list_events, id.clone(), command, move |reply_text| {
                        let tx = tx_text_reply.clone();
                        let inner = inner_for_reply.clone();
                        gtk::glib::spawn_future_local(async move {
                            let pane = inner.borrow().terminal.clone();
                            let cwd = inner.borrow().working_dir.clone().unwrap_or_default();
                            if let Some(snapshot) = pane.get_text_snapshot(100).await {
                                let _ = tx.send(boxxy_claw::engine::ClawMessage::UserMessage {
                                    message: reply_text,
                                    snapshot,
                                    cwd,
                                }).await;
                            }
                        });
                    });
                    *pending_sidebar_buttons.borrow_mut() = Some(btns);
                    
                    indicator_event_clone.hide();
                    if show_on_terminal {
                        popover_event_clone.show(
                            "Boxxy-Claw",
                            diagnosis,
                            crate::ClawProposal::Command(command.clone())
                        );
                    }
                }
                boxxy_claw::engine::ClawEngineEvent::ProposeFileWrite { path, content, .. } => {
                    let tx_file_reply_for_events = claw_sender.clone();
                    let tx_text_reply = claw_sender.clone();
                    let popover = popover_event_clone.clone();
                    let inner_for_reply = inner_for_events.clone();
                    
                    let btns = boxxy_claw::ui::add_file_write_approval_row(&claw_list_events, id.clone(), path, content, move |approved| {
                        let tx = tx_file_reply_for_events.clone();
                        let popover = popover.clone();
                        gtk::glib::spawn_future_local(async move {
                            popover.hide();
                            let _ = tx.send(boxxy_claw::engine::ClawMessage::FileWriteReply {
                                
                                approved,
                            }).await;
                        });
                    },
                    move |reply_text| {
                        let tx = tx_text_reply.clone();
                        let inner = inner_for_reply.clone();
                        gtk::glib::spawn_future_local(async move {
                            let pane = inner.borrow().terminal.clone();
                            let cwd = inner.borrow().working_dir.clone().unwrap_or_default();
                            if let Some(snapshot) = pane.get_text_snapshot(100).await {
                                let _ = tx.send(boxxy_claw::engine::ClawMessage::UserMessage {
                                    message: reply_text,
                                    snapshot,
                                    cwd,
                                }).await;
                            }
                        });
                    });
                    *pending_sidebar_buttons.borrow_mut() = Some(btns);

                    if show_on_terminal {
                        popover_event_clone.show(
                            "Boxxy-Claw: Propose File Edit",
                            &format!("Path: `{path}`\n\nI need to write or edit this file to complete the task."),
                            crate::ClawProposal::FileWrite(path.clone(), content.clone())
                        );
                    }
                }
                boxxy_claw::engine::ClawEngineEvent::ProposeTerminalCommand { command, explanation, .. } => {
                    if !explanation.is_empty() {
                        boxxy_claw::ui::add_diagnosis_row(&claw_list_events, id.clone(), explanation);
                    }
                    
                    let tx_text_reply = claw_sender.clone();
                    let inner_for_reply = inner_for_events.clone();
                    let btns = boxxy_claw::ui::add_approval_row(&claw_list_events, id.clone(), command, move |reply_text| {
                        let tx = tx_text_reply.clone();
                        let inner = inner_for_reply.clone();
                        gtk::glib::spawn_future_local(async move {
                            let pane = inner.borrow().terminal.clone();
                            let cwd = inner.borrow().working_dir.clone().unwrap_or_default();
                            if let Some(snapshot) = pane.get_text_snapshot(100).await {
                                let _ = tx.send(boxxy_claw::engine::ClawMessage::UserMessage {
                                    message: reply_text,
                                    snapshot,
                                    cwd,
                                }).await;
                            }
                        });
                    });
                    *pending_sidebar_buttons.borrow_mut() = Some(btns);
                    
                    if show_on_terminal {
                        popover_event_clone.show(
                            "Boxxy-Claw",
                            explanation,
                            crate::ClawProposal::Command(command.clone())
                        );
                    }
                }
                boxxy_claw::engine::ClawEngineEvent::ProposalResolved => {
                    popover_event_clone.hide();
                    if let Some(btns) = pending_sidebar_buttons.borrow_mut().take() {
                        btns.set_visible(false);
                    }
                }
            }

            cb_clone_events(PaneOutput::ClawEvent(id.clone(), event));
        }
    });

    (claw_popover, claw_indicator, pending_proactive_diagnosis)
}
