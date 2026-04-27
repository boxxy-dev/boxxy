pub mod clipboard;
pub mod file;
pub mod network;
pub mod search;
pub mod system;
pub mod utils;

pub use clipboard::{GetClipboardTool, SetClipboardTool};
pub use file::{FileDeleteTool, FileReadTool, FileWriteTool, ListDirectoryTool};
pub use network::HttpFetchTool;
pub use search::{TavilyProvider, WebSearchTool};
pub use system::{GetSystemInfoTool, KillProcessTool, ListProcessesTool};

/// Trait for handling user approvals for sensitive tools.
/// Implemented by boxxy-claw to bridge with the UI.
#[async_trait::async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// Propose writing a file to the host system.
    async fn propose_file_write(&self, path: String, content: String) -> bool;
    /// Propose deleting a file from the host system.
    async fn propose_file_delete(&self, path: String) -> bool;
    /// Propose killing a process on the host system.
    async fn propose_kill_process(&self, pid: u32, process_name: String) -> bool;
    /// Propose reading the system clipboard.
    async fn propose_get_clipboard(&self) -> bool;
    /// Propose setting the system clipboard.
    async fn propose_set_clipboard(&self, text: String) -> bool;
    /// Report a structured tool result (e.g. process list).
    async fn report_tool_result(&self, tool_name: String, result: String);
    /// Report that a tool call has started.
    async fn report_tool_started(&self, tool_name: String);
    /// Update the agent's thinking status in the UI.
    async fn set_thinking(&self, thinking: bool);
}
