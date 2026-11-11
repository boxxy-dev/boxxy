use crate::engine::{ClawEngineEvent, ScheduledTask, TaskStatus, TaskType};
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
        "Schedule a task to be executed later. Types: 'notification' (reminder), 'command' (a directive for yourself), 'query' (ask a question).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "due_in_seconds": { "type": "integer", "description": "Seconds from now when the task should trigger." },
                "task_type": { "type": "string", "enum": ["notification", "command", "query"] },
                "payload": { "type": "string", "description": "The message to show (notification) or a prompt/directive for yourself to evaluate when the timer hits zero (command/query)." }
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

        let delay_secs = args.due_in_seconds.max(0);
        let due_at = Utc::now() + Duration::seconds(delay_secs);
        let id = Uuid::new_v4();

        let task = ScheduledTask {
            id,
            task_type,
            payload: args.payload,
            due_at,
            status: TaskStatus::Pending,
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
            message: format!("Task scheduled for {delay_secs} seconds from now."),
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
