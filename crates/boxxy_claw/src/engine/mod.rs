pub mod agent;
pub mod context;
pub mod dispatcher;
pub mod session;
pub mod tools;

use boxxy_db::Db;
pub use session::ClawSession;
use std::sync::Arc;
use tokio::sync::Mutex;

pub fn persist_visual_event(
    db_cell: Arc<Mutex<Option<Db>>>,
    session_id: String,
    pane_id: String,
    event: &ClawEngineEvent,
) {
    if let Some(row) = PersistentClawRow::from_engine_event(pane_id, event) {
        tokio::spawn(async move {
            let db_guard = db_cell.lock().await;
            if let Some(db) = &*db_guard {
                let store = boxxy_db::store::Store::new(db.pool());
                if let Ok(json) = serde_json::to_string(&row) {
                    let _ = store.add_claw_event(&session_id, &json).await;
                }
            }
        });
    }
}

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
    /// Mark the session history as visually cleared (soft clear).
    SoftClearHistory,
    /// The engine should initialize or reset its state (new identity, clear history).
    Initialize,
    /// The engine should shut down its resources because Claw mode is deactivated.
    Deactivate,
    /// The agent was evicted because the session was resumed elsewhere.
    Evict,
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
    /// Resume a previously saved session.
    ResumeSession { session_id: String },
    /// Pin or unpin the current session.
    TogglePin(bool),
    /// Cancel a specific scheduled task.
    CancelTask { task_id: uuid::Uuid },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum TaskType {
    Notification,
    Command,
    Query,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum TaskStatus {
    Pending,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScheduledTask {
    pub id: uuid::Uuid,
    pub task_type: TaskType,
    pub payload: String,
    pub due_at: chrono::DateTime<chrono::Utc>,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PersistentClawRow {
    Diagnosis {
        pane_id: String,
        agent_name: Option<String>,
        content: String,
        usage: Option<rig::completion::Usage>,
    },
    Suggested {
        pane_id: String,
        agent_name: Option<String>,
        diagnosis: String,
        command: String,
        usage: Option<rig::completion::Usage>,
    },
    ProcessList {
        pane_id: String,
        agent_name: Option<String>,
        result_json: String,
        usage: Option<rig::completion::Usage>,
    },
}

impl PersistentClawRow {
    #[must_use]
    pub fn from_engine_event(pane_id: String, event: &ClawEngineEvent) -> Option<Self> {
        match event {
            ClawEngineEvent::DiagnosisComplete {
                agent_name,
                diagnosis,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: diagnosis.clone(),
                usage: usage.clone(),
            }),
            ClawEngineEvent::InjectCommand {
                agent_name,
                command,
                diagnosis,
                usage,
                ..
            } => Some(PersistentClawRow::Suggested {
                pane_id,
                agent_name: Some(agent_name.clone()),
                diagnosis: diagnosis.clone(),
                command: command.clone(),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeFileWrite {
                agent_name,
                path,
                content,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed file write to `{path}`:\n```\n{content}\n```"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeFileDelete {
                agent_name,
                path,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed file deletion: `{path}`"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeKillProcess {
                agent_name,
                pid,
                process_name,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed killing process: {process_name} (PID: {pid})"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeGetClipboard {
                agent_name, usage, ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: "Proposed reading from clipboard.".to_string(),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeSetClipboard {
                agent_name,
                text,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed writing to clipboard:\n```\n{text}\n```"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeTerminalCommand {
                agent_name,
                command,
                explanation,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("{explanation}\n\nProposed command:\n```bash\n{command}\n```"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::SystemMessage { text } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: None,
                content: text.clone(),
                usage: None,
            }),
            ClawEngineEvent::ToolResult {
                agent_name,
                tool_name,
                result,
                usage,
                ..
            } if tool_name == "list_processes" => Some(PersistentClawRow::ProcessList {
                pane_id,
                agent_name: Some(agent_name.clone()),
                result_json: result.clone(),
                usage: usage.clone(),
            }),
            _ => None,
        }
    }
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
    Identity {
        agent_name: String,
        pinned: bool,
        total_tokens: u64,
    },
    /// Emitted when the agent was evicted because the session was resumed elsewhere.
    Evicted,
    /// Emitted when the agent requests the UI to switch the terminal's CWD.
    RequestCwdSwitch { path: String },
    /// Emitted to show a generic system message in the sidebar.
    SystemMessage { text: String },
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
    /// Emitted when the set of pending tasks for this agent changes.
    TaskStatusChanged {
        agent_name: String,
        tasks: Vec<ScheduledTask>,
    },
    /// Emitted to restore past interaction history in the sidebar.
    RestoreHistory(Vec<PersistentClawRow>),
    /// Emitted when the pinned status of the session changes.
    PinStatusChanged(bool),
    /// Emitted when a scheduled task has been completed and triggered.
    TaskCompleted { agent_name: String },
}
