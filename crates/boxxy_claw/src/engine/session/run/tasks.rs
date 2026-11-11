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

    pub(super) async fn handle_toggle_pause_task(&self, ctx: &RunContext, task_id: uuid::Uuid) {
        let now = chrono::Utc::now();
        let maybe_updated = {
            let mut state_lock = self.state.lock().await;
            let Some(task) = state_lock
                .pending_tasks
                .iter_mut()
                .find(|t| t.id == task_id && t.status == TaskStatus::Pending)
            else {
                return;
            };
            toggle_task_pause_state(task, now);
            Some((
                state_lock.agent_name.clone(),
                state_lock.character_id.clone(),
                state_lock.pending_tasks.clone(),
            ))
        };

        if let Some((agent_name, character_id, tasks)) = maybe_updated {
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
    }

    pub(super) async fn handle_cancel_all_tasks(&self, ctx: &RunContext) {
        let (agent_name, character_id, tasks) = {
            let mut state_lock = self.state.lock().await;
            state_lock.pending_tasks.clear();
            (
                state_lock.agent_name.clone(),
                state_lock.character_id.clone(),
                Vec::new(),
            )
        };

        crate::engine::tools::tasks::persist_pending_tasks(&self.db, &self.session_id, &tasks)
            .await;

        ctx.workspace
            .update_pane_tasks(self.pane_id.clone(), Vec::new())
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
                    format!("WAKEUP: Custom event `{name}` from `{source_agent}`: {payload}")
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

    pub(super) async fn handle_scheduled_task_triggered(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        prompt: String,
    ) {
        if ctx.current_turn.is_some() {
            ctx.backlog
                .push_back(ClawMessage::ScheduledTaskTriggered { prompt });
            return;
        }

        let current_status = self.state.lock().await.status.clone();
        if current_status == crate::engine::AgentStatus::Sleep {
            let req = crate::engine::TransitionRequest {
                target_state: crate::engine::AgentStatus::Waiting,
                source: crate::engine::TriggerSource::System,
            };
            self.handle_transition(req, &mut ctx.current_turn, &mut ctx.urgent_backlog)
                .await;
        }

        let snapshot = ctx
            .workspace
            .get_pane_snapshot(self.pane_id.clone())
            .await
            .unwrap_or_default();
        let cwd = ctx
            .workspace
            .get_pane_cwd(self.pane_id.clone())
            .await
            .unwrap_or_else(|| ctx.current_dir.clone());
        ctx.current_dir.clone_from(&cwd);
        self.ensure_session_persisted(&cwd).await;

        self.send_ui(ClawEngineEvent::UserMessage {
            content: prompt.clone(),
            images: Vec::new(),
        })
        .await;

        ctx.current_turn = Some(spawn_turn(
            self.pane_id.clone(),
            self.session_id.clone(),
            self.name.clone(),
            &prompt,
            &snapshot,
            session_ctx,
            cwd,
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

    pub(super) async fn tick_scheduled_tasks(&self, ctx: &RunContext) {
        let now = chrono::Utc::now();
        let mut completed_events: Vec<(uuid::Uuid, Option<String>)> = Vec::new();

        let maybe_changed = {
            let mut state_lock = self.state.lock().await;

            for task in &mut state_lock.pending_tasks {
                if !task.is_paused() && task.due_at <= now && task.status == TaskStatus::Pending {
                    if let Some(step_msg) = advance_interval_step(task, now) {
                        completed_events.push((task.id, Some(step_msg.clone())));
                        let text = format!("🔔 {step_msg}");
                        let tx_ui = self.tx_ui.clone();
                        let db = self.db.clone();
                        let session_id = self.session_id.clone();
                        let pane_id = self.pane_id.clone();
                        tokio::spawn(async move {
                            let event = ClawEngineEvent::SystemMessage { text };
                            persist_visual_event(db, session_id, pane_id, &event);
                            let _ = tx_ui.send(event).await;
                        });
                        continue;
                    }

                    let notif_msg =
                        (task.task_type == TaskType::Notification).then(|| task.payload.clone());
                    completed_events.push((task.id, notif_msg));

                    match task.task_type {
                        TaskType::Notification => {
                            let text = format!("🔔 Reminder: {}", task.payload);
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
                            let prompt = format!("[SCHEDULED COMMAND TRIGGERED] {}", task.payload);
                            let tx_self = self.tx_self.clone();
                            tokio::spawn(async move {
                                let _ = tx_self
                                    .send(ClawMessage::ScheduledTaskTriggered { prompt })
                                    .await;
                            });
                        }
                        TaskType::Query => {
                            let prompt = task.payload.clone();
                            let tx_self = self.tx_self.clone();
                            tokio::spawn(async move {
                                let _ = tx_self
                                    .send(ClawMessage::ScheduledTaskTriggered { prompt })
                                    .await;
                            });
                        }
                    }
                    task.status = TaskStatus::Completed;
                }
            }

            if !completed_events.is_empty() {
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

        if let Some((agent_name, character_id, tasks)) = maybe_changed {
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

            for (task_id, message) in completed_events {
                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::TaskCompleted {
                        agent_name: agent_name.clone(),
                        task_id,
                        character_id: character_id.clone(),
                        message,
                    })
                    .await;
            }
        }
    }
}

fn toggle_task_pause_state(
    task: &mut crate::engine::ScheduledTask,
    now: chrono::DateTime<chrono::Utc>,
) {
    task.toggle_pause_at(now);
}

fn format_duration_brief(secs: u32) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Advances a multi-round `IntervalSchedule` when its current phase hits `due_at`,
/// mutating `task` in-place (keeping `Pending` across intermediate phases and setting
/// `Completed` after the final work round) and returning the bell notification text.
fn advance_interval_step(
    task: &mut crate::engine::ScheduledTask,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    use boxxy_claw_protocol::IntervalPhase;

    let iv = task.interval.as_mut()?;
    let title = task.payload.trim();
    let unit = if iv.round_label.trim().is_empty() {
        "Round"
    } else {
        iv.round_label.trim()
    };

    let msg = match iv.phase {
        IntervalPhase::Work => {
            if iv.current_round >= iv.total_rounds {
                let plural = format!("{}s", unit.to_lowercase());
                let done_msg = format!("{title} — All {} {plural} complete!", iv.total_rounds);
                task.status = TaskStatus::Completed;
                return Some(done_msg);
            }

            if iv.break_secs > 0 {
                let completed_round = iv.current_round;
                let rest_str = format_duration_brief(iv.break_secs);
                iv.phase = IntervalPhase::Break;
                iv.phase_started_at = now;
                task.due_at = now + chrono::Duration::seconds(i64::from(iv.break_secs));
                format!(
                    "{title} — {unit} {completed_round}/{} complete! Rest for {rest_str}.",
                    iv.total_rounds
                )
            } else {
                let completed_round = iv.current_round;
                iv.current_round += 1;
                iv.phase = IntervalPhase::Work;
                iv.phase_started_at = now;
                task.due_at = now + chrono::Duration::seconds(i64::from(iv.work_secs));
                let work_str = format_duration_brief(iv.work_secs);
                format!(
                    "{title} — {unit} {completed_round}/{} complete! Start {unit} {}/{} ({work_str}).",
                    iv.total_rounds, iv.current_round, iv.total_rounds
                )
            }
        }
        IntervalPhase::Break => {
            iv.current_round = (iv.current_round + 1).min(iv.total_rounds);
            iv.phase = IntervalPhase::Work;
            iv.phase_started_at = now;
            task.due_at = now + chrono::Duration::seconds(i64::from(iv.work_secs));
            let work_str = format_duration_brief(iv.work_secs);
            format!(
                "{title} — Break over! Start {unit} {}/{} ({work_str}).",
                iv.current_round, iv.total_rounds
            )
        }
    };

    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_claw_protocol::{IntervalPhase, IntervalSchedule, ScheduledTask};
    use chrono::{Duration, Utc};

    #[test]
    fn multi_round_interval_progresses_through_work_and_breaks_without_trailing_break() {
        let t0 = Utc::now();
        let mut task = ScheduledTask {
            id: uuid::Uuid::new_v4(),
            task_type: TaskType::Notification,
            payload: "Workout".to_string(),
            due_at: t0 + Duration::seconds(60),
            status: TaskStatus::Pending,
            interval: Some(IntervalSchedule {
                total_rounds: 2,
                current_round: 1,
                work_secs: 60,
                break_secs: 20,
                phase: IntervalPhase::Work,
                phase_started_at: t0,
                round_label: "Round".to_string(),
            }),
            paused_remaining_ms: None,
        };

        // End of Round 1 -> enters 20s Break
        let t1 = t0 + Duration::seconds(60);
        let m1 = advance_interval_step(&mut task, t1).unwrap();
        assert_eq!(m1, "Workout — Round 1/2 complete! Rest for 20s.");
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.interval.as_ref().unwrap().phase, IntervalPhase::Break);
        assert_eq!(task.interval.as_ref().unwrap().current_round, 1);
        assert_eq!(task.due_at, t1 + Duration::seconds(20));

        // End of Break -> enters Round 2 Work
        let t2 = t1 + Duration::seconds(20);
        let m2 = advance_interval_step(&mut task, t2).unwrap();
        assert_eq!(m2, "Workout — Break over! Start Round 2/2 (1m).");
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.interval.as_ref().unwrap().phase, IntervalPhase::Work);
        assert_eq!(task.interval.as_ref().unwrap().current_round, 2);
        assert_eq!(task.due_at, t2 + Duration::seconds(60));

        // End of Round 2 (final round) -> completes immediately with no extra break
        let t3 = t2 + Duration::seconds(60);
        let m3 = advance_interval_step(&mut task, t3).unwrap();
        assert_eq!(m3, "Workout — All 2 rounds complete!");
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[test]
    fn toggle_task_pause_state_freezes_and_restores_remaining_time_and_phase_progress() {
        let t0 = Utc::now();
        let mut task = ScheduledTask {
            id: uuid::Uuid::new_v4(),
            task_type: TaskType::Notification,
            payload: "Workout".to_string(),
            due_at: t0 + Duration::seconds(60),
            status: TaskStatus::Pending,
            interval: Some(IntervalSchedule {
                total_rounds: 6,
                current_round: 2,
                work_secs: 60,
                break_secs: 20,
                phase: IntervalPhase::Work,
                phase_started_at: t0,
                round_label: "Lap".to_string(),
            }),
            paused_remaining_ms: None,
        };

        // Pause 25 seconds into the 60s lap (35s left)
        let t_pause = t0 + Duration::seconds(25);
        toggle_task_pause_state(&mut task, t_pause);
        assert!(task.is_paused());
        assert_eq!(task.paused_remaining_ms, Some(35_000));

        // Resume 5 minutes later -> still 35s left, and `phase_started_at` reflects 25s elapsed
        let t_resume = t_pause + Duration::minutes(5);
        toggle_task_pause_state(&mut task, t_resume);
        assert!(!task.is_paused());
        assert_eq!(task.due_at, t_resume + Duration::seconds(35));
        assert_eq!(
            task.interval.as_ref().unwrap().phase_started_at,
            t_resume - Duration::seconds(25)
        );
    }
}
