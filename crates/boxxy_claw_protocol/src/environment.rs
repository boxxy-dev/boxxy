//! Privileged host environment interface (`ClawEnvironment`) and process telemetry DTOs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessSample {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f64,
    pub memory_bytes: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_usage: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_memory_bytes: Option<u64>,
}

#[async_trait::async_trait]
pub trait ClawEnvironment: Send + Sync + 'static {
    async fn exec_shell(&self, command: String) -> anyhow::Result<(i32, String, String)>;
    async fn spawn_detached(&self, command: String, cwd: String) -> anyhow::Result<u32>;
    async fn read_file(
        &self,
        path: String,
        start_line: u32,
        end_line: u32,
    ) -> anyhow::Result<String>;
    async fn write_file(&self, path: String, content: String) -> anyhow::Result<()>;
    async fn list_directory(&self, path: String) -> anyhow::Result<Vec<(String, bool, u64)>>;
    async fn delete_file(&self, path: String) -> anyhow::Result<()>;
    async fn get_system_info(&self) -> anyhow::Result<String> {
        Ok("{}".to_string())
    }
    async fn list_processes(&self) -> anyhow::Result<Vec<ProcessSample>> {
        Ok(Vec::new())
    }
    async fn kill_process(&self, pid: u32, signal: i32) -> anyhow::Result<()>;
    async fn get_clipboard(&self) -> anyhow::Result<String>;
    async fn set_clipboard(&self, text: String) -> anyhow::Result<()>;

    /// Returns `(pid, pane_id, idle_secs, cwd, foreground_process, waiting_for_input)`
    /// for all detached background PTY shells kept alive in the daemon.
    async fn list_detached_shells(
        &self,
    ) -> anyhow::Result<Vec<(u32, String, u64, String, String, bool)>> {
        Ok(Vec::new())
    }
}
