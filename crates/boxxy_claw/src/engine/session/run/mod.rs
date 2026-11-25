use std::sync::Arc;

use boxxy_db::Db;
use log::info;

use crate::engine::session::ClawSession;
use crate::engine::{ClawEngineEvent, ClawEnvironment, ClawMessage, context::load_session_context};

pub mod context;
mod proposals;
mod tasks;
mod user;

use self::context::RunContext;

impl ClawSession {
    pub async fn run(mut self, env: Arc<dyn ClawEnvironment>) {
        let workspace = crate::registry::workspace::global_workspace().await;
        let mut ctx = RunContext::new(env, workspace);
        let mut task_interval = tokio::time::interval(std::time::Duration::from_secs(1));

        loop {
            tokio::select! {
                msg_opt = Self::get_next_msg(
                    &self.rx,
                    &mut ctx.urgent_backlog,
                    &mut ctx.backlog,
                    ctx.current_turn.is_some(),
                ) => {
                    let Some(msg) = msg_opt else {
                        break;
                    };

                    self.ensure_initialized(&msg, &mut ctx.is_initialized).await;
                    if self.dispatch_message(msg, &mut ctx).await {
                        break;
                    }
                }
                _ = task_interval.tick() => {
                    self.tick_scheduled_tasks(&ctx).await;
                }
            }
        }

        ctx.workspace.unregister_pane(self.pane_id).await;
    }

    /// Processes a single actor message. Returns `true` if the actor loop should exit
    /// (used to automatically reap transient headless workers once their task completes).
    #[allow(clippy::too_many_lines)]
    async fn dispatch_message(&mut self, msg: ClawMessage, ctx: &mut RunContext) -> bool {
        let session_ctx = self.session_context.clone();

        match msg {
            ClawMessage::SettingsInvalidated => {
                self.handle_settings_invalidated().await;
            }
            ClawMessage::Abort => {
                self.handle_abort(ctx).await;
            }
            ClawMessage::TurnFinished => {
                ctx.current_turn = None;

                let (paused_by_turn, is_sleeping_now) = {
                    let mut state_lock = self.state.lock().await;
                    (
                        std::mem::take(&mut state_lock.last_turn_paused_queue),
                        state_lock.status == crate::engine::AgentStatus::Sleep,
                    )
                };

                if (paused_by_turn || is_sleeping_now)
                    && !ctx.queued_prompts.is_empty()
                    && !ctx.queue_paused
                {
                    ctx.queue_paused = true;
                    self.emit_queue_updated(ctx).await;
                }

                // Apply any in-flight queue removals before deciding what runs next so a click
                // on Remove (`×`) that arrived alongside `TurnFinished` always takes effect.
                let mut idx = 0;
                while idx < ctx.urgent_backlog.len() {
                    match &ctx.urgent_backlog[idx] {
                        ClawMessage::RemoveQueuedPrompt { .. }
                        | ClawMessage::ClearQueuedPrompts => {
                            if let Some(q_msg) = ctx.urgent_backlog.remove(idx) {
                                match q_msg {
                                    ClawMessage::RemoveQueuedPrompt {
                                        id,
                                        recall_to_input,
                                    } => {
                                        self.handle_remove_queued_prompt(ctx, id, recall_to_input)
                                            .await;
                                    }
                                    ClawMessage::ClearQueuedPrompts => {
                                        self.handle_clear_queued_prompts(ctx).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => idx += 1,
                    }
                }

                // If an explicit session/turn interrupt is already waiting in `urgent_backlog`,
                // let the main loop process it first without auto-starting queued work.
                let has_pending_interrupt = ctx.urgent_backlog.iter().any(|m| {
                    matches!(
                        m,
                        ClawMessage::Abort
                            | ClawMessage::CancelPending
                            | ClawMessage::Deactivate
                            | ClawMessage::Evict
                            | ClawMessage::SleepToggle(true)
                    )
                });
                if has_pending_interrupt {
                    {
                        let mut state_lock = self.state.lock().await;
                        state_lock.next_roundtable_turn = None;
                        state_lock.active_speaker = None;
                    }
                    if !ctx.queued_prompts.is_empty() && !ctx.queue_paused {
                        ctx.queue_paused = true;
                        self.emit_queue_updated(ctx).await;
                    }
                    return false;
                }

                // Drain the next queued user follow-up prompt in FIFO order before any
                // autonomous roundtable handoff so user follow-ups execute right after the turn.
                // Emitting `execute_dequeued_prompt` (`UserMessage`) before `emit_queue_updated`
                // lets the UI graduate the queued User row in-place with zero remove+insert flicker.
                if !ctx.queue_paused
                    && !is_sleeping_now
                    && let Some(mut next_queued) = ctx.queued_prompts.pop_front()
                {
                    if !self.incognito {
                        if let Some(latest_snap) =
                            ctx.workspace.get_pane_snapshot(self.pane_id.clone()).await
                        {
                            next_queued.snapshot = latest_snap;
                        }
                        if !ctx.current_dir.is_empty() && ctx.current_dir != "/" {
                            next_queued.cwd.clone_from(&ctx.current_dir);
                        }
                    }
                    self.execute_dequeued_prompt(ctx, &session_ctx, next_queued)
                        .await;
                    self.emit_queue_updated(ctx).await;
                    return false;
                }

                // Check if a multi-character roundtable / debate handoff is queued
                let next_roundtable = {
                    let mut state_lock = self.state.lock().await;
                    if state_lock.status == crate::engine::AgentStatus::Sleep {
                        state_lock.next_roundtable_turn = None;
                        None
                    } else {
                        state_lock.next_roundtable_turn.take()
                    }
                };

                if let Some((next_speaker, handoff_prompt)) = next_roundtable {
                    {
                        let mut state_lock = self.state.lock().await;
                        state_lock.active_speaker = Some(next_speaker);
                    }
                    let snapshot = ctx
                        .workspace
                        .get_pane_snapshot(self.pane_id.clone())
                        .await
                        .unwrap_or_default();
                    ctx.current_turn = Some(crate::engine::turn::spawn_turn(
                        self.pane_id.clone(),
                        self.session_id.clone(),
                        self.name.clone(),
                        &handoff_prompt,
                        &snapshot,
                        &session_ctx,
                        ctx.current_dir.clone(),
                        false,
                        ctx.env.clone(),
                        self.db.clone(),
                        self.state.clone(),
                        self.tx_ui.clone(),
                        self.tx_self.clone(),
                        None,
                        vec![],
                        vec![],
                    ));
                    return false;
                }

                // Automatically reap transient headless workers once their delegated turn finishes
                // and no further backlog items remain.
                let (is_sleeping, is_headless) = {
                    let state_lock = self.state.lock().await;
                    (
                        state_lock.status == crate::engine::AgentStatus::Sleep,
                        state_lock.is_headless,
                    )
                };
                if is_headless && ctx.backlog.is_empty() && !is_sleeping {
                    return true;
                }

                // Preserve Sleep status if a tool in the turn (`subscribe_to_pane`, `await_tasks`)
                // already suspended the agent, or Locking status if the session still holds a lock.
                if !is_sleeping {
                    let target_state = match ctx.workspace.held_lock_for_pane(&self.pane_id).await {
                        Some(resource) => crate::engine::AgentStatus::Locking { resource },
                        None => crate::engine::AgentStatus::Waiting,
                    };
                    let req = crate::engine::TransitionRequest {
                        target_state,
                        source: crate::engine::TriggerSource::System,
                    };
                    self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                        .await;
                }

                if let Some(next_msg) = ctx.backlog.pop_front() {
                    let _ = self.tx_self.send(next_msg).await;
                }
            }
            ClawMessage::WakeSummaryComplete { result } => {
                let mut state_lock = self.state.lock().await;
                state_lock.status = crate::engine::AgentStatus::Waiting;

                match result {
                    Ok(summary) => {
                        state_lock.context_quality = crate::engine::ContextQuality::Full;
                        let trimmed = summary.trim();
                        if !trimmed.is_empty() {
                            state_lock.history.push(rig::message::Message::user(format!(
                                "[WHILE_YOU_SLEPT]\n{trimmed}"
                            )));
                            state_lock.history.push(rig::message::Message::assistant(
                                crate::engine::turn::history::WHILE_YOU_SLEPT_ACK,
                            ));
                        }
                    }
                    Err(e) => {
                        log::warn!("Dreamer fallback triggered: {e}");
                        state_lock.context_quality = crate::engine::ContextQuality::Degraded;
                    }
                }

                let current_quality = state_lock.context_quality.clone();
                let agent_name = self.name.clone();
                drop(state_lock);

                let character_id = self.character_id.clone();
                let _ = self
                    .tx_ui
                    .send(crate::engine::ClawEngineEvent::SessionStateChanged {
                        agent_name,
                        character_id,
                        status: crate::engine::AgentStatus::Waiting,
                    })
                    .await;

                ctx.workspace
                    .set_pane_quality(self.pane_id.clone(), Some(current_quality))
                    .await;

                let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
            }
            ClawMessage::Transition(req) => {
                self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                    .await;
            }
            ClawMessage::WatchdogTimeout { task_id, state } => {
                let state_lock = self.state.lock().await;
                if state_lock.status == state {
                    drop(state_lock);
                    log::warn!("Watchdog expired for task {task_id:?}. Forcing Faulted state.");
                    self.handle_transition(
                        crate::engine::TransitionRequest {
                            target_state: crate::engine::AgentStatus::Faulted {
                                reason: "Watchdog Timeout".to_string(),
                            },
                            source: crate::engine::TriggerSource::System,
                        },
                        &mut ctx.current_turn,
                        &mut ctx.urgent_backlog,
                    )
                    .await;
                }
            }
            ClawMessage::SleepToggle(sleep) => {
                let target_state = if sleep {
                    crate::engine::AgentStatus::Sleep
                } else {
                    crate::engine::AgentStatus::Waiting
                };
                let req = crate::engine::TransitionRequest {
                    target_state,
                    source: crate::engine::TriggerSource::User,
                };
                self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                    .await;
            }
            ClawMessage::Deactivate => {
                info!("Deactivating Claw Session for pane {}...", self.pane_id);
                ctx.abort_btw_tasks(None);
                self.handle_clear_queued_prompts(ctx).await;
                self.handle_abort(ctx).await;
                self.flush_unflushed_history(&ctx.current_dir).await;
                ctx.is_initialized = false;
                *self.db.lock().await = None;

                let mut state_lock = self.state.lock().await;
                state_lock.reset_ephemeral_session_context();
                state_lock.history.clear();
                state_lock.flushed_watermark = 0;
                state_lock.pending_tasks.clear();
                drop(state_lock);

                self.clear_ui_history().await;
                ctx.workspace
                    .update_pane_tasks(self.pane_id.clone(), Vec::new())
                    .await;
                ctx.workspace.unregister_pane(self.pane_id.clone()).await;
            }
            ClawMessage::Evict => {
                info!("Agent in pane {} was EVICTED.", self.pane_id);
                ctx.abort_btw_tasks(None);
                self.handle_clear_queued_prompts(ctx).await;
                self.handle_abort(ctx).await;
                self.flush_unflushed_history(&ctx.current_dir).await;
                ctx.is_initialized = false;
                *self.db.lock().await = None;

                let mut state_lock = self.state.lock().await;
                state_lock.reset_ephemeral_session_context();
                state_lock.history.clear();
                state_lock.flushed_watermark = 0;
                state_lock.pending_tasks.clear();
                drop(state_lock);

                self.clear_ui_history().await;
                let _ = self.tx_ui.send(ClawEngineEvent::Evicted).await;
                ctx.workspace
                    .update_pane_tasks(self.pane_id.clone(), Vec::new())
                    .await;
                ctx.workspace.unregister_pane(self.pane_id.clone()).await;
            }
            ClawMessage::ResumeSession {
                session_id,
                character_id,
            } => {
                ctx.abort_btw_tasks(None);
                self.handle_clear_queued_prompts(ctx).await;
                self.handle_resume_session(
                    session_id,
                    character_id,
                    &ctx.current_dir,
                    &mut ctx.is_initialized,
                )
                .await;
            }
            ClawMessage::Initialize => {
                self.handle_initialize(&ctx.current_dir).await;
            }
            ClawMessage::Reload => {
                info!("Reloading Claw Session state...");
                let new_ctx = load_session_context(&self.character_id);
                self.session_context = new_ctx;
                if let Ok(db) = Db::new().await {
                    *self.db.lock().await = Some(db);
                }
            }
            ClawMessage::TogglePin(pinned) => {
                self.pinned = pinned;
                let mut state_lock = self.state.lock().await;
                state_lock.pinned = pinned;

                let history_json = serde_json::to_string(&state_lock.history).unwrap_or_default();
                let pending_tasks_json =
                    serde_json::to_string(&state_lock.pending_tasks).unwrap_or_default();
                let agent_name_for_db = state_lock.agent_name.clone();
                let session_id_for_db = self.session_id.clone();
                let cwd_for_db = ctx.current_dir.clone();
                let total_tokens_for_db = state_lock.total_tokens as i64;
                let context_tokens_for_db = state_lock.context_tokens as i64;
                let total_cost_for_db = state_lock.total_cost_usd;
                let settings = boxxy_preferences::Settings::load();
                let model_id = settings
                    .resolve_character_model(&self.character_id)
                    .as_ref()
                    .map(|m| format!("{m:?}"))
                    .unwrap_or_default();

                let db_for_persistence = self.db.clone();
                let character_id_for_db = self.character_id.clone();
                let character_display_name_for_db = self.name.clone();
                tokio::spawn(async move {
                    let db_guard = db_for_persistence.lock().await;
                    if let Some(db) = &*db_guard {
                        let store = boxxy_db::store::Store::new(db.pool());
                        let _ = store
                            .upsert_session_state(
                                &session_id_for_db,
                                &agent_name_for_db,
                                &character_id_for_db,
                                &character_display_name_for_db,
                                "",
                                &history_json,
                                &pending_tasks_json,
                                &agent_name_for_db,
                                &cwd_for_db,
                                &model_id,
                                pinned,
                                total_tokens_for_db,
                                context_tokens_for_db,
                                total_cost_for_db,
                            )
                            .await;
                    }
                });
                drop(state_lock);
                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::PinStatusChanged(pinned))
                    .await;
            }
            ClawMessage::ToggleWebSearch(enabled) => {
                let mut state_lock = self.state.lock().await;
                if state_lock.web_search_enabled != enabled {
                    state_lock.web_search_enabled = enabled;
                    state_lock.persistent_agent = None;
                    let settings = boxxy_preferences::Settings::load();
                    let resolved_claw = if self.incognito {
                        settings.claw_model.clone()
                    } else {
                        settings.resolve_character_model(&self.character_id)
                    };
                    state_lock.current_search_label =
                        crate::engine::agent_config::resolve_active_search_label_for_model(
                            &settings,
                            &resolved_claw,
                            enabled,
                        )
                        .to_string();
                    drop(state_lock);
                    let _ = self
                        .tx_ui
                        .send(ClawEngineEvent::WebSearchStatusChanged(enabled))
                        .await;
                }
            }
            ClawMessage::ToggleVibe(enabled) => {
                if enabled {
                    boxxy_preferences::Settings::reload();
                }
                let mut state_lock = self.state.lock().await;
                if state_lock.vibe_enabled != enabled {
                    state_lock.vibe_enabled = enabled;
                    let _ = self
                        .tx_ui
                        .send(ClawEngineEvent::VibeStatusChanged(enabled))
                        .await;
                }
            }
            ClawMessage::ForegroundProcessChanged { process_name } => {
                let status = if process_name.is_empty() {
                    None
                } else {
                    Some(format!("Running: {process_name}"))
                };
                ctx.workspace
                    .set_pane_status(self.pane_id.clone(), status)
                    .await;
            }
            ClawMessage::CommandFinished {
                exit_code,
                snapshot,
                cwd,
            } => {
                self.handle_command_finished(
                    exit_code,
                    snapshot,
                    cwd,
                    &session_ctx,
                    &ctx.env,
                    &mut ctx.current_dir,
                    &mut ctx.current_turn,
                    &mut ctx.backlog,
                )
                .await;
            }
            ClawMessage::RequestLazyDiagnosis => {
                self.handle_request_lazy_diagnosis(
                    &session_ctx,
                    &ctx.env,
                    &mut ctx.current_turn,
                    &mut ctx.backlog,
                )
                .await;
            }
            ClawMessage::ClawQuery {
                prompt_id,
                query,
                snapshot,
                cwd,
                image_attachments,
                image_attachment_meta,
            } => {
                self.handle_claw_query(
                    ctx,
                    &session_ctx,
                    prompt_id,
                    query,
                    snapshot,
                    cwd,
                    image_attachments,
                    image_attachment_meta,
                )
                .await;
            }
            ClawMessage::FileWriteReply { approved } => {
                if approved {
                    let mut state_lock = self.state.lock().await;
                    state_lock.pending_proposal_summary = None;
                    if let Some(reply) = state_lock.pending_file_reply.take() {
                        let _ = reply.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                } else {
                    self.handle_cancel_pending(ctx).await;
                }
            }
            ClawMessage::FileDeleteReply { approved } => {
                if approved {
                    let mut state_lock = self.state.lock().await;
                    state_lock.pending_proposal_summary = None;
                    if let Some(reply) = state_lock.pending_file_delete_reply.take() {
                        let _ = reply.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                } else {
                    self.handle_cancel_pending(ctx).await;
                }
            }
            ClawMessage::KillProcessReply { approved } => {
                if approved {
                    let mut state_lock = self.state.lock().await;
                    state_lock.pending_proposal_summary = None;
                    if let Some(reply) = state_lock.pending_kill_process_reply.take() {
                        let _ = reply.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                } else {
                    self.handle_cancel_pending(ctx).await;
                }
            }
            ClawMessage::BackgroundCommandReply { approved } => {
                if approved {
                    let mut state_lock = self.state.lock().await;
                    state_lock.pending_proposal_summary = None;
                    if let Some(reply) = state_lock.pending_background_reply.take() {
                        let _ = reply.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                } else {
                    self.handle_cancel_pending(ctx).await;
                }
            }
            ClawMessage::TerminalCommandAccepted { target_pane_id } => {
                self.state.lock().await.terminal_command_accepted = true;
                let effective_target = target_pane_id
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| self.pane_id.clone());
                let is_remote = effective_target != self.pane_id;
                let state = self.state.clone();
                let workspace = ctx.workspace.clone();
                let tx_ui = self.tx_ui.clone();
                let tx_self = self.tx_self.clone();
                let agent_name = self.name.clone();
                let char_id = self.character_id.clone();

                tokio::spawn(async move {
                    // Wait 800ms (past the daemon's 500ms foreground-process poll tick).
                    // If the command hasn't emitted OSC 133;D yet AND the target pane is still
                    // running a foreground process (e.g. `top`, `htop`, `vim`, `cachy-update`),
                    // resolve the command as launched and running.
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

                    if is_remote {
                        if !workspace.has_pending_command(&effective_target).await {
                            return;
                        }
                        let fg_status = workspace.get_pane_status(&effective_target).await;
                        let is_running = fg_status
                            .as_deref()
                            .is_some_and(|s| s.starts_with("Running: "));
                        let proc_name = fg_status
                            .as_deref()
                            .and_then(|s| s.strip_prefix("Running: "))
                            .unwrap_or("command")
                            .to_string();

                        // Capture a fresh live snapshot of the target pane to include in the result.
                        let req_id = uuid::Uuid::new_v4();
                        let (snap_tx, snap_rx) = tokio::sync::oneshot::channel();
                        state
                            .lock()
                            .await
                            .pending_scrollbacks
                            .insert(req_id, snap_tx);
                        let _ = tx_ui
                            .send(ClawEngineEvent::RequestScrollback {
                                agent_name,
                                character_id: char_id,
                                target_pane_id: Some(effective_target.clone()),
                                max_lines: 50,
                                offset_lines: 0,
                                request_id: req_id,
                            })
                            .await;
                        let live_snap =
                            tokio::time::timeout(std::time::Duration::from_millis(500), snap_rx)
                                .await
                                .ok()
                                .and_then(std::result::Result::ok)
                                .unwrap_or_default();
                        let live_snap = live_snap.trim();
                        state.lock().await.pending_scrollbacks.remove(&req_id);

                        let alias = workspace
                            .get_pane_alias(&effective_target)
                            .await
                            .unwrap_or_else(|| effective_target.clone());
                        let msg = if is_running {
                            if live_snap.is_empty() {
                                format!(
                                    "Command launched and is currently running (`{proc_name}`) in `{alias}`. Use `read_pane_buffer(agent_name=\"{alias}\")`, `send_keystrokes_to_pane(agent_name=\"{alias}\", keys=...)`, or `subscribe_to_pane(agent_name=\"{alias}\", event_type=\"process_exit\")` if you need to interact with or wait for it."
                                )
                            } else {
                                format!(
                                    "Command launched and is currently running (`{proc_name}`) in `{alias}`.\nLive snapshot:\n{live_snap}"
                                )
                            }
                        } else if !live_snap.is_empty() {
                            format!("Command launched in `{alias}`.\nLive snapshot:\n{live_snap}")
                        } else {
                            format!("Command launched in `{alias}`.")
                        };

                        let _ = workspace
                            .complete_pending_command(&effective_target, 0, msg)
                            .await;
                    } else {
                        let has_local_pending = state.lock().await.pending_terminal_reply.is_some();
                        if !has_local_pending {
                            return;
                        }
                        let fg_status = workspace.get_pane_status(&effective_target).await;
                        if let Some(proc_name) = fg_status
                            .as_deref()
                            .and_then(|s| s.strip_prefix("Running: "))
                        {
                            // Yield control directly to the user's terminal without making a
                            // redundant follow-up LLM call that would re-open the overlay drawer
                            // over an interactive command (e.g. `cachy-update`, `pacman`, `vim`).
                            let _ = tx_self
                                .send(ClawMessage::CompleteRunningTerminalCommand {
                                    process_name: proc_name.to_string(),
                                })
                                .await;
                        }
                    }
                });
            }
            ClawMessage::CompleteRunningTerminalCommand { process_name } => {
                let should_complete = self.state.lock().await.pending_terminal_reply.is_some();
                if !should_complete {
                    return false;
                }

                if let Some(handle) = ctx.current_turn.take() {
                    handle.abort();
                }

                let (speaker_name, speaker_cid) = {
                    let mut state_lock = self.state.lock().await;
                    let (spk_name, spk_cid) = state_lock.effective_speaker();
                    let _ = state_lock.pending_terminal_reply.take();
                    state_lock.terminal_command_accepted = false;
                    // Keep `pending_inject_command = true` so if this agent-originated command
                    // later exits with a non-zero status, origin-aware auto-diagnosis still runs.
                    state_lock.pending_inject_command = true;
                    state_lock.last_turn_paused_queue = true;
                    state_lock.active_speaker = None;

                    let summary = state_lock
                        .pending_proposal_summary
                        .take()
                        .unwrap_or_else(|| format!("terminal_exec `{process_name}`"));
                    let tagged = crate::engine::turn::history::tag_assistant_text(
                        &spk_cid,
                        &spk_name,
                        &format!(
                            "[Executed {summary} — command is running interactively in the terminal (`{process_name}`)]"
                        ),
                    );
                    state_lock
                        .history
                        .push(rig::message::Message::assistant(tagged));
                    (spk_name, spk_cid)
                };

                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: speaker_name,
                        character_id: speaker_cid,
                        is_thinking: false,
                    })
                    .await;

                let settings = boxxy_preferences::Settings::load();
                let p_ctx = crate::engine::turn::persistence::PersistenceContext {
                    session_id: self.session_id.clone(),
                    agent_name: self.name.clone(),
                    user_query: String::new(),
                    cwd: ctx.current_dir.clone(),
                    settings: (*settings).clone(),
                    db: self.db.clone(),
                    tx_self: self.tx_self.clone(),
                };
                crate::engine::turn::persistence::perform_persistence(p_ctx, self.state.clone())
                    .await;

                let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
            }
            ClawMessage::GetClipboardReply { approved } => {
                if approved {
                    let mut state_lock = self.state.lock().await;
                    state_lock.pending_proposal_summary = None;
                    if let Some(reply) = state_lock.pending_get_clipboard_reply.take() {
                        let _ = reply.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                } else {
                    self.handle_cancel_pending(ctx).await;
                }
            }
            ClawMessage::SetClipboardReply { approved } => {
                if approved {
                    let mut state_lock = self.state.lock().await;
                    state_lock.pending_proposal_summary = None;
                    if let Some(reply) = state_lock.pending_set_clipboard_reply.take() {
                        let _ = reply.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                } else {
                    self.handle_cancel_pending(ctx).await;
                }
            }
            ClawMessage::UserMessage {
                message,
                snapshot,
                cwd,
                image_attachments,
                image_attachment_meta,
            } => {
                self.handle_user_message(
                    ctx,
                    &session_ctx,
                    message,
                    snapshot,
                    cwd,
                    image_attachments,
                    image_attachment_meta,
                )
                .await;
            }
            ClawMessage::DelegatedTask {
                source_agent_name,
                prompt,
                request_id,
            } => {
                self.handle_delegated_task(ctx, source_agent_name, prompt, request_id)
                    .await;
            }
            ClawMessage::CancelPending => {
                self.handle_cancel_pending(ctx).await;
            }
            ClawMessage::SoftClearHistory => {
                info!(
                    "Soft-clearing Claw Session visual history for pane {}...",
                    self.pane_id
                );
                self.clear_ui_history().await;
            }
            ClawMessage::CancelTask { task_id } => {
                self.handle_cancel_task(ctx, task_id).await;
            }
            ClawMessage::TogglePauseTask { task_id } => {
                self.handle_toggle_pause_task(ctx, task_id).await;
            }
            ClawMessage::CancelAllTasks => {
                self.handle_cancel_all_tasks(ctx).await;
            }
            ClawMessage::ScheduledTaskTriggered { prompt } => {
                self.handle_scheduled_task_triggered(ctx, &session_ctx, prompt)
                    .await;
            }
            ClawMessage::SubscriptionEvent { event } => {
                self.handle_subscription_event(ctx, &session_ctx, event)
                    .await;
            }
            ClawMessage::ScrollbackReply {
                request_id,
                content,
            } => {
                let mut state = self.state.lock().await;
                if let Some(tx) = state.pending_scrollbacks.remove(&request_id) {
                    let _ = tx.send(content);
                }
            }
            ClawMessage::SpawnAgentReply {
                request_id,
                pane_id,
                alias,
            } => {
                let mut state = self.state.lock().await;
                if let Some(tx) = state.pending_spawns.remove(&request_id) {
                    let _ = tx.send((pane_id, alias));
                }
            }
            ClawMessage::TaskCompletedEvent { task_id, result } => {
                self.handle_task_completed_event(ctx, &session_ctx, task_id, result)
                    .await;
            }
            ClawMessage::RebindPane { new_pane_id } => {
                self.handle_rebind_pane(ctx, new_pane_id).await;
            }
            ClawMessage::BtwQuery {
                request_id,
                question,
                prior_exchange,
                snapshot,
                cwd,
            } => {
                if !self.incognito && self.db.lock().await.is_none()
                    && let Ok(db) = Db::new().await
                {
                    *self.db.lock().await = Some(db);
                }
                if !self.incognito && self.session_context.trim().is_empty() && !self.character_id.is_empty() {
                    self.session_context = load_session_context(&self.character_id);
                }
                ctx.active_btw_tasks.retain(|_, h| !h.is_finished());
                let handle = crate::engine::turn::btw::spawn_btw_query(
                    crate::engine::turn::btw::BtwQueryParams {
                        request_id,
                        pane_id: self.pane_id.clone(),
                        session_id: self.session_id.clone(),
                        host_agent_name: self.name.clone(),
                        question,
                        prior_exchange,
                        snapshot,
                        session_ctx: self.session_context.clone(),
                        cwd,
                        env: ctx.env.clone(),
                        db: self.db.clone(),
                        state: self.state.clone(),
                        tx_ui: self.tx_ui.clone(),
                    },
                );
                ctx.active_btw_tasks.insert(request_id, handle);
            }
            ClawMessage::CancelBtw { request_id } => {
                ctx.abort_btw_tasks(request_id);
            }
            ClawMessage::RemoveQueuedPrompt {
                id,
                recall_to_input,
            } => {
                self.handle_remove_queued_prompt(ctx, id, recall_to_input)
                    .await;
            }
            ClawMessage::ClearQueuedPrompts => {
                self.handle_clear_queued_prompts(ctx).await;
            }
            ClawMessage::SendQueuedPrompt { id, snapshot, cwd } => {
                self.handle_send_queued_prompt(ctx, &session_ctx, id, snapshot, cwd)
                    .await;
            }
        }
        false
    }
}
