//! Bounded file reading tool (`file_read`) with stale-edit hash registration.

use crate::ApprovalHandler;
use crate::approval::{begin_tool, finish_tool, resolve_live_cwd};
use crate::utils::resolve_path;
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const DEFAULT_MAX_LINES: u32 = 500;

#[derive(Deserialize)]
pub struct FileReadArgs {
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

#[derive(Serialize)]
pub struct FileReadOutput {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct FileReadTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for FileReadTool {
    const NAME: &'static str = "file_read";

    type Error = std::io::Error;
    type Args = FileReadArgs;
    type Output = FileReadOutput;

    fn description(&self) -> String {
        "Read the contents of a file on the host system. Use this to inspect code, configuration, or logs before modifying them. By default reads up to 500 lines starting at line 1; pass `start_line` and `end_line` to inspect specific sections of large files.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to read."
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional: The 1-based line number to start reading from (inclusive, default: 1)."
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional: The 1-based line number to end reading at (inclusive, default: start_line + 499)."
                }
            },
            "required": ["path"]
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

        let raw_path = args.path.trim();
        if raw_path.is_empty() {
            let out = FileReadOutput {
                content: String::new(),
                error: Some("`path` must not be empty.".to_string()),
            };
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let start_line = args.start_line.unwrap_or(1).max(1);
        let end_line = match args.end_line {
            Some(0) => {
                let out = FileReadOutput {
                    content: String::new(),
                    error: Some(
                        "`end_line` is 1-based and must be >= `start_line` (not 0).".to_string(),
                    ),
                };
                finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
                return Ok(out);
            }
            Some(end) if end < start_line => {
                let out = FileReadOutput {
                    content: String::new(),
                    error: Some(format!(
                        "`end_line` ({end}) must be greater than or equal to `start_line` ({start_line})."
                    )),
                };
                finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
                return Ok(out);
            }
            Some(end) => end,
            None => start_line.saturating_add(DEFAULT_MAX_LINES - 1),
        };

        let base_dir = resolve_live_cwd(self.approval.as_ref(), &self.current_dir).await;
        let path = resolve_path(&base_dir, raw_path);

        let out = match self.env.read_file(path.clone(), start_line, end_line).await {
            Ok(content) => {
                if let Ok(full_raw) = self.env.read_file(path.clone(), 0, 0).await {
                    self.approval
                        .record_file_read(path, crate::hash_file_content(&full_raw))
                        .await;
                }
                FileReadOutput {
                    content,
                    error: None,
                }
            }
            Err(e) => FileReadOutput {
                content: String::new(),
                error: Some(e.to_string()),
            },
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockApproval, MockEnv};

    #[tokio::test]
    async fn file_read_uses_live_current_dir_when_updated() {
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::approved(Some("/workspace/initial")));
        let tool = FileReadTool {
            env,
            current_dir: "/workspace/stale".to_string(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        let out1 = tool
            .call(
                &mut ctx,
                FileReadArgs {
                    path: "Cargo.toml".to_string(),
                    start_line: None,
                    end_line: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(out1.content, "read:/workspace/initial/Cargo.toml:1-500");

        *approval.live_cwd.lock() = Some("/workspace/subdir".to_string());
        let out2 = tool
            .call(
                &mut ctx,
                FileReadArgs {
                    path: "./src/lib.rs".to_string(),
                    start_line: Some(10),
                    end_line: Some(30),
                },
            )
            .await
            .unwrap();
        assert_eq!(out2.content, "read:/workspace/subdir/src/lib.rs:10-30");
    }

    #[tokio::test]
    async fn file_read_rejects_inverted_and_zero_end_line_ranges() {
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::approved(Some("/workspace")));
        let tool = FileReadTool {
            env,
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        let inverted = tool
            .call(
                &mut ctx,
                FileReadArgs {
                    path: "src/module.rs".to_string(),
                    start_line: Some(25),
                    end_line: Some(10),
                },
            )
            .await
            .unwrap();
        assert!(inverted.error.unwrap().contains("must be greater than"));

        let zero_end = tool
            .call(
                &mut ctx,
                FileReadArgs {
                    path: "src/module.rs".to_string(),
                    start_line: Some(1),
                    end_line: Some(0),
                },
            )
            .await
            .unwrap();
        assert!(zero_end.error.unwrap().contains("1-based"));
        assert_eq!(approval.results.lock().len(), 2);
    }
}
