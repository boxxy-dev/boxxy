use crate::engine::{ClawEngineEvent, ScheduledTask, TaskStatus, TaskType};
use chrono::{Duration, Utc};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
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
}

impl Tool for ScheduleTaskTool {
    const NAME: &'static str = "schedule_task";

    type Error = std::io::Error;
    type Args = ScheduleTaskArgs;
    type Output = ScheduleTaskOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Schedule a task to be executed later. Types: 'notification' (reminder), 'command' (a directive for yourself), 'query' (ask a question).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "due_in_seconds": { "type": "integer", "description": "Seconds from now when the task should trigger." },
                    "task_type": { "type": "string", "enum": ["notification", "command", "query"] },
                    "payload": { "type": "string", "description": "The message to show (notification) or a prompt/directive for yourself to evaluate when the timer hits zero (command/query)." }
                },
                "required": ["due_in_seconds", "task_type", "payload"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        let _ = self.tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name,
                tool_name: Self::NAME.to_string(),
            })
            .await;

        let task_type = match args.task_type.as_str() {
            "notification" => TaskType::Notification,
            "command" => TaskType::Command,
            "query" => TaskType::Query,
            _ => return Err(std::io::Error::other("Invalid task type")),
        };

        let due_at = Utc::now() + Duration::seconds(args.due_in_seconds);
        let id = Uuid::new_v4();

        let task = ScheduledTask {
            id,
            task_type,
            payload: args.payload,
            due_at,
            status: TaskStatus::Pending,
        };

        let (agent_name, tasks, pane_id) = {
            let mut state = self.state.lock().await;
            state.pending_tasks.push(task);
            (
                state.agent_name.clone(),
                state.pending_tasks.clone(),
                state.pane_id.clone(),
            )
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::TaskStatusChanged {
                agent_name,
                tasks: tasks.clone(),
            })
            .await;

        let workspace = crate::registry::workspace::global_workspace().await;
        workspace.update_pane_tasks(pane_id, tasks).await;

        Ok(ScheduleTaskOutput {
            success: true,
            task_id: id.to_string(),
            message: format!(
                "Task scheduled for {} seconds from now.",
                args.due_in_seconds
            ),
        })
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
}

impl Tool for ListTasksTool {
    const NAME: &'static str = "list_my_tasks";

    type Error = std::io::Error;
    type Args = ListTasksArgs;
    type Output = ListTasksOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List all your currently pending scheduled tasks and reminders."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let (agent_name, tasks) = {
            let state = self.state.lock().await;
            (
                state.agent_name.clone(),
                state.pending_tasks.clone(),
            )
        };

        let _ = self.tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name,
                tool_name: Self::NAME.to_string(),
            })
            .await;

        Ok(ListTasksOutput { tasks })
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
}

impl Tool for CancelTaskTool {
    const NAME: &'static str = "cancel_task";

    type Error = std::io::Error;
    type Args = CancelTaskArgs;
    type Output = CancelTaskOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Cancel a previously scheduled task using its UUID.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "The UUID of the task to cancel." }
                },
                "required": ["task_id"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        let _ = self.tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name,
                tool_name: Self::NAME.to_string(),
            })
            .await;

        let id = Uuid::parse_str(&args.task_id)
            .map_err(|e| std::io::Error::other(format!("Invalid UUID: {e}")))?;

        let (found, agent_name, tasks, pane_id) = {
            let mut state = self.state.lock().await;
            let initial_len = state.pending_tasks.len();
            state.pending_tasks.retain(|t| t.id != id);
            (
                state.pending_tasks.len() < initial_len,
                state.agent_name.clone(),
                state.pending_tasks.clone(),
                state.pane_id.clone(),
            )
        };

        if found {
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::TaskStatusChanged {
                    agent_name,
                    tasks: tasks.clone(),
                })
                .await;

            let workspace = crate::registry::workspace::global_workspace().await;
            workspace.update_pane_tasks(pane_id, tasks).await;

            Ok(CancelTaskOutput {
                success: true,
                message: "Task cancelled successfully.".to_string(),
            })
        } else {
            Ok(CancelTaskOutput {
                success: false,
                message: "Task not found.".to_string(),
            })
        }
    }
}
