pub mod character_loader;
pub mod characters;

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageWrapper {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Tokens occupying the model's context window on the final completion call of a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Maximum context window of the model that served the turn, if known from the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

impl From<Usage> for UsageWrapper {
    fn from(u: Usage) -> Self {
        let input = u.input_tokens.unwrap_or(0);
        let output = u.output_tokens.unwrap_or(0);
        let total = u.total_tokens.unwrap_or(input + output);
        Self {
            input_tokens: input,
            output_tokens: output,
            reasoning_tokens: u.reasoning_tokens.filter(|&r| r > 0),
            cached_input_tokens: u.cached_input_tokens.filter(|&c| c > 0),
            cost_usd: u.cost.map(|c| c.total),
            context_tokens: (total > 0).then_some(total),
            context_window: None,
        }
    }
}

impl UsageWrapper {
    /// Total tokens across this turn (`input_tokens + output_tokens`).
    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Context window footprint of the final call in this turn, falling back to `total_tokens()`.
    #[must_use]
    pub fn effective_context_tokens(&self) -> u64 {
        self.context_tokens.unwrap_or_else(|| self.total_tokens())
    }
}

/// Live session-level context and cost metrics displayed in the Claw sidebar.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionUsage {
    /// Tokens in the current context window (from the latest completion request).
    pub context_tokens: u64,
    /// Maximum context window of the active model, if known from the catalog.
    pub context_window: Option<u64>,
    /// Cumulative tokens consumed across all turns in the session.
    pub total_tokens: u64,
    /// Cumulative cost in USD across all turns in the session.
    pub cost_usd: f64,
    /// Number of tool calls executed in the session.
    #[serde(default)]
    pub tools_used: u64,
}

impl SessionUsage {
    /// Context window utilization percentage (rounded to nearest integer, matching OpenCode),
    /// or `None` if the model's context window is unknown.
    #[must_use]
    pub fn context_percent(&self) -> Option<u64> {
        let limit = self.context_window.filter(|&w| w > 0)?;
        let tokens = self.display_tokens();
        Some(((tokens as f64 / limit as f64) * 100.0).round() as u64)
    }

    /// Tokens to display in the Context header (`context_tokens` when non-zero, else `total_tokens`).
    #[must_use]
    pub const fn display_tokens(&self) -> u64 {
        if self.context_tokens > 0 {
            self.context_tokens
        } else {
            self.total_tokens
        }
    }

    ///Applies a turn's `UsageWrapper` to this session accumulator.
    pub fn record_turn(&mut self, usage: &UsageWrapper) {
        let turn_total = usage.total_tokens();
        self.total_tokens = self.total_tokens.saturating_add(turn_total);
        let ctx = usage.effective_context_tokens();
        if ctx > 0 {
            self.context_tokens = ctx;
        }
        if let Some(window) = usage.context_window {
            self.context_window = Some(window);
        }
        if let Some(cost) = usage.cost_usd
            && cost.is_finite()
            && cost > 0.0
        {
            self.cost_usd += cost;
        }
    }
}

/// Formats an integer with ASCII comma thousands separators (e.g. `25236` -> `"25,236"`).
#[must_use]
pub fn format_number_with_commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Formats a USD amount like OpenCode's `Intl.NumberFormat("en-US", { style: "currency", currency: "USD" })`
/// (e.g. `0.05` -> `"$0.05"`).
#[must_use]
pub fn format_usd(amount: f64) -> String {
    let safe = if amount.is_finite() && amount > 0.0 {
        amount
    } else {
        0.0
    };
    format!("${safe:.2}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PersistentClawRow {
    Diagnosis {
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
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
        character_id: Option<String>,
        diagnosis: String,
        command: String,
        usage: Option<UsageWrapper>,
    },
    ProcessList {
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        result_json: String,
        usage: Option<UsageWrapper>,
    },
    ToolCall {
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
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

impl PersistentClawRow {
    pub fn is_overlay_visible(&self) -> bool {
        match self {
            Self::User { .. }
            | Self::Diagnosis { .. }
            | Self::Suggested { .. }
            | Self::SystemMessage { .. }
            | Self::Command { .. } => true,
            Self::ToolCall { .. } | Self::ProcessList { .. } => false,
        }
    }
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
    BackgroundCommandReply {
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
        character_id: String,
        pinned: bool,
        web_search_enabled: bool,
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
    },
    DiagnosisComplete {
        agent_name: String,
        character_id: String,
        diagnosis: String,
        usage: Option<UsageWrapper>,
    },
    /// Forces the UI to cleanly close the Claw drawer and reset action states.
    /// Emitted when the agent silently acknowledges a user rejection ([SILENT_ACK]).
    DismissDrawer,
    /// Requests that the UI raises the window and focuses this specific pane.
    FocusPane,
    InjectCommand {
        agent_name: String,
        character_id: String,
        command: String,
        diagnosis: String,
        usage: Option<UsageWrapper>,
    },
    ProposeFileWrite {
        agent_name: String,
        character_id: String,
        path: String,
        content: String,
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
    },
    RequestCloseAgent {
        target_agent_name: String,
    },
    /// Request scrollback content from the UI (via Correlation ID)
    RequestScrollback {
        agent_name: String,
        character_id: String,
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

#[async_trait::async_trait]
pub trait ClawEnvironment: Send + Sync + 'static {
    async fn exec_shell(&self, command: String) -> anyhow::Result<(i32, String, String)>;
    async fn spawn_detached(&self, command: String, cwd: String) -> anyhow::Result<u32>;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_wrapper_from_rig_usage() {
        let usage = Usage::new()
            .input_tokens(150)
            .output_tokens(50)
            .reasoning_tokens(20)
            .cached_input_tokens(100);

        let wrapper = UsageWrapper::from(usage);
        assert_eq!(wrapper.input_tokens, 150);
        assert_eq!(wrapper.output_tokens, 50);
        assert_eq!(wrapper.reasoning_tokens, Some(20));
        assert_eq!(wrapper.cached_input_tokens, Some(100));
        assert_eq!(wrapper.cost_usd, None);
        assert_eq!(wrapper.context_tokens, Some(200));
        assert_eq!(wrapper.context_window, None);

        let empty_usage = Usage::default();
        let empty_wrapper = UsageWrapper::from(empty_usage);
        assert_eq!(empty_wrapper.input_tokens, 0);
        assert_eq!(empty_wrapper.output_tokens, 0);
        assert_eq!(empty_wrapper.reasoning_tokens, None);
        assert_eq!(empty_wrapper.cached_input_tokens, None);
        assert_eq!(empty_wrapper.context_tokens, None);
    }

    #[test]
    fn test_session_usage_and_formatting() {
        assert_eq!(format_number_with_commas(0), "0");
        assert_eq!(format_number_with_commas(999), "999");
        assert_eq!(format_number_with_commas(1_000), "1,000");
        assert_eq!(format_number_with_commas(25_236), "25,236");
        assert_eq!(format_number_with_commas(1_048_576), "1,048,576");

        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(0.0512), "$0.05");
        assert_eq!(format_usd(1.2), "$1.20");

        let mut su = SessionUsage::default();
        assert_eq!(su.context_percent(), None);

        su.record_turn(&UsageWrapper {
            input_tokens: 20_000,
            output_tokens: 5_236,
            reasoning_tokens: None,
            cached_input_tokens: None,
            cost_usd: Some(0.05),
            context_tokens: Some(25_236),
            context_window: Some(1_048_576),
        });

        assert_eq!(su.total_tokens, 25_236);
        assert_eq!(su.context_tokens, 25_236);
        assert_eq!(su.context_window, Some(1_048_576));
        assert_eq!(su.context_percent(), Some(2));
        assert!((su.cost_usd - 0.05).abs() < 1e-9);
    }
}
