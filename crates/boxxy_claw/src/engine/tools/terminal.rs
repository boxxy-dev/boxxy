use rig::tool::Tool;
use rig::completion::ToolDefinition;
use serde::{Deserialize, Serialize};
use crate::engine::ClawEngineEvent;
use crate::engine::session::SessionState;

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

pub struct TerminalCommandTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
}

impl Tool for TerminalCommandTool {
    const NAME: &'static str = "terminal_exec";
    
    type Error = std::io::Error;
    type Args = TerminalCommandArgs;
    type Output = TerminalCommandOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Execute a command in the user's active terminal. This will prompt the user to 'Accept & Run'. Use this for interactive commands or tasks where the user needs to see the live output.".to_string(),
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
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        {
            let mut state = self.state.lock().await;
            state.pending_terminal_reply = Some(tx);
        }

        if let Err(e) = self.tx_ui.send(ClawEngineEvent::ProposeTerminalCommand {
            command: args.command.clone(),
            explanation: args.explanation.clone(),
        }).await {
            return Err(std::io::Error::other(format!("Failed to send terminal command proposal to UI: {e}")));
        }

        let _ = self.tx_ui.send(ClawEngineEvent::AgentThinking { is_thinking: false }).await;
        let result = rx.await;
        let _ = self.tx_ui.send(ClawEngineEvent::AgentThinking { is_thinking: true }).await;

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
            },
            Err(_) => Ok(TerminalCommandOutput {
                success: false,
                output: "Internal error waiting for terminal command result.".to_string(),
            }),
        }
    }
}
