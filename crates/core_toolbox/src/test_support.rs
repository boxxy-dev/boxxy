//! Shared synthetic in-memory test fixtures for `boxxy-core-toolbox` unit tests.
#![allow(unreachable_pub)]

use crate::{ApprovalHandler, ApprovalOutcome, FileDiff, hash_file_content};
use boxxy_claw_protocol::{ClawEnvironment, ProcessSample};
use parking_lot::Mutex;
use std::collections::HashMap;

pub struct MockEnv {
    pub last_read_path: Mutex<Option<String>>,
    pub file_content: Mutex<String>,
    pub written_content: Mutex<Option<String>>,
    pub write_error: Mutex<Option<String>>,
    pub delete_error: Mutex<Option<String>>,
    pub dir_entries: Mutex<Vec<(String, bool, u64)>>,
    pub spawned_cwd: Mutex<Option<String>>,
    pub spawned_cmd: Mutex<Option<String>>,
    pub killed: Mutex<Vec<(u32, i32)>>,
    pub clipboard_text: Mutex<String>,
    pub clipboard_error: Mutex<Option<String>>,
    pub system_info_json: Mutex<Option<String>>,
    pub processes: Mutex<Vec<ProcessSample>>,
}

impl MockEnv {
    pub fn new() -> Self {
        Self {
            last_read_path: Mutex::new(None),
            file_content: Mutex::new(String::new()),
            written_content: Mutex::new(None),
            write_error: Mutex::new(None),
            delete_error: Mutex::new(None),
            dir_entries: Mutex::new(Vec::new()),
            spawned_cwd: Mutex::new(None),
            spawned_cmd: Mutex::new(None),
            killed: Mutex::new(Vec::new()),
            clipboard_text: Mutex::new("synthetic_clip".to_string()),
            clipboard_error: Mutex::new(None),
            system_info_json: Mutex::new(None),
            processes: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ClawEnvironment for MockEnv {
    async fn exec_shell(&self, _command: String) -> anyhow::Result<(i32, String, String)> {
        Ok((0, String::new(), String::new()))
    }

    async fn spawn_detached(&self, command: String, cwd: String) -> anyhow::Result<u32> {
        *self.spawned_cmd.lock() = Some(command);
        *self.spawned_cwd.lock() = Some(cwd);
        Ok(4242)
    }

    async fn read_file(
        &self,
        path: String,
        start_line: u32,
        end_line: u32,
    ) -> anyhow::Result<String> {
        *self.last_read_path.lock() = Some(path.clone());
        let content = self.file_content.lock().clone();
        if start_line == 0 && end_line == 0 {
            if content.is_empty() {
                Err(anyhow::anyhow!("No such file or directory"))
            } else {
                Ok(content)
            }
        } else {
            Ok(format!("read:{path}:{start_line}-{end_line}"))
        }
    }

    async fn write_file(&self, _path: String, content: String) -> anyhow::Result<()> {
        let maybe_err = self.write_error.lock().clone();
        if let Some(err) = maybe_err {
            return Err(anyhow::anyhow!(err));
        }
        *self.written_content.lock() = Some(content);
        Ok(())
    }

    async fn list_directory(&self, _path: String) -> anyhow::Result<Vec<(String, bool, u64)>> {
        Ok(self.dir_entries.lock().clone())
    }

    async fn delete_file(&self, _path: String) -> anyhow::Result<()> {
        let maybe_err = self.delete_error.lock().clone();
        if let Some(err) = maybe_err {
            return Err(anyhow::anyhow!(err));
        }
        Ok(())
    }

    async fn get_system_info(&self) -> anyhow::Result<String> {
        let maybe_json = self.system_info_json.lock().clone();
        maybe_json.ok_or_else(|| anyhow::anyhow!("System info unavailable"))
    }

    async fn list_processes(&self) -> anyhow::Result<Vec<ProcessSample>> {
        Ok(self.processes.lock().clone())
    }

    async fn kill_process(&self, pid: u32, signal: i32) -> anyhow::Result<()> {
        self.killed.lock().push((pid, signal));
        Ok(())
    }

    async fn get_clipboard(&self) -> anyhow::Result<String> {
        let maybe_err = self.clipboard_error.lock().clone();
        if let Some(err) = maybe_err {
            return Err(anyhow::anyhow!(err));
        }
        Ok(self.clipboard_text.lock().clone())
    }

    async fn set_clipboard(&self, text: String) -> anyhow::Result<()> {
        let maybe_err = self.clipboard_error.lock().clone();
        if let Some(err) = maybe_err {
            return Err(anyhow::anyhow!(err));
        }
        *self.clipboard_text.lock() = text;
        Ok(())
    }
}

pub struct MockApproval {
    pub outcome: ApprovalOutcome,
    pub live_cwd: Mutex<Option<String>>,
    pub started: Mutex<Vec<String>>,
    pub results: Mutex<Vec<(String, String)>>,
    pub diffs: Mutex<Vec<FileDiff>>,
    pub read_hashes: Mutex<HashMap<String, u64>>,
    pub snapshots: Mutex<Vec<(String, Option<String>, u64)>>,
}

impl MockApproval {
    pub fn approved(cwd: Option<&str>) -> Self {
        Self {
            outcome: ApprovalOutcome::Approved,
            live_cwd: Mutex::new(cwd.map(str::to_string)),
            started: Mutex::new(Vec::new()),
            results: Mutex::new(Vec::new()),
            diffs: Mutex::new(Vec::new()),
            read_hashes: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(Vec::new()),
        }
    }

    pub fn with_outcome(outcome: ApprovalOutcome, cwd: Option<&str>) -> Self {
        Self {
            outcome,
            ..Self::approved(cwd)
        }
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for MockApproval {
    async fn propose_file_write(
        &self,
        _path: String,
        _content: String,
        diff: FileDiff,
    ) -> ApprovalOutcome {
        self.diffs.lock().push(diff);
        self.outcome.clone()
    }

    async fn propose_file_delete(&self, _path: String) -> ApprovalOutcome {
        self.outcome.clone()
    }

    async fn propose_background_command(
        &self,
        _command: String,
        _explanation: String,
    ) -> ApprovalOutcome {
        self.outcome.clone()
    }

    async fn propose_kill_process(&self, _pid: u32, _process_name: String) -> ApprovalOutcome {
        self.outcome.clone()
    }

    async fn propose_get_clipboard(&self) -> ApprovalOutcome {
        self.outcome.clone()
    }

    async fn propose_set_clipboard(&self, _text: String) -> ApprovalOutcome {
        self.outcome.clone()
    }

    async fn report_tool_result(&self, tool_name: String, result: String) {
        self.results.lock().push((tool_name, result));
    }

    async fn report_tool_started(&self, tool_name: String) {
        self.started.lock().push(tool_name);
    }

    async fn set_thinking(&self, _thinking: bool) {}

    async fn current_dir(&self) -> Option<String> {
        self.live_cwd.lock().clone()
    }

    async fn record_file_read(&self, path: String, content_hash: u64) {
        self.read_hashes.lock().insert(path, content_hash);
    }

    async fn check_stale_file_before_write(
        &self,
        path: &str,
        disk_content: Option<&str>,
    ) -> Option<String> {
        let disk = disk_content?;
        let known = *self.read_hashes.lock().get(path)?;
        if hash_file_content(disk) == known {
            None
        } else {
            Some(format!(
                "File `{path}` has been modified on disk since you last read or edited it."
            ))
        }
    }

    async fn record_file_snapshot_before_write(
        &self,
        path: String,
        previous_content: Option<String>,
        new_content_hash: u64,
    ) {
        self.read_hashes.lock().insert(path.clone(), new_content_hash);
        self.snapshots
            .lock()
            .push((path, previous_content, new_content_hash));
    }
}
