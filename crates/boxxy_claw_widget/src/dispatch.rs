//! Event dispatch loop for the claw drawer.
//!
//! Pulls `ClawEngineEvent` messages off the per-pane channel, mutates
//! the drawer UI (overlay, indicator, msgbar), and forwards the raw
//! event upstream via `ClawHost::forward_event` so cross-pane consumers
//! (tab badges, swarm router, sidebar status) still see the full stream.
//!
//! Every host-side side-effect (inject a `cd`, show a toast, ask for
//! scrollback) goes through the `ClawHost` trait. The dispatcher itself
//! is surface-agnostic — a non-terminal host could drive the same UI
//! by implementing `ClawHost` differently.

use crate::msgbar::MsgBarComponent;
use crate::{ClawHost, ClawIndicator, OverlayMode, TerminalOverlay, state::OverlayState};
use boxxy_claw_protocol::{
    AgentStatus, ClawEngineEvent, ClawMessage, SessionUsage, TurnTraceItem, UsageWrapper,
};
use boxxy_claw_ui::row_object::ClawRowObject;
use gtk4 as gtk;
use gtk4::gio::prelude::ListModelExt;
use gtk4::glib::object::CastNone;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Apply a turn's `UsageWrapper` to the pane-held `SessionUsage` cell.
#[inline]
fn record_usage(cell: &Cell<SessionUsage>, u: &UsageWrapper) {
    let mut su = cell.get();
    su.record_turn(u);
    cell.set(su);
}

fn ensure_working_row(
    overlay_store: &gtk::gio::ListStore,
    slot: &RefCell<Option<ClawRowObject>>,
    pane_id: &str,
    agent_name: &str,
    character_id: &str,
) -> ClawRowObject {
    if let Some(existing) = slot.borrow().clone() {
        existing.ensure_speaker(agent_name, character_id);
        if !existing.is_working() {
            existing.set_working(true);
        }
        return existing;
    }

    let created = boxxy_claw_ui::add_working_turn_row(
        overlay_store,
        pane_id.to_string(),
        (!agent_name.is_empty()).then(|| agent_name.to_string()),
        (!character_id.is_empty()).then(|| character_id.to_string()),
    );
    *slot.borrow_mut() = Some(created.clone());
    created
}

fn settle_or_prune_working_row(
    overlay_store: &gtk::gio::ListStore,
    slot: &RefCell<Option<ClawRowObject>>,
) {
    let Some(row) = slot.borrow_mut().take() else {
        return;
    };
    row.set_pending_approval_diff(None);
    row.set_active_status(None);
    row.set_working(false);

    if row.is_empty_working_placeholder() {
        let n = overlay_store.n_items();
        for idx in (0..n).rev() {
            if let Some(item) = overlay_store.item(idx).and_downcast::<ClawRowObject>()
                && item == row
            {
                overlay_store.remove(idx);
                break;
            }
        }
    }
}

/// Wire a `ClawEngineEvent` receiver into the drawer UI.
///
/// The signature is wide because the dispatcher is the single point
/// where pane-held state (session status, pin flag, etc.) meets
/// widget-held UI (overlay, indicator). Keeping them as explicit
/// parameters makes the data flow grep-able — no hidden globals,
/// no magic context object.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn spawn_dispatch(
    claw_rx: async_channel::Receiver<ClawEngineEvent>,
    host: Rc<dyn ClawHost>,
    overlay: TerminalOverlay,
    indicator: ClawIndicator,
    msg_bar: Rc<MsgBarComponent>,
    id: String,
    session_status: Rc<RefCell<AgentStatus>>,
    is_pinned: Rc<Cell<bool>>,
    is_web_search: Rc<Cell<bool>>,
    is_vibe: Rc<Cell<bool>>,
    agent_name: Rc<RefCell<String>>,
    session_usage: Rc<Cell<SessionUsage>>,
    pending_tasks: Rc<RefCell<Vec<boxxy_claw_protocol::ScheduledTask>>>,
) {
    let overlay_store = overlay.history_store();
    let active_turn_row: Rc<RefCell<Option<ClawRowObject>>> = Rc::new(RefCell::new(None));
    let ignoring_aborted_turn = Rc::new(Cell::new(false));

    // Immediately settle or prune the working row on the GTK thread when the user
    // cancels via double-Esc or the indicator `×` button, and ignore any late D-Bus
    // deltas that were already in flight before `ClawMessage::Abort` reached the daemon.
    {
        let store_for_cancel = overlay_store.clone();
        let row_for_cancel = active_turn_row.clone();
        let aborted_flag = ignoring_aborted_turn.clone();
        indicator.connect_cancelled(move || {
            aborted_flag.set(true);
            settle_or_prune_working_row(&store_for_cancel, &row_for_cancel);
        });
    }
    {
        let aborted_flag = ignoring_aborted_turn.clone();
        overlay.connect_turn_submitted(move || {
            aborted_flag.set(false);
        });
    }

    gtk::glib::spawn_future_local(async move {
        while let Ok(event) = claw_rx.recv().await {
            match &event {
                ClawEngineEvent::SessionStateChanged { status, .. } => {
                    *session_status.borrow_mut() = status.clone();
                    msg_bar.set_status(status.clone());
                    indicator.set_mode(status.clone());
                    if matches!(
                        status,
                        AgentStatus::Waiting
                            | AgentStatus::Sleep
                            | AgentStatus::Off
                            | AgentStatus::Faulted { .. }
                    ) {
                        ignoring_aborted_turn.set(false);
                        settle_or_prune_working_row(&overlay_store, &active_turn_row);
                    }
                }
                ClawEngineEvent::UserMessage { content, images } => {
                    ignoring_aborted_turn.set(false);
                    settle_or_prune_working_row(&overlay_store, &active_turn_row);
                    boxxy_claw_ui::add_user_row(
                        &overlay_store,
                        id.clone(),
                        content,
                        images.clone(),
                    );
                }
                ClawEngineEvent::ToolCallStarted {
                    agent_name: event_agent_name,
                    character_id,
                    tool_name,
                } => {
                    if !ignoring_aborted_turn.get() {
                        let row = ensure_working_row(
                            &overlay_store,
                            &active_turn_row,
                            &id,
                            event_agent_name,
                            character_id,
                        );
                        row.set_active_status(Some(boxxy_claw_protocol::busy_tool_label(
                            tool_name, false,
                        )));
                        overlay.set_tool_started(event_agent_name, tool_name);
                        overlay.nudge_sticky_scroll();
                    }
                }
                ClawEngineEvent::TurnTraceProgress {
                    agent_name: event_agent_name,
                    character_id,
                    item,
                    active_status,
                } => {
                    if !ignoring_aborted_turn.get() {
                        let row = ensure_working_row(
                            &overlay_store,
                            &active_turn_row,
                            &id,
                            event_agent_name,
                            character_id,
                        );
                        if let Some(status) = active_status {
                            row.set_active_status(Some(status.clone()));
                        }
                        if let Some(trace_item) = item {
                            row.push_trace_item(trace_item.clone());
                        }
                        overlay.nudge_sticky_scroll();
                    }
                }
                ClawEngineEvent::AssistantTextDelta {
                    agent_name: event_agent_name,
                    character_id,
                    text,
                } => {
                    if !ignoring_aborted_turn.get() {
                        let row = ensure_working_row(
                            &overlay_store,
                            &active_turn_row,
                            &id,
                            event_agent_name,
                            character_id,
                        );
                        row.set_active_status(None);
                        row.set_streaming_prose(text);
                        overlay.nudge_sticky_scroll();
                    }
                }
                ClawEngineEvent::AgentThinking {
                    is_thinking,
                    agent_name: event_agent_name,
                    character_id,
                } => {
                    if *is_thinking {
                        if !ignoring_aborted_turn.get() {
                            if !event_agent_name.is_empty() {
                                overlay.set_active_agent(event_agent_name);
                            }
                            if !event_agent_name.is_empty() || !character_id.is_empty() {
                                let _ = ensure_working_row(
                                    &overlay_store,
                                    &active_turn_row,
                                    &id,
                                    event_agent_name,
                                    character_id,
                                );
                            }
                            if !overlay.is_visible() && !host.has_foreground_process() {
                                overlay.show_chat_only(event_agent_name);
                            }
                            overlay.set_thinking(true);
                            overlay.nudge_sticky_scroll();
                        }
                    } else {
                        let was_aborted = ignoring_aborted_turn.replace(false);
                        if was_aborted {
                            settle_or_prune_working_row(&overlay_store, &active_turn_row);
                        } else {
                            let should_prune =
                                active_turn_row.borrow().as_ref().is_some_and(|row| {
                                    row.set_active_status(None);
                                    row.set_working(false);
                                    row.is_empty_working_placeholder()
                                });
                            if should_prune {
                                settle_or_prune_working_row(&overlay_store, &active_turn_row);
                            }
                        }
                        overlay.set_thinking(false);
                    }
                }
                ClawEngineEvent::LazyErrorIndicator { .. } => {
                    indicator.show_lazy_error();
                }
                ClawEngineEvent::DismissDrawer => {
                    // Explicitly close the drawer from the backend. This happens when
                    // the user rejects a proposal, and the agent outputs [SILENT_ACK].
                    ignoring_aborted_turn.set(false);
                    settle_or_prune_working_row(&overlay_store, &active_turn_row);
                    overlay.hide();
                }
                ClawEngineEvent::FocusPane => {
                    host.focus();
                }
                ClawEngineEvent::DiagnosisComplete {
                    diagnosis,
                    trace,
                    agent_name,
                    character_id,
                    usage,
                    typewriter,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    let taken = active_turn_row.borrow_mut().take();
                    if let Some(row) = taken {
                        row.finalize_diagnosis(
                            Some(agent_name.clone()),
                            Some(character_id.clone()),
                            diagnosis.clone(),
                            trace.clone(),
                            usage.clone(),
                            *typewriter,
                        );
                    } else {
                        boxxy_claw_ui::add_diagnosis_row_with_trace(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            Some(character_id.clone()),
                            diagnosis,
                            trace.clone(),
                            *typewriter,
                        );
                    }
                    if overlay.is_visible() || !host.has_foreground_process() {
                        overlay.show(
                            OverlayMode::Claw,
                            agent_name,
                            Some("Diagnosis"),
                            diagnosis,
                            crate::Proposal::None,
                        );
                    } else {
                        overlay.set_state(crate::OverlayState::Idle);
                    }
                }
                ClawEngineEvent::InjectCommand {
                    command,
                    diagnosis,
                    trace,
                    agent_name,
                    character_id,
                    target_pane_id,
                    target_pane_label,
                    auto_approve,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    let taken = active_turn_row.borrow_mut().take();
                    if let Some(row) = taken {
                        row.finalize_suggested(
                            Some(agent_name.clone()),
                            Some(character_id.clone()),
                            diagnosis.clone(),
                            command.clone(),
                            trace.clone(),
                            usage.clone(),
                        );
                    } else {
                        boxxy_claw_ui::add_suggested_row_with_trace(
                            &overlay_store,
                            id.clone(),
                            Some(agent_name.clone()),
                            Some(character_id.clone()),
                            diagnosis,
                            command,
                            trace.clone(),
                        );
                    }
                    if *auto_approve {
                        if let Some(target_id) = target_pane_id
                            .as_deref()
                            .filter(|tid| !tid.is_empty() && *tid != id.as_str())
                        {
                            host.inject_line_to_pane(target_id, command.clone());
                            overlay.show(
                                OverlayMode::Claw,
                                agent_name,
                                Some("Diagnosis"),
                                diagnosis,
                                crate::Proposal::None,
                            );
                        } else {
                            host.inject_line(command.clone());
                            overlay.hide();
                            host.grab_focus();
                        }
                    } else {
                        overlay.show(
                            OverlayMode::Claw,
                            agent_name,
                            Some("Propose Command"),
                            diagnosis,
                            crate::Proposal::Command {
                                command: command.clone(),
                                target_pane_id: target_pane_id.clone(),
                                target_pane_label: target_pane_label.clone(),
                            },
                        );
                    }
                }
                ClawEngineEvent::ProposeFileWrite {
                    path,
                    content,
                    diff,
                    agent_name,
                    character_id,
                    auto_approve,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    if !ignoring_aborted_turn.get() {
                        let resolved_diff = diff.clone().unwrap_or_else(|| {
                            boxxy_claw_protocol::FileDiff::fallback_new(path, content)
                        });
                        let row = ensure_working_row(
                            &overlay_store,
                            &active_turn_row,
                            &id,
                            agent_name,
                            character_id,
                        );
                        if *auto_approve {
                            // Fold completed auto-approved edit directly into the turn's `+ N edits` trace.
                            row.push_trace_item(TurnTraceItem::Edit {
                                diff: resolved_diff,
                                auto_approved: true,
                            });
                            overlay.nudge_sticky_scroll();
                        } else {
                            // Display the expanded diff inside the active turn row while awaiting approval;
                            // once approved, `TurnTraceProgress` folds it into `+ N edits`.
                            row.set_working(false);
                            row.set_pending_approval_diff(Some(&resolved_diff));
                            overlay.show(
                                OverlayMode::Claw,
                                agent_name,
                                Some("Propose File Edit"),
                                "",
                                crate::Proposal::FileWrite {
                                    path: path.clone(),
                                    content: content.clone(),
                                    diff: Some(resolved_diff),
                                },
                            );
                        }
                    }
                }
                ClawEngineEvent::ProposeFileDelete {
                    path,
                    agent_name,
                    character_id,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    if !ignoring_aborted_turn.get() {
                        if let Some(ref row) = *active_turn_row.borrow() {
                            row.set_working(false);
                            row.set_active_status(Some(format!("Confirm deleting `{path}`…")));
                        } else {
                            boxxy_claw_ui::add_file_delete_approval_row(
                                &overlay_store,
                                id.clone(),
                                Some(agent_name.clone()),
                                Some(character_id.clone()),
                                path,
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
                }
                ClawEngineEvent::ProposeKillProcess {
                    pid,
                    process_name,
                    agent_name,
                    character_id,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    if !ignoring_aborted_turn.get() {
                        if let Some(ref row) = *active_turn_row.borrow() {
                            row.set_working(false);
                            row.set_active_status(Some(format!(
                                "Confirm killing {process_name} (PID {pid})…"
                            )));
                        } else {
                            boxxy_claw_ui::add_kill_process_approval_row(
                                &overlay_store,
                                id.clone(),
                                Some(agent_name.clone()),
                                Some(character_id.clone()),
                                *pid,
                                process_name,
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
                }
                ClawEngineEvent::ProposeBackgroundCommand {
                    command,
                    explanation,
                    agent_name,
                    character_id,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    if !ignoring_aborted_turn.get() {
                        if let Some(ref row) = *active_turn_row.borrow() {
                            row.set_working(false);
                            row.set_active_status(Some(format!("Confirm launching `{command}`…")));
                        } else {
                            boxxy_claw_ui::add_approval_row(
                                &overlay_store,
                                id.clone(),
                                Some(agent_name.clone()),
                                Some(character_id.clone()),
                                command,
                            );
                        }
                        overlay.show(
                            OverlayMode::Claw,
                            agent_name,
                            Some("Background Command"),
                            &format!("{explanation}\n\n`{command}`"),
                            crate::Proposal::BackgroundCommand {
                                command: command.clone(),
                                explanation: explanation.clone(),
                            },
                        );
                    }
                }
                ClawEngineEvent::ProposeGetClipboard {
                    agent_name,
                    character_id,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    if !ignoring_aborted_turn.get() {
                        if let Some(ref row) = *active_turn_row.borrow() {
                            row.set_working(false);
                            row.set_active_status(Some("Confirm clipboard read…".to_string()));
                        } else {
                            boxxy_claw_ui::add_clipboard_get_approval_row(
                                &overlay_store,
                                id.clone(),
                                Some(agent_name.clone()),
                                Some(character_id.clone()),
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
                }
                ClawEngineEvent::ProposeSetClipboard {
                    agent_name,
                    character_id,
                    text,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    if !ignoring_aborted_turn.get() {
                        if let Some(ref row) = *active_turn_row.borrow() {
                            row.set_working(false);
                            row.set_active_status(Some("Confirm clipboard write…".to_string()));
                        } else {
                            boxxy_claw_ui::add_clipboard_set_approval_row(
                                &overlay_store,
                                id.clone(),
                                Some(agent_name.clone()),
                                Some(character_id.clone()),
                                text,
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
                }
                ClawEngineEvent::ProposeTerminalCommand {
                    command,
                    explanation,
                    agent_name,
                    character_id,
                    target_pane_id,
                    target_pane_label,
                    auto_approve,
                    usage,
                } => {
                    if let Some(usage) = usage {
                        record_usage(&session_usage, usage);
                    }
                    if !ignoring_aborted_turn.get() {
                        let row = ensure_working_row(
                            &overlay_store,
                            &active_turn_row,
                            &id,
                            agent_name,
                            character_id,
                        );
                        if *auto_approve {
                            row.set_active_status(Some(format!("Running `{command}`…")));
                            if let Some(target_id) = target_pane_id
                                .as_deref()
                                .filter(|tid| !tid.is_empty() && *tid != id.as_str())
                            {
                                host.inject_line_to_pane(target_id, command.clone());
                                host.send_claw(ClawMessage::TerminalCommandAccepted {
                                    target_pane_id: Some(target_id.to_string()),
                                });
                                overlay.enter_waiting_state();
                            } else {
                                host.inject_line(command.clone());
                                host.send_claw(ClawMessage::TerminalCommandAccepted {
                                    target_pane_id: None,
                                });
                                overlay.hide();
                                host.grab_focus();
                            }
                        } else {
                            row.set_working(false);
                            if !explanation.is_empty() {
                                row.set_streaming_prose(explanation);
                            }
                            overlay.show(
                                OverlayMode::Claw,
                                agent_name,
                                Some("Terminal Command"),
                                explanation,
                                crate::Proposal::Command {
                                    command: command.clone(),
                                    target_pane_id: target_pane_id.clone(),
                                    target_pane_label: target_pane_label.clone(),
                                },
                            );
                        }
                    }
                }
                ClawEngineEvent::Identity {
                    agent_name: name,
                    character_id,
                    pinned,
                    web_search_enabled,
                    vibe_enabled,
                    total_tokens: total,
                    context_tokens,
                    context_window,
                    total_cost_usd,
                } => {
                    overlay.set_active_character(name, character_id);
                    indicator.set_identity(name, character_id);
                    indicator.set_vibe(*vibe_enabled);
                    is_pinned.set(*pinned);
                    is_web_search.set(*web_search_enabled);
                    is_vibe.set(*vibe_enabled);
                    agent_name.borrow_mut().clone_from(name);

                    let status = {
                        let mut st = session_status.borrow_mut();
                        if *st == AgentStatus::Off {
                            *st = AgentStatus::Waiting;
                        }
                        st.clone()
                    };
                    msg_bar.set_character(character_id);
                    msg_bar.update_ui(status, *pinned, *web_search_enabled, *vibe_enabled);
                    let prev_tools =
                        if *total == 0 && *context_tokens == 0 && *total_cost_usd == 0.0 {
                            0
                        } else {
                            session_usage.get().tools_used
                        };
                    session_usage.set(SessionUsage {
                        context_tokens: *context_tokens,
                        context_window: *context_window,
                        total_tokens: *total,
                        cost_usd: *total_cost_usd,
                        tools_used: prev_tools,
                    });
                    if overlay.is_visible() {
                        overlay.grab_input_focus();
                    }
                }
                ClawEngineEvent::PinStatusChanged(pinned) => {
                    is_pinned.set(*pinned);
                    let status = session_status.borrow().clone();
                    msg_bar.update_ui(
                        status,
                        *pinned,
                        msg_bar.web_search_state.get(),
                        msg_bar.vibe_state.get(),
                    );
                }
                ClawEngineEvent::WebSearchStatusChanged(enabled) => {
                    is_web_search.set(*enabled);
                    let status = session_status.borrow().clone();
                    msg_bar.update_ui(
                        status,
                        msg_bar.pin_state.get(),
                        *enabled,
                        msg_bar.vibe_state.get(),
                    );
                }
                ClawEngineEvent::VibeStatusChanged(enabled) => {
                    is_vibe.set(*enabled);
                    indicator.set_vibe(*enabled);
                    let status = session_status.borrow().clone();
                    msg_bar.update_ui(
                        status,
                        msg_bar.pin_state.get(),
                        msg_bar.web_search_state.get(),
                        *enabled,
                    );
                    overlay.refresh_character_selector("");
                }
                ClawEngineEvent::Evicted => {
                    ignoring_aborted_turn.set(false);
                    settle_or_prune_working_row(&overlay_store, &active_turn_row);
                    *pending_tasks.borrow_mut() = Vec::new();
                    indicator.set_vibe(false);
                    indicator.set_evicted(true);
                    indicator.hide();
                    overlay.set_indicator_slot_visible(false);
                    overlay.hide();
                    boxxy_claw_ui::add_diagnosis_row(
                        &overlay_store,
                        id.clone(),
                        None,
                        None,
                        "Agent was EVICTED because the session was resumed in another pane.",
                    );
                }
                ClawEngineEvent::RequestCwdSwitch { path } => {
                    host.cd(path.clone());
                }
                ClawEngineEvent::SystemMessage { text } => {
                    boxxy_claw_ui::add_system_message_row(&overlay_store, id.clone(), text);
                }
                ClawEngineEvent::RequestScrollback {
                    max_lines,
                    offset_lines,
                    request_id,
                    target_pane_id,
                    ..
                } => {
                    indicator.set_busy_hidden(host.is_busy());

                    let max_lines = *max_lines;
                    let offset_lines = *offset_lines;
                    let request_id = *request_id;
                    let target_pane = target_pane_id
                        .clone()
                        .filter(|tid| !tid.is_empty() && tid != &id);
                    let snapshot = host.snapshot_pane(target_pane, max_lines, offset_lines);
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
                ClawEngineEvent::ProposalResolved { approved, .. } => {
                    if *approved {
                        ignoring_aborted_turn.set(false);
                        if let Some(ref row) = *active_turn_row.borrow() {
                            row.set_pending_approval_diff(None);
                            row.set_working(true);
                        }
                    } else {
                        settle_or_prune_working_row(&overlay_store, &active_turn_row);
                    }
                    overlay.set_state(OverlayState::Idle);
                }
                ClawEngineEvent::RequestSpawnAgent { .. }
                | ClawEngineEvent::RequestCloseAgent { .. }
                | ClawEngineEvent::RequestReattachShell { .. }
                | ClawEngineEvent::InjectKeystrokes { .. }
                | ClawEngineEvent::TaskCompleted { .. }
                | ClawEngineEvent::LongTaskCompleted { .. }
                | ClawEngineEvent::PushGlobalNotification { .. }
                | ClawEngineEvent::DelegatedTaskReply { .. } => {}
                ClawEngineEvent::ToolResult {
                    tool_name,
                    result,
                    usage,
                    ..
                } => {
                    let mut su = session_usage.get();
                    su.tools_used = su.tools_used.saturating_add(1);
                    if let Some(usage) = usage {
                        su.record_turn(usage);
                    }
                    session_usage.set(su);
                    if tool_name == "run_background_command" {
                        let notice = serde_json::from_str::<serde_json::Value>(result)
                            .ok()
                            .and_then(|v| {
                                let ok = v
                                    .get("success")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false);
                                if ok {
                                    v.get("message")
                                        .and_then(|m| m.as_str())
                                        .map(|m| format!("⚙ {m}"))
                                } else {
                                    v.get("message")
                                        .and_then(|m| m.as_str())
                                        .map(|m| format!("✗ {m}"))
                                }
                            });
                        if let Some(msg) = notice {
                            boxxy_claw_ui::add_system_message_row(&overlay_store, id.clone(), &msg);
                        }

                        if matches!(*overlay.state().borrow(), OverlayState::Pending) {
                            overlay.set_state(OverlayState::Idle);
                        }
                    }
                }
                ClawEngineEvent::TaskStatusChanged { tasks, .. } => {
                    pending_tasks.borrow_mut().clone_from(tasks);
                    let has_pending = tasks
                        .iter()
                        .any(|t| t.status == boxxy_claw_protocol::TaskStatus::Pending);
                    indicator.set_has_tasks(has_pending);
                }
                ClawEngineEvent::RestoreHistory(rows) => {
                    ignoring_aborted_turn.set(false);
                    *active_turn_row.borrow_mut() = None;
                    let mut restored_tools: u64 = 0;
                    for row in rows {
                        if matches!(
                            row,
                            boxxy_claw_protocol::PersistentClawRow::ToolCall { .. }
                                | boxxy_claw_protocol::PersistentClawRow::ProcessList { .. }
                        ) {
                            restored_tools = restored_tools.saturating_add(1);
                        }
                    }
                    let coalesced = boxxy_claw_protocol::coalesce_restored_history(rows.clone());
                    let mut overlay_items = Vec::with_capacity(coalesced.len());
                    for row in coalesced {
                        if row.is_overlay_visible() {
                            overlay_items.push(ClawRowObject::new(row));
                        }
                    }
                    let mut su = session_usage.get();
                    su.tools_used = restored_tools;
                    session_usage.set(su);
                    overlay_store.splice(0, overlay_store.n_items(), &overlay_items);
                    if !rows.is_empty() && overlay.is_visible() {
                        overlay.grab_input_focus();
                    }
                }
            }

            host.forward_event(event);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_claw_protocol::TraceCategory;

    #[test]
    fn settle_or_prune_removes_empty_or_redacted_working_rows_and_settles_non_empty() {
        let store = gtk::gio::ListStore::new::<ClawRowObject>();
        let slot: RefCell<Option<ClawRowObject>> = RefCell::new(None);

        // 1. Working row with active status + empty reasoning block is pruned completely on abort.
        let r1 = ensure_working_row(&store, &slot, "pane-1", "AgentA", "char-1");
        r1.set_active_status(Some("Preparing edit...".to_string()));
        r1.push_trace_item(TurnTraceItem::Thought {
            summary: None,
            content: String::new(),
            duration_ms: Some(50),
        });
        assert_eq!(store.n_items(), 1);
        assert!(r1.is_working());

        settle_or_prune_working_row(&store, &slot);
        assert_eq!(store.n_items(), 0);
        assert!(slot.borrow().is_none());

        // 2. Working row with a completed visible tool step settles (`is_working == false`,
        //    `active_status == None`) and remains in the store.
        let r2 = ensure_working_row(&store, &slot, "pane-1", "AgentA", "char-1");
        r2.set_active_status(Some("Reading file...".to_string()));
        r2.push_trace_item(TurnTraceItem::Tool {
            tool_name: "file_read".to_string(),
            category: TraceCategory::Read,
            label: "Read src/module.rs".to_string(),
            preview: None,
            failed: false,
        });
        assert_eq!(store.n_items(), 1);

        settle_or_prune_working_row(&store, &slot);
        assert_eq!(store.n_items(), 1);
        assert!(!r2.is_working());
        assert_eq!(r2.active_status(), None);
    }
}
