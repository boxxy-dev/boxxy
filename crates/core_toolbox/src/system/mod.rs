//! Host system telemetry, process inspection/control, and detached background spawning tools.

pub mod background;
mod info;
mod kill;
mod processes;

pub use background::{RunBackgroundCommandArgs, RunBackgroundCommandOutput, RunBackgroundCommandTool};
pub use info::{
    CpuInfo, DiskInfo, GetSystemInfoArgs, GetSystemInfoOutput, GetSystemInfoTool, GpuInfo,
    LoadAvgInfo, MemoryInfo, OsInfo, ThermalSensorInfo,
};
pub use kill::{KillProcessArgs, KillProcessOutput, KillProcessTool};
pub use processes::{ListProcessesArgs, ListProcessesOutput, ListProcessesTool, ProcessInfo};
