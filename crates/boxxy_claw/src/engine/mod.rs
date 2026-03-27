pub mod agent;
pub mod context;
pub mod dispatcher;
pub mod session;
pub mod tools;

pub use session::ClawSession;

/// Messages sent from the GTK UI down to the Claw Engine
#[derive(Debug)]
pub enum ClawMessage {
    /// A command finished in the terminal. Used for auto-diagnosis and tracking tool executions.
    CommandFinished {
        exit_code: i32,
        snapshot: String,
        cwd: String,
    },
    /// The user explicitly asked Claw a question via `? query` in the terminal.
    ClawQuery {
        query: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>, // Base64 encoded PNGs
    },
    /// The user sent a message from the UI (e.g. popover reply).
    UserMessage {
        message: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
    },
    /// The user clicked Approve or Reject on a file write proposal.
    FileWriteReply { approved: bool },
    /// The user clicked Approve or Reject on a file deletion proposal.
    FileDeleteReply { approved: bool },
    /// The user clicked Approve or Reject on a process kill proposal.
    KillProcessReply { approved: bool },
    /// The user clicked Approve or Reject on a clipboard read proposal.
    GetClipboardReply { approved: bool },
    /// The user clicked Approve or Reject on a clipboard write proposal.
    SetClipboardReply { approved: bool },
    /// The user requested to diagnose the last failed command (from the Lazy Error Pill).
    RequestLazyDiagnosis,
    /// The user rejected or dismissed a proposal. The agent should cancel any pending tools.
    CancelPending,
    /// The engine should initialize or reset its state (new identity, clear history).
    Initialize,
    /// The engine should shut down its resources because Claw mode is deactivated.
    Deactivate,
    /// The engine should reload its state (database, skills)
    Reload,
    /// Update diagnosis mode dynamically.
    UpdateDiagnosisMode(boxxy_preferences::config::ClawAutoDiagnosisMode),
    /// Update terminal suggestions dynamically.
    /// A task delegated from another agent.
    DelegatedTask {
        source_agent_name: String,
        prompt: String,
        reply_tx: tokio::sync::oneshot::Sender<String>,
    },
    /// The foreground process in the terminal changed.
    ForegroundProcessChanged { process_name: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SpawnLocation {
    VerticalSplit,
    HorizontalSplit,
    NewTab,
}

/// Events sent from the Claw Engine back up to the GTK UI
#[derive(Debug, Clone)]
pub enum ClawEngineEvent {
    /// The agent has finished its diagnosis.
    DiagnosisComplete {
        agent_name: String,
        diagnosis: String,
        usage: Option<rig::completion::Usage>,
    },
    /// The agent suggests a command to be injected into the terminal prompt.
    InjectCommand {
        agent_name: String,
        command: String,
        diagnosis: String,
        usage: Option<rig::completion::Usage>,
    },
    /// The agent proposes to write or edit a file, requiring user approval.
    ProposeFileWrite {
        agent_name: String,
        path: String,
        content: String,
        usage: Option<rig::completion::Usage>,
    },
    /// The agent proposes to delete a file, requiring user approval.
    ProposeFileDelete {
        agent_name: String,
        path: String,
        usage: Option<rig::completion::Usage>,
    },
    /// The agent proposes to kill a process, requiring user approval.
    ProposeKillProcess {
        agent_name: String,
        pid: u32,
        process_name: String,
        usage: Option<rig::completion::Usage>,
    },
    /// The agent proposes to read the system clipboard, requiring user approval.
    ProposeGetClipboard {
        agent_name: String,
        usage: Option<rig::completion::Usage>,
    },
    /// The agent proposes to set the system clipboard, requiring user approval.
    ProposeSetClipboard {
        agent_name: String,
        text: String,
        usage: Option<rig::completion::Usage>,
    },
    /// The agent wants the user to run a command in the terminal and wait for the result.
    ProposeTerminalCommand {
        agent_name: String,
        command: String,
        explanation: String,
        usage: Option<rig::completion::Usage>,
    },
    /// Emitted when the agent starts or stops thinking (for UI indicators).
    AgentThinking {
        agent_name: String,
        is_thinking: bool,
    },
    /// Emitted when a command failed but the agent hasn't analyzed it yet (Lazy mode).
    LazyErrorIndicator { agent_name: String },
    /// Emitted when a proposal is rejected, dismissed, or otherwise resolved so UIs can sync state.
    ProposalResolved { agent_name: String },
    /// Emitted when the agentrequests older lines from the terminal's scrollback buffer.
    #[allow(clippy::type_complexity)]
    RequestScrollback {
        agent_name: String,
        max_lines: usize,
        offset_lines: usize,
        reply: std::sync::Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>,
    },
    /// Emitted to announce the agent's identity to the UI.
    Identity { agent_name: String },
    /// Emitted when the agent requests to spawn a new agent in a split or tab.
    RequestSpawnAgent {
        source_agent_name: String,
        location: SpawnLocation,
        intent: Option<String>,
    },
    /// Emitted when the agent requests to close a specific agent's pane.
    RequestCloseAgent { target_agent_name: String },
    /// Emitted when the agent needs to send raw keystrokes to another agent's pane.
    InjectKeystrokes {
        target_agent_name: String,
        keys: String,
    },
    /// Emitted when a tool produces structured output (e.g. process list).
    ToolResult {
        agent_name: String,
        tool_name: String,
        result: String, // JSON
        usage: Option<rig::completion::Usage>,
    },
}
