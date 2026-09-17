use crate::engine::ClawEngineEvent;
use crate::registry::workspace::global_workspace;
use rig::tool::PortableTool;
use serde::{Deserialize, Serialize};

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
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
}

impl PortableTool for DelegateTaskTool {
    const NAME: &'static str = "delegate_task";
    type Error = std::io::Error;
    type Args = DelegateTaskArgs;
    type Output = DelegateTaskOutput;

    fn description(&self) -> String {
        "Delegate a complex task or ask a question to another agent in the workspace. \
        The target agent will autonomously analyze its pane, run commands if needed (prompting the user), \
        and return its final response back to you. Use this to orchestrate multi-pane workflows (e.g. 'restart the backend server'). \
        CRITICAL WARNING: DO NOT use this tool if the target agent's Status says it is running an interactive TUI application like 'vim', 'nano', or 'htop'. \
        Stateless agents cannot control TUIs via text delegation. If you need to control a TUI in another pane, you MUST use the `send_keystrokes_to_pane` tool instead.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let workspace = global_workspace().await;
        let request_id = uuid::Uuid::new_v4();

        let (my_name, character_id, reply_rx) = {
            let mut state = self.state.lock().await;
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            state.pending_delegations.insert(request_id, reply_tx);
            (
                state.agent_name.clone(),
                state.character_id.clone(),
                reply_rx,
            )
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name: my_name.clone(),
                character_id: character_id.clone(),
                tool_name: Self::NAME.to_string(),
            })
            .await;

        if args.agent_name.to_lowercase() == my_name.to_lowercase() {
            return Ok(DelegateTaskOutput {
                response: "Error: You cannot delegate a task to yourself.".to_string(),
            });
        }

        if let Some(tx) = workspace.get_pane_tx_by_name(&args.agent_name).await {
            let req = crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: my_name,
                prompt: args.prompt,
                request_id,
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
pub struct DelegateTaskAsyncArgs {
    pub agent_name: String,
    pub prompt: String,
}

#[derive(Serialize)]
pub struct DelegateTaskAsyncOutput {
    pub task_id: String,
    pub message: String,
}

pub struct DelegateTaskAsyncTool {
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
}

impl PortableTool for DelegateTaskAsyncTool {
    const NAME: &'static str = "delegate_task_async";
    type Error = std::io::Error;
    type Args = DelegateTaskAsyncArgs;
    type Output = DelegateTaskAsyncOutput;

    fn description(&self) -> String {
        "Delegate a task to another active agent asynchronously. \
        CRITICAL: Use this instead of `spawn_agent` when you need to send a specific command/prompt to an existing agent and wait for the result. \
        Returns a unique `task_id` immediately. You must follow up with `await_tasks([task_id])` to pause execution and retrieve the results once the agent finishes."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "The exact mnemonic name of the target agent (e.g. 'plentiful bream'). Check the Global Radar for names."
                },
                "prompt": {
                    "type": "string",
                    "description": "Detailed instructions for the target agent on what to execute and what to return."
                }
            },
            "required": ["agent_name", "prompt"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let workspace = global_workspace().await;
        let request_id = uuid::Uuid::new_v4();

        let (my_name, character_id, reply_rx) = {
            let mut state = self.state.lock().await;
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            state.pending_delegations.insert(request_id, reply_tx);
            (
                state.agent_name.clone(),
                state.character_id.clone(),
                reply_rx,
            )
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name: my_name.clone(),
                character_id: character_id.clone(),
                tool_name: Self::NAME.to_string(),
            })
            .await;

        if let Some(tx) = workspace.get_pane_tx_by_name(&args.agent_name).await {
            let req = crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: my_name.clone(),
                prompt: args.prompt,
                request_id,
            };

            if let Err(e) = tx.send(req).await {
                return Ok(DelegateTaskAsyncOutput {
                    task_id: "".to_string(),
                    message: format!("Failed to send task to agent: {}", e),
                });
            }

            // Spawn a task to wait for the reply and then publish it back to the EventBus
            let tx_self = {
                let workspace = global_workspace().await;
                workspace.get_pane_tx_by_name(&my_name).await
            };

            tokio::spawn(async move {
                if let Ok(result) = reply_rx.await {
                    if let Some(tx) = tx_self {
                        let _ = tx
                            .send(crate::engine::ClawMessage::TaskCompletedEvent {
                                task_id: request_id,
                                result,
                            })
                            .await;
                    }
                }
            });

            Ok(DelegateTaskAsyncOutput {
                task_id: request_id.to_string(),
                message: format!("Task delegated successfully. Task ID: {}", request_id),
            })
        } else {
            Ok(DelegateTaskAsyncOutput {
                task_id: "".to_string(),
                message: format!("Agent '{}' not found.", args.agent_name),
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
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
}

impl PortableTool for SendKeystrokesTool {
    const NAME: &'static str = "send_keystrokes_to_pane";

    type Error = std::io::Error;
    type Args = SendKeystrokesArgs;
    type Output = SendKeystrokesOutput;

    fn description(&self) -> String {
        "Send raw keystrokes directly into another agent's terminal pane. \
        Crucial for controlling interactive applications like vim, nano, htop, or terminating stuck processes. \
        To send special keys, use escape sequences: '\\e' or '\\u001b' for Escape, '\\r' or '\\n' for Enter, '\\x03' for Ctrl+C (SIGINT), '\\x04' for Ctrl+D. \
        DO NOT output these sequences in a ```bash block. ONLY pass them via this tool."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name: agent_name.clone(),
                character_id,
                tool_name: Self::NAME.to_string(),
            })
            .await;

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
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
}

impl PortableTool for SendCommandToPaneTool {
    const NAME: &'static str = "send_command_to_pane";

    type Error = std::io::Error;
    type Args = SendCommandArgs;
    type Output = SendCommandOutput;

    fn description(&self) -> String {
        "Propose to execute a command in a different terminal pane. \
        The user will be prompted to 'Accept & Run' in that specific pane."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name: agent_name.clone(),
                character_id,
                tool_name: Self::NAME.to_string(),
            })
            .await;

        let workspace = global_workspace().await;
        let request_id = uuid::Uuid::new_v4();

        if let Some(tx) = workspace.get_pane_tx_by_name(&args.agent_name).await {
            let req = crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: "Workspace Automation".to_string(),
                prompt: format!(
                    "I am delegating a command to you. Please evaluate and propose the following command to the user for execution: `{}`",
                    args.command
                ),
                request_id,
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
