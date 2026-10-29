use crate::core::state::AgentState;
use boxxy_claw_protocol::{ClawEnvironment, ProcessSample};
use std::process::Stdio;
use tokio::process::Command;
use zbus::fdo;
use zbus::interface;

pub mod notifier;
pub mod registry;
pub mod sysinfo_sampler;

pub use registry::CharacterRegistry;
pub use sysinfo_sampler::SystemSampler;

pub struct ClawSubsystem {
    state: AgentState,
    sampler: SystemSampler,
}

impl ClawSubsystem {
    pub fn new(state: AgentState) -> Self {
        Self {
            state,
            sampler: SystemSampler::new(),
        }
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
        use tokio::io::AsyncReadExt;

        if !std::path::Path::new(&cwd).exists() {
            return Err(anyhow::anyhow!("Working directory does not exist: {}", cwd));
        }

        let mut child = unsafe {
            Command::new("bash")
                .arg("-c")
                .arg(&command)
                .current_dir(&cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .pre_exec(|| {
                    libc::setsid();
                    Ok(())
                })
                .spawn()?
        };

        let pid = child.id().unwrap_or(0);
        let mut stderr = child.stderr.take();

        tokio::select! {
            result = child.wait() => {
                let status = result?;
                if !status.success() {
                    let mut err_bytes = Vec::new();
                    if let Some(ref mut err) = stderr {
                        let _ = err.take(8192).read_to_end(&mut err_bytes).await;
                    }
                    let stderr_str = String::from_utf8_lossy(&err_bytes).to_string();
                    return Err(anyhow::anyhow!("Process exited immediately: {}", stderr_str));
                }
                // Exited with code 0 immediately — launcher script pattern, still valid
                Ok(pid)
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                // Still running after 250ms. Drain stderr to `sink()` in the background
                // instead of dropping the pipe read-end (which would send SIGPIPE/EPIPE
                // to the detached process on its next stderr write), then reap on exit.
                tokio::spawn(async move {
                    if let Some(mut err) = stderr {
                        let _ = tokio::io::copy(&mut err, &mut tokio::io::sink()).await;
                    }
                    let _ = child.wait().await;
                });
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

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let suggestions = suggest_similar_files(&path).await;
                if suggestions.is_empty() {
                    return Err(anyhow::anyhow!("File not found: {path}"));
                }
                return Err(anyhow::anyhow!(
                    "File not found: {path}\n\nDid you mean one of these?\n{}",
                    suggestions.join("\n")
                ));
            }
            Err(e) => return Err(e.into()),
        };
        if metadata.is_dir() {
            return Err(anyhow::anyhow!(
                "Path is a directory, not a file: {path} (use `list_directory` instead)"
            ));
        }
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
                return Ok(format!("[Read file {path}, 0 lines]\n"));
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

            if end < total_lines {
                result.push_str(&format!(
                    "\n[Output truncated. Continue reading with start_line: {}]",
                    end + 1
                ));
            }

            result = format!(
                "[Showing lines {}-{} of {}]\n{}",
                start + 1,
                end,
                total_lines,
                result
            );

            Ok(result)
        }
    }

    async fn write_file(&self, path: String, content: String) -> anyhow::Result<()> {
        ensure_path_allowed(&path)?;

        if let Some(parent) = std::path::Path::new(&path).parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }

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
        let sampler = self.sampler.clone();
        tokio::task::spawn_blocking(move || sampler.collect_system_info_json()).await?
    }

    async fn list_processes(&self) -> anyhow::Result<Vec<ProcessSample>> {
        let sampler = self.sampler.clone();
        tokio::task::spawn_blocking(move || sampler.collect_processes()).await?
    }

    async fn kill_process(&self, pid: u32, signal: i32) -> anyhow::Result<()> {
        let raw_pid = i32::try_from(pid).map_err(|_| {
            anyhow::anyhow!("Invalid PID {pid}: exceeds maximum positive pid_t")
        })?;
        // Guard against signaling process groups (pid <= 0), init (pid == 1),
        // kill(-1) (which signals every user process on the system), or the daemon itself.
        if raw_pid <= 1 || pid == std::process::id() {
            return Err(anyhow::anyhow!(
                "Refusing to signal protected or invalid PID {pid}"
            ));
        }
        if signal <= 0 || signal > 64 {
            return Err(anyhow::anyhow!("Invalid signal number: {signal}"));
        }

        unsafe {
            if libc::kill(raw_pid, signal) != 0 {
                let err = std::io::Error::last_os_error();
                return Err(anyhow::anyhow!(
                    "Failed to kill process {pid}: {err}"
                ));
            }
        }
        Ok(())
    }

    async fn get_clipboard(&self) -> anyhow::Result<String> {
        tokio::task::spawn_blocking(|| {
            let mut clipboard = arboard::Clipboard::new()?;
            let text = clipboard.get_text()?;
            Ok(text)
        })
        .await?
    }

    async fn set_clipboard(&self, text: String) -> anyhow::Result<()> {
        tokio::task::spawn_blocking(move || {
            let mut clipboard = arboard::Clipboard::new()?;
            clipboard.set_text(text)?;
            Ok(())
        })
        .await?
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

    async fn list_processes(&self) -> fdo::Result<String> {
        let processes = ClawEnvironment::list_processes(self)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        serde_json::to_string(&processes).map_err(|e| fdo::Error::Failed(e.to_string()))
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

async fn suggest_similar_files(missing_path: &str) -> Vec<String> {
    let p = std::path::Path::new(missing_path);
    let Some(parent) = p.parent() else {
        return Vec::new();
    };
    let Some(base) = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .filter(|s| !s.is_empty())
    else {
        return Vec::new();
    };

    let Ok(mut dir) = tokio::fs::read_dir(parent).await else {
        return Vec::new();
    };

    let mut suggestions = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await {
        let candidate_os = entry.file_name();
        let candidate_name = candidate_os.to_string_lossy();
        let candidate_lower = candidate_name.to_lowercase();
        if candidate_lower.contains(&base) || base.contains(&candidate_lower) {
            suggestions.push(parent.join(&candidate_os).to_string_lossy().into_owned());
        }
    }
    suggestions.sort();
    suggestions.truncate(3);
    suggestions
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

fn resolve_deepest_canonical(path: &std::path::Path) -> Option<std::path::PathBuf> {
    if let Ok(c) = std::fs::canonicalize(path) {
        return Some(c);
    }
    use std::path::Component;
    let mut acc = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                acc.pop();
                if !acc.as_os_str().is_empty()
                    && let Ok(canon) = std::fs::canonicalize(&acc)
                {
                    acc = canon;
                }
            }
            c => {
                acc.push(c.as_os_str());
                if let Ok(canon) = std::fs::canonicalize(&acc) {
                    acc = canon;
                }
            }
        }
    }
    (!acc.as_os_str().is_empty()).then_some(acc)
}

fn ensure_path_allowed(path: &str) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    let lexical = normalize_lexical_path(p).to_string_lossy().to_string();
    let canonical = resolve_deepest_canonical(p).map(|cp| cp.to_string_lossy().to_string());

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

        // Symlink ancestor pointing to /etc with multiple non-existent nested segments
        let link_path = tmp.path().join("etc_link");
        std::os::unix::fs::symlink("/etc", &link_path).unwrap();
        let nested_via_symlink = link_path.join("nonexistent_sub").join("..").join("shadow");
        assert!(
            ensure_path_allowed(&nested_via_symlink.to_string_lossy()).is_err(),
            "symlink ancestor with non-existent intermediate segments must still resolve and block /etc/shadow"
        );
    }

    #[tokio::test]
    async fn kill_process_rejects_dangerous_pids_and_signals() {
        let sub = ClawSubsystem::new(AgentState::new());
        // pid 0 (process group), pid 1 (init), u32::MAX (-1 = all user processes), and self PID
        assert!(ClawEnvironment::kill_process(&sub, 0, libc::SIGTERM).await.is_err());
        assert!(ClawEnvironment::kill_process(&sub, 1, libc::SIGTERM).await.is_err());
        assert!(ClawEnvironment::kill_process(&sub, u32::MAX, libc::SIGTERM).await.is_err());
        assert!(
            ClawEnvironment::kill_process(&sub, std::process::id(), libc::SIGTERM)
                .await
                .is_err()
        );
        assert!(ClawEnvironment::kill_process(&sub, 99999, 0).await.is_err());
        assert!(ClawEnvironment::kill_process(&sub, 99999, -9).await.is_err());
    }

    #[tokio::test]
    async fn spawn_detached_survives_stderr_writes_after_window() {
        let sub = ClawSubsystem::new(AgentState::new());
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("survived.txt");

        // Write to stderr AFTER the 250ms window (at 350ms), then create marker file.
        // Without background stderr draining, the write at 350ms receives EPIPE/SIGPIPE.
        let cmd = format!(
            "set -e; sleep 0.35; echo 'post-window stderr log' >&2; printf 'ok' > '{}'",
            marker.display()
        );
        let pid = ClawEnvironment::spawn_detached(
            &sub,
            cmd,
            tmp.path().to_string_lossy().into_owned(),
        )
        .await
        .expect("spawn_detached should succeed");
        assert!(pid > 0);

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let written = tokio::fs::read_to_string(&marker).await.unwrap_or_default();
        assert_eq!(written, "ok");
    }

    #[tokio::test]
    async fn read_file_includes_header_and_suggests_similar_on_not_found_and_write_creates_parents() {
        let sub = ClawSubsystem::new(AgentState::new());
        let tmp = tempfile::tempdir().unwrap();
        let nested_file = tmp.path().join("deep/nested/config_loader.rs");

        // 1. write_file auto-creates missing parent directories
        ClawEnvironment::write_file(
            &sub,
            nested_file.to_string_lossy().into_owned(),
            "line1\nline2\nline3\n".to_string(),
        )
        .await
        .expect("write_file should auto-create parent dirs");

        // 2. read_file always includes [Showing lines X-Y of Z] header
        let read_all = ClawEnvironment::read_file(
            &sub,
            nested_file.to_string_lossy().into_owned(),
            1,
            500,
        )
        .await
        .unwrap();
        assert!(read_all.starts_with("[Showing lines 1-3 of 3]\nline1\nline2\nline3"));

        // 3. Truncated read includes continuation hint
        let read_page = ClawEnvironment::read_file(
            &sub,
            nested_file.to_string_lossy().into_owned(),
            1,
            2,
        )
        .await
        .unwrap();
        assert!(read_page.contains("[Showing lines 1-2 of 3]"));
        assert!(read_page.contains("[Output truncated. Continue reading with start_line: 3]"));

        // 4. Missing file suggests similar sibling filename
        let missing = tmp.path().join("deep/nested/config_loader");
        let err = ClawEnvironment::read_file(&sub, missing.to_string_lossy().into_owned(), 1, 500)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Did you mean one of these?"));
        assert!(err.contains("config_loader.rs"));
    }
}
