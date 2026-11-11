use crate::engine::session::ClawSession;
use crate::engine::{
    ClawEngineEvent, ClawMessage, TaskStatus, TaskType, persist_visual_event, turn::spawn_turn,
};

use super::context::RunContext;

impl ClawSession {
    pub(super) async fn handle_cancel_task(&self, ctx: &RunContext, task_id: uuid::Uuid) {
        let (agent_name, character_id, tasks) = {
            let mut state_lock = self.state.lock().await;
            state_lock.pending_tasks.retain(|t| t.id != task_id);
            (
                state_lock.agent_name.clone(),
                state_lock.character_id.clone(),
                state_lock.pending_tasks.clone(),
            )
        };

        crate::engine::tools::tasks::persist_pending_tasks(&self.db, &self.session_id, &tasks)
            .await;

        ctx.workspace
            .update_pane_tasks(self.pane_id.clone(), tasks.clone())
            .await;

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::TaskStatusChanged {
                agent_name,
                character_id,
                tasks,
            })
            .await;
    }

    pub(super) async fn handle_subscription_event(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        event: crate::engine::ClawEvent,
    ) {
        let current_status = self.state.lock().await.status.clone();

        if let crate::engine::ClawEvent::Custom { name, .. } = &event
            && name == "request_sleep"
            && current_status != crate::engine::AgentStatus::Sleep
        {
            let req = crate::engine::TransitionRequest {
                target_state: crate::engine::AgentStatus::Sleep,
                source: crate::engine::TriggerSource::Swarm { trace_id: vec![] },
            };
            self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                .await;
            return;
        }

        if current_status == crate::engine::AgentStatus::Sleep {
            let req = crate::engine::TransitionRequest {
                target_state: crate::engine::AgentStatus::Waiting,
                source: crate::engine::TriggerSource::System,
            };
            self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                .await;

            let snapshot = ctx
                .workspace
                .get_pane_snapshot(self.pane_id.clone())
                .await
                .unwrap_or_default();
            let prompt = match &event {
                crate::engine::ClawEvent::ProcessExited { pane_id, exit_code } => {
                    let alias = ctx
                        .workspace
                        .get_pane_alias(pane_id)
                        .await
                        .unwrap_or_else(|| pane_id.clone());
                    let peer_snap = ctx
                        .workspace
                        .get_pane_snapshot(pane_id.clone())
                        .await
                        .unwrap_or_default();
                    if peer_snap.is_empty() {
                        format!(
                            "WAKEUP: Process in pane `{alias}` (ID: `{pane_id}`) finished with exit code {exit_code}."
                        )
                    } else {
                        format!(
                            "WAKEUP: Process in pane `{alias}` (ID: `{pane_id}`) finished with exit code {exit_code}.\nLatest output from `{alias}`:\n```\n{peer_snap}\n```"
                        )
                    }
                }
                crate::engine::ClawEvent::OutputMatch {
                    pane_id,
                    regex,
                    matched_text,
                } => {
                    let alias = ctx
                        .workspace
                        .get_pane_alias(pane_id)
                        .await
                        .unwrap_or_else(|| pane_id.clone());
                    format!(
                        "WAKEUP: Output in pane `{alias}` (ID: `{pane_id}`) matched regex `{regex}`:\n```\n{matched_text}\n```"
                    )
                }
                crate::engine::ClawEvent::Custom {
                    source_agent,
                    name,
                    payload,
                } => {
                    format!(
                        "WAKEUP: Custom event `{name}` from `{source_agent}`: {payload}"
                    )
                }
            };

            if ctx.current_turn.is_some() {
                ctx.backlog
                    .push_back(ClawMessage::SubscriptionEvent { event });
                return;
            }

            ctx.current_turn = Some(spawn_turn(
                self.pane_id.clone(),
                self.session_id.clone(),
                self.name.clone(),
                &prompt,
                &snapshot,
                session_ctx,
                ctx.current_dir.clone(),
                false,
                ctx.env.clone(),
                self.db.clone(),
                self.state.clone(),
                self.tx_ui.clone(),
                self.tx_self.clone(),
                None,
                vec![],
            ));
        }
    }

    pub(super) async fn handle_task_completed_event(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        task_id: uuid::Uuid,
        result: String,
    ) {
        let should_wake = {
            let mut state_lock = self.state.lock().await;

            if let Some(tx) = state_lock.pending_delegations.remove(&task_id) {
                let _ = tx.send(result.clone());
            }

            let prev_awaiting_len = state_lock.awaiting_tasks.len();
            state_lock.awaiting_tasks.retain(|id| id != &task_id);
            let was_awaiting = state_lock.awaiting_tasks.len() < prev_awaiting_len;

            for task in &mut state_lock.pending_tasks {
                if task.id == task_id {
                    task.status = TaskStatus::Completed;
                }
            }

            was_awaiting
                && state_lock.status == crate::engine::AgentStatus::Sleep
                && state_lock.awaiting_tasks.is_empty()
        };

        if should_wake {
            let req = crate::engine::TransitionRequest {
                target_state: crate::engine::AgentStatus::Waiting,
                source: crate::engine::TriggerSource::System,
            };
            self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                .await;

            let snapshot = ctx
                .workspace
                .get_pane_snapshot(self.pane_id.clone())
                .await
                .unwrap_or_default();
            let prompt =
                format!("WAKEUP: Async task {task_id} has completed with result: {result}");

            if ctx.current_turn.is_some() {
                ctx.backlog
                    .push_back(ClawMessage::TaskCompletedEvent { task_id, result });
                return;
            }

            ctx.current_turn = Some(spawn_turn(
                self.pane_id.clone(),
                self.session_id.clone(),
                self.name.clone(),
                &prompt,
                &snapshot,
                session_ctx,
                ctx.current_dir.clone(),
                false,
                ctx.env.clone(),
                self.db.clone(),
                self.state.clone(),
                self.tx_ui.clone(),
                self.tx_self.clone(),
                None,
                vec![],
            ));
        }
    }

    pub(super) async fn tick_scheduled_tasks(&self, ctx: &RunContext) {
        let now = chrono::Utc::now();
        let mut completed_id: Option<uuid::Uuid> = None;
        let mut completed_payload: Option<String> = None;

        let maybe_completed = {
            let mut state_lock = self.state.lock().await;
            let mut any_due = false;

            for task in &mut state_lock.pending_tasks {
                if task.due_at <= now && task.status == TaskStatus::Pending {
                    any_due = true;
                    if completed_id.is_none() {
                        completed_id = Some(task.id);
                    }
                    if completed_payload.is_none() && task.task_type == TaskType::Notification {
                        completed_payload = Some(task.payload.clone());
                    }
                    match task.task_type {
                        TaskType::Notification => {
                            let text = task.payload.clone();
                            let tx_ui = self.tx_ui.clone();
                            let db = self.db.clone();
                            let session_id = self.session_id.clone();
                            let pane_id = self.pane_id.clone();
                            tokio::spawn(async move {
                                let event = ClawEngineEvent::SystemMessage { text };
                                persist_visual_event(db, session_id, pane_id, &event);
                                let _ = tx_ui.send(event).await;
                            });
                        }
                        TaskType::Command => {
                            let payload = task.payload.clone();
                            let tx_self = self.tx_self.clone();
                            let pane_id = self.pane_id.clone();
                            let fallback_cwd = ctx.current_dir.clone();
                            let workspace = ctx.workspace.clone();
                            tokio::spawn(async move {
                                let snapshot = workspace
                                    .get_pane_snapshot(pane_id.clone())
                                    .await
                                    .unwrap_or_default();
                                let cwd = workspace
                                    .get_pane_cwd(pane_id)
                                    .await
                                    .unwrap_or(fallback_cwd);
                                let _ = tx_self
                                    .send(ClawMessage::UserMessage {
                                        message: format!(
                                            "[SCHEDULED COMMAND TRIGGERED] {payload}"
                                        ),
                                        snapshot,
                                        cwd,
                                        image_attachments: vec![],
                                    })
                                    .await;
                            });
                        }
                        TaskType::Query => {
                            let message = task.payload.clone();
                            let tx_self = self.tx_self.clone();
                            let pane_id = self.pane_id.clone();
                            let fallback_cwd = ctx.current_dir.clone();
                            let workspace = ctx.workspace.clone();
                            tokio::spawn(async move {
                                let snapshot = workspace
                                    .get_pane_snapshot(pane_id.clone())
                                    .await
                                    .unwrap_or_default();
                                let cwd = workspace
                                    .get_pane_cwd(pane_id)
                                    .await
                                    .unwrap_or(fallback_cwd);
                                let _ = tx_self
                                    .send(ClawMessage::UserMessage {
                                        message,
                                        snapshot,
                                        cwd,
                                        image_attachments: vec![],
                                    })
                                    .await;
                            });
                        }
                    }
                    task.status = TaskStatus::Completed;
                }
            }

            if any_due {
                state_lock
                    .pending_tasks
                    .retain(|t| t.status == TaskStatus::Pending);
                Some((
                    state_lock.agent_name.clone(),
                    state_lock.character_id.clone(),
                    state_lock.pending_tasks.clone(),
                ))
            } else {
                None
            }
        };

        if let Some((agent_name, character_id, tasks)) = maybe_completed {
            crate::engine::tools::tasks::persist_pending_tasks(&self.db, &self.session_id, &tasks)
                .await;

            ctx.workspace
                .update_pane_tasks(self.pane_id.clone(), tasks.clone())
                .await;

            let _ = self
                .tx_ui
                .send(ClawEngineEvent::TaskStatusChanged {
                    agent_name: agent_name.clone(),
                    character_id: character_id.clone(),
                    tasks,
                })
                .await;

            let _ = self
                .tx_ui
                .send(ClawEngineEvent::TaskCompleted {
                    agent_name,
                    task_id: completed_id.unwrap_or_else(uuid::Uuid::nil),
                    character_id,
                    message: completed_payload,
                })
                .await;
        }
    }
}
