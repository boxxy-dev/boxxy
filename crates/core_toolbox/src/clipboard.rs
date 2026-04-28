use crate::ApprovalHandler;
use boxxy_claw_protocol::ClawEnvironment;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct GetClipboardArgs {}

#[derive(Serialize)]
pub struct GetClipboardOutput {
    pub text: String,
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read the current contents of the system clipboard. This requires explicit user approval.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        self.approval.set_thinking(false).await;
        let approved = self.approval.propose_get_clipboard().await;
        self.approval.set_thinking(true).await;

        if approved {
            match self.env.get_clipboard().await {
                Ok(text) => Ok(GetClipboardOutput { text }),
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            Err(std::io::Error::other("User rejected clipboard access."))
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Write text to the system clipboard. This requires explicit user approval."
                    .to_string(),
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
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        self.approval.set_thinking(false).await;
        let approved = self.approval.propose_set_clipboard(args.text.clone()).await;
        self.approval.set_thinking(true).await;

        if approved {
            match self.env.set_clipboard(args.text).await {
                Ok(_) => Ok(SetClipboardOutput { success: true }),
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            Ok(SetClipboardOutput { success: false })
        }
    }
}
