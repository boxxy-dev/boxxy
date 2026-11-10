use crate::core::state::AgentState;
use boxxy_claw_protocol::{ClawEnvironment, ProcessSample};
use std::process::Stdio;
use tokio::process::Command;
use zbus::fdo;
use zbus::interface;

mod fs_ops;
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

const EXEC_SHELL_TIMEOUT_SECS: u64 = 180;

/// Ensures an entire spawned process group (`-pgid`) is terminated with `SIGKILL`
/// if `exec_shell` times out or its future is dropped (e.g. user presses `Esc` twice),
/// preventing orphaned `cargo` / `rustc` / `pytest` child processes.
struct ProcessGroupKillGuard(Option<i32>);

impl ProcessGroupKillGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for ProcessGroupKillGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.0.take()
            && pgid > 1
        {
            unsafe {
                let _ = libc::kill(-pgid, libc::SIGKILL);
            }
        }
    }
}

#[async_trait::async_trait]
impl ClawEnvironment for ClawSubsystem {
    async fn exec_shell(&self, command: String) -> anyhow::Result<(i32, String, String)> {
        let child = unsafe {
            Command::new("bash")
                .arg("-c")
                .arg(command)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .kill_on_drop(true)
                .pre_exec(|| {
                    let _ = libc::syscall(
                        libc::SYS_close_range,
                        3 as libc::c_uint,
                        libc::c_uint::MAX,
                        libc::CLOSE_RANGE_CLOEXEC as libc::c_uint,
                    );
                    Ok(())
                })
                .spawn()?
        };

        let mut pg_guard =
            ProcessGroupKillGuard(child.id().and_then(|pid| i32::try_from(pid).ok()));

        match tokio::time::timeout(
            std::time::Duration::from_secs(EXEC_SHELL_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => {
                pg_guard.disarm();
                let stdout = fs_ops::truncate_head_tail(
                    &String::from_utf8_lossy(&output.stdout),
                    fs_ops::EXEC_SHELL_MAX_OUTPUT_BYTES,
                );
                let stderr = fs_ops::truncate_head_tail(
                    &String::from_utf8_lossy(&output.stderr),
                    fs_ops::EXEC_SHELL_MAX_OUTPUT_BYTES,
                );
                let exit_code = {
                    use std::os::unix::process::ExitStatusExt;
                    output
                        .status
                        .code()
                        .or_else(|| output.status.signal().map(|sig| 128 + sig))
                        .unwrap_or(-1)
                };
                Ok((exit_code, stdout, stderr))
            }
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Ok((
                124,
                String::new(),
                format!(
                    "[ERROR: Command timed out after {EXEC_SHELL_TIMEOUT_SECS} seconds and its process group was terminated. Pass specific test filters or use run_background_command for long-running servers.]"
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
                    let _ = libc::syscall(
                        libc::SYS_close_range,
                        3 as libc::c_uint,
                        libc::c_uint::MAX,
                        libc::CLOSE_RANGE_CLOEXEC as libc::c_uint,
                    );
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
        fs_ops::read_file(path, start_line, end_line).await
    }

    async fn write_file(&self, path: String, content: String) -> anyhow::Result<()> {
        fs_ops::write_file(path, content).await
    }

    async fn list_directory(&self, path: String) -> anyhow::Result<Vec<(String, bool, u64)>> {
        fs_ops::list_directory(path).await
    }

    async fn delete_file(&self, path: String) -> anyhow::Result<()> {
        fs_ops::delete_file(path).await
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
        let raw_pid = i32::try_from(pid)
            .map_err(|_| anyhow::anyhow!("Invalid PID {pid}: exceeds maximum positive pid_t"))?;
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
                return Err(anyhow::anyhow!("Failed to kill process {pid}: {err}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn kill_process_rejects_dangerous_pids_and_signals() {
        let sub = ClawSubsystem::new(AgentState::new());
        // pid 0 (process group), pid 1 (init), u32::MAX (-1 = all user processes), and self PID
        assert!(
            ClawEnvironment::kill_process(&sub, 0, libc::SIGTERM)
                .await
                .is_err()
        );
        assert!(
            ClawEnvironment::kill_process(&sub, 1, libc::SIGTERM)
                .await
                .is_err()
        );
        assert!(
            ClawEnvironment::kill_process(&sub, u32::MAX, libc::SIGTERM)
                .await
                .is_err()
        );
        assert!(
            ClawEnvironment::kill_process(&sub, std::process::id(), libc::SIGTERM)
                .await
                .is_err()
        );
        assert!(ClawEnvironment::kill_process(&sub, 99999, 0).await.is_err());
        assert!(
            ClawEnvironment::kill_process(&sub, 99999, -9)
                .await
                .is_err()
        );
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
}
