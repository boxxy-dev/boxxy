pub mod clipboard;
pub mod file;
pub mod network;
pub mod search;
pub mod system;
pub mod utils;
pub mod workspace_search;

pub use boxxy_claw_protocol::FileDiff;
pub use clipboard::{GetClipboardTool, SetClipboardTool};
pub use file::{
    FileDeleteTool, FileReadTool, FileWriteTool, ListDirectoryTool, ReplaceInFileTool,
};
pub use network::HttpFetchTool;
pub use search::{TavilyProvider, WebSearchTool};
pub use system::{GetSystemInfoTool, KillProcessTool, ListProcessesTool, RunBackgroundCommandTool};
pub use workspace_search::{GlobSearchTool, GrepSearchTool};

/// Result of a user approval prompt for a sensitive tool action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// User clicked Approve.
    Approved,
    /// User clicked Reject (or dismissed the proposal).
    Rejected,
    /// User typed a new message/instruction instead of approving or rejecting.
    Interrupted(String),
}

impl ApprovalOutcome {
    #[inline]
    pub fn is_approved(&self) -> bool {
        matches!(self, Self::Approved)
    }

    /// Formats the error string returned to the LLM when the action was not approved.
    pub fn rejection_message(&self) -> String {
        match self {
            Self::Approved => String::new(),
            Self::Rejected => "[USER_EXPLICIT_REJECT]".to_string(),
            Self::Interrupted(msg) => format!(
                "[USER_INTERRUPTION]: The user did not approve this proposal and instead sent a new instruction: \"{msg}\". Do NOT retry the rejected proposal; address the user's new instruction immediately."
            ),
        }
    }
}

/// Trait for handling user approvals for sensitive tools.
/// Implemented by boxxy-claw to bridge with the UI.
#[async_trait::async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// Propose writing or editing a file on the host system, supplying a pre-computed `FileDiff`.
    async fn propose_file_write(
        &self,
        path: String,
        content: String,
        diff: FileDiff,
    ) -> ApprovalOutcome;
    /// Propose deleting a file from the host system.
    async fn propose_file_delete(&self, path: String) -> ApprovalOutcome;
    /// Propose running a command in the background.
    async fn propose_background_command(
        &self,
        command: String,
        explanation: String,
    ) -> ApprovalOutcome;
    /// Propose killing a process on the host system.
    async fn propose_kill_process(&self, pid: u32, process_name: String) -> ApprovalOutcome;
    /// Propose reading the system clipboard.
    async fn propose_get_clipboard(&self) -> ApprovalOutcome;
    /// Propose setting the system clipboard.
    async fn propose_set_clipboard(&self, text: String) -> ApprovalOutcome;
    /// Report a structured tool result (e.g. process list).
    async fn report_tool_result(&self, tool_name: String, result: String);
    /// Report that a tool call has started.
    async fn report_tool_started(&self, tool_name: String);
    /// Update the agent's thinking status in the UI.
    async fn set_thinking(&self, thinking: bool);
    /// Return the live working directory for the current session/pane, if available.
    async fn current_dir(&self) -> Option<String> {
        None
    }
}
