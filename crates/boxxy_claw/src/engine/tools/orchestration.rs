use crate::engine::{AgentStatus, ClawEngineEvent, ClawEvent};
use crate::registry::workspace::{EventFilter, global_workspace};
use boxxy_core_toolbox::ApprovalHandler;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Deserialize)]
pub struct SubscribeArgs {
    pub agent_name: String,
    pub event_type: String,
    pub regex: Option<String>,
}

#[derive(Serialize)]
pub struct SubscribeOutput {
    pub message: String,
}

use crate::engine::tools::ClawApprovalHandler;

pub struct SubscribeToPaneTool {
    pub pane_id: String,
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub approval: Arc<ClawApprovalHandler>,
}

impl Tool for SubscribeToPaneTool {
    const NAME: &'static str = "subscribe_to_pane";

    type Error = std::io::Error;
    type Args = SubscribeArgs;
    type Output = SubscribeOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Subscribe to events from another agent's pane. \
            CRITICAL: Use this tool to passively wait for events (like errors) in other panes instead of actively running 'watch' commands or parsing logs yourself. \
            When you call this, your agent will enter a 'Suspended' (0 token) state and will be automatically woken up when the event occurs."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "The name of the agent to subscribe to (e.g. 'plentiful bream')."
                    },
                    "event_type": {
                        "type": "string",
                        "description": "Type of event to listen for. Options: 'process_exit', 'output_match'."
                    },
                    "regex": {
                        "type": "string",
                        "description": "If event_type is 'output_match', the regex pattern to look for in the terminal output (e.g. 'error: could not compile')."
                    }
                },
                "required": ["agent_name", "event_type"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        let workspace = global_workspace().await;
        let target_pane_id = workspace.resolve_pane_id_by_name(&args.agent_name).await;

        let filter = match args.event_type.as_str() {
            "process_exit" => EventFilter::ProcessExited {
                pane_id: target_pane_id,
            },
            "output_match" => {
                if let (Some(target_id), Some(re)) = (target_pane_id, args.regex) {
                    EventFilter::OutputMatch {
                        pane_id: target_id,
                        regex: re,
                    }
                } else {
                    return Ok(SubscribeOutput {
                        message: "Error: 'output_match' requires a valid agent_name and regex."
                            .to_string(),
                    });
                }
            }
            _ => {
                return Ok(SubscribeOutput {
                    message: format!("Error: Unknown event_type '{}'", args.event_type),
                });
            }
        };

        workspace.subscribe(self.pane_id.clone(), filter).await;

        // Auto-suspend when subscribing
        {
            let mut state = self.state.lock().await;
            state.status = AgentStatus::Sleep;
            let (agent_name, character_id) = {
                let state = self.state.lock().await;
                (state.agent_name.clone(), state.character_id.clone())
            };

            let _ = self
                .tx_ui
                .send(ClawEngineEvent::SessionStateChanged {
                    agent_name,
                    character_id,
                    status: AgentStatus::Sleep,
                })
                .await;
        }

        let res = SubscribeOutput {
            message: format!(
                "Successfully subscribed to '{}' events from '{}'. You are now suspended until this event occurs.",
                args.event_type, args.agent_name
            ),
        };
        self.approval
            .report_tool_result(
                Self::NAME.to_string(),
                serde_json::to_string(&res).unwrap_or_default(),
            )
            .await;
        Ok(res)
    }
}

#[derive(Deserialize)]
pub struct LockArgs {
    pub resource: String,
}

#[derive(Serialize)]
pub struct LockOutput {
    pub success: bool,
    pub message: String,
}

pub struct AcquireLockTool {
    pub pane_id: String,
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub approval: Arc<ClawApprovalHandler>,
}

impl Tool for AcquireLockTool {
    const NAME: &'static str = "acquire_lock";

    type Error = std::io::Error;
    type Args = LockArgs;
    type Output = LockOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Acquire a global lock on a shared resource (e.g. a file path). \
            Prevents other agents from modifying the same resource simultaneously."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "resource": {
                        "type": "string",
                        "description": "The path or identifier of the resource to lock (e.g. 'src/main.rs')."
                    }
                },
                "required": ["resource"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        let workspace = global_workspace().await;
        match workspace
            .acquire_lock(self.pane_id.clone(), args.resource.clone())
            .await
        {
            Ok(_) => {
                let (agent_name, character_id) = {
                    let state = self.state.lock().await;
                    (state.agent_name.clone(), state.character_id.clone())
                };

                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::SessionStateChanged {
                        agent_name,
                        character_id,
                        status: AgentStatus::Locking {
                            resource: args.resource.clone(),
                        },
                    })
                    .await;

                let res = LockOutput {
                    success: true,
                    message: format!("Successfully locked '{}'.", args.resource),
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&res).unwrap_or_default(),
                    )
                    .await;
                Ok(res)
            }
            Err(e) => {
                let res = LockOutput {
                    success: false,
                    message: e,
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&res).unwrap_or_default(),
                    )
                    .await;
                Ok(res)
            }
        }
    }
}

pub struct ReleaseLockTool {
    pub pane_id: String,
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub approval: Arc<ClawApprovalHandler>,
}

impl Tool for ReleaseLockTool {
    const NAME: &'static str = "release_lock";

    type Error = std::io::Error;
    type Args = LockArgs;
    type Output = LockOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Release a previously acquired lock on a resource.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "resource": {
                        "type": "string",
                        "description": "The path or identifier of the resource to unlock."
                    }
                },
                "required": ["resource"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        let workspace = global_workspace().await;
        workspace
            .release_lock(self.pane_id.clone(), args.resource.clone())
            .await;

        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::SessionStateChanged {
                agent_name,
                character_id,
                status: AgentStatus::Waiting,
            })
            .await;

        let res = LockOutput {
            success: true,
            message: format!("Released lock on '{}'.", args.resource),
        };
        self.approval
            .report_tool_result(
                Self::NAME.to_string(),
                serde_json::to_string(&res).unwrap_or_default(),
            )
            .await;
        Ok(res)
    }
}

#[derive(Deserialize)]
pub struct PublishArgs {
    pub event_name: String,
    pub payload: String,
}

pub struct PublishEventTool {
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub approval: Arc<ClawApprovalHandler>,
}

impl Tool for PublishEventTool {
    const NAME: &'static str = "publish_custom_event";

    type Error = std::io::Error;
    type Args = PublishArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Publish a custom event to the workspace event bus. \
            Other agents subscribed to this event name will be notified."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "event_name": {
                        "type": "string",
                        "description": "The name of the custom event."
                    },
                    "payload": {
                        "type": "string",
                        "description": "Arbitrary data or message to send with the event."
                    }
                },
                "required": ["event_name", "payload"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        let workspace = global_workspace().await;
        workspace
            .publish_event(ClawEvent::Custom {
                source_agent: agent_name,
                name: args.event_name.clone(),
                payload: args.payload,
            })
            .await;

        let res = format!("Successfully published event '{}'.", args.event_name);
        self.approval
            .report_tool_result(Self::NAME.to_string(), res.clone())
            .await;
        Ok(res)
    }
}

#[derive(Deserialize)]
pub struct AwaitArgs {
    pub task_ids: Vec<String>,
}

pub struct AwaitTasksTool {
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub approval: Arc<ClawApprovalHandler>,
}

impl Tool for AwaitTasksTool {
    const NAME: &'static str = "await_tasks";

    type Error = std::io::Error;
    type Args = AwaitArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Suspend execution and wait for one or more delegated async tasks to complete. \
            CRITICAL: Use this immediately after calling `delegate_task_async` to wait for parallel tasks. \
            Do NOT attempt to run the sub-tasks yourself or poll for their status. The engine will wake you up when all tasks finish."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The list of task IDs to wait for (returned from delegate_task_async)."
                    }
                },
                "required": ["task_ids"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        let mut state = self.state.lock().await;
        state.status = AgentStatus::Sleep;
        let (agent_name, character_id) = {
            let state_lock = self.state.lock().await;
            (
                state_lock.agent_name.clone(),
                state_lock.character_id.clone(),
            )
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::SessionStateChanged {
                agent_name,
                character_id,
                status: AgentStatus::Sleep,
            })
            .await;
        let res = format!(
            "Suspending to await {} tasks. You will be woken up once they complete.",
            args.task_ids.len()
        );
        self.approval
            .report_tool_result(Self::NAME.to_string(), res.clone())
            .await;
        Ok(res)
    }
}

#[derive(Deserialize)]
pub struct OrchestrateArgs {
    pub agent_name: String,
    pub action: String, // "suspend", "cancel"
}

pub struct OrchestrateAgentTool {
    pub approval: Arc<ClawApprovalHandler>,
}

impl Tool for OrchestrateAgentTool {
    const NAME: &'static str = "orchestrate_agent";

    type Error = std::io::Error;
    type Args = OrchestrateArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Send a lifecycle command to another agent. \
            Use 'suspend' to request a peer to enter sleep mode, or 'cancel' to stop their current task."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "The name of the target agent (e.g. 'plentiful bream')."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["suspend", "cancel"],
                        "description": "The action to perform on the target agent."
                    }
                },
                "required": ["agent_name", "action"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        let workspace = global_workspace().await;
        if let Some(tx) = workspace.get_pane_tx_by_name(&args.agent_name).await {
            let msg = match args.action.as_str() {
                "suspend" => crate::engine::ClawMessage::SubscriptionEvent {
                    // Sending a dummy event to a non-suspended agent
                    // is a way to trigger the suspension check if we add a 'force' flag,
                    // but for now let's just use it to send a signal.
                    event: crate::engine::ClawEvent::Custom {
                        source_agent: "System".to_string(),
                        name: "request_sleep".to_string(),
                        payload: "External sleep request".to_string(),
                    },
                },
                "cancel" => crate::engine::ClawMessage::CancelPending,
                _ => return Ok(format!("Unknown action '{}'", args.action)),
            };

            let _ = tx.send(msg).await;
            let res = format!(
                "Successfully sent '{}' command to '{}'.",
                args.action, args.agent_name
            );
            self.approval
                .report_tool_result(Self::NAME.to_string(), res.clone())
                .await;
            Ok(res)
        } else {
            let res = format!("Agent '{}' not found.", args.agent_name);
            self.approval
                .report_tool_result(Self::NAME.to_string(), res.clone())
                .await;
            Ok(res)
        }
    }
}
