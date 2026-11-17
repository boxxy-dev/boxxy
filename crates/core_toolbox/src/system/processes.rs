//! Filtered, sorted, bounded running-process inspection tool (`list_processes`).

use crate::ApprovalHandler;
use crate::approval::{begin_tool, finish_tool};
use crate::utils::contains_case_insensitive;
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type ProcessInfo = boxxy_claw_protocol::ProcessSample;

#[derive(Deserialize, Default)]
pub struct ListProcessesArgs {
    pub filter: Option<String>,
    pub sort_by: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct ListProcessesOutput {
    pub processes: Vec<ProcessInfo>,
    pub total_matching: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let output = match self.env.list_processes().await {
            Ok(processes) => {
                let filter_lower = args
                    .filter
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_lowercase);

                let mut items: Vec<ProcessInfo> = match filter_lower.as_deref() {
                    Some(f) => processes
                        .into_iter()
                        .filter(|p| contains_case_insensitive(&p.name, f))
                        .collect(),
                    None => processes,
                };

                let sort_mode = args.sort_by.as_deref().unwrap_or("cpu");
                match sort_mode {
                    "memory" => items.sort_unstable_by(|a, b| {
                        b.memory_bytes
                            .cmp(&a.memory_bytes)
                            .then_with(|| b.cpu_usage.total_cmp(&a.cpu_usage))
                            .then_with(|| a.pid.cmp(&b.pid))
                    }),
                    "gpu" => items.sort_unstable_by(|a, b| {
                        b.gpu_usage
                            .unwrap_or(0.0)
                            .total_cmp(&a.gpu_usage.unwrap_or(0.0))
                            .then_with(|| {
                                b.gpu_memory_bytes
                                    .unwrap_or(0)
                                    .cmp(&a.gpu_memory_bytes.unwrap_or(0))
                            })
                            .then_with(|| b.cpu_usage.total_cmp(&a.cpu_usage))
                            .then_with(|| a.pid.cmp(&b.pid))
                    }),
                    "gpu_memory" => items.sort_unstable_by(|a, b| {
                        b.gpu_memory_bytes
                            .unwrap_or(0)
                            .cmp(&a.gpu_memory_bytes.unwrap_or(0))
                            .then_with(|| {
                                b.gpu_usage
                                    .unwrap_or(0.0)
                                    .total_cmp(&a.gpu_usage.unwrap_or(0.0))
                            })
                            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
                            .then_with(|| a.pid.cmp(&b.pid))
                    }),
                    "io" => items.sort_unstable_by(|a, b| {
                        let io_b = b.read_bytes.saturating_add(b.written_bytes);
                        let io_a = a.read_bytes.saturating_add(a.written_bytes);
                        io_b.cmp(&io_a)
                            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
                            .then_with(|| a.pid.cmp(&b.pid))
                    }),
                    "pid" => items.sort_unstable_by_key(|p| p.pid),
                    _ => items.sort_unstable_by(|a, b| {
                        b.cpu_usage
                            .total_cmp(&a.cpu_usage)
                            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
                            .then_with(|| a.pid.cmp(&b.pid))
                    }),
                }

                let total_matching = items.len();
                let limit = args.limit.unwrap_or(30).clamp(1, 100);
                let truncated = total_matching > limit;
                items.truncate(limit);

                ListProcessesOutput {
                    processes: items,
                    total_matching,
                    truncated,
                    error: None,
                }
            }
            Err(e) => ListProcessesOutput {
                processes: Vec::new(),
                total_matching: 0,
                truncated: false,
                error: Some(format!("Environment Error: {e}")),
            },
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &output).await;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockApproval, MockEnv};

    #[tokio::test]
    async fn list_processes_filters_sorts_and_truncates() {
        let env = Arc::new(MockEnv::new());
        *env.processes.lock() = vec![
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
        ];

        let tool = ListProcessesTool {
            env,
            approval: Arc::new(MockApproval::approved(None)),
        };

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
}
