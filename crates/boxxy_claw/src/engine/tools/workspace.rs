pub mod delegation;
pub use delegation::*;

use crate::engine::{ClawEngineEvent, SpawnLocation};
use crate::registry::workspace::global_workspace;
use rig::tool::PortableTool;
use serde::{Deserialize, Serialize};

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
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for SpawnAgentTool {
    const NAME: &'static str = "spawn_agent";

    type Error = std::io::Error;
    type Args = SpawnAgentArgs;
    type Output = SpawnAgentOutput;

    fn description(&self) -> String {
        "Spawn a new agent in a new terminal pane or tab. \
        Use this to create additional environments to delegate tasks to."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let my_name = self.state.lock().await.agent_name.clone();

        let loc = match args.location.to_lowercase().as_str() {
            "vertical" => SpawnLocation::VerticalSplit,
            "horizontal" => SpawnLocation::HorizontalSplit,
            "tab" => SpawnLocation::NewTab,
            _ => {
                let out = SpawnAgentOutput {
                    message: "Error: location must be 'vertical', 'horizontal', or 'tab'."
                        .to_string(),
                };
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
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

        let out = SpawnAgentOutput {
            message: "Successfully requested to spawn a new agent. It will take a few seconds to boot up and appear on your Workspace Radar.".to_string(),
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
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
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for CloseAgentTool {
    const NAME: &'static str = "close_agent";

    type Error = std::io::Error;
    type Args = CloseAgentArgs;
    type Output = CloseAgentOutput;

    fn description(&self) -> String {
        "Close another agent's terminal pane. Use this to clean up the workspace when an agent's task is fully complete."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "The name (petname, character name, or pane ID) of the agent to close."
                }
            },
            "required": ["agent_name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

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

        let out = CloseAgentOutput {
            message: format!(
                "Successfully requested to close agent '{}'.",
                args.agent_name
            ),
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[derive(Deserialize)]
pub struct ReadPaneArgs {
    pub agent_name: String,
}

#[derive(Serialize)]
pub struct ReadPaneOutput {
    pub content: String,
}

pub struct ReadPaneTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for ReadPaneTool {
    const NAME: &'static str = "read_pane_buffer";

    type Error = std::io::Error;
    type Args = ReadPaneArgs;
    type Output = ReadPaneOutput;

    fn description(&self) -> String {
        "Read the terminal snapshot (last ~50 lines) of another agent in the same workspace. \
        Use this to coordinate with what's happening in other terminals.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "The name (petname, character name, or pane ID) of the agent to read (e.g. 'red-pony')."
                }
            },
            "required": ["agent_name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let workspace = global_workspace().await;
        let out = if let Some(pane_id) = workspace.resolve_pane_id_by_name(&args.agent_name).await {
            match workspace.get_pane_snapshot(pane_id).await {
                Some(content) => ReadPaneOutput { content },
                None => ReadPaneOutput {
                    content: format!("Agent '{}' has no active snapshot.", args.agent_name),
                },
            }
        } else {
            ReadPaneOutput {
                content: format!(
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
pub struct ListAgentsArgs {}

#[derive(Serialize)]
pub struct ListAgentsOutput {
    pub agents: Vec<AgentInfo>,
}

#[derive(Serialize)]
pub struct AgentInfo {
    pub name: String,
    pub id: String,
    pub cwd: String,
    pub last_command: String,
    pub status: String,
}

pub struct ListActiveAgentsTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for ListActiveAgentsTool {
    const NAME: &'static str = "list_active_agents";

    type Error = std::io::Error;
    type Args = ListAgentsArgs;
    type Output = ListAgentsOutput;

    fn description(&self) -> String {
        "Refresh the list of all active agents across the workspace. \
        NOTE: Active peers are ALREADY injected into your `GLOBAL RADAR` every turn — only call this tool if you just spawned a new agent and need to check if it has booted."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let workspace = global_workspace().await;
        let agents = workspace.get_all_agents().await;
        let out = ListAgentsOutput { agents };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[derive(Deserialize)]
pub struct SetIntentArgs {
    pub intent: String,
}

pub struct SetGlobalIntentTool {
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for SetGlobalIntentTool {
    const NAME: &'static str = "set_global_intent";

    type Error = std::io::Error;
    type Args = SetIntentArgs;
    type Output = String;

    fn description(&self) -> String {
        "Leave a note in the global workspace scratchpad (Blackboard). \
        ALL other agents across the application will see this in their 'Radar' and 'Global Intent' sections. \
        Use this to broadcast your current high-level goal or signal that you are performing system-wide operations."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "intent": {
                    "type": "string",
                    "description": "A concise description of your current global objective (e.g. 'Performing system-wide kernel update')."
                }
            },
            "required": ["intent"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let workspace = global_workspace().await;
        workspace.set_global_intent(args.intent.clone()).await;
        let out = format!("Global workspace intent updated: {}", args.intent);
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[derive(Deserialize)]
pub struct AbortAgentArgs {
    pub agent_name: String,
}

#[derive(Serialize)]
pub struct AbortAgentOutput {
    pub message: String,
}

pub struct AbortAgentTaskTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for AbortAgentTaskTool {
    const NAME: &'static str = "abort_agent_task";

    type Error = std::io::Error;
    type Args = AbortAgentArgs;
    type Output = AbortAgentOutput;

    fn description(&self) -> String {
        "Instantly terminate another agent's active thinking or task execution. \
        Use this if you realize a peer is heading down a wrong path or if the global goal has changed."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "The name (petname, character name, or pane ID) of the agent to abort."
                }
            },
            "required": ["agent_name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let workspace = global_workspace().await;
        let out = if let Some(tx) = workspace.get_pane_tx_by_name(&args.agent_name).await {
            let _ = tx.send(crate::engine::ClawMessage::Abort).await;
            AbortAgentOutput {
                message: format!(
                    "Successfully sent ABORT signal to agent '{}'.",
                    args.agent_name
                ),
            }
        } else {
            AbortAgentOutput {
                message: format!("Agent '{}' not found.", args.agent_name),
            }
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}
