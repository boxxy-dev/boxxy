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

        let (my_pane_id, my_session_id, my_name, my_cwd, tx_self, api_keys, ollama_url) = {
            let state = self.state.lock().await;
            (
                state.pane_id.clone(),
                uuid::Uuid::parse_str(&state.session_id).unwrap_or_default(),
                state.agent_name.clone(),
                state.current_dir.clone(),
                state.tx_self.clone(),
                state.api_keys.clone(),
                state.ollama_url.clone(),
            )
        };

        let workspace = crate::registry::workspace::global_workspace().await;
        let mut watch_rx = workspace.register_delegated_task(task_id).await;

        // Create and spawn the headless session sharing the parent's working directory and credentials
        let (session, tx_child) =
            ClawSession::new_headless(my_pane_id, my_session_id, &my_name, args.profile);

        let child_pane_id = session.pane_id.clone();
        let child_session_id = session.session_id.clone();
        let child_name = session.name.clone();
        let child_state = session.state.clone();
        {
            let mut cs = child_state.lock().await;
            if !my_cwd.is_empty() {
                cs.current_dir.clone_from(&my_cwd);
            }
            cs.api_keys = api_keys;
            cs.ollama_url = ollama_url;
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

        // Roll up child worker token/cost spend to the parent session (matching OpenCode's
        // subagent `family.cost`) and wake the parent if it is sleeping on `awaiting_tasks`.
        let parent_state = self.state.clone();
        let parent_tx_ui = self.tx_ui.clone();
        let parent_db = self.approval.db.clone();
        let parent_session_id = self.approval.session_id.clone();
        tokio::spawn(async move {
            while watch_rx.changed().await.is_ok() {
                let maybe_res = watch_rx.borrow_and_update().clone();
                if let Some(result) = maybe_res {
                    let (child_tokens, child_cost) = {
                        let cs = child_state.lock().await;
                        (cs.total_tokens, cs.total_cost_usd)
                    };
                    if child_tokens > 0 || (child_cost.is_finite() && child_cost > 0.0) {
                        let (is_headless, new_total, ctx_tok, ctx_win, new_cost) = {
                            let mut ps = parent_state.lock().await;
                            ps.total_tokens = ps.total_tokens.saturating_add(child_tokens);
                            if child_cost.is_finite() && child_cost > 0.0 {
                                ps.total_cost_usd += child_cost;
                            }
                            (
                                ps.is_headless,
                                ps.total_tokens,
                                ps.context_tokens,
                                ps.current_context_window,
                                ps.total_cost_usd,
                            )
                        };
                        if !is_headless {
                            let db_opt = parent_db.lock().await.clone();
                            if let Some(db_val) = db_opt {
                                let store = boxxy_db::store::Store::new(db_val.pool());
                                let _ = store
                                    .update_session_usage_totals(
                                        &parent_session_id,
                                        i64::try_from(new_total).unwrap_or(i64::MAX),
                                        new_cost,
                                    )
                                    .await;
                            }
                            let _ = parent_tx_ui
                                .send(crate::engine::ClawEngineEvent::UsageUpdated {
                                    total_tokens: new_total,
                                    context_tokens: ctx_tok,
                                    context_window: ctx_win,
                                    total_cost_usd: new_cost,
                                })
                                .await;
                        }
                    }

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
