//! Lazy, on-demand host system and GPU sampler powered by `sysinfo 0.40`.
//!
//! ### Efficiency & Zero-Overhead Guarantees
//! - **Zero startup cost:** `SystemSampler` starts as `None`. No `/proc` reads,
//!   allocations, or dynamic library loads (`libnvidia-ml`, `libvulkan`) occur
//!   when the daemon starts or while tools are unused.
//! - **Warm delta reuse:** CPU and per-process GPU usage in `sysinfo` are
//!   time-delta based. By caching the `System` and `Gpus` instances across tool
//!   invocations, subsequent calls reuse the internal buffer allocations and
//!   compute accurate usage deltas immediately (0ms wait) when enough time has
//!   elapsed since the last refresh.
//! - **Bounded cold-start sampling:** Only on the very first call (or if two
//!   calls arrive within `< MINIMUM_CPU_UPDATE_INTERVAL`), a single short sleep
//!   is performed on the blocking worker thread to establish a valid baseline
//!   delta instead of returning misleading `0.0%` values.
//! - **No thread/task bloat:** `ProcessRefreshKind` explicitly sets
//!   `.without_tasks()` so Linux `/proc/<pid>/task/<tid>` subdirectories are
//!   never traversed.

use boxxy_claw_protocol::ProcessSample;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;
use sysinfo::{
    Components, CpuRefreshKind, DiskRefreshKind, Disks, Gpus, MemoryRefreshKind,
    MINIMUM_CPU_UPDATE_INTERVAL, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System,
};

#[derive(Clone, Default)]
pub struct SystemSampler {
    inner: Arc<Mutex<SamplerState>>,
}

/// Wrapper around `sysinfo::Gpus` that is accessed exclusively behind
/// `SamplerState`'s `Mutex` (single-threaded access at a time).
/// `sysinfo::Gpus` holds `dlopen` handles (`*mut c_void`) on Linux for NVML and
/// Vulkan which are safe to move across threads when synchronized by a `Mutex`.
struct SendGpus(Gpus);

// SAFETY: `SendGpus` is only ever accessed while holding the `SamplerState`
// `Mutex` lock, guaranteeing exclusive access and no concurrent FFI calls.
unsafe impl Send for SendGpus {}

#[derive(Default)]
struct SamplerState {
    system: Option<System>,
    last_cpu_refresh: Option<Instant>,
    last_proc_refresh: Option<Instant>,
    gpus: Option<SendGpus>,
    disks: Option<Disks>,
    components: Option<Components>,
}

impl SystemSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Collects a full system hardware, OS, memory, CPU, GPU, disk, and thermal
    /// snapshot as a JSON string. Must be called from a blocking thread.
    pub fn collect_system_info_json(&self) -> anyhow::Result<String> {
        let mut state = self.inner.lock();

        let cpu_kind = CpuRefreshKind::everything();
        let mem_kind = MemoryRefreshKind::everything();
        let needs_two_pass = state
            .last_cpu_refresh
            .is_none_or(|t| t.elapsed() < MINIMUM_CPU_UPDATE_INTERVAL);

        if let Some(sys) = state.system.as_mut() {
            sys.refresh_memory_specifics(mem_kind);
            sys.refresh_cpu_specifics(cpu_kind);
            if needs_two_pass {
                std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
                sys.refresh_cpu_usage();
            }
        } else {
            let mut sys = System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(cpu_kind)
                    .with_memory(mem_kind),
            )
            .map_err(|e| anyhow::anyhow!("Failed to initialize sysinfo::System: {e}"))?;
            std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
            sys.refresh_cpu_usage();
            state.system = Some(sys);
        }
        state.last_cpu_refresh = Some(Instant::now());

        let sys = state.system.as_ref().expect("system initialized above");
        let total_memory = sys.total_memory();
        let used_memory = sys.used_memory();
        let free_memory = sys.free_memory();
        let available_memory = sys.available_memory();
        let swap_total = sys.total_swap();
        let swap_used = sys.used_swap();

        let cpu_count = sys.cpus().len();
        let cpu_brand = sys
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown".to_string());
        let cpu_frequency_mhz = sys.cpus().first().map(|c| c.frequency()).unwrap_or(0);
        let cpu_load = sys.global_cpu_usage();

        // Disks (storage & kind only; skip per-disk I/O counters for fast snapshot).
        // Deduplicate by (device name, total_space) so Btrfs subvolumes or bind mounts
        // sharing the same underlying block device don't flood the JSON output.
        let disk_refresh = DiskRefreshKind::nothing().with_storage().with_kind();
        let disks_json = match state.disks.as_mut() {
            Some(disks) => {
                disks.refresh_specifics(true, disk_refresh);
                serialize_disks(disks)
            }
            None => match Disks::new_with_refreshed_list_specifics(disk_refresh) {
                Ok(disks) => {
                    let out = serialize_disks(&disks);
                    state.disks = Some(disks);
                    out
                }
                Err(_) => Vec::new(),
            },
        };

        // GPUs (AMD sysfs, NVIDIA NVML, or Vulkan)
        let gpus_json = match state.gpus.as_mut() {
            Some(SendGpus(gpus)) => {
                gpus.refresh(true);
                serialize_gpus(gpus)
            }
            None => match Gpus::new_with_refreshed_list() {
                Ok(gpus) => {
                    let out = serialize_gpus(&gpus);
                    state.gpus = Some(SendGpus(gpus));
                    out
                }
                Err(_) => Vec::new(),
            },
        };

        // Thermal sensors (hwmon / thermal_zone)
        let temps_json = match state.components.as_mut() {
            Some(components) => {
                components.refresh(true);
                serialize_components(components)
            }
            None => match Components::new_with_refreshed_list() {
                Ok(components) => {
                    let out = serialize_components(&components);
                    state.components = Some(components);
                    out
                }
                Err(_) => Vec::new(),
            },
        };

        let load_avg = System::load_average().ok().map(|l| {
            serde_json::json!({
                "one": l.one,
                "five": l.five,
                "fifteen": l.fifteen,
            })
        });

        let info = serde_json::json!({
            "os": {
                "name": System::name().ok(),
                "version": System::os_version().ok(),
                "long_version": System::long_os_version().ok(),
                "kernel": System::kernel_version().ok(),
                "hostname": System::host_name().ok(),
                "arch": System::cpu_arch(),
                "uptime_secs": System::uptime().ok(),
            },
            "memory": {
                "total": total_memory,
                "used": used_memory,
                "free": free_memory,
                "available": available_memory,
                "swap_total": swap_total,
                "swap_used": swap_used,
            },
            "cpu": {
                "count": cpu_count,
                "physical_cores": System::physical_core_count().ok(),
                "brand": cpu_brand,
                "frequency_mhz": cpu_frequency_mhz,
                "load": cpu_load,
                "load_average": load_avg,
            },
            "gpus": gpus_json,
            "disks": disks_json,
            "temperatures": temps_json,
        });

        Ok(info.to_string())
    }

    /// Collects all running processes with accurate CPU, memory, disk I/O, and
    /// GPU usage/VRAM metrics. Must be called from a blocking thread.
    pub fn collect_processes(&self) -> anyhow::Result<Vec<ProcessSample>> {
        let mut state = self.inner.lock();

        // Explicitly disable `.without_tasks()` so Linux threads in `/proc/<pid>/task`
        // are not enumerated as separate processes.
        let proc_kind = ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cpu()
            .with_memory()
            .with_disk_usage()
            .with_exe(sysinfo::UpdateKind::OnlyIfNotSet)
            .with_gpu_usage()
            .with_gpu_memory();

        let delta_kind = ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cpu()
            .with_gpu_usage();

        let needs_two_pass = state
            .last_proc_refresh
            .is_none_or(|t| t.elapsed() < MINIMUM_CPU_UPDATE_INTERVAL);

        if let Some(sys) = state.system.as_mut() {
            sys.refresh_processes_specifics(ProcessesToUpdate::All, true, proc_kind);
            if needs_two_pass {
                std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
                // On the second pass after a short sleep, refresh delta-based metrics
                // (CPU & GPU usage) plus GPU memory so neither gets cleared.
                let second_pass_kind = delta_kind.with_gpu_memory();
                sys.refresh_processes_specifics(ProcessesToUpdate::All, true, second_pass_kind);
            }
        } else {
            let mut sys =
                System::new_with_specifics(RefreshKind::nothing().with_processes(proc_kind))
                    .map_err(|e| anyhow::anyhow!("Failed to initialize sysinfo::System: {e}"))?;
            std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
            let second_pass_kind = delta_kind.with_gpu_memory();
            sys.refresh_processes_specifics(ProcessesToUpdate::All, true, second_pass_kind);
            state.system = Some(sys);
        }

        let now = Instant::now();
        state.last_proc_refresh = Some(now);
        state.last_cpu_refresh = Some(now);

        let sys = state.system.as_ref().expect("system initialized above");
        let mut processes = Vec::with_capacity(sys.processes().len());
        for (pid, process) in sys.processes() {
            // Skip kernel threads if any were captured at top-level `/proc`
            if process.thread_kind() == Some(sysinfo::ThreadKind::Kernel) {
                continue;
            }
            let disk_usage = process.disk_usage();
            let gpu_usage = process.gpu_usage().filter(|u| u.is_finite() && *u > 0.0);
            let gpu_memory_bytes = process.gpu_memory().filter(|m| *m > 0);
            let exe_name = process
                .exe()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            let proc_name = process.name().to_string_lossy().into_owned();
            // On Linux, `process.name()` is `/proc/<pid>/comm`, which can be overridden by
            // the main thread's `pthread_setname_np` (e.g. "GUI Thread" or "MainThread").
            // When `comm` looks like a generic thread label and `exe` is available, prefer
            // the executable basename so the process list stays recognizable.
            let name = match exe_name {
                Some(exe)
                    if !exe.is_empty()
                        && (proc_name.contains("Thread")
                            || proc_name == "Main"
                            || proc_name.is_empty()) =>
                {
                    format!("{exe} ({proc_name})")
                }
                _ => proc_name,
            };

            processes.push(ProcessSample {
                pid: pid.as_u32(),
                name,
                cpu_usage: f64::from(process.cpu_usage()),
                memory_bytes: process.memory(),
                read_bytes: disk_usage.total_read_bytes,
                written_bytes: disk_usage.total_written_bytes,
                gpu_usage,
                gpu_memory_bytes,
            });
        }

        Ok(processes)
    }
}

fn serialize_disks(disks: &Disks) -> Vec<serde_json::Value> {
    let mut seen = std::collections::HashSet::new();
    disks
        .iter()
        .filter(|d| {
            let name = d.name().to_string_lossy().into_owned();
            // If device name is non-empty, deduplicate multiple subvolume mounts of the same partition
            if !name.is_empty() {
                seen.insert((name, d.total_space()))
            } else {
                true
            }
        })
        .map(|d| {
            serde_json::json!({
                "name": d.name().to_string_lossy(),
                "mount_point": d.mount_point().to_string_lossy(),
                "file_system": d.file_system().to_string_lossy(),
                "kind": d.kind().to_string(),
                "total_space": d.total_space(),
                "available_space": d.available_space(),
                "is_removable": d.is_removable(),
                "is_read_only": d.is_read_only(),
            })
        })
        .collect()
}

fn serialize_gpus(gpus: &Gpus) -> Vec<serde_json::Value> {
    gpus.iter()
        .map(|g| {
            serde_json::json!({
                "pci": g.pci().to_string(),
                "vendor": g.vendor(),
                "model": g.model(),
                "usage_percent": g.usage().filter(|u| u.is_finite()),
                "total_vram_bytes": g.total_memory(),
                "used_vram_bytes": g.used_memory(),
            })
        })
        .collect()
}

fn serialize_components(components: &Components) -> Vec<serde_json::Value> {
    components
        .iter()
        .filter_map(|c| {
            let temp = c.temperature().filter(|t| t.is_finite())?;
            Some(serde_json::json!({
                "label": c.label(),
                "temperature_celsius": temp,
                "max_celsius": c.max().filter(|t| t.is_finite()),
                "critical_celsius": c.critical().filter(|t| t.is_finite()),
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_starts_cold_with_zero_allocations() {
        let sampler = SystemSampler::new();
        let guard = sampler.inner.lock();
        assert!(guard.system.is_none());
        assert!(guard.gpus.is_none());
        assert!(guard.disks.is_none());
        assert!(guard.components.is_none());
    }

    #[test]
    fn collect_system_info_and_processes_work_and_warm_cache() {
        let sampler = SystemSampler::new();
        let json_str = sampler
            .collect_system_info_json()
            .expect("collect_system_info_json should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("valid JSON output");

        assert!(parsed.get("os").is_some());
        assert!(parsed["memory"]["total"].as_u64().unwrap_or(0) > 0);
        assert!(parsed["memory"]["available"].as_u64().is_some());
        assert!(parsed["cpu"]["count"].as_u64().unwrap_or(0) > 0);
        assert!(parsed.get("gpus").and_then(|v| v.as_array()).is_some());
        assert!(parsed.get("disks").and_then(|v| v.as_array()).is_some());
        assert!(parsed.get("temperatures").and_then(|v| v.as_array()).is_some());

        let procs = sampler
            .collect_processes()
            .expect("collect_processes should succeed");
        assert!(!procs.is_empty(), "process list should not be empty");
        assert!(
            procs.iter().any(|p| p.pid == std::process::id()),
            "current test process should be present in process list"
        );
    }
}
