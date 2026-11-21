//! User-approved system clipboard read and write tools (`get_clipboard`, `set_clipboard`).

use crate::ApprovalHandler;
use crate::approval::{await_proposal, begin_tool, finish_tool};
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

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        // Intentionally `read_only = false`: `propose_get_clipboard` stores a single-slot
        // `oneshot::Sender` in `SessionState.pending_get_clipboard_reply` and enters
        // `OverlayState::Action`, so it must act as a full serialization barrier.
        rig::completion::ToolAnnotations::default().with_read_only(false)
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        _args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let outcome = await_proposal(
            self.approval.as_ref(),
            self.approval.propose_get_clipboard(),
        )
        .await;

        let out = if outcome.is_approved() {
            match self.env.get_clipboard().await {
                Ok(text) => GetClipboardOutput {
                    success: true,
                    text,
                    error: None,
                },
                Err(e) => GetClipboardOutput {
                    success: false,
                    text: String::new(),
                    error: Some(format!("Environment Error: {e}")),
                },
            }
        } else {
            GetClipboardOutput {
                success: false,
                text: String::new(),
                error: Some(outcome.rejection_message()),
            }
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
        Ok(out)
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
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let outcome = await_proposal(
            self.approval.as_ref(),
            self.approval.propose_set_clipboard(args.text.clone()),
        )
        .await;

        let out = if outcome.is_approved() {
            match self.env.set_clipboard(args.text).await {
                Ok(()) => SetClipboardOutput {
                    success: true,
                    error: None,
                },
                Err(e) => SetClipboardOutput {
                    success: false,
                    error: Some(format!("Environment Error: {e}")),
                },
            }
        } else {
            SetClipboardOutput {
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
    use crate::test_support::{MockApproval, MockEnv};

    #[tokio::test]
    async fn clipboard_tools_return_explicit_reject_on_denial() {
        let env: Arc<dyn ClawEnvironment> = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::with_outcome(ApprovalOutcome::Rejected, None));

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
        assert_eq!(approval.results.lock().len(), 2);
    }
}
