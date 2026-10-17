use crate::engine::ClawMessage;
use rig::tool::PortableTool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Deserialize)]
pub struct SetStateArgs {
    pub state: String,
}

#[derive(Serialize)]
pub struct SetStateOutput {
    pub success: bool,
    pub message: String,
}

pub struct SetAgentStateTool {
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<crate::engine::ClawEngineEvent>,
    pub approval: Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for SetAgentStateTool {
    const NAME: &'static str = "set_agent_state";

    type Error = std::io::Error;
    type Args = SetStateArgs;
    type Output = SetStateOutput;

    fn description(&self) -> String {
        "Force transition the agent to a different mode/state. The only valid input right now is 'sleep'. Use this when the user explicitly asks you to stop, shut down, go away, or sleep.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": "string",
                    "description": "The target state (e.g. 'sleep')",
                    "enum": ["sleep"]
                }
            },
            "required": ["state"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let out = if args.state.to_lowercase() == "sleep" {
            log::debug!("Executing set_agent_state('sleep') tool...");
            let tx_self = {
                let state = self.state.lock().await;
                state.tx_self.clone()
            };

            // Critical: Send the TransitionRequest directly.
            // Sending SleepToggle triggers an urgent UI pop-up because it's meant for User clicks.
            // We want the system to cleanly transition itself.
            let req = crate::engine::TransitionRequest {
                target_state: crate::engine::AgentStatus::Sleep,
                source: crate::engine::TriggerSource::System,
            };

            log::debug!("Sending TransitionRequest to Sleep...");
            if let Err(e) = tx_self.send(ClawMessage::Transition(req)).await {
                log::error!("Failed to send TransitionRequest: {}", e);
            } else {
                log::debug!("Successfully sent TransitionRequest to Sleep.");
            }

            SetStateOutput {
                success: true,
                message: "Transitioning to Sleep mode...".to_string(),
            }
        } else {
            SetStateOutput {
                success: false,
                message: format!("Invalid state: {}", args.state),
            }
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}
