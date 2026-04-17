use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::engine::session::{ClawSession, SessionState};
use boxxy_agent::ipc::claw::AgentClawProxy;

#[derive(Deserialize)]
pub struct SummonHeadlessArgs {
    pub profile: String,
    pub prompt: String,
}

#[derive(Serialize)]
pub struct SummonHeadlessOutput {
    pub task_id: String,
    pub message: String,
}

pub struct SummonHeadlessWorkerTool {
    pub state: Arc<Mutex<SessionState>>,
    pub claw_proxy: AgentClawProxy<'static>,
}

impl Tool for SummonHeadlessWorkerTool {
    const NAME: &'static str = "summon_headless_worker";
    type Error = std::io::Error;
    type Args = SummonHeadlessArgs;
    type Output = SummonHeadlessOutput;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Spawns a transient background agent for parallel tasks without disturbing the user's terminal UI. \
            The new agent will have its own context and tools. \
            Returns a `task_id` that you MUST `await_tasks([task_id])` on to retrieve the results."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "profile": {
                        "type": "string",
                        "description": "The specific system instructions for this transient worker (e.g. 'You are an expert Python debugger')."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The specific task instruction or command for the new agent to execute."
                    }
                },
                "required": ["profile", "prompt"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let task_id = uuid::Uuid::new_v4();

        let (my_pane_id, my_session_id, my_name) = {
            let state = self.state.lock().await;
            (
                state.pane_id.clone(),
                uuid::Uuid::parse_str(&state.session_id).unwrap_or_default(),
                state.agent_name.clone()
            )
        };

        // Create and spawn the headless session
        let (session, tx_child) = ClawSession::new_headless(
            my_pane_id,
            my_session_id,
            args.profile,
        );

        let child_name = session.name.clone();
        session.start(self.claw_proxy.clone());

        // Create a oneshot channel for the response to map back to our task execution
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        // Send the delegated task
        let _ = tx_child.send(crate::engine::ClawMessage::DelegatedTask {
            source_agent_name: my_name.clone(),
            prompt: args.prompt,
            reply_tx,
        }).await;

        // Register the task in our own state so we can await it
        {
            let mut state = self.state.lock().await;
            state.pending_tasks.push(crate::engine::ScheduledTask {
                id: task_id,
                payload: format!("Headless worker {} finished task.", child_name),
                status: crate::engine::TaskStatus::Pending,
                task_type: crate::engine::TaskType::Notification,
                due_at: chrono::Utc::now(),
            });

            // To get tx_self, we need to ask the workspace registry
            let workspace = crate::registry::workspace::global_workspace().await;
            let tx_self = workspace.get_pane_tx_by_name(&my_name).await.unwrap();
            
            tokio::spawn(async move {
                let result_str = match reply_rx.await {
                    Ok(msg) => msg,
                    Err(_) => "Task failed or worker crashed.".to_string(),
                };
                let _ = tx_self.send(crate::engine::ClawMessage::TaskCompletedEvent {
                    task_id,
                    result: result_str,
                }).await;
            });
        }

        Ok(SummonHeadlessOutput {
            task_id: task_id.to_string(),
            message: format!("Successfully summoned headless worker '{}' (Task ID: {}). Use `await_tasks` to wait for its completion.", child_name, task_id),
        })
    }
}
