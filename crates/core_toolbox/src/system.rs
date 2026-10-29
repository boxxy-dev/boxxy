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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_version: Option<String>,
    pub kernel: Option<String>,
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    #[serde(default)]
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Serialize, Deserialize)]
pub struct LoadAvgInfo {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Serialize, Deserialize)]
pub struct CpuInfo {
    pub count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_cores: Option<usize>,
    pub brand: String,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub frequency_mhz: u64,
    pub load: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_average: Option<LoadAvgInfo>,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

#[derive(Serialize, Deserialize)]
pub struct GpuInfo {
    pub pci: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_vram_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_vram_bytes: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_system: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub total_space: u64,
    pub available_space: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_removable: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_read_only: bool,
}

#[derive(Serialize, Deserialize)]
pub struct ThermalSensorInfo {
    pub label: String,
    pub temperature_celsius: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_celsius: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_celsius: Option<f32>,
}

#[derive(Serialize, Deserialize)]
pub struct GetSystemInfoOutput {
    pub os: OsInfo,
    pub memory: MemoryInfo,
    pub cpu: CpuInfo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpus: Vec<GpuInfo>,
    pub disks: Vec<DiskInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub temperatures: Vec<ThermalSensorInfo>,
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
        "Get a summary of system information, including OS, CPU, RAM, GPUs (model, usage %, VRAM), Disk space, and thermal sensor temperatures. Use this to understand the hardware and environment you are operating in.".to_string()
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

pub type ProcessInfo = boxxy_claw_protocol::ProcessSample;

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
        "List running processes on the system, including PID, name, CPU usage, memory usage, disk I/O, and GPU usage / GPU memory (when active). By default returns the top 30 processes sorted by CPU usage. Pass `filter` to search by process name or `sort_by` ('cpu', 'memory', 'gpu', 'gpu_memory', 'io', 'pid') to rank differently.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Optional case-insensitive substring filter on process name (e.g. 'cargo', 'node', 'firefox', 'ollama')."
                },
                "sort_by": {
                    "type": "string",
                    "enum": ["cpu", "memory", "gpu", "gpu_memory", "io", "pid"],
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
                    .filter(|p| {
                        filter_lower
                            .as_ref()
                            .is_none_or(|f| p.name.to_lowercase().contains(f))
                    })
                    .collect();

                match args.sort_by.as_deref().unwrap_or("cpu") {
                    "memory" => items.sort_by(|a, b| {
                        b.memory_bytes.cmp(&a.memory_bytes).then_with(|| {
                            b.cpu_usage
                                .partial_cmp(&a.cpu_usage)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                    }),
                    "gpu" => items.sort_by(|a, b| {
                        let gpu_b = b.gpu_usage.unwrap_or(0.0);
                        let gpu_a = a.gpu_usage.unwrap_or(0.0);
                        gpu_b
                            .partial_cmp(&gpu_a)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| {
                                b.gpu_memory_bytes
                                    .unwrap_or(0)
                                    .cmp(&a.gpu_memory_bytes.unwrap_or(0))
                            })
                            .then_with(|| {
                                b.cpu_usage
                                    .partial_cmp(&a.cpu_usage)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                    }),
                    "gpu_memory" => items.sort_by(|a, b| {
                        b.gpu_memory_bytes
                            .unwrap_or(0)
                            .cmp(&a.gpu_memory_bytes.unwrap_or(0))
                            .then_with(|| {
                                b.gpu_usage
                                    .unwrap_or(0.0)
                                    .partial_cmp(&a.gpu_usage.unwrap_or(0.0))
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
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
            Ok(serde_json::json!({
                "os": {
                    "name": "Linux",
                    "version": "6.12",
                    "long_version": "Linux 6.12",
                    "kernel": "6.12.0",
                    "hostname": "boxxy-host",
                    "arch": "x86_64",
                    "uptime_secs": 3600
                },
                "memory": {
                    "total": 16_000_000_000_u64,
                    "used": 8_000_000_000_u64,
                    "free": 2_000_000_000_u64,
                    "available": 8_000_000_000_u64,
                    "swap_total": 4_000_000_000_u64,
                    "swap_used": 0_u64
                },
                "cpu": {
                    "count": 16,
                    "physical_cores": 8,
                    "brand": "Synthetic CPU",
                    "frequency_mhz": 4200,
                    "load": 14.5,
                    "load_average": { "one": 1.2, "five": 0.8, "fifteen": 0.5 }
                },
                "gpus": [
                    {
                        "pci": "0000:03:00.0",
                        "vendor": "AMD",
                        "model": "Radeon RX Synthetic",
                        "usage_percent": 32.0,
                        "total_vram_bytes": 16_000_000_000_u64,
                        "used_vram_bytes": 4_000_000_000_u64
                    }
                ],
                "disks": [
                    {
                        "name": "nvme0n1p2",
                        "mount_point": "/",
                        "file_system": "ext4",
                        "kind": "SSD",
                        "total_space": 500_000_000_000_u64,
                        "available_space": 300_000_000_000_u64,
                        "is_removable": false,
                        "is_read_only": false
                    }
                ],
                "temperatures": [
                    {
                        "label": "k10temp Tctl",
                        "temperature_celsius": 54.5,
                        "max_celsius": 70.0,
                        "critical_celsius": 95.0
                    }
                ]
            })
            .to_string())
        }
        async fn list_processes(&self) -> anyhow::Result<Vec<ProcessInfo>> {
            Ok(vec![
                ProcessInfo {
                    pid: 10,
                    name: "init".to_string(),
                    cpu_usage: 0.1,
                    memory_bytes: 1024,
                    read_bytes: 0,
                    written_bytes: 0,
                    gpu_usage: None,
                    gpu_memory_bytes: None,
                },
                ProcessInfo {
                    pid: 20,
                    name: "cargo-build".to_string(),
                    cpu_usage: 85.5,
                    memory_bytes: 500_000,
                    read_bytes: 1000,
                    written_bytes: 2000,
                    gpu_usage: None,
                    gpu_memory_bytes: None,
                },
                ProcessInfo {
                    pid: 30,
                    name: "cargo-test".to_string(),
                    cpu_usage: 42.0,
                    memory_bytes: 900_000,
                    read_bytes: 500,
                    written_bytes: 500,
                    gpu_usage: Some(15.0),
                    gpu_memory_bytes: Some(256 * 1024 * 1024),
                },
                ProcessInfo {
                    pid: 40,
                    name: "firefox".to_string(),
                    cpu_usage: 12.0,
                    memory_bytes: 2_000_000,
                    read_bytes: 100,
                    written_bytes: 100,
                    gpu_usage: Some(68.5),
                    gpu_memory_bytes: Some(1024 * 1024 * 1024),
                },
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

        // Sort by GPU usage descending across all processes
        let gpu_out = tool
            .call(
                &mut ctx,
                ListProcessesArgs {
                    filter: None,
                    sort_by: Some("gpu".to_string()),
                    limit: Some(2),
                },
            )
            .await
            .unwrap();
        assert_eq!(gpu_out.processes.len(), 2);
        assert_eq!(gpu_out.processes[0].name, "firefox");
        assert_eq!(gpu_out.processes[0].gpu_usage, Some(68.5));
        assert_eq!(gpu_out.processes[1].name, "cargo-test");
    }

    #[tokio::test]
    async fn get_system_info_parses_gpus_thermals_and_extended_fields() {
        let tool = GetSystemInfoTool {
            env: Arc::new(MockProcEnv),
            approval: Arc::new(NoopApproval),
        };
        let mut ctx = ToolContext::default();
        let out = tool.call(&mut ctx, GetSystemInfoArgs {}).await.unwrap();

        assert_eq!(out.os.arch.as_deref(), Some("x86_64"));
        assert_eq!(out.memory.available, 8_000_000_000);
        assert_eq!(out.cpu.physical_cores, Some(8));
        assert_eq!(out.gpus.len(), 1);
        assert_eq!(out.gpus[0].model.as_deref(), Some("Radeon RX Synthetic"));
        assert_eq!(out.gpus[0].usage_percent, Some(32.0));
        assert_eq!(out.disks[0].kind, "SSD");
        assert_eq!(out.temperatures.len(), 1);
        assert_eq!(out.temperatures[0].temperature_celsius, 54.5);
    }
}
