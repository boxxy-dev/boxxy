//! Headless POSIX PTY driver exercising `PtyRegistry` detach/reattach ring buffering
//! and Boxxy's zero-config Bash `OSC 7` / `OSC 133` shell integration.

use anyhow::{Context, Result, anyhow};
use boxxy_agent::pty::registry::{DetachOutcome, PtyRegistry};
use boxxy_agent::pty::shell_integration::{apply_shell_integration, ensure_integration_scripts_in};
use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl, open};
use nix::pty::{Winsize, grantpt, posix_openpt, ptsname_r, unlockpt};
use nix::sys::signal::{Signal, killpg};
use nix::sys::stat::Mode;
use nix::unistd::Pid;
use parking_lot::Mutex;
use std::os::fd::{AsRawFd as _, OwnedFd};
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
                nix::unistd::setsid().map_err(std::io::Error::from)?;
                if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
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
            match nix::unistd::write(&self.writer_fd, &input[offset..]) {
                Ok(n) => offset += n,
                Err(Errno::EINTR) => {}
                Err(err) => return Err(std::io::Error::from(err).into()),
            }
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
            && self.pid != std::process::id()
        {
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
        }
        let _ = self.child.start_kill();
    }
}

fn open_pty_pair(cols: u16, rows: u16) -> Result<(OwnedFd, OwnedFd, OwnedFd, OwnedFd)> {
    let master =
        posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_CLOEXEC).context("posix_openpt")?;
    grantpt(&master).context("grantpt")?;
    unlockpt(&master).context("unlockpt")?;
    let slave_name = ptsname_r(&master).context("ptsname_r")?;

    let slave_stdin = open(
        Path::new(&slave_name),
        OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .context("open slave PTY")?;
    let slave_stdout = slave_stdin.try_clone().context("dup slave stdout")?;
    let slave_stderr = slave_stdin.try_clone().context("dup slave stderr")?;

    let ws = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        let _ = libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws);
    }

    let master_owned = OwnedFd::from(master);
    Ok((master_owned, slave_stdin, slave_stdout, slave_stderr))
}

fn spawn_ui_reader(
    fd: OwnedFd,
    captured: Arc<Mutex<Vec<u8>>>,
    notify: Arc<Notify>,
) -> Result<JoinHandle<()>> {
    let raw_flags = fcntl(&fd, FcntlArg::F_GETFL).context("F_GETFL on PTY reader FD")?;
    let flags = OFlag::from_bits_retain(raw_flags);
    if !flags.contains(OFlag::O_NONBLOCK) {
        fcntl(&fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))
            .context("F_SETFL O_NONBLOCK on PTY reader FD")?;
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
            let result = guard.try_io(|inner| loop {
                match nix::unistd::read(inner.get_ref(), &mut buf) {
                    Ok(n) => return Ok(n),
                    Err(Errno::EINTR) => {}
                    Err(err) => return Err(std::io::Error::from(err)),
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
