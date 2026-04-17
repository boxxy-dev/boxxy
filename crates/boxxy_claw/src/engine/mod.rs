pub mod agent;
pub mod context;
pub mod dispatcher;
pub mod fsm;
pub mod session;
pub mod summarization;
pub mod tools;
pub mod turn;

pub use fsm::state::*;

use boxxy_db::Db;
use gtk4::glib;
use gtk4::subclass::prelude::*;
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
            let db_val = {
                let db_guard = db_cell.lock().await;
                db_guard.as_ref().cloned()
            };
            if let Some(db) = db_val {
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
    /// Request a state transition via the FSM.
    Transition(TransitionRequest),
    /// Fired when a state watchdog expires.
    WatchdogTimeout {
        task_id: uuid::Uuid,
        state: AgentStatus,
    },
    /// Fired when the Dreamer finishes summarizing a Hard Wake delta.
    WakeSummaryComplete {
        result: Result<String, String>,
    },
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
    /// Manually trigger Sleep mode or Wake up.
    SleepToggle(bool),
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
    /// Toggle web search for the current session.
    ToggleWebSearch(bool),
    /// Cancel a specific scheduled task.
    CancelTask { task_id: uuid::Uuid },
    /// An event from the internal EventBus has occurred.
    SubscriptionEvent {
        event: ClawEvent,
    },
    /// A delegated async task has completed.
    TaskCompletedEvent {
        task_id: uuid::Uuid,
        result: String,
    },
    /// Explicitly abort the current turn and clear the queue.
    Abort,
    /// Internal message sent when a background turn finishes.
    TurnFinished,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum SpawnLocation {
    NewTab,
    VerticalSplit,
    HorizontalSplit,
}

/// Internal events for agent-to-agent orchestration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ClawEvent {
    /// A process exited in a specific pane.
    ProcessExited {
        pane_id: String,
        exit_code: i32,
    },
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

mod imp {
    use super::*;
    use gtk4::glib;
    use gtk4::prelude::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct ClawRowObject {
        pub row: RefCell<Option<PersistentClawRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ClawRowObject {
        const NAME: &'static str = "ClawRowObject";
        type Type = super::ClawRowObject;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for ClawRowObject {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: std::sync::LazyLock<Vec<glib::ParamSpec>> =
                std::sync::LazyLock::new(|| {
                    vec![glib::ParamSpecString::builder("content").build()]
                });
            PROPERTIES.as_ref()
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "content" => {
                    let row = self.row.borrow();
                    match row.as_ref().unwrap() {
                        PersistentClawRow::Diagnosis { content, .. } => content.to_value(),
                        PersistentClawRow::User { content, .. } => content.to_value(),
                        PersistentClawRow::Suggested { diagnosis, .. } => diagnosis.to_value(),
                        PersistentClawRow::ProcessList { result_json, .. } => {
                            result_json.to_value()
                        }
                        PersistentClawRow::ToolCall { result, .. } => result.to_value(),
                        PersistentClawRow::Command { command, .. } => command.to_value(),
                    }
                }
                _ => unimplemented!(),
            }
        }
    }
}

glib::wrapper! {
    pub struct ClawRowObject(ObjectSubclass<imp::ClawRowObject>);
}

impl ClawRowObject {
    pub fn new(row: PersistentClawRow) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().row.replace(Some(row));
        obj
    }

    pub fn get_row(&self) -> PersistentClawRow {
        self.imp().row.borrow().as_ref().unwrap().clone()
    }
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
    User {
        pane_id: String,
        content: String,
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
    ToolCall {
        pane_id: String,
        agent_name: Option<String>,
        tool_name: String,
        result: String,
        usage: Option<rig::completion::Usage>,
    },
    Command {
        command: String,
        exit_code: i32,
    },
}

impl PersistentClawRow {
    #[must_use]
    pub fn from_engine_event(pane_id: String, event: &ClawEngineEvent) -> Option<Self> {
        match event {
            ClawEngineEvent::UserMessage { content, .. } => Some(PersistentClawRow::User {
                pane_id,
                content: content.clone(),
            }),
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
            ClawEngineEvent::ProposeGetClipboard { agent_name, usage } => {
                Some(PersistentClawRow::Diagnosis {
                    pane_id,
                    agent_name: Some(agent_name.clone()),
                    content: "Proposed reading from clipboard.".to_string(),
                    usage: usage.clone(),
                })
            }
            ClawEngineEvent::ProposeSetClipboard {
                agent_name,
                text,
                usage,
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed writing to clipboard:\n```\n{text}\n```"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeTerminalCommand {
                command,
                explanation,
                agent_name,
                usage,
            } => Some(PersistentClawRow::Suggested {
                pane_id,
                agent_name: Some(agent_name.clone()),
                diagnosis: explanation.clone(),
                command: command.clone(),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ToolResult {
                agent_name,
                tool_name,
                result,
                usage,
            } => {
                if tool_name == "list_processes" {
                    Some(PersistentClawRow::ProcessList {
                        pane_id,
                        agent_name: Some(agent_name.clone()),
                        result_json: result.clone(),
                        usage: usage.clone(),
                    })
                } else {
                    Some(PersistentClawRow::ToolCall {
                        pane_id,
                        agent_name: Some(agent_name.clone()),
                        tool_name: tool_name.clone(),
                        result: result.clone(),
                        usage: usage.clone(),
                    })
                }
            }
            _ => None,
        }
    }
}

/// Events emitted from the Claw Engine up to the GTK UI
#[derive(Debug, Clone)]
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
    /// Broad state change for the agent (Sleeping, Awaiting, Locking, etc.)
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
        usage: Option<rig::completion::Usage>,
    },
    InjectCommand {
        agent_name: String,
        command: String,
        diagnosis: String,
        usage: Option<rig::completion::Usage>,
    },
    ProposeFileWrite {
        agent_name: String,
        path: String,
        content: String,
        usage: Option<rig::completion::Usage>,
    },
    ProposeFileDelete {
        agent_name: String,
        path: String,
        usage: Option<rig::completion::Usage>,
    },
    ProposeKillProcess {
        agent_name: String,
        pid: u32,
        process_name: String,
        usage: Option<rig::completion::Usage>,
    },
    ProposeGetClipboard {
        agent_name: String,
        usage: Option<rig::completion::Usage>,
    },
    ProposeSetClipboard {
        agent_name: String,
        text: String,
        usage: Option<rig::completion::Usage>,
    },
    ProposeTerminalCommand {
        agent_name: String,
        command: String,
        explanation: String,
        usage: Option<rig::completion::Usage>,
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
    RequestScrollback {
        agent_name: String,
        max_lines: usize,
        offset_lines: usize,
        reply: Arc<Mutex<Option<tokio::sync::oneshot::Sender<String>>>>,
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
        usage: Option<rig::completion::Usage>,
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
        task_id: uuid::Uuid,
    },
    PushGlobalNotification {
        title: String,
        message: String,
    },
}
