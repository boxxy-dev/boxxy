//! Bidirectional IPC envelopes between the GTK UI (`ClawMessage`) and the Claw Engine (`ClawEngineEvent`).

use crate::{
    AgentStatus, ClawEvent, FileDiff, PersistentClawRow, ScheduledTask, SpawnLocation,
    TransitionRequest, TurnTraceItem, UsageWrapper,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Metadata pairing an attached base64 image with its inline prompt token and filename.
///
/// Enables cursor-ordered `[Text, Image, Text, Image]` multimodal interleaving.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageAttachmentMeta {
    /// Inline token appearing in the user's prompt text (e.g. `"[Image 1]"` or `"@diagram.png"`).
    #[serde(default)]
    pub token: String,
    /// Original filename or display label (e.g. `"photo.jpg"`, `"Pasted Image"`).
    #[serde(default)]
    pub label: String,
}

/// Messages sent from the GTK UI down to an active `ClawSession`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClawMessage {
    Transition(TransitionRequest),
    WatchdogTimeout {
        task_id: Uuid,
        state: AgentStatus,
    },
    WakeSummaryComplete {
        result: Result<String, String>,
    },
    CommandFinished {
        exit_code: i32,
        snapshot: String,
        cwd: String,
    },
    ClawQuery {
        query: String,
        snapshot: String,
        cwd: String,
        #[serde(default)]
        image_attachments: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_attachment_meta: Vec<ImageAttachmentMeta>,
    },
    UserMessage {
        message: String,
        snapshot: String,
        cwd: String,
        #[serde(default)]
        image_attachments: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_attachment_meta: Vec<ImageAttachmentMeta>,
    },
    FileWriteReply {
        approved: bool,
    },
    FileDeleteReply {
        approved: bool,
    },
    BackgroundCommandReply {
        approved: bool,
    },
    TerminalCommandAccepted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane_id: Option<String>,
    },
    /// Internal signal emitted when a local `terminal_exec` command is still actively
    /// running in the foreground after the launch grace window, allowing the session
    /// to yield control to the terminal and finish the turn without an extra LLM call.
    CompleteRunningTerminalCommand {
        process_name: String,
    },
    KillProcessReply {
        approved: bool,
    },
    GetClipboardReply {
        approved: bool,
    },
    SetClipboardReply {
        approved: bool,
    },
    RequestLazyDiagnosis,
    CancelPending,
    SoftClearHistory,
    Initialize,
    Deactivate,
    Evict,
    Reload,
    SleepToggle(bool),
    /// A task delegated from another agent (via Correlation ID).
    DelegatedTask {
        source_agent_name: String,
        prompt: String,
        request_id: Uuid,
    },
    /// Response to a `RequestScrollback` event.
    ScrollbackReply {
        request_id: Uuid,
        content: String,
    },
    /// Response to a `RequestSpawnAgent` event with the newly created pane's ID and alias.
    SpawnAgentReply {
        request_id: Uuid,
        pane_id: String,
        alias: String,
    },
    ForegroundProcessChanged {
        process_name: String,
    },
    ResumeSession {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        character_id: Option<String>,
    },
    TogglePin(bool),
    ToggleWebSearch(bool),
    ToggleVibe(bool),
    CancelTask {
        task_id: Uuid,
    },
    TogglePauseTask {
        task_id: Uuid,
    },
    CancelAllTasks,
    ScheduledTaskTriggered {
        prompt: String,
    },
    SubscriptionEvent {
        event: ClawEvent,
    },
    TaskCompletedEvent {
        task_id: Uuid,
        result: String,
    },
    Abort,
    TurnFinished,
    SettingsInvalidated,
    RebindPane {
        new_pane_id: String,
    },
    /// Ephemeral `/btw` side question executed against the settled session context
    /// without interrupting an active turn or mutating conversation history.
    BtwQuery {
        request_id: Uuid,
        question: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prior_exchange: Option<(String, String)>,
        snapshot: String,
        cwd: String,
    },
    /// Cancels an in-flight `/btw` side question (or all active side questions when `None`).
    CancelBtw {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<Uuid>,
    },
}

/// Events emitted from the Claw Engine up to the GTK UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClawEngineEvent {
    Identity {
        agent_name: String,
        character_id: String,
        pinned: bool,
        web_search_enabled: bool,
        #[serde(default)]
        vibe_enabled: bool,
        total_tokens: u64,
        #[serde(default)]
        context_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
        #[serde(default)]
        total_cost_usd: f64,
    },
    AgentThinking {
        is_thinking: bool,
        agent_name: String,
        character_id: String,
    },
    SessionStateChanged {
        agent_name: String,
        character_id: String,
        status: AgentStatus,
    },
    UserMessage {
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        text_attachments: Vec<String>,
    },
    DiagnosisComplete {
        agent_name: String,
        character_id: String,
        diagnosis: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        trace: Vec<TurnTraceItem>,
        usage: Option<UsageWrapper>,
        #[serde(default)]
        typewriter: bool,
    },
    /// Live reasoning/tool/skill trace update during an active turn.
    TurnTraceProgress {
        agent_name: String,
        character_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<TurnTraceItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_status: Option<String>,
    },
    /// Coalesced live text snapshot for the visible prose of the active turn.
    AssistantTextDelta {
        agent_name: String,
        character_id: String,
        text: String,
    },
    /// Forces the UI to cleanly close the Claw drawer and reset action states.
    /// Emitted when the agent silently acknowledges a user rejection (`[SILENT_ACK]`).
    DismissDrawer,
    /// Requests that the UI raises the window and focuses this specific pane.
    FocusPane,
    InjectCommand {
        agent_name: String,
        character_id: String,
        command: String,
        diagnosis: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        trace: Vec<TurnTraceItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane_label: Option<String>,
        #[serde(default)]
        auto_approve: bool,
        usage: Option<UsageWrapper>,
    },
    ProposeFileWrite {
        agent_name: String,
        character_id: String,
        path: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<FileDiff>,
        #[serde(default)]
        auto_approve: bool,
        usage: Option<UsageWrapper>,
    },
    ProposeFileDelete {
        agent_name: String,
        character_id: String,
        path: String,
        usage: Option<UsageWrapper>,
    },
    ProposeKillProcess {
        agent_name: String,
        character_id: String,
        pid: u32,
        process_name: String,
        usage: Option<UsageWrapper>,
    },
    ProposeBackgroundCommand {
        agent_name: String,
        character_id: String,
        command: String,
        explanation: String,
        usage: Option<UsageWrapper>,
    },
    ProposeGetClipboard {
        agent_name: String,
        character_id: String,
        usage: Option<UsageWrapper>,
    },
    ProposeSetClipboard {
        agent_name: String,
        character_id: String,
        text: String,
        usage: Option<UsageWrapper>,
    },
    ProposeTerminalCommand {
        agent_name: String,
        character_id: String,
        command: String,
        explanation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane_label: Option<String>,
        #[serde(default)]
        auto_approve: bool,
        usage: Option<UsageWrapper>,
    },
    ProposalResolved {
        agent_name: String,
        character_id: String,
        approved: bool,
    },
    SystemMessage {
        text: String,
    },
    RequestSpawnAgent {
        source_agent_name: String,
        location: SpawnLocation,
        intent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<Uuid>,
    },
    RequestCloseAgent {
        target_agent_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane_id: Option<String>,
    },
    RequestReattachShell {
        pid: u32,
        cwd: Option<String>,
    },
    /// Request scrollback content from the UI (via Correlation ID).
    RequestScrollback {
        agent_name: String,
        character_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane_id: Option<String>,
        max_lines: usize,
        offset_lines: usize,
        request_id: Uuid,
    },
    /// Reply to a `DelegatedTask` message.
    DelegatedTaskReply {
        request_id: Uuid,
        result: String,
    },
    InjectKeystrokes {
        target_agent_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_pane_id: Option<String>,
        keys: String,
    },
    RequestCwdSwitch {
        path: String,
    },
    ToolResult {
        agent_name: String,
        character_id: String,
        tool_name: String,
        result: String,
        usage: Option<UsageWrapper>,
    },
    ToolCallStarted {
        agent_name: String,
        character_id: String,
        tool_name: String,
    },
    LazyErrorIndicator {
        visible: bool,
        agent_name: String,
        character_id: String,
    },
    PinStatusChanged(bool),
    WebSearchStatusChanged(bool),
    VibeStatusChanged(bool),
    TaskStatusChanged {
        tasks: Vec<ScheduledTask>,
        agent_name: String,
        character_id: String,
    },
    RestoreHistory(Vec<PersistentClawRow>),
    Evicted,
    TaskCompleted {
        agent_name: String,
        task_id: Uuid,
        character_id: String,
        message: Option<String>,
    },
    LongTaskCompleted {
        agent_name: String,
        character_id: String,
        duration_secs: u64,
        message: String,
    },
    PushGlobalNotification {
        title: String,
        message: String,
    },
    /// Live reasoning/text progress for an ephemeral `/btw` side question.
    BtwProgress {
        request_id: Uuid,
        agent_name: String,
        character_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_delta: Option<String>,
    },
    /// Final answer (or error) for an ephemeral `/btw` side question.
    BtwComplete {
        request_id: Uuid,
        agent_name: String,
        character_id: String,
        question: String,
        answer: String,
        #[serde(default)]
        model_label: String,
        #[serde(default)]
        duration_ms: u64,
        usage: Option<UsageWrapper>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

impl ClawEngineEvent {
    /// Returns the attributing `character_id` when this event originated from a character turn.
    #[must_use]
    pub fn character_id(&self) -> Option<&str> {
        match self {
            Self::Identity { character_id, .. }
            | Self::AgentThinking { character_id, .. }
            | Self::SessionStateChanged { character_id, .. }
            | Self::DiagnosisComplete { character_id, .. }
            | Self::TurnTraceProgress { character_id, .. }
            | Self::AssistantTextDelta { character_id, .. }
            | Self::InjectCommand { character_id, .. }
            | Self::ProposeFileWrite { character_id, .. }
            | Self::ProposeFileDelete { character_id, .. }
            | Self::ProposeKillProcess { character_id, .. }
            | Self::ProposeBackgroundCommand { character_id, .. }
            | Self::ProposeGetClipboard { character_id, .. }
            | Self::ProposeSetClipboard { character_id, .. }
            | Self::ProposeTerminalCommand { character_id, .. }
            | Self::ProposalResolved { character_id, .. }
            | Self::RequestScrollback { character_id, .. }
            | Self::ToolResult { character_id, .. }
            | Self::ToolCallStarted { character_id, .. }
            | Self::LazyErrorIndicator { character_id, .. }
            | Self::TaskStatusChanged { character_id, .. }
            | Self::TaskCompleted { character_id, .. }
            | Self::LongTaskCompleted { character_id, .. }
            | Self::BtwProgress { character_id, .. }
            | Self::BtwComplete { character_id, .. } => Some(character_id),
            Self::UserMessage { .. }
            | Self::DismissDrawer
            | Self::FocusPane
            | Self::SystemMessage { .. }
            | Self::RequestSpawnAgent { .. }
            | Self::RequestCloseAgent { .. }
            | Self::RequestReattachShell { .. }
            | Self::DelegatedTaskReply { .. }
            | Self::InjectKeystrokes { .. }
            | Self::RequestCwdSwitch { .. }
            | Self::PinStatusChanged(_)
            | Self::WebSearchStatusChanged(_)
            | Self::VibeStatusChanged(_)
            | Self::RestoreHistory(_)
            | Self::Evicted
            | Self::PushGlobalNotification { .. } => None,
        }
    }

    /// Returns the attributing `agent_name` when carried by this event.
    #[must_use]
    pub fn agent_name(&self) -> Option<&str> {
        match self {
            Self::Identity { agent_name, .. }
            | Self::AgentThinking { agent_name, .. }
            | Self::SessionStateChanged { agent_name, .. }
            | Self::DiagnosisComplete { agent_name, .. }
            | Self::TurnTraceProgress { agent_name, .. }
            | Self::AssistantTextDelta { agent_name, .. }
            | Self::InjectCommand { agent_name, .. }
            | Self::ProposeFileWrite { agent_name, .. }
            | Self::ProposeFileDelete { agent_name, .. }
            | Self::ProposeKillProcess { agent_name, .. }
            | Self::ProposeBackgroundCommand { agent_name, .. }
            | Self::ProposeGetClipboard { agent_name, .. }
            | Self::ProposeSetClipboard { agent_name, .. }
            | Self::ProposeTerminalCommand { agent_name, .. }
            | Self::ProposalResolved { agent_name, .. }
            | Self::RequestScrollback { agent_name, .. }
            | Self::ToolResult { agent_name, .. }
            | Self::ToolCallStarted { agent_name, .. }
            | Self::LazyErrorIndicator { agent_name, .. }
            | Self::TaskStatusChanged { agent_name, .. }
            | Self::TaskCompleted { agent_name, .. }
            | Self::LongTaskCompleted { agent_name, .. }
            | Self::BtwProgress { agent_name, .. }
            | Self::BtwComplete { agent_name, .. } => Some(agent_name),
            Self::UserMessage { .. }
            | Self::DismissDrawer
            | Self::FocusPane
            | Self::SystemMessage { .. }
            | Self::RequestSpawnAgent { .. }
            | Self::RequestCloseAgent { .. }
            | Self::RequestReattachShell { .. }
            | Self::DelegatedTaskReply { .. }
            | Self::InjectKeystrokes { .. }
            | Self::RequestCwdSwitch { .. }
            | Self::PinStatusChanged(_)
            | Self::WebSearchStatusChanged(_)
            | Self::VibeStatusChanged(_)
            | Self::RestoreHistory(_)
            | Self::Evicted
            | Self::PushGlobalNotification { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claw_message_and_engine_event_roundtrip_with_serde_defaults() {
        // Omitted `image_attachments` defaults cleanly to empty Vec
        let raw_query =
            r#"{"ClawQuery":{"query":"check status","snapshot":"","cwd":"/workspace"}}"#;
        let msg: ClawMessage = serde_json::from_str(raw_query).unwrap();
        assert_eq!(
            msg,
            ClawMessage::ClawQuery {
                query: "check status".to_string(),
                snapshot: String::new(),
                cwd: "/workspace".to_string(),
                image_attachments: Vec::new(),
                image_attachment_meta: Vec::new(),
            }
        );

        let with_meta = ClawMessage::ClawQuery {
            query: "inspect [Image 1]".to_string(),
            snapshot: String::new(),
            cwd: "/workspace".to_string(),
            image_attachments: vec!["iVBORw0KGgo=".to_string()],
            image_attachment_meta: vec![ImageAttachmentMeta {
                token: "[Image 1]".to_string(),
                label: "diagram.png".to_string(),
            }],
        };
        let encoded_meta = serde_json::to_string(&with_meta).unwrap();
        let decoded_meta: ClawMessage = serde_json::from_str(&encoded_meta).unwrap();
        assert_eq!(decoded_meta, with_meta);

        let ev = ClawEngineEvent::DiagnosisComplete {
            agent_name: "red-pony".to_string(),
            character_id: "char-1".to_string(),
            diagnosis: "Build succeeded.".to_string(),
            trace: Vec::new(),
            usage: None,
            typewriter: false,
        };
        assert_eq!(ev.character_id(), Some("char-1"));
        assert_eq!(ev.agent_name(), Some("red-pony"));

        let json = serde_json::to_string(&ev).unwrap();
        let decoded: ClawEngineEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ev);
    }
}
