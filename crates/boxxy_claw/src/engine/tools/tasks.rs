use crate::engine::{ClawEngineEvent, ScheduledTask, TaskStatus, TaskType};
use boxxy_claw_protocol::{IntervalPhase, IntervalSchedule};
use chrono::{Duration, Utc};
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct ScheduleTaskArgs {
    pub due_in_seconds: i64,
    pub task_type: String, // "notification", "command", "query"
    pub payload: String,
    #[serde(default)]
    pub rounds: Option<u32>,
    #[serde(default)]
    pub break_seconds: Option<u32>,
    #[serde(default)]
    pub round_label: Option<String>,
}

#[derive(Serialize)]
pub struct ScheduleTaskOutput {
    pub success: bool,
    pub task_id: String,
    pub message: String,
}

pub struct ScheduleTaskTool {
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub approval: Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for ScheduleTaskTool {
    const NAME: &'static str = "schedule_task";

    type Error = std::io::Error;
    type Args = ScheduleTaskArgs;
    type Output = ScheduleTaskOutput;

    fn description(&self) -> String {
        "Schedule a one-off reminder/task OR a multi-round interval timer (boxing laps, Pomodoro, HIIT, workouts, study blocks) in a SINGLE call. Types: 'notification' (reminder/timer), 'command' (directive for yourself), 'query' (prompt). When the user asks for multiple rounds, laps, or work/break intervals, ALWAYS pass `rounds` (>= 2) and `break_seconds` in ONE single `schedule_task` call (setting `due_in_seconds` to the duration of one active round and `payload` to the session title like 'Boxing') instead of calling `schedule_task` multiple times — the UI renders a dedicated live round tracker card.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "due_in_seconds": {
                    "type": "integer",
                    "description": "Seconds from now when a single task triggers, OR the duration in seconds of each active round/lap when `rounds >= 2`."
                },
                "task_type": {
                    "type": "string",
                    "enum": ["notification", "command", "query"]
                },
                "payload": {
                    "type": "string",
                    "description": "Reminder message (single notification), session title such as 'Boxing' or 'Pomodoro' (when `rounds >= 2`), or directive/prompt (command/query)."
                },
                "rounds": {
                    "type": "integer",
                    "description": "Optional number of rounds/laps (`>= 2`) for repeating interval sessions (e.g. 6 for a 6-round boxing session). Round 1 starts immediately."
                },
                "break_seconds": {
                    "type": "integer",
                    "description": "Optional rest/break duration in seconds between rounds when `rounds >= 2` (e.g. 20 for a 20s break between rounds; 0 for back-to-back laps)."
                },
                "round_label": {
                    "type": "string",
                    "description": "Optional singular unit name for each interval when `rounds >= 2` (e.g. 'Round', 'Lap', 'Pomodoro', 'Set'). Defaults to 'Round'."
                }
            },
            "required": ["due_in_seconds", "task_type", "payload"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let task_type = match args.task_type.as_str() {
            "notification" => TaskType::Notification,
            "command" => TaskType::Command,
            "query" => TaskType::Query,
            _ => return Err(std::io::Error::other("Invalid task type")),
        };

        let now = Utc::now();
        let total_rounds = args.rounds.unwrap_or(1).clamp(1, 200);
        let (due_at, interval, confirm_msg) = if total_rounds >= 2
            && task_type == TaskType::Notification
        {
            let work_secs = u32::try_from(args.due_in_seconds.clamp(1, 86_400)).unwrap_or(60);
            let break_secs = args.break_seconds.unwrap_or(0).min(86_400);
            let label = args
                .round_label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Round")
                .to_string();
            let first_due = now + Duration::seconds(i64::from(work_secs));
            let schedule = IntervalSchedule {
                total_rounds,
                current_round: 1,
                work_secs,
                break_secs,
                phase: IntervalPhase::Work,
                phase_started_at: now,
                round_label: label.clone(),
            };
            let msg = if break_secs > 0 {
                format!(
                    "Started {total_rounds}-{} interval session '{}' ({} 1/{total_rounds} active now: {work_secs}s each with {break_secs}s breaks).",
                    label.to_lowercase(),
                    args.payload,
                    label
                )
            } else {
                format!(
                    "Started {total_rounds}-{} interval session '{}' ({} 1/{total_rounds} active now: {work_secs}s each).",
                    label.to_lowercase(),
                    args.payload,
                    label
                )
            };
            (first_due, Some(schedule), msg)
        } else {
            let delay_secs = args.due_in_seconds.max(0);
            (
                now + Duration::seconds(delay_secs),
                None,
                format!("Task scheduled for {delay_secs} seconds from now."),
            )
        };

        let id = Uuid::new_v4();

        let task = ScheduledTask {
            id,
            task_type,
            payload: args.payload,
            due_at,
            status: TaskStatus::Pending,
            interval,
            paused_remaining_ms: None,
        };

        let (agent_name, character_id, tasks, pane_id) = {
            let mut state = self.state.lock().await;
            state.pending_tasks.push(task);
            (
                state.agent_name.clone(),
                state.character_id.clone(),
                state.pending_tasks.clone(),
                state.pane_id.clone(),
            )
        };

        persist_pending_tasks(&self.approval.db, &self.approval.session_id, &tasks).await;

        let workspace = crate::registry::workspace::global_workspace().await;
        workspace.update_pane_tasks(pane_id, tasks.clone()).await;

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::TaskStatusChanged {
                agent_name,
                character_id,
                tasks,
            })
            .await;

        let out = ScheduleTaskOutput {
            success: true,
            task_id: id.to_string(),
            message: confirm_msg,
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[derive(Deserialize)]
pub struct ListTasksArgs {}

#[derive(Serialize)]
pub struct ListTasksOutput {
    pub tasks: Vec<ScheduledTask>,
}

pub struct ListTasksTool {
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub approval: Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for ListTasksTool {
    const NAME: &'static str = "list_my_tasks";

    type Error = std::io::Error;
    type Args = ListTasksArgs;
    type Output = ListTasksOutput;

    fn description(&self) -> String {
        "List all your currently pending scheduled tasks and reminders.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        _args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let tasks = self.state.lock().await.pending_tasks.clone();
        let out = ListTasksOutput { tasks };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[derive(Deserialize)]
pub struct CancelTaskArgs {
    pub task_id: String,
}

#[derive(Serialize)]
pub struct CancelTaskOutput {
    pub success: bool,
    pub message: String,
}

pub struct CancelTaskTool {
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub approval: Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for CancelTaskTool {
    const NAME: &'static str = "cancel_task";

    type Error = std::io::Error;
    type Args = CancelTaskArgs;
    type Output = CancelTaskOutput;

    fn description(&self) -> String {
        "Cancel a previously scheduled task using its UUID.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "The UUID of the task to cancel." }
            },
            "required": ["task_id"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let id = Uuid::parse_str(&args.task_id)
            .map_err(|e| std::io::Error::other(format!("Invalid UUID: {e}")))?;

        let (found, agent_name, character_id, tasks, pane_id) = {
            let mut state = self.state.lock().await;
            let initial_len = state.pending_tasks.len();
            state.pending_tasks.retain(|t| t.id != id);
            (
                state.pending_tasks.len() < initial_len,
                state.agent_name.clone(),
                state.character_id.clone(),
                state.pending_tasks.clone(),
                state.pane_id.clone(),
            )
        };

        let out = if found {
            persist_pending_tasks(&self.approval.db, &self.approval.session_id, &tasks).await;
            let workspace = crate::registry::workspace::global_workspace().await;
            workspace.update_pane_tasks(pane_id, tasks.clone()).await;

            let _ = self
                .tx_ui
                .send(ClawEngineEvent::TaskStatusChanged {
                    agent_name,
                    character_id,
                    tasks,
                })
                .await;

            CancelTaskOutput {
                success: true,
                message: "Task cancelled successfully.".to_string(),
            }
        } else {
            CancelTaskOutput {
                success: false,
                message: "Task not found.".to_string(),
            }
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

pub(crate) async fn persist_pending_tasks(
    db_cell: &Arc<Mutex<Option<boxxy_db::Db>>>,
    session_id: &str,
    tasks: &[ScheduledTask],
) {
    if session_id.is_empty() {
        return;
    }
    let db_opt = db_cell.lock().await.clone();
    let Some(db) = db_opt else {
        return;
    };
    let Ok(tasks_json) = serde_json::to_string(tasks) else {
        return;
    };
    let _ = sqlx::query(
        "UPDATE sessions SET pending_tasks_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(tasks_json)
    .bind(session_id)
    .execute(db.pool())
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_db::store::Store;

    #[tokio::test]
    async fn persist_pending_tasks_updates_session_row_in_sqlite() {
        let db = boxxy_db::Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());
        store
            .ensure_session_exists("sess-tasks-1", "char-1", "Niko", "/tmp/proj")
            .await
            .unwrap();

        let db_arc = Arc::new(Mutex::new(Some(db.clone())));
        let sample = vec![ScheduledTask {
            id: Uuid::new_v4(),
            task_type: TaskType::Notification,
            payload: "Check build".to_string(),
            due_at: Utc::now() + Duration::seconds(60),
            status: TaskStatus::Pending,
            interval: None,
            paused_remaining_ms: None,
        }];

        persist_pending_tasks(&db_arc, "sess-tasks-1", &sample).await;

        let loaded = store.get_session("sess-tasks-1").await.unwrap().unwrap();
        let decoded: Vec<ScheduledTask> =
            serde_json::from_str(loaded.pending_tasks_json.as_deref().unwrap()).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].payload, "Check build");

        // Clearing tasks updates SQLite back to `[]` so fired/cancelled tasks do not resurrect
        persist_pending_tasks(&db_arc, "sess-tasks-1", &[]).await;
        let cleared = store.get_session("sess-tasks-1").await.unwrap().unwrap();
        assert_eq!(cleared.pending_tasks_json.as_deref(), Some("[]"));
    }
}
