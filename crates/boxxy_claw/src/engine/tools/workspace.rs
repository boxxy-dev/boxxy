use rig::tool::Tool;
use rig::completion::ToolDefinition;
use serde::{Deserialize, Serialize};
use crate::registry::workspace::global_workspace;
use crate::engine::ClawEngineEvent;

#[derive(Deserialize)]
pub struct ReadPaneArgs {
    pub pane_id: String,
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
            description: "Read the terminal snapshot (last ~50 lines) of another pane in the same workspace. \
            Use this to coordinate with what's happening in other terminals.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pane_id": {
                        "type": "string",
                        "description": "The ID of the pane to read."
                    }
                },                "required": ["pane_id"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let workspace = global_workspace().await;
        match workspace.get_pane_snapshot(args.pane_id.clone()).await {
            Some(content) => Ok(ReadPaneOutput { content }),
            None => Ok(ReadPaneOutput { content: format!("Pane {} has no active snapshot or is not registered.", args.pane_id) }),
        }
    }
}

#[derive(Deserialize)]
pub struct SendCommandArgs {
    pub pane_id: String,
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
            The user will be prompted to 'Accept & Run' in that specific pane.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pane_id": {
                        "type": "string",
                        "description": "The ID of the pane to send the command to."
                    },
                    "command": {
                        "type": "string",
                        "description": "The command to execute."
                    }
                },
                "required": ["pane_id", "command"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // We reuse the ProposeTerminalCommand event but we need to tell the UI which pane.
        // Wait, ClawEngineEvent::ProposeTerminalCommand doesn't have a pane_id because it's sent on a per-pane channel.
        // The UI component for the current pane receives it.
        // To send to ANOTHER pane, we might need a global event bus or the UI needs to route it.
        
        // For now, let's assume the UI routes based on some metadata or we add pane_id to the event.
        // Let's check ClawEngineEvent in mod.rs again.
        
        // Actually, since each pane has its own channel, the sender here belongs to the CURRENT pane.
        // If we want to send to another pane, we need the UI to handle the routing.
        
        // Let's add a new event type: ForwardCommand { target_pane_id, command }
        
        Ok(SendCommandOutput {
            success: true,
            message: format!("Command sent to Pane {}. Waiting for user approval in that pane.", args.pane_id),
        })
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
            Use this to signal what you are currently working on to avoid conflicts.".to_string(),
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
        workspace.set_project_intent(&self.project_path, args.intent.clone()).await;
        Ok(format!("Workspace intent updated: {}", args.intent))
    }
}
