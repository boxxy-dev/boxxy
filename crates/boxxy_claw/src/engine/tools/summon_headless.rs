use crate::engine::ClawEnvironment;
use crate::engine::session::{ClawSession, SessionState};
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

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
    pub env: Arc<dyn ClawEnvironment>,
    pub tx_ui: async_channel::Sender<crate::engine::ClawEngineEvent>,
}

impl Tool for SummonHeadlessWorkerTool {
    const NAME: &'static str = "summon_headless_worker";
    type Error = std::io::Error;
    type Args = SummonHeadlessArgs;
    type Output = SummonHeadlessOutput;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Spawns a background agent with a specific profile to complete a task. Use this when you want to continue working in your pane while another agent handles a sub-task. The results will be delivered back to you.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "profile": {
                        "type": "string",
                        "description": "The personality/skill profile for the worker (e.g. 'rust-expert', 'documentation-writer')."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The specific task instructions for the worker."
                    }
                },
                "required": ["profile", "prompt"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let task_id = uuid::Uuid::new_v4();

        let (my_pane_id, my_session_id, my_name, reply_rx) = {
            let mut state = self.state.lock().await;
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            state.pending_delegations.insert(task_id, reply_tx);
            (
                state.pane_id.clone(),
                uuid::Uuid::parse_str(&state.session_id).unwrap_or_default(),
                state.agent_name.clone(),
                reply_rx,
            )
        };

        let _ = self.tx_ui
            .send(crate::engine::ClawEngineEvent::ToolCallStarted {
                agent_name: my_name.clone(),
                tool_name: Self::NAME.to_string(),
            })
            .await;

        // Create and spawn the headless session
        let (session, tx_child) =
            ClawSession::new_headless(my_pane_id, my_session_id, &my_name, args.profile);

        let child_name = session.name.clone();
        session.start(self.env.clone());

        // Send the delegated task with correlation ID
        let _ = tx_child
            .send(crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: my_name.clone(),
                prompt: args.prompt,
                request_id: task_id,
            })
            .await;

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

            // Re-broadcast back to ourselves when it finishes
            let tx_self = state.tx_self.clone();
            tokio::spawn(async move {
                if let Ok(result) = reply_rx.await {
                    let _ = tx_self
                        .send(crate::engine::ClawMessage::TaskCompletedEvent { task_id, result })
                        .await;
                }
            });
        }

        Ok(SummonHeadlessOutput {
            task_id: task_id.to_string(),
            message: format!(
                "Successfully summoned headless worker '{}' (Task ID: {}). Use `await_tasks` to wait for its completion.",
                child_name, task_id
            ),
        })
    }
}
