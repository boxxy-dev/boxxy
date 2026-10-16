use std::sync::Arc;

use boxxy_db::Db;
use log::info;

use crate::engine::session::ClawSession;
use crate::engine::{
    ClawEngineEvent, ClawEnvironment, ClawMessage, context::load_session_context,
};

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
                    self.dispatch_message(msg, &mut ctx).await;
                }
                _ = task_interval.tick() => {
                    self.tick_scheduled_tasks(&mut ctx).await;
                }
            }
        }

        ctx.workspace.unregister_pane(self.pane_id).await;
    }

    async fn dispatch_message(&mut self, msg: ClawMessage, ctx: &mut RunContext) {
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

                let req = crate::engine::TransitionRequest {
                    target_state: crate::engine::AgentStatus::Waiting,
                    source: crate::engine::TriggerSource::System,
                };
                self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                    .await;

                if let Some(next_msg) = ctx.urgent_backlog.pop_front() {
                    let _ = self.tx_self.send(next_msg).await;
                } else if let Some(next_msg) = ctx.backlog.pop_front() {
                    let _ = self.tx_self.send(next_msg).await;
                }
            }
            ClawMessage::WakeSummaryComplete { result } => {
                let mut state_lock = self.state.lock().await;
                state_lock.status = crate::engine::AgentStatus::Waiting;

                match result {
                    Ok(summary) => {
                        state_lock.context_quality = crate::engine::ContextQuality::Full;
                        state_lock.history.push(rig::message::Message::user(format!(
                            "[WHILE_YOU_SLEPT]\n{}",
                            summary
                        )));
                    }
                    Err(e) => {
                        log::warn!("Dreamer fallback triggered: {}", e);
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
                    log::warn!(
                        "Watchdog expired for task {:?}. Forcing Faulted state.",
                        task_id
                    );
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
                let _ = self.tx_self.send(ClawMessage::Abort).await;
                self.flush_unflushed_history(&ctx.current_dir).await;
                ctx.is_initialized = false;
                *self.db.lock().await = None;

                let mut state_lock = self.state.lock().await;
                state_lock.history.clear();
                state_lock.flushed_watermark = 0;
                state_lock.pending_tasks.clear();
                drop(state_lock);

                self.clear_ui_history().await;
                ctx.workspace.unregister_pane(self.pane_id.clone()).await;
            }
            ClawMessage::Evict => {
                info!("Agent in pane {} was EVICTED.", self.pane_id);
                let _ = self.tx_self.send(ClawMessage::Abort).await;
                self.flush_unflushed_history(&ctx.current_dir).await;
                ctx.is_initialized = false;
                *self.db.lock().await = None;

                let mut state_lock = self.state.lock().await;
                state_lock.history.clear();
                state_lock.flushed_watermark = 0;
                state_lock.pending_tasks.clear();
                drop(state_lock);

                self.clear_ui_history().await;
                let _ = self.tx_ui.send(ClawEngineEvent::Evicted).await;
                ctx.workspace.unregister_pane(self.pane_id.clone()).await;
            }
            ClawMessage::ResumeSession { session_id } => {
                self.handle_resume_session(session_id, &ctx.current_dir, &mut ctx.is_initialized)
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
                let settings = boxxy_preferences::Settings::load();
                let model_id = settings
                    .claw_model
                    .as_ref()
                    .map(|m| format!("{:?}", m))
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
                    state_lock.current_search_label =
                        crate::engine::agent_config::resolve_active_search_label(
                            &settings,
                            enabled,
                        )
                        .to_string();
                    let _ = self
                        .tx_ui
                        .send(ClawEngineEvent::WebSearchStatusChanged(enabled))
                        .await;
                }
            }
            ClawMessage::ForegroundProcessChanged { process_name } => {
                let status = if process_name.is_empty() {
                    None
                } else {
                    Some(format!("Running: {}", process_name))
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
                query,
                snapshot,
                cwd,
                image_attachments,
            } => {
                self.handle_claw_query(ctx, &session_ctx, query, snapshot, cwd, image_attachments)
                    .await;
            }
            ClawMessage::FileWriteReply { approved } => {
                let mut state_lock = self.state.lock().await;
                if let Some(reply) = state_lock.pending_file_reply.take() {
                    let _ = reply.send(approved);
                }
            }
            ClawMessage::FileDeleteReply { approved } => {
                let mut state_lock = self.state.lock().await;
                if let Some(reply) = state_lock.pending_file_delete_reply.take() {
                    let _ = reply.send(approved);
                }
            }
            ClawMessage::KillProcessReply { approved } => {
                let mut state_lock = self.state.lock().await;
                if let Some(reply) = state_lock.pending_kill_process_reply.take() {
                    let _ = reply.send(approved);
                }
            }
            ClawMessage::BackgroundCommandReply { approved } => {
                let mut state_lock = self.state.lock().await;
                if let Some(reply) = state_lock.pending_background_reply.take() {
                    let _ = reply.send(approved);
                }
            }
            ClawMessage::GetClipboardReply { approved } => {
                let mut state_lock = self.state.lock().await;
                if let Some(reply) = state_lock.pending_get_clipboard_reply.take() {
                    let _ = reply.send(approved);
                }
            }
            ClawMessage::SetClipboardReply { approved } => {
                let mut state_lock = self.state.lock().await;
                if let Some(reply) = state_lock.pending_set_clipboard_reply.take() {
                    let _ = reply.send(approved);
                }
            }
            ClawMessage::UserMessage {
                message,
                snapshot,
                cwd,
                image_attachments,
            } => {
                self.handle_user_message(
                    ctx,
                    &session_ctx,
                    message,
                    snapshot,
                    cwd,
                    image_attachments,
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
                self.handle_cancel_pending().await;
            }
            ClawMessage::SoftClearHistory => {
                info!(
                    "Soft-clearing Claw Session visual history for pane {}...",
                    self.pane_id
                );
                let db_guard = self.db.lock().await;
                if let Some(db) = &*db_guard {
                    let store = boxxy_db::store::Store::new(db.pool());
                    let _ = store.mark_session_cleared(&self.session_id).await;
                }
                self.clear_ui_history().await;
            }
            ClawMessage::CancelTask { task_id } => {
                self.handle_cancel_task(ctx, task_id).await;
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
            ClawMessage::TaskCompletedEvent { task_id, result } => {
                self.handle_task_completed_event(ctx, &session_ctx, task_id, result)
                    .await;
            }
        }
    }
}
