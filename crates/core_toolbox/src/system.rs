use crate::ApprovalHandler;
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod background;

pub use background::RunBackgroundCommandTool;

// --- SYSTEM INFO ---

#[derive(Deserialize)]
pub struct GetSystemInfoArgs {}

#[derive(Serialize, Deserialize)]
pub struct OsInfo {
    pub name: Option<String>,
    pub version: Option<String>,
    pub kernel: Option<String>,
    pub hostname: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Serialize, Deserialize)]
pub struct CpuInfo {
    pub count: usize,
    pub brand: String,
    pub load: f32,
}

#[derive(Serialize, Deserialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub total_space: u64,
    pub available_space: u64,
}

#[derive(Serialize, Deserialize)]
pub struct GetSystemInfoOutput {
    pub os: OsInfo,
    pub memory: MemoryInfo,
    pub cpu: CpuInfo,
    pub disks: Vec<DiskInfo>,
}

/// Tool for retrieving detailed information about the host system.
pub struct GetSystemInfoTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for GetSystemInfoTool {
    const NAME: &'static str = "get_system_info";

    type Error = std::io::Error;
    type Args = GetSystemInfoArgs;
    type Output = GetSystemInfoOutput;

    fn description(&self) -> String {
        "Get a summary of system information, including OS, CPU, RAM, and Disk space. Use this to understand the environment you are operating in.".to_string()
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
        match self.env.get_system_info().await {
            Ok(info_json) => {
                let output: GetSystemInfoOutput =
                    serde_json::from_str(&info_json).map_err(|e| {
                        std::io::Error::other(format!("Failed to parse system info: {e}"))
                    })?;

                self.approval
                    .report_tool_result(Self::NAME.to_string(), info_json)
                    .await;

                Ok(output)
            }
            Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
        }
    }
}

// --- LIST PROCESSES ---

#[derive(Deserialize, Default)]
pub struct ListProcessesArgs {
    pub filter: Option<String>,
    pub sort_by: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f64,
    pub memory_bytes: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
}

#[derive(Serialize)]
pub struct ListProcessesOutput {
    pub processes: Vec<ProcessInfo>,
    pub total_matching: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

pub struct ListProcessesTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for ListProcessesTool {
    const NAME: &'static str = "list_processes";

    type Error = std::io::Error;
    type Args = ListProcessesArgs;
    type Output = ListProcessesOutput;

    fn description(&self) -> String {
        "List running processes on the system, including PID, name, CPU usage, memory usage, and disk I/O. By default returns the top 30 processes sorted by CPU usage. Pass `filter` to search by process name or `sort_by` ('cpu', 'memory', 'io', 'pid') to rank differently.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Optional case-insensitive substring filter on process name (e.g. 'cargo', 'node', 'firefox')."
                },
                "sort_by": {
                    "type": "string",
                    "enum": ["cpu", "memory", "io", "pid"],
                    "description": "Sort order for returned processes. Defaults to 'cpu'."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of processes to return (1-100, default: 30)."
                }
            }
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
        match self.env.list_processes().await {
            Ok(processes) => {
                let filter_lower = args
                    .filter
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_lowercase);

                let mut items: Vec<ProcessInfo> = processes
                    .into_iter()
                    .filter(|(_, name, _, _, _, _)| {
                        filter_lower
                            .as_ref()
                            .is_none_or(|f| name.to_lowercase().contains(f))
                    })
                    .map(
                        |(pid, name, cpu_usage, memory_bytes, read_bytes, written_bytes)| {
                            ProcessInfo {
                                pid,
                                name,
                                cpu_usage,
                                memory_bytes,
                                read_bytes,
                                written_bytes,
                            }
                        },
                    )
                    .collect();

                match args.sort_by.as_deref().unwrap_or("cpu") {
                    "memory" => items.sort_by(|a, b| {
                        b.memory_bytes.cmp(&a.memory_bytes).then_with(|| {
                            b.cpu_usage
                                .partial_cmp(&a.cpu_usage)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                    }),
                    "io" => items.sort_by(|a, b| {
                        let io_b = b.read_bytes.saturating_add(b.written_bytes);
                        let io_a = a.read_bytes.saturating_add(a.written_bytes);
                        io_b.cmp(&io_a).then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
                    }),
                    "pid" => items.sort_by_key(|p| p.pid),
                    _ => items.sort_by(|a, b| {
                        b.cpu_usage
                            .partial_cmp(&a.cpu_usage)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
                    }),
                }

                let total_matching = items.len();
                let limit = args.limit.unwrap_or(30).clamp(1, 100);
                let truncated = total_matching > limit;
                items.truncate(limit);

                let output = ListProcessesOutput {
                    processes: items,
                    total_matching,
                    truncated,
                };

                // Report to UI for structured rendering
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&output).unwrap_or_default(),
                    )
                    .await;

                Ok(output)
            }
            Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
        }
    }
}

// --- KILL PROCESS ---

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
                "pid": { "type": "number" },
                "process_name": { "type": "string" },
                "signal": { "type": "number", "description": "The signal to send (e.g. 15 for SIGTERM, 9 for SIGKILL). Defaults to 15." }
            },
            "required": ["pid", "process_name"]
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
        let outcome = self
            .approval
            .propose_kill_process(args.pid, args.process_name.clone())
            .await;
        self.approval.set_thinking(true).await;

        if outcome.is_approved() {
            let signal = args.signal.unwrap_or(15);
            match self.env.kill_process(args.pid, signal).await {
                Ok(()) => {
                    let out = KillProcessOutput {
                        success: true,
                        message: format!(
                            "Successfully sent signal {} to process {}",
                            signal, args.pid
                        ),
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
            let out = KillProcessOutput {
                success: false,
                message: outcome.rejection_message(),
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

    struct MockProcEnv;

    #[async_trait::async_trait]
    impl ClawEnvironment for MockProcEnv {
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
        async fn get_system_info(&self) -> anyhow::Result<String> {
            Ok("{}".to_string())
        }
        async fn list_processes(&self) -> anyhow::Result<Vec<(u32, String, f64, u64, u64, u64)>> {
            Ok(vec![
                (10, "init".to_string(), 0.1, 1024, 0, 0),
                (20, "cargo-build".to_string(), 85.5, 500_000, 1000, 2000),
                (30, "cargo-test".to_string(), 42.0, 900_000, 500, 500),
                (40, "firefox".to_string(), 12.0, 2_000_000, 100, 100),
            ])
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

    struct NoopApproval;

    #[async_trait::async_trait]
    impl ApprovalHandler for NoopApproval {
        async fn propose_file_write(
            &self,
            _: String,
            _: String,
            _: crate::FileDiff,
        ) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_file_delete(&self, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_background_command(
            &self,
            _: String,
            _: String,
        ) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_kill_process(&self, _: u32, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_get_clipboard(&self) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn propose_set_clipboard(&self, _: String) -> crate::ApprovalOutcome {
            crate::ApprovalOutcome::Approved
        }
        async fn report_tool_result(&self, _: String, _: String) {}
        async fn report_tool_started(&self, _: String) {}
        async fn set_thinking(&self, _: bool) {}
    }

    #[tokio::test]
    async fn list_processes_filters_sorts_and_truncates() {
        let tool = ListProcessesTool {
            env: Arc::new(MockProcEnv),
            approval: Arc::new(NoopApproval),
        };

        // Filter by 'cargo', sorted by memory descending
        let mut ctx = ToolContext::default();
        let out = tool
            .call(
                &mut ctx,
                ListProcessesArgs {
                    filter: Some("CARGO".to_string()),
                    sort_by: Some("memory".to_string()),
                    limit: Some(1),
                },
            )
            .await
            .unwrap();

        assert_eq!(out.total_matching, 2);
        assert!(out.truncated);
        assert_eq!(out.processes.len(), 1);
        assert_eq!(out.processes[0].name, "cargo-test");
        assert_eq!(out.processes[0].pid, 30);
    }
}
