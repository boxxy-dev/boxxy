use crate::engine::ClawEnvironment;
use crate::engine::session::{ClawSession, SessionState};
use rig::tool::{Tool, ToolContext};
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
    pub approval: Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for SummonHeadlessWorkerTool {
    const NAME: &'static str = "summon_headless_worker";
    type Error = std::io::Error;
    type Args = SummonHeadlessArgs;
    type Output = SummonHeadlessOutput;

    fn description(&self) -> String {
        "Spawns a background agent with a specific profile to complete a task. Use this when you want to continue working in your pane while another agent handles a sub-task. The results will be delivered back to you."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
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
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let task_id = uuid::Uuid::new_v4();

        let (my_pane_id, my_session_id, my_name, my_cwd, tx_self) = {
            let state = self.state.lock().await;
            (
                state.pane_id.clone(),
                uuid::Uuid::parse_str(&state.session_id).unwrap_or_default(),
                state.agent_name.clone(),
                state.current_dir.clone(),
                state.tx_self.clone(),
            )
        };

        let workspace = crate::registry::workspace::global_workspace().await;
        let mut watch_rx = workspace.register_delegated_task(task_id).await;

        // Create and spawn the headless session sharing the parent's working directory
        let (session, tx_child) =
            ClawSession::new_headless(my_pane_id, my_session_id, &my_name, args.profile);

        let child_pane_id = session.pane_id.clone();
        let child_session_id = session.session_id.clone();
        let child_name = session.name.clone();
        if !my_cwd.is_empty() {
            session.state.lock().await.current_dir.clone_from(&my_cwd);
        }
        workspace
            .register_pane_tx(child_pane_id.clone(), tx_child.clone())
            .await;
        workspace
            .update_pane_state(
                child_pane_id,
                Some(child_session_id),
                Some(child_name.clone()),
                if my_cwd.is_empty() {
                    "/".to_string()
                } else {
                    my_cwd
                },
                None,
                None,
            )
            .await;

        session.start(self.env.clone());

        // Send the delegated task with correlation ID
        if let Err(e) = tx_child
            .send(crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: my_name,
                prompt: args.prompt,
                request_id: task_id,
            })
            .await
        {
            workspace.remove_delegated_task(task_id).await;
            return Err(std::io::Error::other(format!(
                "Failed to dispatch task to headless worker: {e}"
            )));
        }

        // Notify parent session if it is sleeping on `awaiting_tasks` when the worker completes
        tokio::spawn(async move {
            while watch_rx.changed().await.is_ok() {
                let maybe_res = watch_rx.borrow_and_update().clone();
                if let Some(result) = maybe_res {
                    let _ = tx_self
                        .send(crate::engine::ClawMessage::TaskCompletedEvent { task_id, result })
                        .await;
                    break;
                }
            }
        });

        let out = SummonHeadlessOutput {
            task_id: task_id.to_string(),
            message: format!(
                "Successfully summoned headless worker '{child_name}' (Task ID: {task_id}). Use `await_tasks` to wait for its completion."
            ),
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}
