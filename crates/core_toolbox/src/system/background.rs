use crate::ApprovalHandler;
use boxxy_claw_protocol::ClawEnvironment;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct RunBackgroundCommandArgs {
    pub command: String,
    pub explanation: String,
    pub cwd: String,
}

#[derive(Serialize)]
pub struct RunBackgroundCommandOutput {
    pub success: bool,
    pub pid: Option<u32>,
    pub message: String,
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Launch a long-running application, GUI program, dev server, or background script. Crucially, this runs completely detached in the background and DOES NOT block the interactive terminal. You MUST provide the `cwd` to ensure the process starts in the correct directory. It returns the PID of the spawned process.".to_string(),
            parameters: serde_json::json!({
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
                        "description": "The absolute path of the directory where the command should be executed."
                    }
                },
                "required": ["command", "explanation", "cwd"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        self.approval.set_thinking(false).await;
        let approved = self
            .approval
            .propose_background_command(args.command.clone(), args.explanation.clone())
            .await;
        self.approval.set_thinking(true).await;

        if approved {
            match self
                .env
                .spawn_detached(args.command.clone(), args.cwd.clone())
                .await
            {
                Ok(pid) => {
                    let out = RunBackgroundCommandOutput {
                        success: true,
                        pid: Some(pid),
                        message: format!(
                            "Successfully launched process in background with PID: {}",
                            pid
                        ),
                    };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
                Err(e) => {
                    let out = RunBackgroundCommandOutput {
                        success: false,
                        pid: None,
                        message: format!("Failed to spawn background process: {}", e),
                    };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
            }
        } else {
            let out = RunBackgroundCommandOutput {
                success: false,
                pid: None,
                message: "[USER_EXPLICIT_REJECT]".to_string(),
            };
            self.approval
                .report_tool_result(
                    Self::NAME.to_string(),
                    serde_json::to_string(&out).unwrap_or_default(),
                )
                .await;
            Ok(out)
        }
    }
}
