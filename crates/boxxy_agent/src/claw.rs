use std::process::Stdio;
use tokio::process::Command;
use zbus::fdo;
use zbus::interface;

#[derive(Default)]
pub struct AgentClaw;

#[interface(name = "dev.boxxy.BoxxyTerminal.AgentClaw")]
impl AgentClaw {
    /// Execute a non-interactive shell command on the host.
    async fn exec_shell(&self, command: String) -> fdo::Result<(i32, String, String)> {
        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| fdo::Error::Failed(format!("Failed to spawn bash: {}", e)))?;

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to wait for output: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        Ok((exit_code, stdout, stderr))
    }

    /// Read a file from the host system.
    async fn read_file(&self, path: String, start_line: u32, end_line: u32) -> fdo::Result<String> {
        let blacklisted = load_blacklist();

        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(fdo::Error::Failed(
                    "Path is blacklisted for security reasons".to_string(),
                ));
            }
        }

        if start_line == 0 && end_line == 0 {
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => Ok(content),
                Err(e) => Err(fdo::Error::Failed(format!("Failed to read file: {}", e))),
            }
        } else {
            // Line-based reading
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
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
                Err(e) => Err(fdo::Error::Failed(format!("Failed to read file: {}", e))),
            }
        }
    }

    /// Write a file to the host system.
    async fn write_file(&self, path: String, content: String) -> fdo::Result<()> {
        let blacklisted = load_blacklist();

        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(fdo::Error::Failed(
                    "Path is blacklisted for security reasons".to_string(),
                ));
            }
        }

        match tokio::fs::write(&path, content).await {
            Ok(_) => Ok(()),
            Err(e) => Err(fdo::Error::Failed(format!("Failed to write file: {}", e))),
        }
    }

    /// List contents of a directory. Returns (name, is_dir, size).
    async fn list_directory(&self, path: String) -> fdo::Result<Vec<(String, bool, u64)>> {
        let blacklisted = load_blacklist();
        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(fdo::Error::Failed(
                    "Path is blacklisted for security reasons".to_string(),
                ));
            }
        }

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to read directory: {}", e)))?;

        while let Ok(Some(entry)) = dir.next_entry().await {
            let metadata = entry.metadata().await.ok();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            entries.push((name, is_dir, size));
        }

        Ok(entries)
    }

    /// Delete a file from the host system.
    async fn delete_file(&self, path: String) -> fdo::Result<()> {
        let blacklisted = load_blacklist();
        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(fdo::Error::Failed(
                    "Path is blacklisted for security reasons".to_string(),
                ));
            }
        }

        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to delete file: {}", e)))
    }

    /// Get system information summary.
    async fn get_system_info(&self) -> fdo::Result<String> {
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

    /// List running processes. Returns (pid, name, cpu_usage, memory_bytes, read_bytes, written_bytes).
    async fn list_processes(&self) -> fdo::Result<Vec<(u32, String, f64, u64, u64, u64)>> {
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

    /// Kill a process by PID.
    async fn kill_process(&self, pid: u32, signal: i32) -> fdo::Result<()> {
        unsafe {
            if libc::kill(pid as i32, signal) != 0 {
                return Err(fdo::Error::Failed(format!(
                    "Failed to kill process {}: kill failed",
                    pid
                )));
            }
        }
        Ok(())
    }

    /// Get text content from the host clipboard.
    async fn get_clipboard(&self) -> fdo::Result<String> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|e| fdo::Error::Failed(format!("Failed to open clipboard: {}", e)))?;
        clipboard
            .get_text()
            .map_err(|e| fdo::Error::Failed(format!("Failed to read clipboard: {}", e)))
    }

    /// Set text content to the host clipboard.
    async fn set_clipboard(&self, text: String) -> fdo::Result<()> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|e| fdo::Error::Failed(format!("Failed to open clipboard: {}", e)))?;
        clipboard
            .set_text(text)
            .map_err(|e| fdo::Error::Failed(format!("Failed to write to clipboard: {}", e)))
    }
}

/// Load blacklist from config file or return defaults.
fn load_blacklist() -> Vec<String> {
    let mut blacklisted = vec![
        "/etc/shadow".to_string(),
        "/etc/gshadow".to_string(),
        "/root/.ssh".to_string(),
        ".ssh/id_rsa".to_string(),
        ".ssh/id_ed25519".to_string(),
    ];

    if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        let blacklist_path = dirs.config_dir().join("boxxyclaw").join("BLACKLIST.md");
        if let Ok(content) = std::fs::read_to_string(&blacklist_path) {
            // Read from file, replacing defaults
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
    }
    blacklisted
}
