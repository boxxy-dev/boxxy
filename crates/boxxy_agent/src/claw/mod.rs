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

const EXEC_SHELL_TIMEOUT_SECS: u64 = 30;
const EXEC_SHELL_MAX_OUTPUT_BYTES: usize = 32 * 1024;
const READ_FILE_MAX_RAW_BYTES: u64 = 5 * 1024 * 1024;
const READ_FILE_MAX_SLICE_BYTES: usize = 64 * 1024;

#[async_trait::async_trait]
impl ClawEnvironment for ClawSubsystem {
    async fn exec_shell(&self, command: String) -> anyhow::Result<(i32, String, String)> {
        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        match tokio::time::timeout(
            std::time::Duration::from_secs(EXEC_SHELL_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => {
                let stdout = truncate_head_tail(
                    &String::from_utf8_lossy(&output.stdout),
                    EXEC_SHELL_MAX_OUTPUT_BYTES,
                );
                let stderr = truncate_head_tail(
                    &String::from_utf8_lossy(&output.stderr),
                    EXEC_SHELL_MAX_OUTPUT_BYTES,
                );
                let exit_code = output.status.code().unwrap_or(-1);
                Ok((exit_code, stdout, stderr))
            }
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Ok((
                124,
                String::new(),
                format!(
                    "[ERROR: Command timed out after {EXEC_SHELL_TIMEOUT_SECS} seconds and was terminated. Do not run interactive or blocking commands via sys_shell_exec.]"
                ),
            )),
        }
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
        ensure_path_allowed(&path)?;

        let metadata = tokio::fs::metadata(&path).await?;
        if metadata.len() > READ_FILE_MAX_RAW_BYTES {
            return Err(anyhow::anyhow!(
                "File is too large ({} bytes > 5 MB limit) to read into memory",
                metadata.len()
            ));
        }

        let bytes = tokio::fs::read(&path).await?;
        let probe_len = bytes.len().min(8192);
        if bytes[..probe_len].contains(&0) {
            return Err(anyhow::anyhow!(
                "File appears to be binary and cannot be read as text: {path}"
            ));
        }

        let content = String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());

        if start_line == 0 && end_line == 0 {
            Ok(content)
        } else {
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len() as u32;

            if total_lines == 0 {
                return Ok(String::new());
            }

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

            if result.len() > READ_FILE_MAX_SLICE_BYTES {
                let cut = result.floor_char_boundary(READ_FILE_MAX_SLICE_BYTES);
                result.truncate(cut);
                result.push_str(
                    "\n\n[NOTE: Output capped at 64 KB. Specify a narrower start_line/end_line range to read more.]",
                );
            }

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
        ensure_path_allowed(&path)?;

        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn list_directory(&self, path: String) -> anyhow::Result<Vec<(String, bool, u64)>> {
        ensure_path_allowed(&path)?;

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&path).await?;

        while let Some(entry) = dir.next_entry().await? {
            let metadata = entry.metadata().await.ok();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            entries.push((name, is_dir, size));
        }

        // Deterministic sort: directories first, then case-insensitive alphabetical
        entries.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });

        Ok(entries)
    }

    async fn delete_file(&self, path: String) -> anyhow::Result<()> {
        ensure_path_allowed(&path)?;

        tokio::fs::remove_file(&path).await?;
        Ok(())
    }

    async fn get_system_info(&self) -> anyhow::Result<String> {
        tokio::task::spawn_blocking(|| {
            use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};
            let mut sys = System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::everything())
                    .with_memory(MemoryRefreshKind::everything()),
            );
            sys.refresh_cpu_all();
            sys.refresh_memory();

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
        })
        .await?
    }

    async fn list_processes(&self) -> anyhow::Result<Vec<(u32, String, f64, u64, u64, u64)>> {
        tokio::task::spawn_blocking(|| {
            use sysinfo::{ProcessRefreshKind, RefreshKind, System};
            let sys = System::new_with_specifics(
                RefreshKind::nothing().with_processes(
                    ProcessRefreshKind::nothing()
                        .with_cpu()
                        .with_memory()
                        .with_disk_usage(),
                ),
            );

            let mut processes = Vec::with_capacity(sys.processes().len());
            for (pid, process) in sys.processes() {
                let disk_usage = process.disk_usage();
                processes.push((
                    pid.as_u32(),
                    process.name().to_string_lossy().to_string(),
                    f64::from(process.cpu_usage()),
                    process.memory(),
                    disk_usage.total_read_bytes,
                    disk_usage.total_written_bytes,
                ));
            }

            Ok(processes)
        })
        .await?
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

    async fn list_detached_shells(
        &self,
    ) -> anyhow::Result<Vec<(u32, String, u64, String, String, bool)>> {
        let list = crate::pty::query_detached_sessions_detailed(&self.state.pty_registry).await;
        Ok(list
            .into_iter()
            .map(|s| {
                (
                    s.pid,
                    s.pane_id,
                    s.idle_secs,
                    s.cwd,
                    s.foreground_process,
                    s.waiting_for_input,
                )
            })
            .collect())
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

fn truncate_head_tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let head_budget = max_bytes / 4;
    let tail_budget = max_bytes.saturating_sub(head_budget);
    let head_end = s.floor_char_boundary(head_budget);
    let tail_start = s.ceil_char_boundary(s.len().saturating_sub(tail_budget));
    let omitted = tail_start.saturating_sub(head_end);
    format!(
        "{}\n\n...[truncated {omitted} bytes of output]...\n\n{}",
        &s[..head_end],
        &s[tail_start..]
    )
}

fn normalize_lexical_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            c => normalized.push(c.as_os_str()),
        }
    }
    normalized
}

fn ensure_path_allowed(path: &str) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    let lexical = normalize_lexical_path(p).to_string_lossy().to_string();
    let canonical = std::fs::canonicalize(p)
        .ok()
        .or_else(|| {
            let parent = p.parent()?;
            let file_name = p.file_name()?;
            Some(std::fs::canonicalize(parent).ok()?.join(file_name))
        })
        .map(|cp| cp.to_string_lossy().to_string());

    let blacklisted = load_blacklist();
    for black_path in &blacklisted {
        if path.contains(black_path)
            || lexical.contains(black_path)
            || canonical.as_ref().is_some_and(|c| c.contains(black_path))
        {
            return Err(anyhow::anyhow!("Path is blacklisted for security reasons"));
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_head_tail_preserves_both_ends_and_utf8_boundaries() {
        let input = format!("START_MARKER_{}_END_MARKER", "x".repeat(1000));
        let truncated = truncate_head_tail(&input, 200);
        assert!(truncated.starts_with("START_MARKER_"));
        assert!(truncated.ends_with("_END_MARKER"));
        assert!(truncated.contains("...[truncated "));
    }

    #[test]
    fn ensure_path_allowed_blocks_blacklisted_and_dotdot_traversal() {
        assert!(ensure_path_allowed("/etc/shadow").is_err());
        assert!(ensure_path_allowed("/tmp/../etc/shadow").is_err());
        assert!(ensure_path_allowed("/home/someone/.ssh/id_ed25519").is_err());

        let tmp = tempfile::tempdir().unwrap();
        let safe_file = tmp.path().join("notes.txt");
        assert!(ensure_path_allowed(&safe_file.to_string_lossy()).is_ok());
    }
}
