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
    },
    /// The user sent a message from the UI (e.g. popover reply).
    UserMessage {
        message: String,
        snapshot: String,
        cwd: String,
    },
    /// The user clicked Approve or Reject on a file write proposal.
    FileWriteReply {
        approved: bool,
    },
    /// The user requested to diagnose the last failed command (from the Lazy Error Pill).
    RequestLazyDiagnosis,
    /// The user rejected or dismissed a proposal. The agent should cancel any pending tools.
    CancelPending,
    /// The engine should reload its state (database, skills)
    Reload,
}

/// Events sent from the Claw Engine back up to the GTK UI
#[derive(Debug, Clone)]
pub enum ClawEngineEvent {
    /// The agent has finished its diagnosis.
    DiagnosisComplete {
        diagnosis: String,
    },
    /// The agent suggests a command to be injected into the terminal prompt.
    InjectCommand {
        command: String,
        diagnosis: String,
    },
    /// The agent proposes to write or edit a file, requiring user approval.
    ProposeFileWrite {
        path: String,
        content: String,
    },
    /// The agent wants the user to run a command in the terminal and wait for the result.
    ProposeTerminalCommand {
        command: String,
        explanation: String,
    },
    /// Emitted when the agent starts or stops thinking (for UI indicators).
    AgentThinking {
        is_thinking: bool,
    },
    /// Emitted when a command failed but the agent hasn't analyzed it yet (Lazy mode).
    LazyErrorIndicator,
    /// Emitted when a proposal is rejected, dismissed, or otherwise resolved so UIs can sync state.
    ProposalResolved,
}
