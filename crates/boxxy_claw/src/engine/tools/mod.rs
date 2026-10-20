pub mod orchestration;
pub mod scrollback;
pub mod skills;
pub mod tasks;
pub mod terminal;
pub mod workspace;

use crate::engine::session::SessionState;
use crate::engine::{ClawEngineEvent, ClawEnvironment};
use boxxy_core_toolbox::ApprovalHandler;
use rig::tool::PortableTool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use boxxy_db::Db;

#[derive(Clone)]
pub struct ClawApprovalHandler {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: Arc<Mutex<SessionState>>,
    pub db: Arc<Mutex<Option<Db>>>,
    pub session_id: String,
    pub pane_id: String,
}

impl ClawApprovalHandler {
    pub async fn begin_tool(&self, tool_name: &'static str) {
        self.report_tool_started(tool_name.to_string()).await;
        boxxy_telemetry::track_tool_use(tool_name).await;
    }

    pub async fn finish_tool<T: Serialize + ?Sized>(&self, tool_name: &'static str, output: &T) {
        self.report_tool_result(
            tool_name.to_string(),
            serde_json::to_string(output).unwrap_or_default(),
        )
        .await;
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for ClawApprovalHandler {
    async fn propose_file_write(
        &self,
        path: String,
        content: String,
        diff: boxxy_claw_protocol::FileDiff,
    ) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        {
            let mut state = self.state.lock().await;
            state.pending_file_reply = Some(tx);
        }

        let event = ClawEngineEvent::ProposeFileWrite {
            agent_name,
            character_id,
            path,
            content,
            diff: Some(diff),
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if self.tx_ui.send(event).await.is_err() {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_file_delete(&self, path: String) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        {
            let mut state = self.state.lock().await;
            state.pending_file_delete_reply = Some(tx);
        }

        let event = ClawEngineEvent::ProposeFileDelete {
            agent_name,
            character_id,
            path,
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if self.tx_ui.send(event).await.is_err() {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_kill_process(&self, pid: u32, process_name: String) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        {
            let mut state = self.state.lock().await;
            state.pending_kill_process_reply = Some(tx);
        }

        let event = ClawEngineEvent::ProposeKillProcess {
            agent_name,
            character_id,
            pid,
            process_name,
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if self.tx_ui.send(event).await.is_err() {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_background_command(&self, command: String, explanation: String) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        {
            let mut state = self.state.lock().await;
            state.pending_background_reply = Some(tx);
        }

        let event = ClawEngineEvent::ProposeBackgroundCommand {
            agent_name,
            character_id,
            command,
            explanation,
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if self.tx_ui.send(event).await.is_err() {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_get_clipboard(&self) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        {
            let mut state = self.state.lock().await;
            state.pending_get_clipboard_reply = Some(tx);
        }

        let event = ClawEngineEvent::ProposeGetClipboard {
            agent_name,
            character_id,
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if self.tx_ui.send(event).await.is_err() {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn propose_set_clipboard(&self, text: String) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        {
            let mut state = self.state.lock().await;
            state.pending_set_clipboard_reply = Some(tx);
        }

        let event = ClawEngineEvent::ProposeSetClipboard {
            agent_name,
            character_id,
            text,
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if self.tx_ui.send(event).await.is_err() {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    async fn report_tool_result(&self, tool_name: String, result: String) {
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };

        let event = ClawEngineEvent::ToolResult {
            agent_name,
            character_id,
            tool_name,
            result,
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        let _ = self.tx_ui.send(event).await;
    }

    async fn report_tool_started(&self, tool_name: String) {
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name,
                character_id,
                tool_name,
            })
            .await;
    }

    async fn set_thinking(&self, thinking: bool) {
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            (state.agent_name.clone(), state.character_id.clone())
        };
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name,
                character_id,
                is_thinking: thinking,
            })
            .await;
    }

    async fn current_dir(&self) -> Option<String> {
        let dir = self.state.lock().await.current_dir.clone();
        if dir.is_empty() { None } else { Some(dir) }
    }
}

#[async_trait::async_trait]
impl boxxy_mcp::rig_bridge::tool::McpToolObserver for ClawApprovalHandler {
    async fn on_tool_started(&self, tool_name: String) {
        self.report_tool_started(tool_name).await;
        boxxy_telemetry::track_tool_use(boxxy_telemetry::EXTERNAL_MCP_TOOL_NAME).await;
    }

    async fn on_tool_result(&self, tool_name: String, result: String) {
        self.report_tool_result(tool_name, result).await;
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
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<ClawApprovalHandler>,
}

impl PortableTool for SysShellTool {
    const NAME: &'static str = "sys_shell_exec";

    type Error = std::io::Error;
    type Args = SysShellArgs;
    type Output = SysShellOutput;

    fn description(&self) -> String {
        "Execute non-interactive bash commands on the host system (30s timeout) to inspect, build, or test (e.g. `git status`, `git diff`, `cargo check`). Prefer structured tools (`file_read`, `replace_in_file`, `grep_search`, `glob_search`, `list_processes`) whenever possible. DO NOT use interactive commands (like `top` without `-b`, `less`, `vim`) or long-running servers (use `run_background_command` instead).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The exact bash command to execute."
                }
            },
            "required": ["command"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let base_dir = self
            .approval
            .current_dir()
            .await
            .unwrap_or_else(|| self.current_dir.clone());
        let command = if base_dir.is_empty() {
            args.command
        } else {
            format!(
                "cd '{}' && {}",
                base_dir.replace('\'', "'\\''"),
                args.command
            )
        };
        match self.env.exec_shell(command).await {
            Ok((exit_code, stdout, stderr)) => {
                let out = SysShellOutput {
                    stdout,
                    stderr,
                    exit_code,
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
    }
}
pub mod set_state;
pub mod summon_headless;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::session::ClawSession;

    struct CaptureCmdEnv {
        commands: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl ClawEnvironment for CaptureCmdEnv {
        async fn exec_shell(&self, command: String) -> anyhow::Result<(i32, String, String)> {
            self.commands.lock().unwrap().push(command);
            Ok((0, "ok".to_string(), String::new()))
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
        async fn get_system_info(&self) -> anyhow::Result<String> {
            Ok("{}".to_string())
        }
        async fn list_processes(&self) -> anyhow::Result<Vec<(u32, String, f64, u64, u64, u64)>> {
            Ok(vec![])
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

    #[tokio::test]
    async fn sys_shell_tool_follows_live_session_cwd() {
        let (session, _tx, _rx_ui) = ClawSession::new("pane_cwd".to_string());
        let state = session.state.clone();
        let approval = Arc::new(ClawApprovalHandler {
            tx_ui: session.tx_ui.clone(),
            state: state.clone(),
            db: Arc::new(Mutex::new(None)),
            session_id: session.session_id.clone(),
            pane_id: "pane_cwd".to_string(),
        });

        let env = Arc::new(CaptureCmdEnv {
            commands: std::sync::Mutex::new(Vec::new()),
        });

        let tool = SysShellTool {
            env: env.clone(),
            current_dir: "/initial/dir".to_string(),
            approval,
        };

        // Turn 1: live CWD in SessionState is "/turn1/dir"
        state.lock().await.current_dir = "/turn1/dir".to_string();
        tool.call(SysShellArgs {
            command: "cargo check".to_string(),
        })
        .await
        .unwrap();

        // Turn 2: user cd'd to "/turn2/new_dir" without rebuilding SysShellTool
        state.lock().await.current_dir = "/turn2/new_dir".to_string();
        tool.call(SysShellArgs {
            command: "cargo test".to_string(),
        })
        .await
        .unwrap();

        let recorded = env.commands.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec![
                "cd '/turn1/dir' && cargo check".to_string(),
                "cd '/turn2/new_dir' && cargo test".to_string(),
            ]
        );
    }
}
