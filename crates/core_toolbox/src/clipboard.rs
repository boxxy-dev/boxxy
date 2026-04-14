use crate::ApprovalHandler;
use boxxy_agent::ipc::AgentClawProxy;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// --- GET CLIPBOARD ---

#[derive(Deserialize)]
pub struct GetClipboardArgs {}

#[derive(Serialize)]
pub struct GetClipboardOutput {
    pub text: String,
}

pub struct GetClipboardTool {
    pub proxy: AgentClawProxy<'static>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for GetClipboardTool {
    const NAME: &'static str = "get_clipboard";

    type Error = std::io::Error;
    type Args = GetClipboardArgs;
    type Output = GetClipboardOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read the current text content of the system clipboard. This will prompt the user for permission.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        self.approval.set_thinking(false).await;
        let approved = self.approval.propose_get_clipboard().await;
        self.approval.set_thinking(true).await;

        if approved {
            match self.proxy.get_clipboard().await {
                Ok(text) => {
                    let out = GetClipboardOutput { text };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
                Err(e) => Err(std::io::Error::other(format!("IPC Error: {e}"))),
            }
        } else {
            Err(std::io::Error::other("[USER_EXPLICIT_REJECT]"))
        }
    }
}

// --- SET CLIPBOARD ---

#[derive(Deserialize)]
pub struct SetClipboardArgs {
    pub text: String,
}

#[derive(Serialize)]
pub struct SetClipboardOutput {
    pub success: bool,
}

pub struct SetClipboardTool {
    pub proxy: AgentClawProxy<'static>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for SetClipboardTool {
    const NAME: &'static str = "set_clipboard";

    type Error = std::io::Error;
    type Args = SetClipboardArgs;
    type Output = SetClipboardOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Set the system clipboard to the provided text. This will prompt the user for permission.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text to copy to the clipboard."
                    }
                },
                "required": ["text"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        self.approval.set_thinking(false).await;
        let approved = self.approval.propose_set_clipboard(args.text.clone()).await;
        self.approval.set_thinking(true).await;

        if approved {
            match self.proxy.set_clipboard(args.text).await {
                Ok(()) => {
                    let out = SetClipboardOutput { success: true };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
                Err(e) => Err(std::io::Error::other(format!("IPC Error: {e}"))),
            }
        } else {
            let out = SetClipboardOutput { success: false };
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
