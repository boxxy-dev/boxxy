pub mod memory;
pub mod scrollback;
pub mod skills;
pub mod terminal;
pub mod workspace;

use crate::engine::ClawEngineEvent;
use crate::engine::session::SessionState;
use boxxy_agent::ipc::AgentClawProxy;
use boxxy_core_toolbox::ApprovalHandler;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct ClawApprovalHandler {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: Arc<Mutex<SessionState>>,
}

#[async_trait::async_trait]
impl ApprovalHandler for ClawApprovalHandler {
    async fn propose_file_write(&self, path: String, content: String) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_file_reply = Some(tx);
        }

        if self
            .tx_ui
            .send(ClawEngineEvent::ProposeFileWrite {
                agent_name,
                path,
                content,
            })
            .await
            .is_err()
        {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_file_delete(&self, path: String) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_file_delete_reply = Some(tx);
        }

        if self
            .tx_ui
            .send(ClawEngineEvent::ProposeFileDelete { agent_name, path })
            .await
            .is_err()
        {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_kill_process(&self, pid: u32, process_name: String) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_kill_process_reply = Some(tx);
        }

        if self
            .tx_ui
            .send(ClawEngineEvent::ProposeKillProcess {
                agent_name,
                pid,
                process_name,
            })
            .await
            .is_err()
        {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_get_clipboard(&self) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_get_clipboard_reply = Some(tx);
        }

        if self
            .tx_ui
            .send(ClawEngineEvent::ProposeGetClipboard { agent_name })
            .await
            .is_err()
        {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_set_clipboard(&self, text: String) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_set_clipboard_reply = Some(tx);
        }

        if self
            .tx_ui
            .send(ClawEngineEvent::ProposeSetClipboard { agent_name, text })
            .await
            .is_err()
        {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn report_tool_result(&self, tool_name: String, result: String) {
        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ToolResult {
                agent_name,
                tool_name,
                result,
            })
            .await;
    }

    async fn set_thinking(&self, thinking: bool) {
        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name,
                is_thinking: thinking,
            })
            .await;
    }
}

#[derive(Deserialize)]
pub struct SysShellArgs {
    pub command: String,
}

#[derive(Serialize)]
pub struct SysShellOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Tool for executing host-level commands via `boxxy-agent` IPC
pub struct SysShellTool {
    pub proxy: AgentClawProxy<'static>,
    pub current_dir: String,
}

impl Tool for SysShellTool {
    const NAME: &'static str = "sys_shell_exec";

    type Error = std::io::Error;
    type Args = SysShellArgs;
    type Output = SysShellOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Execute non-interactive bash commands on the host system to diagnose or repair issues. DO NOT use interactive commands (like top without -b, or less).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The exact bash command to execute."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let command = if self.current_dir.is_empty() {
            args.command
        } else {
            format!(
                "cd '{}' && {}",
                self.current_dir.replace('\'', "'\\''"),
                args.command
            )
        };
        match self.proxy.exec_shell(command).await {
            Ok((exit_code, stdout, stderr)) => Ok(SysShellOutput {
                stdout,
                stderr,
                exit_code,
            }),
            Err(e) => Err(std::io::Error::other(format!("IPC Error: {e}"))),
        }
    }
}
