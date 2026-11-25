//! Headless POSIX PTY driver exercising `PtyRegistry` detach/reattach ring buffering
//! and Boxxy's zero-config Bash `OSC 7` / `OSC 133` shell integration.

use anyhow::{Context, Result, anyhow};
use boxxy_agent::pty::registry::{DetachOutcome, PtyRegistry};
use boxxy_agent::pty::shell_integration::{apply_shell_integration, ensure_integration_scripts_in};
use nix::fcntl::OFlag;
use nix::pty::{grantpt, posix_openpt, ptsname, unlockpt};
use parking_lot::Mutex;
use std::ffi::CString;
use std::os::fd::{AsRawFd as _, FromRawFd as _, IntoRawFd as _, OwnedFd};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::process::{Child, Command};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Live headless PTY session backed by a real Linux pseudoterminal and `/bin/bash` child.
pub struct PtyShellSession {
    pid: u32,
    pane_id: String,
    owner_bus_name: String,
    registry: PtyRegistry,
    writer_fd: OwnedFd,
    captured: Arc<Mutex<Vec<u8>>>,
    output_notify: Arc<Notify>,
    ui_reader_task: Option<JoinHandle<()>>,
    child: Child,
}

impl PtyShellSession {
    /// Spawns an isolated interactive `/bin/bash` child attached to a fresh PTY in `cwd`,
    /// wired with Boxxy's `OSC 7` / `OSC 133` shell integration scripts and registered in `registry`.
    ///
    /// Uses a synthetic `HOME` inside `sandbox_root` so host user dotfiles (`~/.bashrc`)
    /// are never sourced during automated tests.
    ///
    /// # Errors
    /// Returns an error if opening the PTY or spawning `/bin/bash` fails.
    pub async fn spawn_bash(
        registry: PtyRegistry,
        pane_id: impl Into<String>,
        owner_bus_name: impl Into<String>,
        cwd: &Path,
        sandbox_root: &Path,
    ) -> Result<Self> {
        let pane_id = pane_id.into();
        let owner_bus_name = owner_bus_name.into();

        let synthetic_home = sandbox_root.join("synthetic_home");
        tokio::fs::create_dir_all(&synthetic_home)
            .await
            .context("create synthetic HOME")?;
        let integration_dir = sandbox_root.join("shell-integration");
        ensure_integration_scripts_in(&integration_dir)
            .context("materialize shell integration scripts")?;

        let (master_owned, slave_stdin, slave_stdout, slave_stderr) = open_pty_pair(80, 24)?;
        let daemon_master_dup = master_owned.try_clone().context("dup daemon master FD")?;
        let reader_master_dup = master_owned.try_clone().context("dup UI reader FD")?;

        let mut argv = vec!["/bin/bash".to_string()];
        let mut env_vars = vec![
            ("HOME".to_string(), synthetic_home.to_string_lossy().into_owned()),
            ("TERM".to_string(), "xterm-256color".to_string()),
            (
                "PATH".to_string(),
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
            ),
            ("PS1".to_string(), "boxxy-e2e$ ".to_string()),
        ];
        apply_shell_integration(&mut argv, &mut env_vars, &integration_dir);

        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(cwd)
            .env_clear()
            .envs(env_vars)
            .stdin(Stdio::from(slave_stdin))
            .stdout(Stdio::from(slave_stdout))
            .stderr(Stdio::from(slave_stderr));

        let raw_tty_fd = cmd.as_std().get_envs().count(); // no-op touch before pre_exec
        let _ = raw_tty_fd;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let _ = libc::ioctl(0, libc::TIOCSCTTY, 0);
                Ok(())
            });
        }

        let child = cmd.spawn().context("spawn /bin/bash on PTY")?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow!("spawned /bin/bash exited before reporting PID"))?;

        registry
            .register_with_owner(
                pid,
                pane_id.clone(),
                owner_bus_name.clone(),
                cwd.to_string_lossy().into_owned(),
                false,
                Some(daemon_master_dup),
            )
            .await;

        let captured = Arc::new(Mutex::new(Vec::with_capacity(4096)));
        let output_notify = Arc::new(Notify::new());
        let ui_reader_task = Some(spawn_ui_reader(
            reader_master_dup,
            Arc::clone(&captured),
            Arc::clone(&output_notify),
        )?);

        Ok(Self {
            pid,
            pane_id,
            owner_bus_name,
            registry,
            writer_fd: master_owned,
            captured,
            output_notify,
            ui_reader_task,
            child,
        })
    }

    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    #[must_use]
    pub fn pane_id(&self) -> &str {
        &self.pane_id
    }

    #[must_use]
    pub fn owner_bus_name(&self) -> &str {
        &self.owner_bus_name
    }

    /// Writes raw bytes (commands or keystrokes) into the PTY master.
    ///
    /// # Errors
    /// Returns an error if writing to the PTY master descriptor fails.
    pub fn send_bytes(&self, input: &[u8]) -> Result<()> {
        let mut offset = 0;
        while offset < input.len() {
            let n = unsafe {
                libc::write(
                    self.writer_fd.as_raw_fd(),
                    input[offset..].as_ptr().cast(),
                    input.len() - offset,
                )
            };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err.into());
            }
            offset += usize::try_from(n).unwrap_or(0);
        }
        Ok(())
    }

    /// Sends a command line terminated with `\n`.
    ///
    /// # Errors
    /// Returns an error if writing to the PTY master fails.
    pub fn send_line(&self, line: &str) -> Result<()> {
        let mut payload = Vec::with_capacity(line.len() + 1);
        payload.extend_from_slice(line.as_bytes());
        payload.push(b'\n');
        self.send_bytes(&payload)
    }

    /// Returns a UTF-8 lossy snapshot of all bytes captured by the attached UI reader so far.
    #[must_use]
    pub fn output_lossy(&self) -> String {
        let guard = self.captured.lock();
        String::from_utf8_lossy(&guard).into_owned()
    }

    /// Waits without polling until the attached UI reader buffer contains `needle`.
    ///
    /// # Errors
    /// Returns a timeout error if `needle` does not appear within `timeout`.
    pub async fn wait_for_output(&self, needle: &str, timeout: Duration) -> Result<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.output_notify.notified();
            {
                let snap = self.output_lossy();
                if snap.contains(needle) {
                    return Ok(snap);
                }
            }
            tokio::time::timeout_at(deadline, notified)
                .await
                .map_err(|_| {
                    anyhow!(
                        "Timed out after {timeout:?} waiting for PTY output containing {needle:?}.\nCaptured so far:\n{}",
                        self.output_lossy()
                    )
                })?;
        }
    }

    /// Waits without polling until at least `expected_completions` `OSC 133;D;<exit_code>` markers
    /// have arrived on the PTY stream, returning `(semantic_snapshot, latest_exit_code)`.
    ///
    /// # Errors
    /// Returns a timeout error if `expected_completions` is not reached within `timeout`.
    pub async fn wait_for_osc133_done(
        &self,
        expected_completions: usize,
        timeout: Duration,
    ) -> Result<(String, i32)> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.output_notify.notified();
            {
                let (semantic, exits) = self.semantic_snapshot_and_exits();
                if exits.len() >= expected_completions
                    && let Some(&last_exit) = exits.last()
                {
                    return Ok((semantic, last_exit));
                }
            }
            tokio::time::timeout_at(deadline, notified)
                .await
                .map_err(|_| {
                    anyhow!(
                        "Timed out after {timeout:?} waiting for {expected_completions} OSC 133;D completions.\nCaptured so far:\n{:?}",
                        self.output_lossy()
                    )
                })?;
        }
    }

    /// Parses the captured PTY byte stream into `boxxy-vte`'s structured `OSC 133` semantic
    /// markdown format (`[PROMPT]`, `[COMMAND]`, `[OUTPUT]`) and returns all observed `OSC 133;D`
    /// exit codes in chronological order.
    #[must_use]
    pub fn semantic_snapshot_and_exits(&self) -> (String, Vec<i32>) {
        parse_osc133_semantic_stream(&self.output_lossy())
    }

    /// Enables or disables background persistence on `PtyRegistry` for this PTY.
    pub async fn set_persistence(&self, enabled: bool) {
        self.registry.set_persistence(self.pid, enabled).await;
    }

    /// Simulates the UI detaching (or crashing via `release_owner`), stopping the UI reader first
    /// so subsequent PTY output flows exclusively into `PtyRegistry`'s detached 4 MB ring buffer.
    pub async fn simulate_ui_disconnect(&mut self) -> Vec<(u32, DetachOutcome)> {
        if let Some(handle) = self.ui_reader_task.take() {
            handle.abort();
            let _ = handle.await;
        }
        self.registry.release_owner(&self.owner_bus_name).await
    }

    /// Reattaches to a detached `PtyRegistry` session, returning the buffered ring-buffer output
    /// and resuming live UI capture from the fresh master FD returned by the registry.
    ///
    /// # Errors
    /// Returns an error if the session was not detached or re-registering the reader fails.
    pub async fn reattach(&mut self) -> Result<String> {
        let (ring_bytes, fresh_fd) = self
            .registry
            .reattach(self.pid)
            .await
            .ok_or_else(|| anyhow!("PtyRegistry::reattach({}) returned None", self.pid))?;

        {
            let mut guard = self.captured.lock();
            guard.extend_from_slice(&ring_bytes);
        }
        self.output_notify.notify_waiters();
        self.ui_reader_task = Some(spawn_ui_reader(
            fresh_fd,
            Arc::clone(&self.captured),
            Arc::clone(&self.output_notify),
        )?);

        Ok(String::from_utf8_lossy(&ring_bytes).into_owned())
    }
}

impl Drop for PtyShellSession {
    fn drop(&mut self) {
        if let Some(handle) = self.ui_reader_task.take() {
            handle.abort();
        }
        if let Ok(pgid) = i32::try_from(self.pid)
            && pgid > 1
        {
            unsafe {
                let _ = libc::kill(-pgid, libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}

fn open_pty_pair(cols: u16, rows: u16) -> Result<(OwnedFd, OwnedFd, OwnedFd, OwnedFd)> {
    let master =
        posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_CLOEXEC).context("posix_openpt")?;
    grantpt(&master).context("grantpt")?;
    unlockpt(&master).context("unlockpt")?;
    // SAFETY: `master` is a valid unlocked PTY master descriptor on this thread.
    let slave_name = unsafe { ptsname(&master) }.context("ptsname")?;
    let slave_cstr = CString::new(slave_name).context("slave path CString")?;

    let raw_slave = unsafe {
        libc::open(
            slave_cstr.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if raw_slave < 0 {
        return Err(anyhow!(
            "open slave PTY failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `raw_slave` was just opened and is exclusively owned here.
    let slave_stdin = unsafe { OwnedFd::from_raw_fd(raw_slave) };
    let slave_stdout = slave_stdin.try_clone().context("dup slave stdout")?;
    let slave_stderr = slave_stdin.try_clone().context("dup slave stderr")?;

    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        let _ = libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws);
    }

    // SAFETY: Converting `PtyMaster` into `OwnedFd` transfers ownership once.
    let master_owned = unsafe { OwnedFd::from_raw_fd(master.into_raw_fd()) };
    Ok((master_owned, slave_stdin, slave_stdout, slave_stderr))
}

fn spawn_ui_reader(
    fd: OwnedFd,
    captured: Arc<Mutex<Vec<u8>>>,
    notify: Arc<Notify>,
) -> Result<JoinHandle<()>> {
    unsafe {
        let raw = fd.as_raw_fd();
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags >= 0 {
            let _ = libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    // SAFETY: `fd` is an exclusively-owned `OwnedFd` kept valid for the lifetime of `async_fd`.
    let async_fd = unsafe { AsyncFd::register_with_interest(fd, Interest::READABLE) }
        .context("register PTY reader AsyncFd")?;

    Ok(tokio::spawn(async move {
        let mut buf = [0_u8; 4096];
        loop {
            let Ok(mut guard) = async_fd.readable().await else {
                break;
            };
            let result = guard.try_io(|inner| {
                let n = unsafe {
                    libc::read(
                        inner.get_ref().as_raw_fd(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                    )
                };
                if n < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(usize::try_from(n).unwrap_or(0))
                }
            });

            match result {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    captured.lock().extend_from_slice(&buf[..n]);
                    notify.notify_waiters();
                }
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Ok(Err(_)) => break,
                Err(_would_block) => {}
            }
        }
    }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SemanticZone {
    None,
    Prompt,
    Command,
    Output,
}

fn close_zone(out: &mut String, zone: &mut SemanticZone) {
    if *zone != SemanticZone::None {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");
        *zone = SemanticZone::None;
    }
}

fn open_zone(out: &mut String, zone: &mut SemanticZone, next: SemanticZone) {
    if *zone == next {
        return;
    }
    close_zone(out, zone);
    match next {
        SemanticZone::None => {}
        SemanticZone::Prompt => out.push_str("\n[PROMPT]\n```text\n"),
        SemanticZone::Command => out.push_str("\n[COMMAND]\n```bash\n"),
        SemanticZone::Output => out.push_str("\n[OUTPUT]\n```text\n"),
    }
    *zone = next;
}

fn parse_osc133_semantic_stream(raw: &str) -> (String, Vec<i32>) {
    let bytes = raw.as_bytes();
    let mut out = String::new();
    let mut exits = Vec::new();
    let mut zone = SemanticZone::None;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == 0x1b {
            if bytes.get(i + 1) == Some(&b']') {
                // OSC sequence: `\x1b] ... (\x07 | \x1b\\)`
                let payload_start = i + 2;
                let mut end = payload_start;
                let mut next_i = bytes.len();
                while end < bytes.len() {
                    if bytes[end] == 0x07 {
                        next_i = end + 1;
                        break;
                    }
                    if bytes[end] == 0x1b && bytes.get(end + 1) == Some(&b'\\') {
                        next_i = end + 2;
                        break;
                    }
                    end += 1;
                }
                let osc = &raw[payload_start..end];
                if let Some(rest) = osc.strip_prefix("133;") {
                    let kind = rest.split(';').next().unwrap_or("");
                    match kind {
                        "A" => open_zone(&mut out, &mut zone, SemanticZone::Prompt),
                        "B" => open_zone(&mut out, &mut zone, SemanticZone::Command),
                        "C" => open_zone(&mut out, &mut zone, SemanticZone::Output),
                        "D" => {
                            close_zone(&mut out, &mut zone);
                            let code = rest
                                .split(';')
                                .nth(1)
                                .and_then(|s| s.trim().parse::<i32>().ok())
                                .unwrap_or(0);
                            exits.push(code);
                        }
                        _ => {}
                    }
                }
                i = next_i;
                continue;
            } else if bytes.get(i + 1) == Some(&b'[') {
                // CSI sequence: `\x1b[ ... <0x40..=0x7E>`
                let mut end = i + 2;
                while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                    end += 1;
                }
                i = (end + 1).min(bytes.len());
                continue;
            }
        }

        let ch = raw[i..].chars().next().unwrap_or('\0');
        if ch != '\r' && ch != '\0' {
            out.push(ch);
        }
        i += ch.len_utf8().max(1);
    }

    close_zone(&mut out, &mut zone);
    (out, exits)
}
