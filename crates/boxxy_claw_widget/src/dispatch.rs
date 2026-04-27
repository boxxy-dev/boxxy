//! Event dispatch loop for the claw drawer.
//!
//! Pulls `ClawEngineEvent` messages off the per-pane channel, mutates
//! the drawer UI (overlay, indicator, msgbar, sidebar log), and forwards
//! the raw event upstream via `ClawHost::forward_event` so cross-pane
//! consumers (tab badges, swarm router) still see the full stream.
//!
//! Every host-side side-effect (inject a `cd`, show a toast, ask for
//! scrollback) goes through the `ClawHost` trait. The dispatcher itself
//! is surface-agnostic — a non-terminal host could drive the same UI
//! by implementing `ClawHost` differently.

use crate::msgbar::MsgBarComponent;
use crate::{ClawHost, ClawIndicator, OverlayMode, TerminalOverlay};
use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage, UsageWrapper};
use gtk4 as gtk;
use gtk4::gio;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Per-turn token accounting: sum input+output since the protocol
/// dropped its materialised `total_tokens` field.
#[inline]
fn usage_total(u: &UsageWrapper) -> u64 {
    u.input_tokens + u.output_tokens
}

/// Wire a `ClawEngineEvent` receiver into the drawer UI + sidebar log.
///
/// The signature is wide because the dispatcher is the single point
/// where pane-held state (session status, pin flag, etc.) meets
/// widget-held UI (overlay, indicator, sidebar store). Keeping them as
/// explicit parameters makes the data flow grep-able — no hidden
/// globals, no magic context object.
#[allow(clippy::too_many_arguments)]
pub fn spawn_dispatch(
    claw_rx: async_channel::Receiver<ClawEngineEvent>,
    host: Rc<dyn ClawHost>,
    overlay: TerminalOverlay,
    indicator: ClawIndicator,
    msg_bar: Rc<MsgBarComponent>,
    sidebar_store: gio::ListStore,
    id: String,
    session_status: Rc<RefCell<AgentStatus>>,
    is_pinned: Rc<Cell<bool>>,
    is_web_search: Rc<Cell<bool>>,
    agent_name: Rc<RefCell<String>>,
    total_tokens: Rc<Cell<u64>>,
) {
    let overlay_store = overlay.history_store();

    gtk::glib::spawn_future_local(async move {
        while let Ok(event) = claw_rx.recv().await {
            match &event {
                ClawEngineEvent::SessionStateChanged { status, .. } => {
                    *session_status.borrow_mut() = status.clone();
                    msg_bar.set_status(status.clone());
                    indicator.set_mode(status.clone());
                }
                ClawEngineEvent::UserMessage { content } => {
                    boxxy_claw_ui::add_user_row(&sidebar_store, id.clone(), content);
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_user_row(&overlay_store, id.clone(), content);
                    }
                }
                ClawEngineEvent::ToolCallStarted {
                    agent_name: event_agent_name,
                    tool_name,
                } => {
                    indicator.show_thinking(event_agent_name, Some(tool_name));
                    overlay.set_indicator_slot_visible(true);
                }
                ClawEngineEvent::AgentThinking {
                    is_thinking,
                    agent_name: event_agent_name,
                } => {
                    overlay.set_thinking(*is_thinking);
                    if *is_thinking {
                        msg_bar.set_input_sensitive(false);
                        indicator.show_thinking(event_agent_name, None);
                        if !overlay.is_visible() {
                            overlay.show_chat_only(event_agent_name);
                        }
                        overlay.set_indicator_slot_visible(true);
                    } else {
                        msg_bar.set_input_sensitive(true);
                        indicator.hide();
                        overlay.set_indicator_slot_visible(false);
                    }
                }
                ClawEngineEvent::LazyErrorIndicator { .. } => {
                    indicator.show_lazy_error();
                    overlay.set_indicator_slot_visible(true);
                }
                ClawEngineEvent::DismissDrawer => {
                    // Explicitly close the drawer from the backend. This happens when
                    // the user rejects a proposal, and the agent outputs [SILENT_ACK].
                    overlay.hide();
                }
                ClawEngineEvent::DiagnosisComplete {
                    diagnosis,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    boxxy_claw_ui::add_diagnosis_row(
                        &sidebar_store,
                        id.clone(),
                        Some(agent_name.clone()),
                        diagnosis,
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_diagnosis_row(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            diagnosis,
                        );
                    }
                    indicator.hide();
                    overlay.set_indicator_slot_visible(false);
                    overlay.show(
                        OverlayMode::Claw,
                        agent_name,
                        Some("Diagnosis"),
                        diagnosis,
                        crate::Proposal::None,
                    );
                }
                // Proposal events: the in-terminal popover owns user
                // approval. The sidebar gets a read-only log entry so
                // the history view reflects that the agent proposed
                // something — the approval-row helpers format it as a
                // Diagnosis row and ignore their callback args.
                ClawEngineEvent::InjectCommand {
                    command,
                    diagnosis,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    if !diagnosis.is_empty() {
                        boxxy_claw_ui::add_diagnosis_row(
                            &sidebar_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            diagnosis,
                        );
                        if overlay.history_mode() {
                            boxxy_claw_ui::add_diagnosis_row(
                                &overlay_store,
                                id.clone(),
                                Some(agent_name.clone()),
                                diagnosis,
                            );
                        }
                    }
                    boxxy_claw_ui::add_approval_row(
                        &sidebar_store,
                        id.clone(),
                        Some(agent_name.clone()),
                        command,
                        |_| {},
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_approval_row(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            command,
                            |_| {},
                        );
                    }
                    indicator.hide();
                    overlay.set_indicator_slot_visible(false);
                    overlay.show(
                        OverlayMode::Claw,
                        agent_name,
                        Some("Propose Command"),
                        diagnosis,
                        crate::Proposal::Command(command.clone()),
                    );
                }
                ClawEngineEvent::ProposeFileWrite {
                    path,
                    content,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    boxxy_claw_ui::add_file_write_approval_row(
                        &sidebar_store,
                        id.clone(),
                        Some(agent_name.clone()),
                        path,
                        content,
                        |_| {},
                        |_| {},
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_file_write_approval_row(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            path,
                            content,
                            |_| {},
                            |_| {},
                        );
                    }
                    overlay.show(
                        OverlayMode::Claw,
                        agent_name,
                        Some("Propose File Edit"),
                        &format!(
                            "Path: `{path}`\n\nI need to write or edit this file to complete the task."
                        ),
                        crate::Proposal::FileWrite {
                            path: path.clone(),
                            content: content.clone(),
                        },
                    );
                }
                ClawEngineEvent::ProposeFileDelete {
                    path,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    boxxy_claw_ui::add_file_delete_approval_row(
                        &sidebar_store,
                        id.clone(),
                        Some(agent_name.clone()),
                        path,
                        |_| {},
                        |_| {},
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_file_delete_approval_row(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            path,
                            |_| {},
                            |_| {},
                        );
                    }
                    overlay.show(
                        OverlayMode::Claw,
                        agent_name,
                        Some("Delete File"),
                        &format!("I want to DELETE this file:\n\n`{path}`"),
                        crate::Proposal::FileDelete { path: path.clone() },
                    );
                }
                ClawEngineEvent::ProposeKillProcess {
                    pid,
                    process_name,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    boxxy_claw_ui::add_kill_process_approval_row(
                        &sidebar_store,
                        id.clone(),
                        Some(agent_name.clone()),
                        *pid,
                        process_name,
                        |_| {},
                        |_| {},
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_kill_process_approval_row(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            *pid,
                            process_name,
                            |_| {},
                            |_| {},
                        );
                    }
                    overlay.show(
                        OverlayMode::Claw,
                        agent_name,
                        Some("Kill Process"),
                        &format!("I want to KILL this process:\n\nPID: {pid} ({process_name})"),
                        crate::Proposal::KillProcess {
                            pid: *pid,
                            name: process_name.clone(),
                        },
                    );
                }
                ClawEngineEvent::ProposeGetClipboard { agent_name, usage } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    boxxy_claw_ui::add_clipboard_get_approval_row(
                        &sidebar_store,
                        id.clone(),
                        Some(agent_name.clone()),
                        |_| {},
                        |_| {},
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_clipboard_get_approval_row(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            |_| {},
                            |_| {},
                        );
                    }
                    overlay.show(
                        OverlayMode::Claw,
                        agent_name,
                        Some("Read Clipboard"),
                        "I want to read your clipboard.",
                        crate::Proposal::GetClipboard,
                    );
                }
                ClawEngineEvent::ProposeSetClipboard {
                    agent_name,
                    text,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    boxxy_claw_ui::add_clipboard_set_approval_row(
                        &sidebar_store,
                        id.clone(),
                        Some(agent_name.clone()),
                        text,
                        |_| {},
                        |_| {},
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_clipboard_set_approval_row(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            text,
                            |_| {},
                            |_| {},
                        );
                    }
                    overlay.show(
                        OverlayMode::Claw,
                        agent_name,
                        Some("Write Clipboard"),
                        &format!("I want to write this to your clipboard:\n\n{text}"),
                        crate::Proposal::SetClipboard(text.clone()),
                    );
                }
                ClawEngineEvent::ProposeTerminalCommand {
                    command,
                    explanation,
                    agent_name,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    if !explanation.is_empty() {
                        boxxy_claw_ui::add_diagnosis_row(
                            &sidebar_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            explanation,
                        );
                        if overlay.history_mode() {
                            boxxy_claw_ui::add_diagnosis_row(
                                &overlay_store,
                                id.clone(),
                                Some(agent_name.clone()),
                                explanation,
                            );
                        }
                    }
                    boxxy_claw_ui::add_approval_row(
                        &sidebar_store,
                        id.clone(),
                        Some(agent_name.clone()),
                        command,
                        |_| {},
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_approval_row(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            command,
                            |_| {},
                        );
                    }
                    overlay.show(
                        OverlayMode::Claw,
                        agent_name,
                        Some("Terminal Command"),
                        explanation,
                        crate::Proposal::Command(command.clone()),
                    );
                }
                ClawEngineEvent::Identity {
                    agent_name: name,
                    character_id,
                    pinned,
                    web_search_enabled,
                    total_tokens: total,
                } => {
                    overlay.set_active_agent(name);
                    indicator.set_identity(name, character_id);
                    is_pinned.set(*pinned);
                    is_web_search.set(*web_search_enabled);
                    *agent_name.borrow_mut() = name.clone();

                    let status = session_status.borrow().clone();
                    msg_bar.set_character(character_id);
                    msg_bar.update_ui(status, *pinned, *web_search_enabled);
                    total_tokens.set(*total);
                }
                ClawEngineEvent::PinStatusChanged(pinned) => {
                    is_pinned.set(*pinned);
                    let status = session_status.borrow().clone();
                    msg_bar.update_ui(status, *pinned, msg_bar.web_search_state.get());
                }
                ClawEngineEvent::WebSearchStatusChanged(enabled) => {
                    is_web_search.set(*enabled);
                    let status = session_status.borrow().clone();
                    msg_bar.update_ui(status, msg_bar.pin_state.get(), *enabled);
                }
                ClawEngineEvent::Evicted => {
                    indicator.set_evicted(true);
                    indicator.hide();
                    overlay.set_indicator_slot_visible(false);
                    overlay.hide();
                    boxxy_claw_ui::add_diagnosis_row(
                        &sidebar_store,
                        id.clone(),
                        None,
                        "Agent was EVICTED because the session was resumed in another pane.",
                    );
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_diagnosis_row(
                            &overlay_store,
                            id.clone(),
                            None,
                            "Agent was EVICTED because the session was resumed in another pane.",
                        );
                    }
                }
                ClawEngineEvent::RequestCwdSwitch { path } => {
                    // Host decides how/whether to cd — busy check,
                    // path validation, and user notifications on failure
                    // all live on the host side.
                    host.cd(path.clone());
                }
                ClawEngineEvent::SystemMessage { text } => {
                    boxxy_claw_ui::add_system_message_row(&sidebar_store, id.clone(), text);
                    if overlay.history_mode() {
                        boxxy_claw_ui::add_system_message_row(&overlay_store, id.clone(), text);
                    }
                }
                ClawEngineEvent::RequestScrollback {
                    max_lines,
                    offset_lines,
                    request_id,
                    ..
                } => {
                    // Hide the badge while a TUI owns the host —
                    // scrollback requests during vim/less would
                    // otherwise be visually noisy without being useful.
                    indicator.set_visible(!host.is_busy());

                    let max_lines = *max_lines;
                    let offset_lines = *offset_lines;
                    let request_id = *request_id;
                    let snapshot = host.snapshot(max_lines, offset_lines);
                    let host_for_reply = host.clone();
                    gtk::glib::spawn_future_local(async move {
                        let content = snapshot
                            .await
                            .unwrap_or_else(|| "Error: Failed to fetch scrollback.".to_string());
                        host_for_reply.send_claw(ClawMessage::ScrollbackReply {
                            request_id,
                            content,
                        });
                    });
                }
                ClawEngineEvent::ProposalResolved { .. } => {
                    overlay.hide();
                }
                ClawEngineEvent::RequestSpawnAgent { .. }
                | ClawEngineEvent::RequestCloseAgent { .. }
                | ClawEngineEvent::InjectKeystrokes { .. } => {
                    // Handled upstream by the window orchestrator via
                    // forward_event — no widget side-effect here.
                }
                ClawEngineEvent::ToolResult {
                    agent_name,
                    tool_name,
                    result,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        total_tokens.set(total_tokens.get() + usage_total(usage));
                    }
                    if tool_name == "list_processes" {
                        boxxy_claw_ui::add_process_list_row(
                            &sidebar_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            result,
                            |_, _| {},
                        );
                    } else {
                        boxxy_claw_ui::add_tool_call_row(
                            &sidebar_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            tool_name,
                            result,
                        );
                    }
                }
                ClawEngineEvent::TaskStatusChanged { tasks, .. } => {
                    let has_pending = tasks
                        .iter()
                        .any(|t| t.status == boxxy_claw_protocol::TaskStatus::Pending);
                    indicator.set_has_tasks(has_pending);
                }
                ClawEngineEvent::RestoreHistory(rows) => {
                    // Bulk append history items to minimize UI layout passes.
                    let mut items = Vec::with_capacity(rows.len());
                    for row in rows {
                        items.push(boxxy_claw_ui::row_object::ClawRowObject::new(row.clone()));
                    }
                    sidebar_store.remove_all();
                    sidebar_store.extend_from_slice(&items);
                    if overlay.history_mode() {
                        // Independent rebuild for the overlay store — cheap
                        // because ClawRowObject wraps an Arc-ish persistent
                        // row, so the duplicate is just a second GObject
                        // wrapper around the same data.
                        let mut overlay_items = Vec::with_capacity(rows.len());
                        for row in rows {
                            if row.is_overlay_visible() {
                                overlay_items.push(boxxy_claw_ui::row_object::ClawRowObject::new(
                                    row.clone(),
                                ));
                            }
                        }
                        overlay_store.remove_all();
                        overlay_store.extend_from_slice(&overlay_items);
                    }
                }
                ClawEngineEvent::TaskCompleted { .. }
                | ClawEngineEvent::PushGlobalNotification { .. }
                | ClawEngineEvent::DelegatedTaskReply { .. } => {
                    // Routed by the window orchestrator / swarm layer;
                    // forwarded below.
                }
            }

            host.forward_event(event);
        }
    });
}
