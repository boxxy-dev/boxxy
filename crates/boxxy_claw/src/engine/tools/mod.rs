pub mod orchestration;
pub mod scrollback;
pub mod skills;
pub mod tasks;
pub mod terminal;
pub mod workspace;

use crate::engine::session::SessionState;
use crate::engine::{ClawEngineEvent, ClawEnvironment};
use boxxy_core_toolbox::{ApprovalHandler, ApprovalOutcome};
use rig::tool::{Tool, ToolContext};
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
    ) -> ApprovalOutcome {
        let (agent_name, character_id, vibe_enabled, cwd) = {
            let state = self.state.lock().await;
            let (name, cid) = state.effective_speaker();
            (
                name,
                cid,
                state.vibe_enabled,
                state.current_dir.clone(),
            )
        };

        let settings = boxxy_preferences::Settings::load();
        let auto_approve = vibe_enabled
            && crate::engine::vibe_guard::is_safe_vibe_path(
                &path,
                &cwd,
                &settings.vibe_trusted_dirs,
            );

        if auto_approve {
            let event = ClawEngineEvent::ProposeFileWrite {
                agent_name,
                character_id,
                path,
                // In auto-approve (/vibe) mode, the write is executed immediately by the tool
                // and the UI only renders `diff`; omit full file `content` to avoid cloning
                // multi-megabyte strings across the event channel during long sessions.
                content: String::new(),
                diff: Some(diff),
                auto_approve: true,
                usage: None,
            };

            crate::engine::persist_visual_event(
                self.db.clone(),
                self.session_id.clone(),
                self.pane_id.clone(),
                &event,
            );

            let _ = self.tx_ui.send(event).await;
            return ApprovalOutcome::Approved;
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut state = self.state.lock().await;
            state.pending_file_reply = Some(tx);
            let preview: String = content.chars().take(400).collect();
            state.pending_proposal_summary =
                Some(format!("file_write to `{path}`:\n```\n{preview}\n```"));
        }

        let event = ClawEngineEvent::ProposeFileWrite {
            agent_name,
            character_id,
            path,
            content,
            diff: Some(diff),
            auto_approve: false,
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if self.tx_ui.send(event).await.is_err() {
            return ApprovalOutcome::Rejected;
        }

        rx.await.unwrap_or(ApprovalOutcome::Rejected)
    }

    async fn propose_file_delete(&self, path: String) -> ApprovalOutcome {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            state.effective_speaker()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_file_delete_reply = Some(tx);
            state.pending_proposal_summary = Some(format!("file_delete `{path}`"));
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
            return ApprovalOutcome::Rejected;
        }

        rx.await.unwrap_or(ApprovalOutcome::Rejected)
    }

    async fn propose_kill_process(&self, pid: u32, process_name: String) -> ApprovalOutcome {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            state.effective_speaker()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_kill_process_reply = Some(tx);
            state.pending_proposal_summary =
                Some(format!("kill_process PID {pid} ({process_name})"));
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
            return ApprovalOutcome::Rejected;
        }

        rx.await.unwrap_or(ApprovalOutcome::Rejected)
    }

    async fn propose_background_command(
        &self,
        command: String,
        explanation: String,
    ) -> ApprovalOutcome {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            state.effective_speaker()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_background_reply = Some(tx);
            state.pending_proposal_summary =
                Some(format!("run_background_command `{command}` ({explanation})"));
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
            return ApprovalOutcome::Rejected;
        }

        rx.await.unwrap_or(ApprovalOutcome::Rejected)
    }

    async fn propose_get_clipboard(&self) -> ApprovalOutcome {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            state.effective_speaker()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_get_clipboard_reply = Some(tx);
            state.pending_proposal_summary = Some("get_clipboard".to_string());
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
            return ApprovalOutcome::Rejected;
        }

        rx.await.unwrap_or(ApprovalOutcome::Rejected)
    }

    async fn propose_set_clipboard(&self, text: String) -> ApprovalOutcome {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            state.effective_speaker()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_set_clipboard_reply = Some(tx);
            state.pending_proposal_summary = Some("set_clipboard".to_string());
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
            return ApprovalOutcome::Rejected;
        }

        rx.await.unwrap_or(ApprovalOutcome::Rejected)
    }

    async fn report_tool_result(&self, tool_name: String, result: String) {
        let (agent_name, character_id) = {
            let state = self.state.lock().await;
            state.effective_speaker()
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
            state.effective_speaker()
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
            state.effective_speaker()
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

    async fn record_file_read(&self, path: String, content_hash: u64) {
        let mut state = self.state.lock().await;
        state.read_file_hashes.insert(path, content_hash);
    }

    async fn check_stale_file_before_write(
        &self,
        path: &str,
        disk_content: Option<&str>,
    ) -> Option<String> {
        let Some(disk) = disk_content else {
            return None;
        };
        let state = self.state.lock().await;
        let Some(&known_hash) = state.read_file_hashes.get(path) else {
            return None;
        };
        let current_hash = boxxy_core_toolbox::hash_file_content(disk);
        if current_hash != known_hash {
            Some(format!(
                "File `{path}` has been modified on disk since you last read or edited it. Call `file_read` on `{path}` first so you do not overwrite manual user changes."
            ))
        } else {
            None
        }
    }

    async fn record_file_snapshot_before_write(
        &self,
        path: String,
        previous_content: Option<String>,
        new_content_hash: u64,
    ) {
        const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
        let mut state = self.state.lock().await;
        state.read_file_hashes.insert(path.clone(), new_content_hash);

        // Only capture the pre-turn state of `path` the first time it is modified in a turn
        // so multiple edits to the same file within one turn roll back all the way to the
        // pre-turn baseline on `/undo`.
        if !state
            .current_turn_snapshots
            .iter()
            .any(|(p, _)| p == &path)
        {
            let current_bytes: usize = state
                .current_turn_snapshots
                .iter()
                .map(|(_, c)| c.as_ref().map_or(0, String::len))
                .sum();
            let add_bytes = previous_content.as_ref().map_or(0, String::len);
            if current_bytes.saturating_add(add_bytes) <= MAX_SNAPSHOT_BYTES {
                state.current_turn_snapshots.push((path, previous_content));
            }
        }
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
    pub timeout_secs: Option<u64>,
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

impl Tool for SysShellTool {
    const NAME: &'static str = "sys_shell_exec";

    type Error = std::io::Error;
    type Args = SysShellArgs;
    type Output = SysShellOutput;

    fn description(&self) -> String {
        "Execute non-interactive bash commands in the background on the host system (default 120s timeout, up to 180s) to inspect, build, or test (e.g. `git status`, `git diff`, `cargo check`, `cargo test`, `pytest`) without polluting the user's interactive terminal. Prefer structured tools (`file_read`, `replace_in_file`, `grep_search`, `glob_search`, `list_processes`, `get_system_info`) when possible, and only resort to `sys_shell_exec` when no structured tool provides the needed detail. DO NOT use interactive commands (like `top` without `-b`, `less`, `vim`) or long-running servers (use `run_background_command` instead).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The exact bash command to execute."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Optional timeout in seconds (default: 120, max: 180)."
                }
            },
            "required": ["command"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let timeout_secs = args.timeout_secs.unwrap_or(120).clamp(5, 180);
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
        match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            self.env.exec_shell(command),
        )
        .await
        {
            Ok(Ok((exit_code, stdout, stderr))) => {
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
            Ok(Err(e)) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            Err(_) => {
                let out = SysShellOutput {
                    stdout: String::new(),
                    stderr: format!(
                        "[ERROR: Command timed out after {timeout_secs} seconds. Pass a higher `timeout_secs` (up to 180) or narrow the test/build target.]"
                    ),
                    exit_code: 124,
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

        let mut ctx = ToolContext::default();
        // Turn 1: live CWD in SessionState is "/turn1/dir"
        state.lock().await.current_dir = "/turn1/dir".to_string();
        tool.call(
            &mut ctx,
            SysShellArgs {
                command: "cargo check".to_string(),
                timeout_secs: None,
            },
        )
        .await
        .unwrap();

        // Turn 2: user cd'd to "/turn2/new_dir" without rebuilding SysShellTool
        state.lock().await.current_dir = "/turn2/new_dir".to_string();
        tool.call(
            &mut ctx,
            SysShellArgs {
                command: "cargo test".to_string(),
                timeout_secs: Some(60),
            },
        )
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

    #[tokio::test]
    async fn stale_file_guard_and_snapshot_ledger_preserve_baseline() {
        let (session, _tx, _rx_ui) = ClawSession::new("pane_undo".to_string());
        let state = session.state.clone();
        let (tx_ui, _rx) = async_channel::unbounded();
        let approval = ClawApprovalHandler {
            tx_ui,
            state: state.clone(),
            db: Arc::new(Mutex::new(None)),
            session_id: session.session_id.clone(),
            pane_id: "pane_undo".to_string(),
        };

        let path = "/tmp/example_project/main.rs".to_string();
        let v1 = "fn main() { println!(\"v1\"); }";
        let v2 = "fn main() { println!(\"v2\"); }";
        let v3 = "fn main() { println!(\"v3\"); }";

        // 1. Agent reads v1
        approval
            .record_file_read(path.clone(), boxxy_core_toolbox::hash_file_content(v1))
            .await;

        // 2. Disk still has v1 -> not stale
        assert!(
            approval
                .check_stale_file_before_write(&path, Some(v1))
                .await
                .is_none()
        );

        // 3. User externally edits file to "manual edit" -> stale!
        let stale_err = approval
            .check_stale_file_before_write(&path, Some("fn main() { /* manual */ }"))
            .await;
        assert!(stale_err.is_some());

        // 4. Agent edits v1 -> v2, then v2 -> v3 in the same turn; snapshot must keep v1 as baseline
        approval
            .record_file_snapshot_before_write(
                path.clone(),
                Some(v1.to_string()),
                boxxy_core_toolbox::hash_file_content(v2),
            )
            .await;
        approval
            .record_file_snapshot_before_write(
                path.clone(),
                Some(v2.to_string()),
                boxxy_core_toolbox::hash_file_content(v3),
            )
            .await;

        let lock = state.lock().await;
        assert_eq!(lock.current_turn_snapshots.len(), 1);
        assert_eq!(lock.current_turn_snapshots[0].0, path);
        assert_eq!(lock.current_turn_snapshots[0].1.as_deref(), Some(v1));
    }
}
