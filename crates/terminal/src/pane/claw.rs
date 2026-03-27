use super::{PaneInner, PendingDiagnosis};
use crate::PaneOutput;
use crate::claw_indicator::ClawIndicator;
use crate::overlay::{OverlayMode, TerminalOverlay};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) fn setup_claw(
    widget: &gtk::Overlay,
    inner: &Rc<RefCell<PaneInner>>,
    id: String,
    claw_sender: async_channel::Sender<boxxy_claw::engine::ClawMessage>,
    claw_rx: async_channel::Receiver<boxxy_claw::engine::ClawEngineEvent>,
    claw_message_list: gtk::ListBox,
    callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    spawn_intent: Option<String>,
    total_tokens: Rc<Cell<u64>>,
) -> (TerminalOverlay, ClawIndicator, PendingDiagnosis) {
    let pending_proactive_diagnosis =
        Rc::new(RefCell::new(None::<(String, crate::TerminalProposal)>));
    let pending_diag_clone = pending_proactive_diagnosis.clone();

    // Provide the initial intent if one was passed in
    if let Some(intent) = spawn_intent {
        let tx = claw_sender.clone();
        let inner_clone = inner.clone();
        gtk::glib::spawn_future_local(async move {
            let pane = inner_clone.borrow().terminal.clone();
            let cwd = inner_clone.borrow().working_dir.clone().unwrap_or_default();
            if let Some(snapshot) = pane.get_text_snapshot(100, 0).await {
                let _ = tx
                    .send(boxxy_claw::engine::ClawMessage::UserMessage {
                        message: intent,
                        snapshot,
                        cwd,
                        image_attachments: vec![],
                    })
                    .await;
            }
        });
    }

    let inner_clone_for_cmd = Rc::downgrade(inner);
    let inner_clone_for_reply = Rc::downgrade(inner);
    let inner_clone_for_file_reply = Rc::downgrade(inner);
    let inner_clone_for_cancel = Rc::downgrade(inner);
    let tx_user_reply = claw_sender.clone();
    let tx_file_reply = claw_sender.clone();
    let tx_lazy_reply = claw_sender.clone();

    let claw_popover_self_ref: Rc<RefCell<Option<TerminalOverlay>>> = Rc::new(RefCell::new(None));
    let cb_focus = callback.clone();

    let id_for_focus = id.clone();
    let claw_popover = TerminalOverlay::new(
        move |cmd: String| {
            if let Some(inner) = inner_clone_for_cmd.upgrade() {
                let mut bytes = cmd.as_bytes().to_vec();
                bytes.push(b'\r');
                inner.borrow().terminal.write_all(bytes);
                inner.borrow().terminal.grab_focus();
            }
        },
        move |(reply, image_attachments)| {
            let tx = tx_user_reply.clone();
            let inner_opt = inner_clone_for_reply.upgrade();
            if let Some(inner) = inner_opt {
                let pane = inner.borrow().terminal.clone();
                let cwd = inner.borrow().working_dir.clone().unwrap_or_default();
                inner.borrow().terminal.grab_focus();
                gtk::glib::spawn_future_local(async move {
                    if let Some(snapshot) = pane.get_text_snapshot(100, 0).await {
                        let _ = tx
                            .send(boxxy_claw::engine::ClawMessage::UserMessage {
                                message: reply,
                                snapshot,
                                cwd,
                                image_attachments,
                            })
                            .await;
                    }
                });
            }
        },
        move |approved| {
            if let Some(inner) = inner_clone_for_file_reply.upgrade() {
                inner.borrow().terminal.grab_focus();
            }

            let tx = tx_file_reply.clone();
            gtk::glib::spawn_future_local(async move {
                let _ = tx
                    .send(boxxy_claw::engine::ClawMessage::FileWriteReply { approved })
                    .await;
            });
        },
        move |_proposal| {
            cb_focus(PaneOutput::FocusClawSidebar(id_for_focus.clone()));
        },
        {
            let tx_cancel = claw_sender.clone();
            move |mode| {
                if let Some(inner) = inner_clone_for_cancel.upgrade() {
                    inner.borrow().terminal.grab_focus();
                }
                if mode == OverlayMode::Claw {
                    let tx = tx_cancel.clone();
                    gtk::glib::spawn_future_local(async move {
                        let _ = tx
                            .send(boxxy_claw::engine::ClawMessage::CancelPending)
                            .await;
                    });
                }
            }
        },
    );
    *claw_popover_self_ref.borrow_mut() = Some(claw_popover.clone());
    widget.add_overlay(claw_popover.widget());

    let popover_clone = claw_popover.clone();
    let claw_indicator = ClawIndicator::new(
        || {},
        move || {
            let tx = tx_lazy_reply.clone();
            gtk::glib::spawn_future_local(async move {
                let _ = tx
                    .send(boxxy_claw::engine::ClawMessage::RequestLazyDiagnosis {})
                    .await;
            });
        },
        move || {
            if let Some((diag, proposal)) = pending_diag_clone.borrow_mut().take() {
                popover_clone.show(OverlayMode::Claw, "Boxxy-Claw", &diag, proposal);
            }
        },
    );
    widget.add_overlay(claw_indicator.widget());

    let cb_clone_events = callback.clone();
    let popover_event_clone = claw_popover.clone();
    let indicator_event_clone = claw_indicator.clone();
    let claw_list_events = claw_message_list.clone();
    let inner_for_events = inner.clone();
    let total_tokens_for_events = total_tokens.clone();

    gtk::glib::spawn_future_local(async move {
        while let Ok(event) = claw_rx.recv().await {
            match &event {
                boxxy_claw::engine::ClawEngineEvent::AgentThinking { is_thinking, .. } => {
                    if *is_thinking {
                        indicator_event_clone.show_thinking();
                    } else {
                        indicator_event_clone.hide();
                    }
                }
                boxxy_claw::engine::ClawEngineEvent::LazyErrorIndicator { .. } => {
                    indicator_event_clone.show_lazy_error();
                }
                boxxy_claw::engine::ClawEngineEvent::DiagnosisComplete {
                    diagnosis,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    boxxy_claw::ui::add_diagnosis_row(
                        &claw_list_events,
                        id.clone(),
                        Some(agent_name.clone()),
                        diagnosis,
                    );
                    indicator_event_clone.hide();
                    popover_event_clone.show(
                        OverlayMode::Claw,
                        &format!("Agent: {agent_name}"),
                        diagnosis,
                        crate::TerminalProposal::None,
                    );
                }
                boxxy_claw::engine::ClawEngineEvent::InjectCommand {
                    command,
                    diagnosis,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    if !diagnosis.is_empty() {
                        boxxy_claw::ui::add_diagnosis_row(
                            &claw_list_events,
                            id.clone(),
                            Some(agent_name.clone()),
                            diagnosis,
                        );
                    }

                    boxxy_claw::ui::add_approval_row(
                        &claw_list_events,
                        id.clone(),
                        Some(agent_name.clone()),
                        command,
                        |_| {},
                    );

                    indicator_event_clone.hide();
                    popover_event_clone.show(
                        OverlayMode::Claw,
                        &format!("Agent: {agent_name}"),
                        diagnosis,
                        crate::TerminalProposal::Command(command.clone()),
                    );
                }
                boxxy_claw::engine::ClawEngineEvent::ProposeFileWrite {
                    path,
                    content,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    boxxy_claw::ui::add_file_write_approval_row(
                        &claw_list_events,
                        id.clone(),
                        Some(agent_name.clone()),
                        path,
                        content,
                        |_| {},
                        |_| {},
                    );

                    popover_event_clone.show(
                        OverlayMode::Claw,
                        &format!("Agent {agent_name}: Propose File Edit"),
                        &format!("Path: `{path}`\n\nI need to write or edit this file to complete the task."),
                        crate::TerminalProposal::FileWrite(path.clone(), content.clone())
                    );
                }
                boxxy_claw::engine::ClawEngineEvent::ProposeFileDelete { path, agent_name, usage } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    boxxy_claw::ui::add_file_delete_approval_row(
                        &claw_list_events,
                        id.clone(),
                        Some(agent_name.clone()),
                        path,
                        |_| {},
                        |_| {},
                    );

                    popover_event_clone.show(
                        OverlayMode::Claw,
                        &format!("Agent: {agent_name}"),
                        &format!("I want to DELETE this file:\n\n`{path}`"),
                        crate::TerminalProposal::Command(format!("rm '{path}'")),
                    );
                }
                boxxy_claw::engine::ClawEngineEvent::ProposeKillProcess {
                    pid,
                    process_name,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    boxxy_claw::ui::add_kill_process_approval_row(
                        &claw_list_events,
                        id.clone(),
                        Some(agent_name.clone()),
                        *pid,
                        process_name,
                        |_| {},
                        |_| {},
                    );

                    popover_event_clone.show(
                        OverlayMode::Claw,
                        &format!("Agent: {agent_name}"),
                        &format!("I want to KILL this process:\n\nPID: {pid} ({process_name})"),
                        crate::TerminalProposal::Command(format!("kill {pid}")),
                    );
                }
                boxxy_claw::engine::ClawEngineEvent::ProposeGetClipboard { agent_name, usage } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    boxxy_claw::ui::add_clipboard_get_approval_row(
                        &claw_list_events,
                        id.clone(),
                        Some(agent_name.clone()),
                        |_| {},
                        |_| {},
                    );

                    popover_event_clone.show(
                        OverlayMode::Claw,
                        &format!("Agent: {agent_name}"),
                        "I want to read your clipboard.",
                        crate::TerminalProposal::None,
                    );
                }
                boxxy_claw::engine::ClawEngineEvent::ProposeSetClipboard { agent_name, text, usage } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    boxxy_claw::ui::add_clipboard_set_approval_row(
                        &claw_list_events,
                        id.clone(),
                        Some(agent_name.clone()),
                        text,
                        |_| {},
                        |_| {},
                    );

                    popover_event_clone.show(
                        OverlayMode::Claw,
                        &format!("Agent: {agent_name}"),
                        &format!("I want to write this to your clipboard:\n\n{text}"),
                        crate::TerminalProposal::None,
                    );
                }
                boxxy_claw::engine::ClawEngineEvent::ProposeTerminalCommand {
                    command,
                    explanation,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    if !explanation.is_empty() {
                        boxxy_claw::ui::add_diagnosis_row(
                            &claw_list_events,
                            id.clone(),
                            Some(agent_name.clone()),
                            explanation,
                        );
                    }

                    boxxy_claw::ui::add_approval_row(
                        &claw_list_events,
                        id.clone(),
                        Some(agent_name.clone()),
                        command,
                        |_| {},
                    );

                    popover_event_clone.show(
                        OverlayMode::Claw,
                        &format!("Agent: {agent_name}"),
                        explanation,
                        crate::TerminalProposal::Command(command.clone()),
                    );
                }
                boxxy_claw::engine::ClawEngineEvent::Identity { agent_name } => {
                    inner_for_events
                        .borrow()
                        .agent_badge
                        .set_identity(agent_name);
                }
                boxxy_claw::engine::ClawEngineEvent::RequestScrollback {
                    max_lines,
                    offset_lines,
                    reply,
                    ..
                } => {
                    let pane_inner = inner_for_events.borrow();
                    let pane = pane_inner.terminal.clone();

                    if pane.is_alt_screen() {
                        pane_inner.agent_badge.set_visible(false);
                    } else {
                        pane_inner.agent_badge.set_visible(true);
                    }

                    let max_lines = *max_lines;
                    let offset_lines = *offset_lines;
                    let reply = reply.clone();
                    gtk::glib::spawn_future_local(async move {
                        let mut sender_opt = reply.lock().await;
                        if let Some(sender) = sender_opt.take() {
                            if let Some(snapshot) =
                                pane.get_text_snapshot(max_lines, offset_lines).await
                            {
                                let _ = sender.send(snapshot);
                            } else {
                                let _ =
                                    sender.send("Error: Failed to fetch scrollback.".to_string());
                            }
                        }
                    });
                }
                boxxy_claw::engine::ClawEngineEvent::ProposalResolved { .. } => {
                    popover_event_clone.hide();
                }
                boxxy_claw::engine::ClawEngineEvent::RequestSpawnAgent { .. }
                | boxxy_claw::engine::ClawEngineEvent::RequestCloseAgent { .. }
                | boxxy_claw::engine::ClawEngineEvent::InjectKeystrokes { .. } => {
                    // Handled upstream by TerminalComponent / Window
                }
                boxxy_claw::engine::ClawEngineEvent::ToolResult {
                    agent_name,
                    tool_name,
                    result,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens_for_events.set(total_tokens_for_events.get() + usage.total_tokens);
                    }
                    if tool_name == "list_processes" {
                        boxxy_claw::ui::add_process_list_row(
                            &claw_list_events,
                            id.clone(),
                            Some(agent_name.clone()),
                            result,
                            |_, _| {},
                        );
                    }
                }
            }

            cb_clone_events(PaneOutput::ClawEvent(id.clone(), event));
        }
    });

    (claw_popover, claw_indicator, pending_proactive_diagnosis)
}
