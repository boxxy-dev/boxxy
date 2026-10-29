pub mod delegation;
pub use delegation::*;

use crate::engine::{ClawEngineEvent, SpawnLocation};
use crate::registry::workspace::global_workspace;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct SpawnAgentArgs {
    pub location: String,
    pub intent: Option<String>,
}

#[derive(Serialize)]
pub struct SpawnAgentOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub message: String,
}

pub struct SpawnAgentTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for SpawnAgentTool {
    const NAME: &'static str = "spawn_agent";

    type Error = std::io::Error;
    type Args = SpawnAgentArgs;
    type Output = SpawnAgentOutput;

    fn description(&self) -> String {
        "Spawn a new terminal pane or tab (either as a plain companion terminal that you can control directly via `terminal_exec(..., target_pane=...)`, `read_pane_buffer`, and `send_keystrokes_to_pane`, or with an `intent` for a peer agent). Returns the newly created `pane_id` and `alias` (e.g. `pane-2`) immediately so you can use it in the same turn."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "Where to spawn the new pane. Must be exactly one of: 'vertical', 'horizontal', or 'tab'."
                },
                "intent": {
                    "type": "string",
                    "description": "Optional: omit this when spawning a plain companion terminal that you will drive yourself via `terminal_exec(..., target_pane=...)`. Only pass `intent` if you want a separate peer AI character to be claimed on the new pane."
                }
            },
            "required": ["location"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let loc = match args.location.to_lowercase().as_str() {
            "vertical" => SpawnLocation::VerticalSplit,
            "horizontal" => SpawnLocation::HorizontalSplit,
            "tab" => SpawnLocation::NewTab,
            _ => {
                let out = SpawnAgentOutput {
                    pane_id: None,
                    alias: None,
                    message: "Error: location must be 'vertical', 'horizontal', or 'tab'."
                        .to_string(),
                };
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let request_id = uuid::Uuid::new_v4();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let my_name = {
            let mut state = self.state.lock().await;
            state.pending_spawns.insert(request_id, reply_tx);
            state.agent_name.clone()
        };

        if let Err(e) = self
            .tx_ui
            .send(ClawEngineEvent::RequestSpawnAgent {
                source_agent_name: my_name,
                location: loc,
                intent: args.intent,
                request_id: Some(request_id),
            })
            .await
        {
            self.state.lock().await.pending_spawns.remove(&request_id);
            return Err(std::io::Error::other(format!(
                "Failed to send spawn request: {e}"
            )));
        }

        let out = match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
            Ok(Ok((pane_id, alias))) => {
                let workspace = global_workspace().await;
                let resolved_alias = if alias.is_empty() {
                    workspace
                        .get_pane_alias(&pane_id)
                        .await
                        .unwrap_or_else(|| pane_id.clone())
                } else {
                    alias
                };
                SpawnAgentOutput {
                    pane_id: Some(pane_id.clone()),
                    alias: Some(resolved_alias.clone()),
                    message: format!(
                        "Spawned new terminal pane '{resolved_alias}' (pane_id: {pane_id}). You can now pass `target_pane: \"{resolved_alias}\"` to `terminal_exec`, `read_pane_buffer`, `send_keystrokes_to_pane`, or `subscribe_to_pane`."
                    ),
                }
            }
            _ => {
                self.state.lock().await.pending_spawns.remove(&request_id);
                SpawnAgentOutput {
                    pane_id: None,
                    alias: None,
                    message: "Successfully requested to spawn a new pane.".to_string(),
                }
            }
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

impl Tool for CloseAgentTool {
    const NAME: &'static str = "close_agent";

    type Error = std::io::Error;
    type Args = CloseAgentArgs;
    type Output = CloseAgentOutput;

    fn description(&self) -> String {
        "Close another terminal pane (by alias like 'pane-2', petname, character name, or pane ID). Use this to clean up the workspace when a task in another pane is complete."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "The alias (e.g. 'pane-2'), petname, character name, or pane ID of the pane to close."
                }
            },
            "required": ["agent_name"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let workspace = global_workspace().await;
        let target_pane_id = workspace.resolve_pane_id_by_name(&args.agent_name).await;

        if let Err(e) = self
            .tx_ui
            .send(ClawEngineEvent::RequestCloseAgent {
                target_agent_name: args.agent_name.clone(),
                target_pane_id,
            })
            .await
        {
            return Err(std::io::Error::other(format!(
                "Failed to send close request: {e}"
            )));
        }

        let out = CloseAgentOutput {
            message: format!(
                "Successfully requested to close pane '{}'.",
                args.agent_name
            ),
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[derive(Deserialize)]
pub struct ReadPaneArgs {
    #[serde(alias = "target", alias = "target_pane")]
    pub agent_name: String,
    pub lines: Option<usize>,
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

impl Tool for ReadPaneTool {
    const NAME: &'static str = "read_pane_buffer";

    type Error = std::io::Error;
    type Args = ReadPaneArgs;
    type Output = ReadPaneOutput;

    fn description(&self) -> String {
        "Read the live terminal snapshot of another pane in the workspace (either a plain companion terminal like 'pane-2' or another agent's pane). \
        Use this to inspect compiler output, logs, or command results in other terminals.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "The alias (e.g. 'pane-2'), petname, character name, or pane ID of the pane to read."
                },
                "lines": {
                    "type": "integer",
                    "description": "Optional number of lines to read from the bottom of the pane (defaults to 100)."
                }
            },
            "required": ["agent_name"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let workspace = global_workspace().await;
        let out = if let Some(pane_id) = workspace.resolve_pane_id_by_name(&args.agent_name).await {
            let max_lines = args.lines.unwrap_or(100).clamp(1, 500);
            let request_id = uuid::Uuid::new_v4();
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let (my_name, my_char_id) = {
                let mut state = self.state.lock().await;
                state.pending_scrollbacks.insert(request_id, reply_tx);
                (state.agent_name.clone(), state.character_id.clone())
            };

            let _ = self
                .tx_ui
                .send(ClawEngineEvent::RequestScrollback {
                    agent_name: my_name,
                    character_id: my_char_id,
                    target_pane_id: Some(pane_id.clone()),
                    max_lines,
                    offset_lines: 0,
                    request_id,
                })
                .await;

            match tokio::time::timeout(std::time::Duration::from_millis(1500), reply_rx).await {
                Ok(Ok(live_content))
                    if !live_content.starts_with("Error:") && !live_content.is_empty() =>
                {
                    workspace
                        .update_pane_state(
                            pane_id,
                            None,
                            None,
                            String::new(),
                            None,
                            Some(live_content.clone()),
                        )
                        .await;
                    ReadPaneOutput {
                        content: live_content,
                    }
                }
                _ => {
                    self.state.lock().await.pending_scrollbacks.remove(&request_id);
                    match workspace.get_pane_snapshot(pane_id).await {
                        Some(content) => ReadPaneOutput { content },
                        None => ReadPaneOutput {
                            content: format!("Pane '{}' has no active snapshot.", args.agent_name),
                        },
                    }
                }
            }
        } else {
            ReadPaneOutput {
                content: format!(
                    "Pane '{}' not found in workspace registry.",
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
    pub alias: String,
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

impl Tool for ListActiveAgentsTool {
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        _args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
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

impl Tool for SetGlobalIntentTool {
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
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

impl Tool for AbortAgentTaskTool {
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
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

#[derive(Deserialize)]
pub struct RestoreBackgroundShellArgs {
    pub target: String,
}

#[derive(Serialize)]
pub struct RestoreBackgroundShellOutput {
    pub message: String,
}

pub struct RestoreBackgroundShellTool {
    pub env: std::sync::Arc<dyn crate::engine::ClawEnvironment>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for RestoreBackgroundShellTool {
    const NAME: &'static str = "restore_background_shell";

    type Error = std::io::Error;
    type Args = RestoreBackgroundShellArgs;
    type Output = RestoreBackgroundShellOutput;

    fn description(&self) -> String {
        "Restore (reattach) one or all detached background shells into a live terminal tab in the Boxxy window. \
        Pass a process name (e.g. 'top', 'cachy-update'), a PID (e.g. '111172'), or 'all' to restore every detached background shell."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "The process name (e.g. 'top'), PID (e.g. '111172'), or 'all' to restore all detached background shells."
                }
            },
            "required": ["target"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let query = args.target.trim().to_lowercase();

        let shells = self
            .env
            .list_detached_shells()
            .await
            .unwrap_or_default();

        if shells.is_empty() {
            let out = RestoreBackgroundShellOutput {
                message: "There are currently no detached background shells to restore."
                    .to_string(),
            };
            self.approval.finish_tool(Self::NAME, &out).await;
            return Ok(out);
        }

        let matched: Vec<_> = shells
            .into_iter()
            .filter(|(pid, _pane_id, _idle, _cwd, fg_proc, _waiting)| {
                query == "all"
                    || pid.to_string() == query
                    || fg_proc.to_lowercase().contains(&query)
            })
            .collect();

        if matched.is_empty() {
            let out = RestoreBackgroundShellOutput {
                message: format!(
                    "No detached background shell matched '{}'.",
                    args.target
                ),
            };
            self.approval.finish_tool(Self::NAME, &out).await;
            return Ok(out);
        }

        let mut restored_labels = Vec::with_capacity(matched.len());
        if query == "all" {
            for (pid, _pane_id, _idle, _cwd, fg_proc, _waiting) in &matched {
                let label = if fg_proc.is_empty() {
                    format!("PID {pid}")
                } else {
                    format!("{fg_proc} (PID {pid})")
                };
                restored_labels.push(label);
            }
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::RequestReattachShell { pid: 0, cwd: None })
                .await;
        } else {
            for (pid, _pane_id, _idle, cwd, fg_proc, _waiting) in matched {
                let label = if fg_proc.is_empty() {
                    format!("PID {pid}")
                } else {
                    format!("{fg_proc} (PID {pid})")
                };
                let cwd_opt = (!cwd.is_empty()).then_some(cwd);
                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::RequestReattachShell { pid, cwd: cwd_opt })
                    .await;
                restored_labels.push(label);
            }
        }

        let out = RestoreBackgroundShellOutput {
            message: format!(
                "Restored background shell(s) into new terminal tab(s): {}.",
                restored_labels.join(", ")
            ),
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}
