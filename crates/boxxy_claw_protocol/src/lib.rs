use chrono::{DateTime, Utc};
use rig::completion::Usage;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ContextQuality {
    Full,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Off,
    Sleep,
    Waiting,
    Working,
    Locking { resource: String },
    Faulted { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TriggerSource {
    User,
    Swarm { trace_id: Vec<Uuid> },
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionRequest {
    pub target_state: AgentStatus,
    pub source: TriggerSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskType {
    Notification,
    Command,
    Query,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Pending,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub task_type: TaskType,
    pub payload: String,
    pub due_at: DateTime<Utc>,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SpawnLocation {
    NewTab,
    VerticalSplit,
    HorizontalSplit,
}

/// Internal events for agent-to-agent orchestration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClawEvent {
    /// A process exited in a specific pane.
    ProcessExited { pane_id: String, exit_code: i32 },
    /// A specific regex matched the terminal output of a pane.
    OutputMatch {
        pane_id: String,
        matched_text: String,
        regex: String,
    },
    /// A custom event published by an agent.
    Custom {
        source_agent: String,
        name: String,
        payload: String,
    },
}

/// Serializable wrapper for rig::completion::Usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageWrapper {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl From<Usage> for UsageWrapper {
    fn from(u: Usage) -> Self {
        Self {
            input_tokens: u.input_tokens as u64,
            output_tokens: u.output_tokens as u64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PersistentClawRow {
    Diagnosis {
        pane_id: String,
        agent_name: Option<String>,
        content: String,
        usage: Option<UsageWrapper>,
    },
    User {
        pane_id: String,
        content: String,
    },
    Suggested {
        pane_id: String,
        agent_name: Option<String>,
        diagnosis: String,
        command: String,
        usage: Option<UsageWrapper>,
    },
    ProcessList {
        pane_id: String,
        agent_name: Option<String>,
        result_json: String,
        usage: Option<UsageWrapper>,
    },
    ToolCall {
        pane_id: String,
        agent_name: Option<String>,
        tool_name: String,
        result: String,
        usage: Option<UsageWrapper>,
    },
    Command {
        command: String,
        exit_code: i32,
    },
    SystemMessage {
        pane_id: String,
        content: String,
    },
}

/// Messages sent from the GTK UI down to the Claw Engine
#[derive(Debug, Serialize, Deserialize)]
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
    /// A task delegated from another agent (via Correlation ID)
    DelegatedTask {
        source_agent_name: String,
        prompt: String,
        request_id: Uuid,
    },
    /// Response to a RequestScrollback event
    ScrollbackReply {
        request_id: Uuid,
        content: String,
    },
    ForegroundProcessChanged {
        process_name: String,
    },
    ResumeSession {
        session_id: String,
    },
    TogglePin(bool),
    ToggleWebSearch(bool),
    CancelTask {
        task_id: Uuid,
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
}

/// Events emitted from the Claw Engine up to the GTK UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClawEngineEvent {
    Identity {
        agent_name: String,
        pinned: bool,
        web_search_enabled: bool,
        total_tokens: u64,
    },
    AgentThinking {
        is_thinking: bool,
        agent_name: String,
    },
    SessionStateChanged {
        agent_name: String,
        status: AgentStatus,
    },
    UserMessage {
        content: String,
    },
    DiagnosisComplete {
        agent_name: String,
        diagnosis: String,
        usage: Option<UsageWrapper>,
    },
    /// Forces the UI to cleanly close the Claw drawer and reset action states.
    /// Emitted when the agent silently acknowledges a user rejection ([SILENT_ACK]).
    DismissDrawer,
    InjectCommand {
        agent_name: String,
        command: String,
        diagnosis: String,
        usage: Option<UsageWrapper>,
    },
    ProposeFileWrite {
        agent_name: String,
        path: String,
        content: String,
        usage: Option<UsageWrapper>,
    },
    ProposeFileDelete {
        agent_name: String,
        path: String,
        usage: Option<UsageWrapper>,
    },
    ProposeKillProcess {
        agent_name: String,
        pid: u32,
        process_name: String,
        usage: Option<UsageWrapper>,
    },
    ProposeGetClipboard {
        agent_name: String,
        usage: Option<UsageWrapper>,
    },
    ProposeSetClipboard {
        agent_name: String,
        text: String,
        usage: Option<UsageWrapper>,
    },
    ProposeTerminalCommand {
        agent_name: String,
        command: String,
        explanation: String,
        usage: Option<UsageWrapper>,
    },
    ProposalResolved {
        agent_name: String,
        approved: bool,
    },
    SystemMessage {
        text: String,
    },
    RequestSpawnAgent {
        source_agent_name: String,
        location: SpawnLocation,
        intent: Option<String>,
    },
    RequestCloseAgent {
        target_agent_name: String,
    },
    /// Request scrollback content from the UI (via Correlation ID)
    RequestScrollback {
        agent_name: String,
        max_lines: usize,
        offset_lines: usize,
        request_id: Uuid,
    },
    /// Reply to a DelegatedTask message
    DelegatedTaskReply {
        request_id: Uuid,
        result: String,
    },
    InjectKeystrokes {
        target_agent_name: String,
        keys: String,
    },
    RequestCwdSwitch {
        path: String,
    },
    ToolResult {
        agent_name: String,
        tool_name: String,
        result: String,
        usage: Option<UsageWrapper>,
    },
    LazyErrorIndicator {
        visible: bool,
        agent_name: String,
    },
    PinStatusChanged(bool),
    WebSearchStatusChanged(bool),
    TaskStatusChanged {
        tasks: Vec<ScheduledTask>,
        agent_name: String,
    },
    RestoreHistory(Vec<PersistentClawRow>),
    Evicted,
    TaskCompleted {
        agent_name: String,
        task_id: Uuid,
    },
    PushGlobalNotification {
        title: String,
        message: String,
    },
}

#[async_trait::async_trait]
pub trait ClawEnvironment: Send + Sync + 'static {
    async fn exec_shell(&self, command: String) -> anyhow::Result<(i32, String, String)>;
    async fn read_file(
        &self,
        path: String,
        start_line: u32,
        end_line: u32,
    ) -> anyhow::Result<String>;
    async fn write_file(&self, path: String, content: String) -> anyhow::Result<()>;
    async fn list_directory(&self, path: String) -> anyhow::Result<Vec<(String, bool, u64)>>;
    async fn delete_file(&self, path: String) -> anyhow::Result<()>;
    async fn get_system_info(&self) -> anyhow::Result<String>;
    async fn list_processes(&self) -> anyhow::Result<Vec<(u32, String, f64, u64, u64, u64)>>;
    async fn kill_process(&self, pid: u32, signal: i32) -> anyhow::Result<()>;
    async fn get_clipboard(&self) -> anyhow::Result<String>;
    async fn set_clipboard(&self, text: String) -> anyhow::Result<()>;
}
