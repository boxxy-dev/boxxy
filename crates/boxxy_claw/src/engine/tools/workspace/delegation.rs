use crate::engine::ClawEngineEvent;
use crate::registry::workspace::global_workspace;
use rig::tool::{Tool, ToolContext};
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
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for DelegateTaskTool {
    const NAME: &'static str = "delegate_task";
    type Error = std::io::Error;
    type Args = DelegateTaskArgs;
    type Output = DelegateTaskOutput;

    fn description(&self) -> String {
        "Delegate a complex task or ask a question to another agent in the workspace and wait synchronously for its reply. \
        The target agent will autonomously analyze its pane, run commands if needed (prompting the user), \
        and return its final response back to you. For fanning out multiple parallel tasks, prefer `delegate_task_async` + `await_tasks`. \
        CRITICAL WARNING: DO NOT use this tool if the target agent's Status says it is running an interactive TUI application like 'vim', 'nano', or 'htop'. \
        Stateless agents cannot control TUIs via text delegation. If you need to control a TUI in another pane, you MUST use the `send_keystrokes_to_pane` tool instead.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "The name (petname, character name, or pane ID) of the agent to delegate to (e.g. 'red-pony')."
                },
                "prompt": {
                    "type": "string",
                    "description": "The instruction or question for the target agent."
                }
            },
            "required": ["agent_name", "prompt"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let workspace = global_workspace().await;
        let request_id = uuid::Uuid::new_v4();

        let (my_name, my_pane_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.pane_id.clone())
        };

        let resolved_target_id = workspace.resolve_pane_id_by_name(&args.agent_name).await;
        if args.agent_name.eq_ignore_ascii_case(&my_name)
            || resolved_target_id.as_deref() == Some(my_pane_id.as_str())
        {
            let out = DelegateTaskOutput {
                response: "Error: You cannot delegate a task to yourself.".to_string(),
            };
            self.approval.finish_tool(Self::NAME, &out).await;
            return Ok(out);
        }

        let out = if let Some(ref tid) = resolved_target_id
            && let Some(tx) = workspace.get_pane_tx_by_id(tid).await
        {
            workspace.register_delegated_task(request_id).await;
            let req = crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: my_name,
                prompt: args.prompt,
                request_id,
            };

            if let Err(e) = tx.send(req).await {
                workspace.remove_delegated_task(request_id).await;
                DelegateTaskOutput {
                    response: format!("Failed to send task to agent: {e}"),
                }
            } else {
                match workspace
                    .wait_for_delegated_task(request_id, std::time::Duration::from_secs(120))
                    .await
                {
                    Ok(response) => DelegateTaskOutput { response },
                    Err(e) => DelegateTaskOutput {
                        response: format!("Agent failed to respond: {e}"),
                    },
                }
            }
        } else {
            DelegateTaskOutput {
                response: format!(
                    "Agent '{}' not found in workspace registry.",
                    args.agent_name
                ),
            }
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
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
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for DelegateTaskAsyncTool {
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
                    "description": "The name (petname, character name, or pane ID) of the target agent (e.g. 'plentiful-bream'). Check the Global Radar for names."
                },
                "prompt": {
                    "type": "string",
                    "description": "Detailed instructions for the target agent on what to execute and what to return."
                }
            },
            "required": ["agent_name", "prompt"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let workspace = global_workspace().await;
        let request_id = uuid::Uuid::new_v4();

        let (my_name, my_pane_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.pane_id.clone())
        };

        let resolved_target_id = workspace.resolve_pane_id_by_name(&args.agent_name).await;
        if args.agent_name.eq_ignore_ascii_case(&my_name)
            || resolved_target_id.as_deref() == Some(my_pane_id.as_str())
        {
            let out = DelegateTaskAsyncOutput {
                task_id: String::new(),
                message: "Error: You cannot delegate a task to yourself.".to_string(),
            };
            self.approval.finish_tool(Self::NAME, &out).await;
            return Ok(out);
        }

        let out = if let Some(ref tid) = resolved_target_id
            && let Some(tx) = workspace.get_pane_tx_by_id(tid).await
        {
            workspace.register_delegated_task(request_id).await;
            let req = crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: my_name,
                prompt: args.prompt,
                request_id,
            };

            if let Err(e) = tx.send(req).await {
                workspace.remove_delegated_task(request_id).await;
                DelegateTaskAsyncOutput {
                    task_id: String::new(),
                    message: format!("Failed to send task to agent: {e}"),
                }
            } else {
                DelegateTaskAsyncOutput {
                    task_id: request_id.to_string(),
                    message: format!("Task delegated successfully. Task ID: {request_id}"),
                }
            }
        } else {
            DelegateTaskAsyncOutput {
                task_id: String::new(),
                message: format!("Agent '{}' not found.", args.agent_name),
            }
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[derive(Deserialize)]
pub struct SendKeystrokesArgs {
    #[serde(alias = "target", alias = "target_pane")]
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
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for SendKeystrokesTool {
    const NAME: &'static str = "send_keystrokes_to_pane";

    type Error = std::io::Error;
    type Args = SendKeystrokesArgs;
    type Output = SendKeystrokesOutput;

    fn description(&self) -> String {
        "Send raw keystrokes directly into another terminal pane (by alias like 'pane-2', petname, character name, or pane ID) or your own pane. \
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
                    "description": "The alias (e.g. 'pane-2'), petname, character name, or pane ID of the pane to send keystrokes to."
                },
                "keys": {
                    "type": "string",
                    "description": "The raw keystrokes to send. Make sure to append '\\r' if you want to press Enter."
                }
            },
            "required": ["agent_name", "keys"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        log::debug!(
            "SendKeystrokesTool called with target '{}' and keys: {:?}",
            args.agent_name,
            args.keys
        );

        let workspace = global_workspace().await;
        let target_pane_id = workspace.resolve_pane_id_by_name(&args.agent_name).await;

        if let Err(e) = self
            .tx_ui
            .send(ClawEngineEvent::InjectKeystrokes {
                target_agent_name: args.agent_name.clone(),
                target_pane_id,
                keys: args.keys.clone(),
            })
            .await
        {
            log::error!("SendKeystrokesTool failed to send event: {e}");
            return Err(std::io::Error::other(format!(
                "Failed to send keystrokes: {e}"
            )));
        }

        let out = SendKeystrokesOutput {
            success: true,
            message: format!("Keystrokes sent to pane '{}'.", args.agent_name),
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
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

impl Tool for SendCommandToPaneTool {
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
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
                    message: format!("Failed to send command to agent: {e}"),
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
