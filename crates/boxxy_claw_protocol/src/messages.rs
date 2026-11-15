//! Bidirectional IPC envelopes between the GTK UI (`ClawMessage`) and the Claw Engine (`ClawEngineEvent`).

use crate::{
    AgentStatus, ClawEvent, FileDiff, PersistentClawRow, ScheduledTask, SpawnLocation,
    TransitionRequest, TurnTraceItem, UsageWrapper,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
        image_attachments: Vec<String>,
    },
    UserMessage {
        message: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
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
}
