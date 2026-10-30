use crate::engine::{AgentStatus, ClawEngineEvent, ClawEvent};
use crate::registry::workspace::{EventFilter, global_workspace};
use boxxy_core_toolbox::ApprovalHandler;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Deserialize)]
pub struct SubscribeArgs {
    #[serde(alias = "target", alias = "target_pane")]
    pub agent_name: String,
    #[serde(alias = "wait_for")]
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

    fn description(&self) -> String {
        "Subscribe to events from another terminal pane (by alias like 'pane-2', petname, character name, or pane ID). \
        CRITICAL: Use this tool to passively wait for events (like command completion or errors) in other panes instead of actively running 'watch' commands or polling logs yourself. \
        When you call this, your agent will enter a 'Suspended' (0 token) state and will be automatically woken up when the event occurs."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "The alias (e.g. 'pane-2'), petname, character name, or pane ID of the pane to subscribe to."
                },
                "event_type": {
                    "type": "string",
                    "description": "Type of event to listen for. Options: 'process_exit' (or 'command_exit'), 'output_match'."
                },
                "regex": {
                    "type": "string",
                    "description": "If event_type is 'output_match', the regex pattern to look for in the terminal output (e.g. 'error: could not compile')."
                }
            },
            "required": ["agent_name", "event_type"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let workspace = global_workspace().await;
        let target_pane_id = workspace.resolve_pane_id_by_name(&args.agent_name).await;

        let filter = match args.event_type.as_str() {
            "process_exit" | "command_exit" => EventFilter::ProcessExited {
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

        let is_new = workspace.subscribe(self.pane_id.clone(), filter).await;

        // Auto-suspend when subscribing (single lock acquisition — prevents deadlock)
        let (agent_name, character_id) = {
            let mut state = self.state.lock().await;
            state.status = AgentStatus::Sleep;
            state.sleep_timestamp = Some(chrono::Utc::now().timestamp_millis());
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

        let res = if is_new {
            SubscribeOutput {
                message: format!(
                    "Successfully subscribed to '{}' events from '{}'. Do NOT call any more tools now — end your turn with a brief message to the user so your session suspends and wakes automatically when the event occurs.",
                    args.event_type, args.agent_name
                ),
            }
        } else {
            SubscribeOutput {
                message: format!(
                    "Already subscribed to '{}' events from '{}'. Stop calling tools and output your final response now so your session can suspend.",
                    args.event_type, args.agent_name
                ),
            }
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

    fn description(&self) -> String {
        "Acquire a global lock on a shared resource (e.g. a file path). \
        Prevents other agents from modifying the same resource simultaneously."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "The path or identifier of the resource to lock (e.g. 'src/main.rs')."
                }
            },
            "required": ["resource"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
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

    fn description(&self) -> String {
        "Release a previously acquired lock on a resource.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "The path or identifier of the resource to unlock."
                }
            },
            "required": ["resource"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
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

    fn description(&self) -> String {
        "Publish a custom event to the workspace event bus. \
        Other agents subscribed to this event name will be notified."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
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

    fn description(&self) -> String {
        "Suspend execution and wait for one or more delegated async tasks to complete. \
        CRITICAL: Use this immediately after calling `delegate_task_async` to wait for parallel tasks. \
        Do NOT attempt to run the sub-tasks yourself or poll for their status. The engine will wake you up when all tasks finish."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "The list of task IDs to wait for (returned from delegate_task_async)."
                }
            },
            "required": ["task_ids"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let (agent_name, character_id) = {
            let mut state = self.state.lock().await;
            for id_str in &args.task_ids {
                if let Ok(uuid) = uuid::Uuid::parse_str(id_str)
                    && !state.awaiting_tasks.contains(&uuid)
                {
                    state.awaiting_tasks.push(uuid);
                }
            }
            state.status = AgentStatus::Sleep;
            state.sleep_timestamp = Some(chrono::Utc::now().timestamp_millis());
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

    fn description(&self) -> String {
        "Send a lifecycle command to another agent. \
        Use 'suspend' to request a peer to enter sleep mode, or 'cancel' to stop their current task."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
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

#[derive(Deserialize)]
pub struct InviteCharacterArgs {
    #[serde(alias = "agent_name", alias = "target")]
    pub character: String,
    pub topic: String,
    #[serde(default)]
    pub debate: bool,
}

#[derive(Serialize)]
pub struct InviteCharacterOutput {
    pub success: bool,
    pub message: String,
}

pub struct InviteCharacterTool {
    pub pane_id: String,
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub approval: Arc<ClawApprovalHandler>,
}

impl Tool for InviteCharacterTool {
    const NAME: &'static str = "invite_character";

    type Error = std::io::Error;
    type Args = InviteCharacterArgs;
    type Output = InviteCharacterOutput;

    fn description(&self) -> String {
        "Invite another character (e.g. 'levi-kujo', 'kuro', 'niko-una') into your current overlay conversation so you can chat, collaborate, or debate directly in front of the user. \
        If the invited character is active in another terminal pane in the swarm, they will automatically bring their live pane awareness into this conversation. \
        Set `debate: true` when you want a multi-round back-and-forth exchange with them."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "character": {
                    "type": "string",
                    "description": "The character name, slug (e.g. 'levi-kujo', 'kuro'), or swarm petname to invite into the overlay."
                },
                "topic": {
                    "type": "string",
                    "description": "The topic, question, or context you are inviting them to discuss or debate."
                },
                "debate": {
                    "type": "boolean",
                    "description": "Set to true if you want to engage in a multi-round back-and-forth conversation/debate with this character."
                }
            },
            "required": ["character", "topic"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let catalog = crate::engine::session::roundtable::get_active_catalog();

        let (host_identity, current_speaker_id) = {
            let state = self.state.lock().await;
            let host_slug = catalog
                .iter()
                .find(|c| c.config.id == state.character_id)
                .map(|c| c.config.name.clone())
                .unwrap_or_else(|| state.agent_name.to_lowercase().replace(' ', "-"));
            let host = crate::engine::session::roundtable::SpeakerIdentity {
                character_id: state.character_id.clone(),
                display_name: state.agent_name.clone(),
                slug: host_slug,
                is_guest: false,
                home_pane_id: None,
            };
            let (_, active_id) = state.effective_speaker();
            (host, active_id)
        };

        let out = if let Some(guest) = crate::engine::session::roundtable::resolve_speaker_token(
            &args.character,
            &host_identity.character_id,
            &self.pane_id,
            &catalog,
        ) {
            if guest.character_id == current_speaker_id {
                InviteCharacterOutput {
                    success: false,
                    message: "You cannot invite yourself to the conversation.".to_string(),
                }
            } else {
                let guest_name = guest.display_name.clone();
                let guest_slug = guest.slug.clone();
                {
                    let mut state = self.state.lock().await;
                    state
                        .roundtable
                        .invite_guest(host_identity, guest, args.topic.clone());
                    if args.debate {
                        state.roundtable.auto_alternate = true;
                    }
                }
                InviteCharacterOutput {
                    success: true,
                    message: format!(
                        "Invited {} (@{}) to the overlay conversation. Conclude your current message addressing {} so they can immediately take the next turn in the overlay.",
                        guest_name, guest_slug, guest_name
                    ),
                }
            }
        } else {
            let available: Vec<String> = catalog
                .iter()
                .map(|c| format!("{} (@{})", c.config.display_name, c.config.name))
                .collect();
            InviteCharacterOutput {
                success: false,
                message: format!(
                    "Character '{}' not found. Available characters: {}",
                    args.character,
                    available.join(", ")
                ),
            }
        };

        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::session::ClawSession;

    #[tokio::test]
    async fn subscribe_and_await_tasks_do_not_deadlock_and_update_sleep_state() {
        let (session, _tx, rx_ui) = ClawSession::new("pane_leader".to_string());
        let state = session.state.clone();
        let tx_ui = session.tx_ui.clone();
        let approval = Arc::new(ClawApprovalHandler {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            db: Arc::new(Mutex::new(None)),
            session_id: session.session_id.clone(),
            pane_id: "pane_leader".to_string(),
        });

        let workspace = global_workspace().await;
        workspace
            .update_pane_state(
                "pane_worker".to_string(),
                None,
                Some("red-pony".to_string()),
                "/tmp".to_string(),
                None,
                None,
            )
            .await;

        let sub_tool = SubscribeToPaneTool {
            pane_id: "pane_leader".to_string(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        let sub_res = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sub_tool.call(
                &mut ctx,
                SubscribeArgs {
                    agent_name: "red-pony".to_string(),
                    event_type: "process_exit".to_string(),
                    regex: None,
                },
            ),
        )
        .await
        .expect("SubscribeToPaneTool should not deadlock")
        .unwrap();

        assert!(sub_res.message.contains("Successfully subscribed"));
        assert_eq!(state.lock().await.status, AgentStatus::Sleep);

        // Reset status to Working and test AwaitTasksTool
        state.lock().await.status = AgentStatus::Working;
        let t1 = uuid::Uuid::new_v4();
        let t2 = uuid::Uuid::new_v4();

        let await_tool = AwaitTasksTool {
            state: state.clone(),
            tx_ui,
            approval,
        };

        let await_res = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            await_tool.call(
                &mut ctx,
                AwaitArgs {
                    task_ids: vec![t1.to_string(), t2.to_string()],
                },
            ),
        )
        .await
        .expect("AwaitTasksTool should not deadlock")
        .unwrap();

        assert!(await_res.contains("Suspending to await 2 tasks"));
        let state_guard = state.lock().await;
        assert_eq!(state_guard.status, AgentStatus::Sleep);
        assert_eq!(state_guard.awaiting_tasks, vec![t1, t2]);
        assert!(rx_ui.try_recv().is_ok());
    }
}
