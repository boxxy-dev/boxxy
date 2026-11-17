//! High-level structured host, workspace, network, and web-search tools for Boxxy agents.

pub mod approval;
pub mod blacklist;
pub mod clipboard;
pub mod file;
pub mod network;
pub mod search;
pub mod system;
#[cfg(test)]
mod test_support;
pub mod utils;
pub mod workspace_search;

pub use approval::{ApprovalHandler, ApprovalOutcome};
pub use boxxy_claw_protocol::FileDiff;
pub use clipboard::{GetClipboardTool, SetClipboardTool};
pub use file::{
    FileDeleteTool, FileReadTool, FileWriteTool, ListDirectoryTool, ReplaceInFileTool,
};
pub use network::HttpFetchTool;
pub use search::{TavilyProvider, WebSearchTool};
pub use system::{
    GetSystemInfoTool, KillProcessTool, ListProcessesTool, RunBackgroundCommandTool,
};
pub use workspace_search::{GlobSearchTool, GrepSearchTool};

/// Computes a deterministic 64-bit FNV-1a hash of file content for stale-file detection.
#[must_use]
pub fn hash_file_content(content: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in content.as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
