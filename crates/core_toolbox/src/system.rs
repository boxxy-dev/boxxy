use crate::ApprovalHandler;
use boxxy_claw_protocol::ClawEnvironment;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Get a summary of system information, including OS, CPU, RAM, and Disk space. Use this to understand the environment you are operating in.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.report_tool_started(Self::NAME.to_string()).await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        match self.env.get_system_info().await {
            Ok(info_json) => {
                let output: GetSystemInfoOutput =
                    serde_json::from_str(&info_json).map_err(|e| {
                        std::io::Error::other(format!("Failed to parse system info: {e}"))
                    })?;

                Ok(output)
            }
            Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
        }
    }
}

// --- LIST PROCESSES ---

#[derive(Deserialize)]
pub struct ListProcessesArgs {}

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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List all running processes on the system, including their PID, name, CPU usage, memory usage, and disk I/O.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.report_tool_started(Self::NAME.to_string()).await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        match self.env.list_processes().await {
            Ok(processes) => {
                let output = ListProcessesOutput {
                    processes: processes
                        .into_iter()
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
                        .collect(),
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Terminate a running process. This will prompt the user for approval. You MUST provide the PID and the name of the process for clarity.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pid": { "type": "number" },
                    "process_name": { "type": "string" },
                    "signal": { "type": "number", "description": "The signal to send (e.g. 15 for SIGTERM, 9 for SIGKILL). Defaults to 15." }
                },
                "required": ["pid", "process_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.report_tool_started(Self::NAME.to_string()).await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        self.approval.set_thinking(false).await;
        let approved = self
            .approval
            .propose_kill_process(args.pid, args.process_name.clone())
            .await;
        self.approval.set_thinking(true).await;

        if approved {
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
                message: "[USER_EXPLICIT_REJECT]".to_string(),
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
