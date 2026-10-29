use crate::ApprovalHandler;
use crate::utils::resolve_path;
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

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let base_dir = self.approval.current_dir().await.unwrap_or_default();
        let resolved_cwd = match args.cwd.as_deref().map(str::trim) {
            Some(c) if !c.is_empty() => resolve_path(&base_dir, c),
            _ if !base_dir.is_empty() => base_dir,
            _ => ".".to_string(),
        };

        self.approval.set_thinking(false).await;
        let outcome = self
            .approval
            .propose_background_command(args.command.clone(), args.explanation.clone())
            .await;
        self.approval.set_thinking(true).await;

        if outcome.is_approved() {
            match self
                .env
                .spawn_detached(args.command.clone(), resolved_cwd)
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
                message: outcome.rejection_message(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct TrackCwdEnv {
        spawned_cwd: Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl ClawEnvironment for TrackCwdEnv {
        async fn exec_shell(&self, _: String) -> anyhow::Result<(i32, String, String)> {
            Ok((0, String::new(), String::new()))
        }
        async fn spawn_detached(&self, _command: String, cwd: String) -> anyhow::Result<u32> {
            *self.spawned_cwd.lock().unwrap() = Some(cwd);
            Ok(4242)
        }
        async fn read_file(&self, _: String, _: u32, _: u32) -> anyhow::Result<String> {
            Ok(String::new())
        }
        async fn write_file(&self, _: String, _: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn list_directory(&self, _: String) -> anyhow::Result<Vec<(String, bool, u64)>> {
            Ok(vec![])
        }
        async fn delete_file(&self, _: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn kill_process(&self, _: u32, _: i32) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get_clipboard(&self) -> anyhow::Result<String> {
            Ok(String::new())
        }
        async fn set_clipboard(&self, _: String) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct ApproveWithCwd(String);

    #[async_trait::async_trait]
    impl ApprovalHandler for ApproveWithCwd {
        async fn propose_file_write(
            &self,
            _: String,
            _: String,
            _: crate::FileDiff,
        ) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_file_delete(&self, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_background_command(
            &self,
            _: String,
            _: String,
        ) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_kill_process(&self, _: u32, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_get_clipboard(&self) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_set_clipboard(&self, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn report_tool_result(&self, _: String, _: String) {}
        async fn report_tool_started(&self, _: String) {}
        async fn set_thinking(&self, _: bool) {}
        async fn current_dir(&self) -> Option<String> {
            Some(self.0.clone())
        }
    }

    #[tokio::test]
    async fn run_background_command_defaults_and_resolves_cwd() {
        let env = Arc::new(TrackCwdEnv {
            spawned_cwd: Mutex::new(None),
        });
        let approval = Arc::new(ApproveWithCwd("/workspace/project".to_string()));
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
        assert_eq!(
            env.spawned_cwd.lock().unwrap().as_deref(),
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
            env.spawned_cwd.lock().unwrap().as_deref(),
            Some("/workspace/project/frontend")
        );
    }
}
