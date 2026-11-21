//! Host OS, CPU, RAM, GPU, disk, and thermal sensor telemetry tool (`get_system_info`).

use crate::ApprovalHandler;
use crate::approval::{begin_tool, report_error};
use boxxy_claw_protocol::ClawEnvironment;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

#[allow(clippy::trivially_copy_pass_by_ref)] // serde `skip_serializing_if` requires `&T`
const fn is_zero_u64(v: &u64) -> bool {
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

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        rig::completion::ToolAnnotations::default()
            .with_read_only(true)
            .with_open_world(false)
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        _args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        match self.env.get_system_info().await {
            Ok(info_json) => match serde_json::from_str::<GetSystemInfoOutput>(&info_json) {
                Ok(output) => {
                    self.approval
                        .report_tool_result(Self::NAME.to_string(), info_json)
                        .await;
                    Ok(output)
                }
                Err(e) => {
                    let msg = format!("Failed to parse system info: {e}");
                    report_error(self.approval.as_ref(), Self::NAME, &msg).await;
                    Err(std::io::Error::other(msg))
                }
            },
            Err(e) => {
                let msg = format!("Environment Error: {e}");
                report_error(self.approval.as_ref(), Self::NAME, &msg).await;
                Err(std::io::Error::other(msg))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockApproval, MockEnv};

    #[tokio::test]
    async fn get_system_info_parses_gpus_thermals_and_extended_fields() {
        let env = Arc::new(MockEnv::new());
        *env.system_info_json.lock() = Some(
            serde_json::json!({
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
            .to_string(),
        );

        let tool = GetSystemInfoTool {
            env,
            approval: Arc::new(MockApproval::approved(None)),
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
        assert!((out.temperatures[0].temperature_celsius - 54.5).abs() < f32::EPSILON);
    }
}
