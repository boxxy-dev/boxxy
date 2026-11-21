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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockApproval, MockEnv};
    use rig::tool::Tool as _;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn footprint_conflict_matrix_allows_parallel_reads_and_serializes_writes_and_approvals() {
        let env: Arc<dyn boxxy_claw_protocol::ClawEnvironment> = Arc::new(MockEnv::new());
        let approval: Arc<dyn ApprovalHandler> =
            Arc::new(MockApproval::approved(Some("/workspace")));

        let file_read = FileReadTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({ "path": "src/lib.rs" })).unwrap());

        let list_dir = ListDirectoryTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({})).unwrap());

        let glob_search = GlobSearchTool {
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({ "pattern": "**/*.rs" })).unwrap());

        let grep_search = GrepSearchTool {
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({ "pattern": "fn " })).unwrap());

        let sys_info = GetSystemInfoTool {
            env: env.clone(),
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({})).unwrap());

        let list_procs = ListProcessesTool {
            env: env.clone(),
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({})).unwrap());

        let http_fetch = HttpFetchTool {
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({ "url": "https://example.com" })).unwrap());

        let file_write = FileWriteTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        }
        .footprint(
            &serde_json::from_value(json!({ "path": "src/lib.rs", "content": "// demo\n" }))
                .unwrap(),
        );

        let replace_in_file = ReplaceInFileTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        }
        .footprint(
            &serde_json::from_value(
                json!({ "path": "src/lib.rs", "old_string": "a", "new_string": "b" }),
            )
            .unwrap(),
        );

        let file_delete = FileDeleteTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({ "path": "src/old.rs" })).unwrap());

        let get_clipboard = GetClipboardTool {
            env: env.clone(),
            approval: approval.clone(),
        }
        .footprint(&serde_json::from_value(json!({})).unwrap());

        // Pure read-only tools never conflict with each other under `ordered_by_footprint(true)`
        let reads = [
            &file_read,
            &list_dir,
            &glob_search,
            &grep_search,
            &sys_info,
            &list_procs,
            &http_fetch,
        ];
        for a in &reads {
            for b in &reads {
                assert!(
                    !a.conflicts_with(b),
                    "expected read-only tools not to conflict: {a:?} vs {b:?}"
                );
            }
        }

        // Mutating and approval-gated tools strictly serialize against reads and each other
        assert!(file_read.conflicts_with(&replace_in_file));
        assert!(file_read.conflicts_with(&file_write));
        assert!(file_read.conflicts_with(&file_delete));
        assert!(file_read.conflicts_with(&get_clipboard));
        assert!(file_write.conflicts_with(&get_clipboard));
        assert!(replace_in_file.conflicts_with(&get_clipboard));
    }
}
