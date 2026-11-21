//! Deterministic directory listing tool (`list_directory`).

use crate::ApprovalHandler;
use crate::approval::{begin_tool, finish_tool, resolve_live_cwd};
use crate::utils::{cmp_case_insensitive, resolve_dir_path};
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const MAX_DIR_ENTRIES: usize = 250;

#[derive(Deserialize)]
pub struct ListDirectoryArgs {
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Serialize)]
pub struct ListDirectoryOutput {
    pub entries: Vec<DirectoryEntry>,
    pub total_entries: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct ListDirectoryTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for ListDirectoryTool {
    const NAME: &'static str = "list_directory";

    type Error = std::io::Error;
    type Args = ListDirectoryArgs;
    type Output = ListDirectoryOutput;

    fn description(&self) -> String {
        "List the contents of a directory on the host system (sorted with directories first, capped at 250 entries). Use this to discover files and subdirectories.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list (default: current directory)."
                }
            }
        })
    }

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        rig::completion::ToolAnnotations::default()
            .with_read_only(true)
            .with_open_world(false)
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let base_dir = resolve_live_cwd(self.approval.as_ref(), &self.current_dir).await;
        let path = resolve_dir_path(&base_dir, args.path.as_deref());

        let output = match self.env.list_directory(path).await {
            Ok(mut entries) => {
                // Sort directories first, then allocation-free case-insensitive name order
                entries.sort_by(|a, b| {
                    b.1.cmp(&a.1)
                        .then_with(|| cmp_case_insensitive(&a.0, &b.0))
                });
                let total_entries = entries.len();
                let truncated = total_entries > MAX_DIR_ENTRIES;
                ListDirectoryOutput {
                    entries: entries
                        .into_iter()
                        .take(MAX_DIR_ENTRIES)
                        .map(|(name, is_dir, size)| DirectoryEntry { name, is_dir, size })
                        .collect(),
                    total_entries,
                    truncated,
                    error: None,
                }
            }
            Err(e) => ListDirectoryOutput {
                entries: Vec::new(),
                total_entries: 0,
                truncated: false,
                error: Some(e.to_string()),
            },
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &output).await;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockApproval, MockEnv};

    #[tokio::test]
    async fn list_directory_sorts_dirs_first_and_truncates_at_250() {
        let env = Arc::new(MockEnv::new());
        let mut raw = vec![
            ("zebra.txt".to_string(), false, 10),
            ("alpha.txt".to_string(), false, 20),
            ("src".to_string(), true, 0),
            ("assets".to_string(), true, 0),
        ];
        for i in 0..260 {
            raw.push((format!("file_{i:03}.txt"), false, 1));
        }
        *env.dir_entries.lock() = raw;

        let approval = Arc::new(MockApproval::approved(Some("/workspace")));
        let tool = ListDirectoryTool {
            env,
            current_dir: "/workspace".to_string(),
            approval,
        };

        let mut ctx = ToolContext::default();
        let out = tool
            .call(&mut ctx, ListDirectoryArgs { path: None })
            .await
            .unwrap();
        assert_eq!(out.total_entries, 264);
        assert!(out.truncated);
        assert_eq!(out.entries.len(), 250);
        assert_eq!(out.entries[0].name, "assets");
        assert!(out.entries[0].is_dir);
        assert_eq!(out.entries[1].name, "src");
        assert!(out.entries[1].is_dir);
        assert_eq!(out.entries[2].name, "alpha.txt");
        assert!(!out.entries[2].is_dir);
    }
}
