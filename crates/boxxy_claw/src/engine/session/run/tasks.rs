use crate::engine::session::ClawSession;
use crate::engine::{
    ClawEngineEvent, ClawMessage, TaskStatus, TaskType, persist_visual_event, turn::spawn_turn,
};

use super::context::RunContext;

impl ClawSession {
    pub(super) async fn handle_cancel_task(&mut self, ctx: &mut RunContext, task_id: uuid::Uuid) {
        let mut state_lock = self.state.lock().await;
        state_lock.pending_tasks.retain(|t| t.id != task_id);
        let agent_name = state_lock.agent_name.clone();
        let character_id = state_lock.character_id.clone();
        let tasks = state_lock.pending_tasks.clone();
        drop(state_lock);

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
        &mut self,
        ctx: &mut RunContext,
        session_ctx: &str,
        event: crate::engine::ClawEvent,
    ) {
        let state_lock = self.state.lock().await;

        if let crate::engine::ClawEvent::Custom { name, .. } = &event
            && name == "request_sleep"
            && state_lock.status != crate::engine::AgentStatus::Sleep
        {
            drop(state_lock);
            let req = crate::engine::TransitionRequest {
                target_state: crate::engine::AgentStatus::Sleep,
                source: crate::engine::TriggerSource::Swarm { trace_id: vec![] },
            };
            self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                .await;
            return;
        }

        if state_lock.status == crate::engine::AgentStatus::Sleep {
            drop(state_lock);

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
            let prompt = format!(
                "WAKEUP: An event you subscribed to has occurred: {:?}",
                event
            );

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
        &mut self,
        ctx: &mut RunContext,
        session_ctx: &str,
        task_id: uuid::Uuid,
        result: String,
    ) {
        let mut state_lock = self.state.lock().await;

        if let Some(tx) = state_lock.pending_delegations.remove(&task_id) {
            let _ = tx.send(result.clone());
        }

        state_lock.awaiting_tasks.retain(|id| id != &task_id);

        for task in &mut state_lock.pending_tasks {
            if task.id == task_id {
                task.status = TaskStatus::Completed;
            }
        }

        if state_lock.status == crate::engine::AgentStatus::Sleep
            && state_lock.awaiting_tasks.is_empty()
        {
            drop(state_lock);

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
            let prompt = format!(
                "WAKEUP: Async task {} has completed with result: {}",
                task_id, result
            );

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

    pub(super) async fn tick_scheduled_tasks(&mut self, ctx: &mut RunContext) {
        let now = chrono::Utc::now();
        let mut state_lock = self.state.lock().await;
        let mut completed_indices = Vec::new();
        let mut completed_payload: Option<String> = None;

        for (i, task) in state_lock.pending_tasks.iter_mut().enumerate() {
            if task.due_at <= now && task.status == TaskStatus::Pending {
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
                    TaskType::Command | TaskType::Query => {
                        let message = task.payload.clone();
                        let tx_self = self.tx_self.clone();
                        let pane_id = self.pane_id.clone();
                        let workspace = ctx.workspace.clone();
                        tokio::spawn(async move {
                            if let Some(snapshot) =
                                workspace.get_pane_snapshot(pane_id.clone()).await
                            {
                                let cwd = workspace
                                    .get_pane_cwd(pane_id)
                                    .await
                                    .unwrap_or_else(|| "/".to_string());
                                let _ = tx_self
                                    .send(ClawMessage::UserMessage {
                                        message,
                                        snapshot,
                                        cwd,
                                        image_attachments: vec![],
                                    })
                                    .await;
                            }
                        });
                    }
                }
                task.status = TaskStatus::Completed;
                completed_indices.push(i);
            }
        }

        if !completed_indices.is_empty() {
            state_lock
                .pending_tasks
                .retain(|t| t.status == TaskStatus::Pending);
            let agent_name = state_lock.agent_name.clone();
            let character_id = state_lock.character_id.clone();
            let agent_name_for_event = agent_name.clone();
            let tasks = state_lock.pending_tasks.clone();
            drop(state_lock);

            ctx.workspace
                .update_pane_tasks(self.pane_id.clone(), tasks.clone())
                .await;

            let _ = self
                .tx_ui
                .send(ClawEngineEvent::TaskStatusChanged {
                    agent_name,
                    character_id: character_id.clone(),
                    tasks,
                })
                .await;

            let _ = self
                .tx_ui
                .send(ClawEngineEvent::TaskCompleted {
                    agent_name: agent_name_for_event,
                    task_id: uuid::Uuid::nil(),
                    character_id,
                    message: completed_payload,
                })
                .await;
        }
    }
}
