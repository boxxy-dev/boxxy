//! Host file deletion tool (`file_delete`).

use crate::ApprovalHandler;
use crate::approval::{await_proposal, begin_tool, finish_tool, resolve_live_cwd};
use crate::utils::resolve_path;
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct FileDeleteArgs {
    pub path: String,
}

#[derive(Serialize)]
pub struct FileDeleteOutput {
    pub success: bool,
    /// Explicit error feedback for the LLM to prevent retry loops on user rejection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct FileDeleteTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for FileDeleteTool {
    const NAME: &'static str = "file_delete";

    type Error = std::io::Error;
    type Args = FileDeleteArgs;
    type Output = FileDeleteOutput;

    fn description(&self) -> String {
        "Delete a file from the host system. This action is permanent and prompts the user for approval.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to delete."
                }
            },
            "required": ["path"]
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
            let out = FileDeleteOutput {
                success: false,
                error: Some("`path` must not be empty.".to_string()),
            };
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let base_dir = resolve_live_cwd(self.approval.as_ref(), &self.current_dir).await;
        let path = resolve_path(&base_dir, raw_path);

        let outcome = await_proposal(
            self.approval.as_ref(),
            self.approval.propose_file_delete(path.clone()),
        )
        .await;

        let out = if outcome.is_approved() {
            match self.env.delete_file(path).await {
                Ok(()) => FileDeleteOutput {
                    success: true,
                    error: None,
                },
                Err(e) => FileDeleteOutput {
                    success: false,
                    error: Some(format!("Environment Error: {e}")),
                },
            }
        } else {
            FileDeleteOutput {
                success: false,
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
    use crate::file::{FileWriteArgs, FileWriteTool};
    use crate::test_support::{MockApproval, MockEnv};

    #[tokio::test]
    async fn file_write_and_delete_return_explicit_reject_token() {
        let mut ctx = ToolContext::default();
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::with_outcome(
            ApprovalOutcome::Rejected,
            Some("/workspace"),
        ));

        let write_tool = FileWriteTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };
        let write_out = write_tool
            .call(
                &mut ctx,
                FileWriteArgs {
                    path: "a.txt".to_string(),
                    content: "hello".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(!write_out.success);
        assert_eq!(write_out.error.as_deref(), Some("[USER_EXPLICIT_REJECT]"));

        let delete_tool = FileDeleteTool {
            env,
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };
        let delete_out = delete_tool
            .call(
                &mut ctx,
                FileDeleteArgs {
                    path: "a.txt".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(!delete_out.success);
        assert_eq!(delete_out.error.as_deref(), Some("[USER_EXPLICIT_REJECT]"));
        assert_eq!(approval.results.lock().len(), 2);
    }
}
