//! Full-file write tool (`file_write`) with stale-file protection and pre-approval diffing.

use crate::ApprovalHandler;
use crate::approval::{await_proposal, begin_tool, finish_tool, resolve_live_cwd};
use crate::utils::resolve_path;
use boxxy_claw_protocol::{ClawEnvironment, FileDiff};
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct FileWriteArgs {
    pub path: String,
    pub content: String,
}

#[derive(Serialize)]
pub struct FileWriteOutput {
    pub success: bool,
    pub total_lines: usize,
    /// Explicit error feedback for the LLM so user rejections or stale-file errors do not loop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct FileWriteTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for FileWriteTool {
    const NAME: &'static str = "file_write";

    type Error = std::io::Error;
    type Args = FileWriteArgs;
    type Output = FileWriteOutput;

    fn description(&self) -> String {
        "Write content to a file on the host system, overwriting if it already exists. Missing parent directories are created automatically (do not run `mkdir -p` beforehand). Always prompts the user for approval. For partial changes to existing files, use `replace_in_file` instead.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to write."
                },
                "content": {
                    "type": "string",
                    "description": "The new content for the file."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let raw_path = args.path.trim();
        if raw_path.is_empty() {
            let out = FileWriteOutput {
                success: false,
                total_lines: 0,
                error: Some("`path` must not be empty.".to_string()),
            };
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let base_dir = resolve_live_cwd(self.approval.as_ref(), &self.current_dir).await;
        let path = resolve_path(&base_dir, raw_path);

        let existing_res = self.env.read_file(path.clone(), 0, 0).await;
        if let Ok(ref existing) = existing_res
            && let Some(stale_err) = self
                .approval
                .check_stale_file_before_write(&path, Some(existing))
                .await
        {
            let out = FileWriteOutput {
                success: false,
                total_lines: 0,
                error: Some(stale_err),
            };
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        // Move buffers into `spawn_blocking` and return them back to avoid cloning multi-MB files
        let path_for_diff = path.clone();
        let content = args.content;
        let (diff, previous_content, content) = match tokio::task::spawn_blocking(move || {
            let (diff, prev) = match existing_res {
                Ok(existing) => {
                    let d = FileDiff::compute(&path_for_diff, Some(&existing), &content);
                    (d, Some(existing))
                }
                Err(e) if e.to_string().contains("appears to be binary") => {
                    (FileDiff::binary(&path_for_diff), None)
                }
                Err(_) => (FileDiff::compute(&path_for_diff, None, &content), None),
            };
            (diff, prev, content)
        })
        .await
        {
            Ok(tuple) => tuple,
            Err(e) => {
                let out = FileWriteOutput {
                    success: false,
                    total_lines: 0,
                    error: Some(format!("Diff computation failed: {e}")),
                };
                finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let outcome = await_proposal(
            self.approval.as_ref(),
            self.approval
                .propose_file_write(path.clone(), content.clone(), diff),
        )
        .await;

        let out = if outcome.is_approved() {
            let total_lines = content.lines().count();
            let new_hash = crate::hash_file_content(&content);
            match self.env.write_file(path.clone(), content).await {
                Ok(()) => {
                    // Only commit `/undo` snapshot & hash after disk write succeeds
                    self.approval
                        .record_file_snapshot_before_write(path, previous_content, new_hash)
                        .await;
                    FileWriteOutput {
                        success: true,
                        total_lines,
                        error: None,
                    }
                }
                Err(e) => FileWriteOutput {
                    success: false,
                    total_lines: 0,
                    error: Some(format!("Environment Error: {e}")),
                },
            }
        } else {
            FileWriteOutput {
                success: false,
                total_lines: 0,
                error: Some(outcome.rejection_message()),
            }
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApprovalOutcome;
    use crate::test_support::{MockApproval, MockEnv};

    #[tokio::test]
    async fn file_write_returns_user_interruption_payload_when_steered() {
        let mut ctx = ToolContext::default();
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::with_outcome(
            ApprovalOutcome::Interrupted("use a generic greeting instead".to_string()),
            Some("/workspace"),
        ));
        let write_tool = FileWriteTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval,
        };
        let out = write_tool
            .call(
                &mut ctx,
                FileWriteArgs {
                    path: "script.sh".to_string(),
                    content: "printf 'Hello, world!'".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(!out.success);
        let err = out.error.unwrap();
        assert!(err.contains("[USER_INTERRUPTION]"));
        assert!(err.contains("use a generic greeting instead"));
        assert!(env.written_content.lock().is_none());
    }

    #[tokio::test]
    async fn file_write_computes_created_and_modified_diffs() {
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::approved(Some("/workspace")));
        let tool = FileWriteTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        tool.call(
            &mut ctx,
            FileWriteArgs {
                path: "src/new_mod.rs".to_string(),
                content: "pub fn hello() {}\n".to_string(),
            },
        )
        .await
        .unwrap();

        *env.file_content.lock() = "pub fn hello() {\n    old();\n}\n".to_string();
        tool.call(
            &mut ctx,
            FileWriteArgs {
                path: "src/existing_mod.rs".to_string(),
                content: "pub fn hello() {\n    new();\n}\n".to_string(),
            },
        )
        .await
        .unwrap();

        {
            let captured = approval.diffs.lock();
            assert_eq!(captured.len(), 2);
            assert_eq!(captured[0].kind, boxxy_claw_protocol::DiffKind::Created);
            assert_eq!(captured[0].stats.added_lines, 1);
            assert_eq!(captured[1].kind, boxxy_claw_protocol::DiffKind::Modified);
            assert_eq!(captured[1].stats.added_lines, 1);
            assert_eq!(captured[1].stats.removed_lines, 1);
        }

        // Overwriting `src/new_mod.rs` after an external disk edit is blocked as stale
        let stale = tool
            .call(
                &mut ctx,
                FileWriteArgs {
                    path: "src/new_mod.rs".to_string(),
                    content: "pub fn overwrite() {}\n".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(!stale.success);
        assert!(stale.error.unwrap().contains("modified on disk"));
    }

    #[tokio::test]
    async fn file_write_does_not_poison_snapshot_when_disk_write_fails() {
        let env = Arc::new(MockEnv::new());
        *env.write_error.lock() = Some("Permission denied".to_string());
        let approval = Arc::new(MockApproval::approved(Some("/workspace")));
        let tool = FileWriteTool {
            env,
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        let out = tool
            .call(
                &mut ctx,
                FileWriteArgs {
                    path: "src/locked.rs".to_string(),
                    content: "pub fn demo() {}\n".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.error.unwrap().contains("Permission denied"));
        assert!(approval.snapshots.lock().is_empty());
        assert_eq!(approval.results.lock().len(), 1);
    }
}
