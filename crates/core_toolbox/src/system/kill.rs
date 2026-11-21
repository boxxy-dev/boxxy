//! User-approved process termination tool (`kill_process`).

use crate::ApprovalHandler;
use crate::approval::{await_proposal, begin_tool, finish_tool};
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const DEFAULT_SIGNAL: i32 = 15; // SIGTERM
const MAX_UNIX_SIGNAL: i32 = 64;

#[derive(Deserialize)]
pub struct KillProcessArgs {
    pub pid: u32,
    pub process_name: String,
    pub signal: Option<i32>,
}

#[derive(Serialize)]
pub struct KillProcessOutput {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl KillProcessOutput {
    fn err(msg: impl Into<String>) -> Self {
        let message = msg.into();
        Self {
            success: false,
            error: Some(message.clone()),
            message,
        }
    }
}

pub struct KillProcessTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for KillProcessTool {
    const NAME: &'static str = "kill_process";

    type Error = std::io::Error;
    type Args = KillProcessArgs;
    type Output = KillProcessOutput;

    fn description(&self) -> String {
        "Terminate a running process. This will prompt the user for approval. You MUST provide the PID and the name of the process for clarity.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "Positive process ID (> 1) of the target process."
                },
                "process_name": {
                    "type": "string",
                    "description": "Name of the target process shown in the approval dialog."
                },
                "signal": {
                    "type": "integer",
                    "description": "The Unix signal to send (e.g. 15 for SIGTERM, 9 for SIGKILL). Defaults to 15."
                }
            },
            "required": ["pid", "process_name"]
        })
    }

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        rig::completion::ToolAnnotations::default()
            .with_read_only(false)
            .with_destructive(true)
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        if args.pid <= 1 || i32::try_from(args.pid).is_err() {
            let out = KillProcessOutput::err(format!(
                "Refusing to signal protected or out-of-range PID {}.",
                args.pid
            ));
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let signal = args.signal.unwrap_or(DEFAULT_SIGNAL);
        if !(1..=MAX_UNIX_SIGNAL).contains(&signal) {
            let out = KillProcessOutput::err(format!(
                "Invalid signal number {signal}: must be between 1 and {MAX_UNIX_SIGNAL}."
            ));
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let clean_name = args.process_name.trim();
        if clean_name.is_empty() {
            let out = KillProcessOutput::err("`process_name` must not be empty.");
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let display_name = if signal == DEFAULT_SIGNAL {
            clean_name.to_string()
        } else {
            format!("{clean_name} (signal {signal})")
        };

        let outcome = await_proposal(
            self.approval.as_ref(),
            self.approval.propose_kill_process(args.pid, display_name),
        )
        .await;

        let out = if outcome.is_approved() {
            match self.env.kill_process(args.pid, signal).await {
                Ok(()) => KillProcessOutput {
                    success: true,
                    message: format!("Successfully sent signal {signal} to process {}", args.pid),
                    error: None,
                },
                Err(e) => KillProcessOutput::err(format!("Environment Error: {e}")),
            }
        } else {
            KillProcessOutput::err(outcome.rejection_message())
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
    async fn kill_process_rejects_protected_pids_and_bad_signals_before_prompting() {
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::approved(None));
        let tool = KillProcessTool {
            env: env.clone(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        for bad_pid in [0, 1, u32::MAX] {
            let out = tool
                .call(
                    &mut ctx,
                    KillProcessArgs {
                        pid: bad_pid,
                        process_name: "demo".to_string(),
                        signal: None,
                    },
                )
                .await
                .unwrap();
            assert!(!out.success);
        }

        let bad_sig = tool
            .call(
                &mut ctx,
                KillProcessArgs {
                    pid: 500,
                    process_name: "demo".to_string(),
                    signal: Some(999),
                },
            )
            .await
            .unwrap();
        assert!(!bad_sig.success);
        assert!(env.killed.lock().is_empty());

        let valid = tool
            .call(
                &mut ctx,
                KillProcessArgs {
                    pid: 500,
                    process_name: "demo".to_string(),
                    signal: Some(9),
                },
            )
            .await
            .unwrap();
        assert!(valid.success);
        assert_eq!(*env.killed.lock(), vec![(500, 9)]);
    }
}
