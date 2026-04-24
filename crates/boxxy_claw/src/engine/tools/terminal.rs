use crate::engine::ClawEngineEvent;
use crate::engine::session::SessionState;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct TerminalCommandArgs {
    pub command: String,
    pub explanation: String,
}

#[derive(Serialize)]
pub struct TerminalCommandOutput {
    pub success: bool,
    pub output: String,
}

use boxxy_db::Db;

pub struct TerminalCommandTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
    pub db: std::sync::Arc<tokio::sync::Mutex<Option<Db>>>,
    pub session_id: String,
    pub pane_id: String,
}

impl Tool for TerminalCommandTool {
    const NAME: &'static str = "terminal_exec";

    type Error = std::io::Error;
    type Args = TerminalCommandArgs;
    type Output = TerminalCommandOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Execute a command in the user's active terminal. This will prompt the user to 'Accept & Run'. Use this for interactive commands or tasks where the user needs to see the live output. \
            CRITICAL BUG PREVENTION: After using this tool, NEVER wrap the command or its output in a markdown code block (like ```bash or ```fish) in your text response. Doing so will cause the terminal to mistakenly prompt the user to execute it a second time in an infinite loop. Just explain what happened in plain text.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute in the terminal."
                    },
                    "explanation": {
                        "type": "string",
                        "description": "A brief explanation of why this command is being run."
                    }
                },
                "required": ["command", "explanation"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut state = self.state.lock().await;
            state.pending_terminal_reply = Some(tx);
        }

        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        let event = ClawEngineEvent::ProposeTerminalCommand {
            agent_name: agent_name.clone(),
            command: args.command.clone(),
            explanation: args.explanation.clone(),
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if let Err(e) = self.tx_ui.send(event).await {
            return Err(std::io::Error::other(format!(
                "Failed to send terminal command proposal to UI: {e}"
            )));
        }

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name: agent_name.clone(),
                is_thinking: false,
            })
            .await;
        let result = rx.await;
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name,
                is_thinking: true,
            })
            .await;

        match result {
            Ok(Ok(output)) => Ok(TerminalCommandOutput {
                success: true,
                output,
            }),
            Ok(Err(err)) => {
                // If it's a specific user reject, pass it directly
                if err == "[USER_EXPLICIT_REJECT]" {
                    Ok(TerminalCommandOutput {
                        success: false,
                        output: err,
                    })
                } else {
                    Ok(TerminalCommandOutput {
                        success: false,
                        output: err,
                    })
                }
            }
            Err(_) => Ok(TerminalCommandOutput {
                success: false,
                output: "Internal error waiting for terminal command result.".to_string(),
            }),
        }
    }
}
