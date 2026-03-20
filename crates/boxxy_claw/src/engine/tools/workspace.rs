use crate::engine::{ClawEngineEvent, SpawnLocation};

#[derive(Deserialize)]
pub struct SpawnAgentArgs {
    pub location: String,
    pub intent: Option<String>,
}

#[derive(Serialize)]
pub struct SpawnAgentOutput {
    pub message: String,
}

pub struct SpawnAgentTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
}

impl Tool for SpawnAgentTool {
    const NAME: &'static str = "spawn_agent";

    type Error = std::io::Error;
    type Args = SpawnAgentArgs;
    type Output = SpawnAgentOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Spawn a new agent in a new terminal pane or tab. \
            Use this to create additional environments to delegate tasks to."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "Where to spawn the new agent. Must be exactly one of: 'vertical', 'horizontal', or 'tab'."
                    },
                    "intent": {
                        "type": "string",
                        "description": "An optional initial instruction or task for the new agent to immediately start working on."
                    }
                },
                "required": ["location"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let my_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        let loc = match args.location.to_lowercase().as_str() {
            "vertical" => SpawnLocation::VerticalSplit,
            "horizontal" => SpawnLocation::HorizontalSplit,
            "tab" => SpawnLocation::NewTab,
            _ => {
                return Ok(SpawnAgentOutput {
                    message: "Error: location must be 'vertical', 'horizontal', or 'tab'."
                        .to_string(),
                });
            }
        };

        if let Err(e) = self
            .tx_ui
            .send(ClawEngineEvent::RequestSpawnAgent {
                source_agent_name: my_name,
                location: loc,
                intent: args.intent,
            })
            .await
        {
            return Err(std::io::Error::other(format!(
                "Failed to send spawn request: {e}"
            )));
        }

        Ok(SpawnAgentOutput {
            message: "Successfully requested to spawn a new agent. It will take a few seconds to boot up and appear on your Workspace Radar.".to_string(),
        })
    }
}

#[derive(Deserialize)]
pub struct CloseAgentArgs {
    pub agent_name: String,
}

#[derive(Serialize)]
pub struct CloseAgentOutput {
    pub message: String,
}

pub struct CloseAgentTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
}

impl Tool for CloseAgentTool {
    const NAME: &'static str = "close_agent";

    type Error = std::io::Error;
    type Args = CloseAgentArgs;
    type Output = CloseAgentOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Close another agent's terminal pane. Use this to clean up the workspace when an agent's task is fully complete."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "The exact name of the agent to close."
                    }
                },
                "required": ["agent_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let Err(e) = self
            .tx_ui
            .send(ClawEngineEvent::RequestCloseAgent {
                target_agent_name: args.agent_name.clone(),
            })
            .await
        {
            return Err(std::io::Error::other(format!(
                "Failed to send close request: {e}"
            )));
        }

        Ok(CloseAgentOutput {
            message: format!(
                "Successfully requested to close agent '{}'.",
                args.agent_name
            ),
        })
    }
}
use crate::registry::workspace::global_workspace;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ReadPaneArgs {
    pub agent_name: String,
}

#[derive(Serialize)]
pub struct ReadPaneOutput {
    pub content: String,
}

pub struct ReadPaneTool;

impl Tool for ReadPaneTool {
    const NAME: &'static str = "read_pane_buffer";

    type Error = std::io::Error;
    type Args = ReadPaneArgs;
    type Output = ReadPaneOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read the terminal snapshot (last ~50 lines) of another agent in the same workspace. \
            Use this to coordinate with what's happening in other terminals.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "The name of the agent to read (e.g. 'Red Pony')."
                    }
                },                "required": ["agent_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let workspace = global_workspace().await;
        if let Some(pane_id) = workspace.resolve_pane_id_by_name(&args.agent_name).await {
            match workspace.get_pane_snapshot(pane_id).await {
                Some(content) => Ok(ReadPaneOutput { content }),
                None => Ok(ReadPaneOutput {
                    content: format!("Agent '{}' has no active snapshot.", args.agent_name),
                }),
            }
        } else {
            Ok(ReadPaneOutput {
                content: format!(
                    "Agent '{}' not found in workspace registry.",
                    args.agent_name
                ),
            })
        }
    }
}

#[derive(Deserialize)]
pub struct DelegateTaskArgs {
    pub agent_name: String,
    pub prompt: String,
}

#[derive(Serialize)]
pub struct DelegateTaskOutput {
    pub response: String,
}

pub struct DelegateTaskTool {
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
}

impl Tool for DelegateTaskTool {
    const NAME: &'static str = "delegate_task";
    type Error = std::io::Error;
    type Args = DelegateTaskArgs;
    type Output = DelegateTaskOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Delegate a complex task or ask a question to another agent in the workspace. \
            The target agent will autonomously analyze its pane, run commands if needed (prompting the user), \
            and return its final response back to you. Use this to orchestrate multi-pane workflows (e.g. 'restart the backend server').".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "The name of the agent to delegate to (e.g. 'Red Pony')."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The instruction or question for the target agent."
                    }
                },
                "required": ["agent_name", "prompt"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let workspace = global_workspace().await;

        let my_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        if args.agent_name.to_lowercase() == my_name.to_lowercase() {
            return Ok(DelegateTaskOutput {
                response: "Error: You cannot delegate a task to yourself.".to_string(),
            });
        }

        if let Some(tx) = workspace.get_pane_tx_by_name(&args.agent_name).await {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

            let req = crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: my_name,
                prompt: args.prompt,
                reply_tx,
            };

            if let Err(e) = tx.send(req).await {
                return Ok(DelegateTaskOutput {
                    response: format!("Failed to send task to agent: {}", e),
                });
            }

            match reply_rx.await {
                Ok(response) => Ok(DelegateTaskOutput { response }),
                Err(_) => Ok(DelegateTaskOutput {
                    response: "Agent failed to respond or was terminated.".to_string(),
                }),
            }
        } else {
            Ok(DelegateTaskOutput {
                response: format!(
                    "Agent '{}' not found in workspace registry.",
                    args.agent_name
                ),
            })
        }
    }
}

#[derive(Deserialize)]
pub struct SendKeystrokesArgs {
    pub agent_name: String,
    pub keys: String,
}

#[derive(Serialize)]
pub struct SendKeystrokesOutput {
    pub success: bool,
    pub message: String,
}

pub struct SendKeystrokesTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
}

impl Tool for SendKeystrokesTool {
    const NAME: &'static str = "send_keystrokes_to_pane";

    type Error = std::io::Error;
    type Args = SendKeystrokesArgs;
    type Output = SendKeystrokesOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Send raw keystrokes directly into another agent's terminal pane. \
            Crucial for controlling interactive applications like vim, nano, htop, or terminating stuck processes. \
            To send special keys, use escape sequences: '\\e' or '\\u001b' for Escape, '\\r' or '\\n' for Enter, '\\x03' for Ctrl+C (SIGINT), '\\x04' for Ctrl+D. \
            DO NOT output these sequences in a ```bash block. ONLY pass them via this tool."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "The exact name of the agent to send keystrokes to."
                    },
                    "keys": {
                        "type": "string",
                        "description": "The raw keystrokes to send. Make sure to append '\\r' if you want to press Enter."
                    }
                },
                "required": ["agent_name", "keys"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        log::debug!(
            "SendKeystrokesTool called with target '{}' and keys: {:?}",
            args.agent_name,
            args.keys
        );

        if let Err(e) = self
            .tx_ui
            .send(ClawEngineEvent::InjectKeystrokes {
                target_agent_name: args.agent_name.clone(),
                keys: args.keys.clone(),
            })
            .await
        {
            log::error!("SendKeystrokesTool failed to send event: {}", e);
            return Err(std::io::Error::other(format!(
                "Failed to send keystrokes: {e}"
            )));
        }

        Ok(SendKeystrokesOutput {
            success: true,
            message: format!("Keystrokes sent to agent '{}'.", args.agent_name),
        })
    }
}

#[derive(Deserialize)]
pub struct SendCommandArgs {
    pub agent_name: String,
    pub command: String,
}

#[derive(Serialize)]
pub struct SendCommandOutput {
    pub success: bool,
    pub message: String,
}

pub struct SendCommandToPaneTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
}

impl Tool for SendCommandToPaneTool {
    const NAME: &'static str = "send_command_to_pane";

    type Error = std::io::Error;
    type Args = SendCommandArgs;
    type Output = SendCommandOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Propose to execute a command in a different terminal pane. \
            The user will be prompted to 'Accept & Run' in that specific pane."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "The name of the agent to send the command to."
                    },
                    "command": {
                        "type": "string",
                        "description": "The command to execute."
                    }
                },
                "required": ["agent_name", "command"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let workspace = global_workspace().await;

        if let Some(tx) = workspace.get_pane_tx_by_name(&args.agent_name).await {
            let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();

            let req = crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: "Workspace Automation".to_string(),
                prompt: format!(
                    "I am delegating a command to you. Please evaluate and propose the following command to the user for execution: `{}`",
                    args.command
                ),
                reply_tx,
            };

            if let Err(e) = tx.send(req).await {
                return Ok(SendCommandOutput {
                    success: false,
                    message: format!("Failed to send command to agent: {}", e),
                });
            }

            Ok(SendCommandOutput {
                success: true,
                message: format!(
                    "Command proposal sent to Agent '{}'. Waiting for user approval in that pane.",
                    args.agent_name
                ),
            })
        } else {
            Ok(SendCommandOutput {
                success: false,
                message: format!(
                    "Agent '{}' not found in workspace registry.",
                    args.agent_name
                ),
            })
        }
    }
}

#[derive(Deserialize)]
pub struct SetIntentArgs {
    pub intent: String,
}

pub struct SetWorkspaceIntentTool {
    pub project_path: String,
}

impl Tool for SetWorkspaceIntentTool {
    const NAME: &'static str = "set_workspace_intent";

    type Error = std::io::Error;
    type Args = SetIntentArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Leave a note in the shared workspace scratchpad. \
            Other agents in the same project will see this in their 'Radar'. \
            Use this to signal what you are currently working on to avoid conflicts."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "intent": {
                        "type": "string",
                        "description": "A concise description of your current task and goals (e.g. 'Refactoring auth middleware in src/auth.rs')."
                    }
                },
                "required": ["intent"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let workspace = global_workspace().await;
        workspace
            .set_project_intent(&self.project_path, args.intent.clone())
            .await;
        Ok(format!("Workspace intent updated: {}", args.intent))
    }
}
