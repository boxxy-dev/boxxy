use crate::core::state::AgentState;
use boxxy_claw_protocol::ClawEnvironment;
use std::process::Stdio;
use tokio::process::Command;
use zbus::fdo;
use zbus::interface;

pub mod notifier;
pub mod registry;

pub use registry::CharacterRegistry;

pub struct ClawSubsystem {
    state: AgentState,
}

impl ClawSubsystem {
    pub fn new(state: AgentState) -> Self {
        Self { state }
    }
}

#[async_trait::async_trait]
impl ClawEnvironment for ClawSubsystem {
    async fn exec_shell(&self, command: String) -> anyhow::Result<(i32, String, String)> {
        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let output = child.wait_with_output().await?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        Ok((exit_code, stdout, stderr))
    }

    async fn spawn_detached(&self, command: String, cwd: String) -> anyhow::Result<u32> {
        use std::os::unix::process::CommandExt;

        if !std::path::Path::new(&cwd).exists() {
            return Err(anyhow::anyhow!("Working directory does not exist: {}", cwd));
        }

        let mut child = unsafe {
            Command::new("bash")
                .arg("-c")
                .arg(&command)
                .current_dir(&cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::piped())
                .pre_exec(|| {
                    libc::setsid();
                    Ok(())
                })
                .spawn()?
        };

        let pid = child.id().unwrap_or(0);

        tokio::select! {
            result = child.wait_with_output() => {
                let out = result?;
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    return Err(anyhow::anyhow!("Process exited immediately: {}", stderr));
                }
                // Exited with code 0 immediately — launcher script pattern, still valid
                Ok(pid)
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                // Still running — drop the piped stderr handle, detach fully
                Ok(pid)
            }
        }
    }

    async fn read_file(
        &self,
        path: String,
        start_line: u32,
        end_line: u32,
    ) -> anyhow::Result<String> {
        let blacklisted = load_blacklist();

        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(anyhow::anyhow!("Path is blacklisted for security reasons"));
            }
        }

        if start_line == 0 && end_line == 0 {
            let content = tokio::fs::read_to_string(&path).await?;
            Ok(content)
        } else {
            let content = tokio::fs::read_to_string(&path).await?;
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len() as u32;

            let start = if start_line > 0 {
                start_line.saturating_sub(1)
            } else {
                0
            };
            let end = if end_line > 0 {
                end_line.min(total_lines)
            } else {
                total_lines
            };

            if start >= total_lines {
                return Ok(format!(
                    "[WARNING: Start line {} exceeds total lines {}]",
                    start + 1,
                    total_lines
                ));
            }

            let selected_lines = &lines[start as usize..end as usize];
            let mut result = selected_lines.join("\n");

            if start > 0 || end < total_lines {
                result = format!(
                    "[Showing lines {}-{} of {}]\n{}",
                    start + 1,
                    end,
                    total_lines,
                    result
                );
            }

            Ok(result)
        }
    }

    async fn write_file(&self, path: String, content: String) -> anyhow::Result<()> {
        let blacklisted = load_blacklist();

        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(anyhow::anyhow!("Path is blacklisted for security reasons"));
            }
        }

        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn list_directory(&self, path: String) -> anyhow::Result<Vec<(String, bool, u64)>> {
        let blacklisted = load_blacklist();
        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(anyhow::anyhow!("Path is blacklisted for security reasons"));
            }
        }

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&path).await?;

        while let Some(entry) = dir.next_entry().await? {
            let metadata = entry.metadata().await.ok();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            entries.push((name, is_dir, size));
        }

        Ok(entries)
    }

    async fn delete_file(&self, path: String) -> anyhow::Result<()> {
        let blacklisted = load_blacklist();
        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(anyhow::anyhow!("Path is blacklisted for security reasons"));
            }
        }

        tokio::fs::remove_file(&path).await?;
        Ok(())
    }

    async fn get_system_info(&self) -> anyhow::Result<String> {
        use sysinfo::{Disks, System};
        let mut sys = System::new_all();
        sys.refresh_all();

        let disks = Disks::new_with_refreshed_list();

        let info = serde_json::json!({
            "os": {
                "name": System::name(),
                "version": System::os_version(),
                "kernel": System::kernel_version(),
                "hostname": System::host_name(),
            },
            "memory": {
                "total": sys.total_memory(),
                "used": sys.used_memory(),
                "free": sys.free_memory(),
                "swap_total": sys.total_swap(),
                "swap_used": sys.used_swap(),
            },
            "cpu": {
                "count": sys.cpus().len(),
                "brand": sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_else(|| "Unknown".to_string()),
                "load": sys.global_cpu_usage(),
            },
            "disks": disks.iter().map(|d| {
                serde_json::json!({
                    "name": d.name().to_string_lossy(),
                    "mount_point": d.mount_point().to_string_lossy(),
                    "total_space": d.total_space(),
                    "available_space": d.available_space(),
                })
            }).collect::<Vec<_>>(),
        });

        Ok(info.to_string())
    }

    async fn list_processes(&self) -> anyhow::Result<Vec<(u32, String, f64, u64, u64, u64)>> {
        use sysinfo::System;
        let mut sys = System::new_all();
        sys.refresh_all();

        let mut processes = Vec::new();
        for (pid, process) in sys.processes() {
            let disk_usage = process.disk_usage();
            processes.push((
                pid.as_u32(),
                process.name().to_string_lossy().to_string(),
                process.cpu_usage() as f64,
                process.memory(),
                disk_usage.total_read_bytes,
                disk_usage.total_written_bytes,
            ));
        }

        Ok(processes)
    }

    async fn kill_process(&self, pid: u32, signal: i32) -> anyhow::Result<()> {
        unsafe {
            if libc::kill(pid as i32, signal) != 0 {
                return Err(anyhow::anyhow!(
                    "Failed to kill process {}: kill failed",
                    pid
                ));
            }
        }
        Ok(())
    }

    async fn get_clipboard(&self) -> anyhow::Result<String> {
        let mut clipboard = arboard::Clipboard::new()?;
        let text = clipboard.get_text()?;
        Ok(text)
    }

    async fn set_clipboard(&self, text: String) -> anyhow::Result<()> {
        let mut clipboard = arboard::Clipboard::new()?;
        clipboard.set_text(text)?;
        Ok(())
    }
}

#[interface(name = "dev.boxxy.BoxxyTerminal.Agent.Claw")]
impl ClawSubsystem {
    async fn exec_shell(&self, command: String) -> fdo::Result<(i32, String, String)> {
        ClawEnvironment::exec_shell(self, command)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn spawn_detached(&self, command: String, cwd: String) -> fdo::Result<u32> {
        ClawEnvironment::spawn_detached(self, command, cwd)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn read_file(&self, path: String, start_line: u32, end_line: u32) -> fdo::Result<String> {
        ClawEnvironment::read_file(self, path, start_line, end_line)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn write_file(&self, path: String, content: String) -> fdo::Result<()> {
        ClawEnvironment::write_file(self, path, content)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn list_directory(&self, path: String) -> fdo::Result<Vec<(String, bool, u64)>> {
        ClawEnvironment::list_directory(self, path)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn delete_file(&self, path: String) -> fdo::Result<()> {
        ClawEnvironment::delete_file(self, path)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn get_system_info(&self) -> fdo::Result<String> {
        ClawEnvironment::get_system_info(self)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn list_processes(&self) -> fdo::Result<Vec<(u32, String, f64, u64, u64, u64)>> {
        ClawEnvironment::list_processes(self)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn kill_process(&self, pid: u32, signal: i32) -> fdo::Result<()> {
        ClawEnvironment::kill_process(self, pid, signal)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn get_clipboard(&self) -> fdo::Result<String> {
        ClawEnvironment::get_clipboard(self)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn set_clipboard(&self, text: String) -> fdo::Result<()> {
        ClawEnvironment::set_clipboard(self, text)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}

fn load_blacklist() -> Vec<String> {
    let mut blacklisted = vec![
        "/etc/shadow".to_string(),
        "/etc/gshadow".to_string(),
        "/root/.ssh".to_string(),
        ".ssh/id_rsa".to_string(),
        ".ssh/id_ed25519".to_string(),
    ];

    let config_dir = boxxy_sys_utils::get_config_dir();
    let blacklist_path = config_dir.join("boxxyclaw").join("BLACKLIST.md");
    if let Ok(content) = std::fs::read_to_string(&blacklist_path) {
        let mut new_blacklist = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                new_blacklist.push(trimmed.to_string());
            }
        }
        if !new_blacklist.is_empty() {
            blacklisted = new_blacklist;
        }
    }
    blacklisted
}
