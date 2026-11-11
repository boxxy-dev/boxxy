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
        let is_wildcard = matches!(
            args.agent_name.trim().to_ascii_lowercase().as_str(),
            "" | "*" | "any" | "all"
        );
        let target_pane_id = if is_wildcard {
            None
        } else {
            workspace.resolve_pane_id_by_name(&args.agent_name).await
        };

        let filter = match args.event_type.as_str() {
            "process_exit" | "command_exit" => {
                if !is_wildcard && target_pane_id.is_none() {
                    let res = SubscribeOutput {
                        message: format!(
                            "Error: Target pane '{}' not found in workspace.",
                            args.agent_name
                        ),
                    };
                    self.approval.finish_tool(Self::NAME, &res).await;
                    return Ok(res);
                }
                EventFilter::ProcessExited {
                    pane_id: target_pane_id,
                }
            }
            "output_match" => {
                if let (Some(target_id), Some(re)) = (target_pane_id, args.regex) {
                    if let Err(e) = regex::RegexBuilder::new(&re).size_limit(1 << 20).build() {
                        let res = SubscribeOutput {
                            message: format!("Error: Invalid or overly complex regex '{re}': {e}"),
                        };
                        self.approval.finish_tool(Self::NAME, &res).await;
                        return Ok(res);
                    }
                    EventFilter::OutputMatch {
                        pane_id: target_id,
                        regex: re,
                    }
                } else {
                    let res = SubscribeOutput {
                        message: "Error: 'output_match' requires a valid agent_name and regex."
                            .to_string(),
                    };
                    self.approval.finish_tool(Self::NAME, &res).await;
                    return Ok(res);
                }
            }
            _ => {
                let res = SubscribeOutput {
                    message: format!("Error: Unknown event_type '{}'", args.event_type),
                };
                self.approval.finish_tool(Self::NAME, &res).await;
                return Ok(res);
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
            Ok(()) => {
                let status = AgentStatus::Locking {
                    resource: args.resource.clone(),
                };
                let (agent_name, character_id) = {
                    let mut state = self.state.lock().await;
                    state.status = status.clone();
                    (state.agent_name.clone(), state.character_id.clone())
                };

                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::SessionStateChanged {
                        agent_name,
                        character_id,
                        status,
                    })
                    .await;

                let res = LockOutput {
                    success: true,
                    message: format!("Successfully locked '{}'.", args.resource),
                };
                self.approval.finish_tool(Self::NAME, &res).await;
                Ok(res)
            }
            Err(e) => {
                let res = LockOutput {
                    success: false,
                    message: e,
                };
                self.approval.finish_tool(Self::NAME, &res).await;
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

        let next_status = match workspace.held_lock_for_pane(&self.pane_id).await {
            Some(remaining) => AgentStatus::Locking {
                resource: remaining,
            },
            None => AgentStatus::Waiting,
        };

        let (agent_name, character_id) = {
            let mut state = self.state.lock().await;
            state.status = next_status.clone();
            (state.agent_name.clone(), state.character_id.clone())
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::SessionStateChanged {
                agent_name,
                character_id,
                status: next_status,
            })
            .await;

        let res = LockOutput {
            success: true,
            message: format!("Released lock on '{}'.", args.resource),
        };
        self.approval.finish_tool(Self::NAME, &res).await;
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
        let workspace = global_workspace().await;

        let mut registered_uuids = Vec::new();
        for id_str in &args.task_ids {
            if let Ok(uuid) = uuid::Uuid::parse_str(id_str)
                && workspace.has_delegated_task(uuid).await
            {
                registered_uuids.push(uuid);
            }
        }

        if !registered_uuids.is_empty() {
            let mut results = Vec::with_capacity(registered_uuids.len());
            for uuid in registered_uuids {
                match workspace
                    .wait_for_delegated_task(uuid, std::time::Duration::from_secs(120))
                    .await
                {
                    Ok(output) => {
                        results.push(format!("Task {uuid} result:\n{output}"));
                    }
                    Err(err) => {
                        results.push(format!("Task {uuid} failed: {err}"));
                    }
                }
            }
            let res = results.join("\n\n");
            self.approval
                .report_tool_result(Self::NAME.to_string(), res.clone())
                .await;
            return Ok(res);
        }

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
                .find(|c| c.config.id == state.character_id).map_or_else(|| state.agent_name.to_lowercase().replace(' ', "-"), |c| c.config.name.clone());
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
                let (invited, max_rounds) = {
                    let mut state = self.state.lock().await;
                    let ok = state
                        .roundtable
                        .invite_guest(host_identity, guest, args.topic.clone());
                    if ok && args.debate {
                        state.roundtable.auto_alternate = true;
                    }
                    (ok, state.roundtable.max_rounds.max(1))
                };
                if invited {
                    InviteCharacterOutput {
                        success: true,
                        message: format!(
                            "Invited {guest_name} (@{guest_slug}) to the overlay conversation (turn limit: {max_rounds}). Conclude your current message addressing {guest_name} so they can immediately take the next turn in the overlay."
                        ),
                    }
                } else {
                    InviteCharacterOutput {
                        success: false,
                        message: format!(
                            "Character chat turn limit ({max_rounds}) has already been reached for this exchange. Conclude your current message and return control to the user."
                        ),
                    }
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

#[derive(Deserialize)]
pub struct RequestAssistanceArgs {
    #[serde(alias = "skill", alias = "domain")]
    pub capability: String,
    pub prompt: String,
}

#[derive(Serialize)]
pub struct RequestAssistanceOutput {
    pub success: bool,
    pub routed_to: Option<String>,
    pub response: String,
}

pub struct RequestAssistanceTool {
    pub pane_id: String,
    pub state: Arc<Mutex<crate::engine::session::SessionState>>,
    pub approval: Arc<ClawApprovalHandler>,
}

impl Tool for RequestAssistanceTool {
    const NAME: &'static str = "request_assistance";

    type Error = std::io::Error;
    type Args = RequestAssistanceArgs;
    type Output = RequestAssistanceOutput;

    fn description(&self) -> String {
        "Route a task or question to a specialized peer agent in the workspace based on capability, active skill, or working directory (e.g. 'rust', 'css', 'docker') and wait synchronously for their response."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "capability": {
                    "type": "string",
                    "description": "The required capability, skill, or domain (e.g. 'rust', 'rust-expert', 'frontend')."
                },
                "prompt": {
                    "type": "string",
                    "description": "Detailed instructions or question for the matched specialist peer."
                }
            },
            "required": ["capability", "prompt"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let workspace = global_workspace().await;

        let my_name = self.state.lock().await.agent_name.clone();

        let out = if let Some((_peer_id, peer_name, tx)) = workspace
            .find_peer_for_capability(&args.capability, &self.pane_id)
            .await
        {
            let request_id = uuid::Uuid::new_v4();
            workspace.register_delegated_task(request_id).await;
            let req = crate::engine::ClawMessage::DelegatedTask {
                source_agent_name: my_name,
                prompt: args.prompt,
                request_id,
            };
            if let Err(e) = tx.send(req).await {
                workspace.remove_delegated_task(request_id).await;
                RequestAssistanceOutput {
                    success: false,
                    routed_to: Some(peer_name),
                    response: format!("Failed to send task to specialist peer: {e}"),
                }
            } else {
                match workspace
                    .wait_for_delegated_task(request_id, std::time::Duration::from_secs(120))
                    .await
                {
                    Ok(response) => RequestAssistanceOutput {
                        success: true,
                        routed_to: Some(peer_name),
                        response,
                    },
                    Err(e) => RequestAssistanceOutput {
                        success: false,
                        routed_to: Some(peer_name),
                        response: format!("Specialist peer failed to respond: {e}"),
                    },
                }
            }
        } else {
            RequestAssistanceOutput {
                success: false,
                routed_to: None,
                response: format!(
                    "No active peer agent matched capability '{}'. Check GLOBAL RADAR or use `summon_headless_worker`.",
                    args.capability
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

    #[tokio::test]
    async fn lock_tools_track_session_status_and_request_assistance_routes_to_peer() {
        let (session, _tx, _rx_ui) = ClawSession::new("pane_caller".to_string());
        let state = session.state.clone();
        let tx_ui = session.tx_ui.clone();
        let approval = Arc::new(ClawApprovalHandler {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            db: Arc::new(Mutex::new(None)),
            session_id: session.session_id.clone(),
            pane_id: "pane_caller".to_string(),
        });

        // 1. Acquire two locks, release one -> status stays Locking until second is released
        let acq = AcquireLockTool {
            pane_id: "pane_caller".to_string(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval.clone(),
        };
        let rel = ReleaseLockTool {
            pane_id: "pane_caller".to_string(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval.clone(),
        };
        let mut ctx = ToolContext::default();
        acq.call(
            &mut ctx,
            LockArgs {
                resource: "./src/parser.rs".to_string(),
            },
        )
        .await
        .unwrap();
        acq.call(
            &mut ctx,
            LockArgs {
                resource: "src/lexer.rs".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            state.lock().await.status,
            AgentStatus::Locking { .. }
        ));

        // Release normalized `src/parser.rs` -> `src/lexer.rs` is still locked!
        rel.call(
            &mut ctx,
            LockArgs {
                resource: "src/parser.rs".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            state.lock().await.status,
            AgentStatus::Locking {
                resource: "src/lexer.rs".to_string()
            }
        );

        rel.call(
            &mut ctx,
            LockArgs {
                resource: "src/lexer.rs".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(state.lock().await.status, AgentStatus::Waiting);

        // 2. RequestAssistanceTool routes to a specialist peer and awaits its reply
        let workspace = global_workspace().await;
        let (peer_tx, peer_rx) = async_channel::unbounded();
        workspace
            .register_pane_tx("pane_rust_spec".to_string(), peer_tx)
            .await;
        workspace
            .update_pane_state(
                "pane_rust_spec".to_string(),
                None,
                Some("Rust_Pane".to_string()),
                "/tmp/proj".to_string(),
                None,
                None,
            )
            .await;
        workspace
            .update_pane_skills("pane_rust_spec".to_string(), vec!["rust-expert".to_string()])
            .await;

        let ws_responder = workspace.clone();
        tokio::spawn(async move {
            if let Ok(crate::engine::ClawMessage::DelegatedTask { request_id, .. }) =
                peer_rx.recv().await
            {
                ws_responder
                    .complete_delegated_task(request_id, "Lifetime issue fixed.".to_string())
                    .await;
            }
        });

        let assist_tool = RequestAssistanceTool {
            pane_id: "pane_caller".to_string(),
            state,
            approval,
        };
        let out = assist_tool
            .call(
                &mut ctx,
                RequestAssistanceArgs {
                    capability: "rust".to_string(),
                    prompt: "Check borrow lifetime".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(out.routed_to.as_deref(), Some("Rust_Pane"));
        assert_eq!(out.response, "Lifetime issue fixed.");
    }
}
