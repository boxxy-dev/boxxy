use crate::ApprovalHandler;
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct GetClipboardArgs {}

#[derive(Serialize)]
pub struct GetClipboardOutput {
    pub success: bool,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Tool for reading the current contents of the system clipboard.
pub struct GetClipboardTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for GetClipboardTool {
    const NAME: &'static str = "get_clipboard";

    type Error = std::io::Error;
    type Args = GetClipboardArgs;
    type Output = GetClipboardOutput;

    fn description(&self) -> String {
        "Read the current contents of the system clipboard. This requires explicit user approval.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        _args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        self.approval.set_thinking(false).await;
        let outcome = self.approval.propose_get_clipboard().await;
        self.approval.set_thinking(true).await;

        if outcome.is_approved() {
            match self.env.get_clipboard().await {
                Ok(text) => {
                    let out = GetClipboardOutput {
                        success: true,
                        text,
                        error: None,
                    };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            let out = GetClipboardOutput {
                success: false,
                text: String::new(),
                error: Some(outcome.rejection_message()),
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

#[derive(Deserialize)]
pub struct SetClipboardArgs {
    pub text: String,
}

#[derive(Serialize)]
pub struct SetClipboardOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Tool for writing text to the system clipboard.
pub struct SetClipboardTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for SetClipboardTool {
    const NAME: &'static str = "set_clipboard";

    type Error = std::io::Error;
    type Args = SetClipboardArgs;
    type Output = SetClipboardOutput;

    fn description(&self) -> String {
        "Write text to the system clipboard. This requires explicit user approval.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to copy to the clipboard."
                }
            },
            "required": ["text"]
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
        self.approval.set_thinking(false).await;
        let outcome = self.approval.propose_set_clipboard(args.text.clone()).await;
        self.approval.set_thinking(true).await;

        if outcome.is_approved() {
            match self.env.set_clipboard(args.text).await {
                Ok(()) => {
                    let out = SetClipboardOutput {
                        success: true,
                        error: None,
                    };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            let out = SetClipboardOutput {
                success: false,
                error: Some(outcome.rejection_message()),
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

    struct DummyEnv;

    #[async_trait::async_trait]
    impl ClawEnvironment for DummyEnv {
        async fn exec_shell(&self, _: String) -> anyhow::Result<(i32, String, String)> {
            Ok((0, String::new(), String::new()))
        }
        async fn spawn_detached(&self, _: String, _: String) -> anyhow::Result<u32> {
            Ok(1)
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
            Ok("clip".to_string())
        }
        async fn set_clipboard(&self, _: String) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct RejectApproval {
        results: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl ApprovalHandler for RejectApproval {
        async fn propose_file_write(
            &self,
            _: String,
            _: String,
            _: crate::FileDiff,
        ) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Rejected
        }
        async fn propose_file_delete(&self, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Rejected
        }
        async fn propose_background_command(
            &self,
            _: String,
            _: String,
        ) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Rejected
        }
        async fn propose_kill_process(&self, _: u32, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Rejected
        }
        async fn propose_get_clipboard(&self) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Rejected
        }
        async fn propose_set_clipboard(&self, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Rejected
        }
        async fn report_tool_result(&self, _name: String, result: String) {
            self.results.lock().unwrap().push(result);
        }
        async fn report_tool_started(&self, _: String) {}
        async fn set_thinking(&self, _: bool) {}
    }

    #[tokio::test]
    async fn clipboard_tools_return_explicit_reject_on_denial() {
        let env: Arc<dyn ClawEnvironment> = Arc::new(DummyEnv);
        let approval = Arc::new(RejectApproval {
            results: Mutex::new(Vec::new()),
        });

        let get_tool = GetClipboardTool {
            env: env.clone(),
            approval: approval.clone(),
        };
        let mut ctx = ToolContext::default();
        let get_out = get_tool
            .call(&mut ctx, GetClipboardArgs {})
            .await
            .unwrap();
        assert!(!get_out.success);
        assert_eq!(get_out.error.as_deref(), Some("[USER_EXPLICIT_REJECT]"));

        let set_tool = SetClipboardTool {
            env,
            approval: approval.clone(),
        };
        let set_out = set_tool
            .call(
                &mut ctx,
                SetClipboardArgs {
                    text: "secret".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(!set_out.success);
        assert_eq!(set_out.error.as_deref(), Some("[USER_EXPLICIT_REJECT]"));
        assert_eq!(approval.results.lock().unwrap().len(), 2);
    }
}
