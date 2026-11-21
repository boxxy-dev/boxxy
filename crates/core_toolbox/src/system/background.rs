//! Detached background application and server launcher tool (`run_background_command`).

use crate::ApprovalHandler;
use crate::approval::{await_proposal, begin_tool, finish_tool};
use crate::utils::resolve_dir_path;
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct RunBackgroundCommandArgs {
    pub command: String,
    pub explanation: String,
    pub cwd: Option<String>,
}

#[derive(Serialize)]
pub struct RunBackgroundCommandOutput {
    pub success: bool,
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct RunBackgroundCommandTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for RunBackgroundCommandTool {
    const NAME: &'static str = "run_background_command";

    type Error = std::io::Error;
    type Args = RunBackgroundCommandArgs;
    type Output = RunBackgroundCommandOutput;

    fn description(&self) -> String {
        "Launch a long-running application, GUI program, dev server, or background script. Crucially, this runs completely detached in the background and DOES NOT block the interactive terminal. If `cwd` is omitted, it defaults to the current pane's working directory. Returns the PID of the spawned process.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The exact shell command to launch the background process."
                },
                "explanation": {
                    "type": "string",
                    "description": "A brief, 1-sentence explanation of what is being launched for the user approval dialog."
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional: The directory where the command should be executed (defaults to current working directory)."
                }
            },
            "required": ["command", "explanation"]
        })
    }

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        rig::completion::ToolAnnotations::default().with_read_only(false)
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let trimmed_cmd = args.command.trim();
        if trimmed_cmd.is_empty() {
            let msg = "`command` must not be empty.".to_string();
            let out = RunBackgroundCommandOutput {
                success: false,
                pid: None,
                command: String::new(),
                message: msg.clone(),
                error: Some(msg),
            };
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let base_dir = self.approval.current_dir().await.unwrap_or_default();
        let resolved_cwd = resolve_dir_path(&base_dir, args.cwd.as_deref());

        let outcome = await_proposal(
            self.approval.as_ref(),
            self.approval
                .propose_background_command(args.command.clone(), args.explanation),
        )
        .await;

        let out = if outcome.is_approved() {
            match self
                .env
                .spawn_detached(args.command.clone(), resolved_cwd)
                .await
            {
                Ok(pid) => RunBackgroundCommandOutput {
                    success: true,
                    pid: Some(pid),
                    command: args.command,
                    message: format!(
                        "Successfully launched process in background with PID: {pid}"
                    ),
                    error: None,
                },
                Err(e) => {
                    let msg = format!("Failed to spawn background process: {e}");
                    RunBackgroundCommandOutput {
                        success: false,
                        pid: None,
                        command: args.command,
                        message: msg.clone(),
                        error: Some(msg),
                    }
                }
            }
        } else {
            let msg = outcome.rejection_message();
            RunBackgroundCommandOutput {
                success: false,
                pid: None,
                command: args.command,
                message: msg.clone(),
                error: Some(msg),
            }
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
    async fn run_background_command_defaults_and_resolves_cwd() {
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::approved(Some("/workspace/project")));
        let tool = RunBackgroundCommandTool {
            env: env.clone(),
            approval,
        };

        let mut ctx = ToolContext::default();
        let out = tool
            .call(
                &mut ctx,
                RunBackgroundCommandArgs {
                    command: "npm run dev".to_string(),
                    explanation: "Start dev server".to_string(),
                    cwd: None,
                },
            )
            .await
            .unwrap();
        assert!(out.success);
        assert_eq!(out.pid, Some(4242));
        assert_eq!(out.command, "npm run dev");
        assert_eq!(
            env.spawned_cwd.lock().as_deref(),
            Some("/workspace/project")
        );

        let out_rel = tool
            .call(
                &mut ctx,
                RunBackgroundCommandArgs {
                    command: "npm run dev".to_string(),
                    explanation: "Start frontend".to_string(),
                    cwd: Some("frontend".to_string()),
                },
            )
            .await
            .unwrap();
        assert!(out_rel.success);
        assert_eq!(
            env.spawned_cwd.lock().as_deref(),
            Some("/workspace/project/frontend")
        );
    }
}
