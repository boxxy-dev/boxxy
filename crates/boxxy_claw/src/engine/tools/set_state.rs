use crate::engine::{AgentStatus, ClawMessage};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
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
}

impl Tool for SetAgentStateTool {
    const NAME: &'static str = "set_agent_state";

    type Error = std::io::Error;
    type Args = SetStateArgs;
    type Output = SetStateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Force transition the agent to a different mode/state. The only valid input right now is 'sleep'. Use this when the user explicitly asks you to stop, shut down, go away, or sleep.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "description": "The target state (e.g. 'sleep')",
                        "enum": ["sleep"]
                    }
                },
                "required": ["state"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        if args.state.to_lowercase() == "sleep" {
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

            Ok(SetStateOutput {
                success: true,
                message: "Transitioning to Sleep mode...".to_string(),
            })
        } else {
            Ok(SetStateOutput {
                success: false,
                message: format!("Invalid state: {}", args.state),
            })
        }
    }
}
