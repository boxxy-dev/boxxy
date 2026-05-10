pub mod registry;

use crate::core::state::AgentState;
use crate::ipc::pty::SpawnOptions;
use nix::fcntl::OFlag;
use nix::pty::{PtyMaster, grantpt, posix_openpt, ptsname, unlockpt};
use nix::unistd::{User, getuid};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use zbus::fdo;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;

fn is_leaked_multiplexer_prefix(key: &str) -> bool {
    key.starts_with("TMUX")
        || key.starts_with("ZELLIJ")
        || key.starts_with("KITTY_")
        || key.starts_with("ALACRITTY_")
        || key.starts_with("VSCODE_")
}

const LEAKED_MULTIPLEXER_EXACT: &[&str] = &["STY", "WINDOW", "WT_SESSION"];

fn is_leaked_multiplexer_var(key: &str) -> bool {
    is_leaked_multiplexer_prefix(key) || LEAKED_MULTIPLEXER_EXACT.contains(&key)
}

pub struct PtySubsystem {
    state: AgentState,
}

impl PtySubsystem {
    pub fn new(state: AgentState) -> Self {
        Self { state }
    }

    /// Internal helper to read the foreground process from /proc without &self.
    fn get_foreground_process_internal(pid: u32) -> fdo::Result<String> {
        let stat_path = format!("/proc/{}/stat", pid);
        let stat_content = match std::fs::read_to_string(&stat_path) {
            Ok(c) => c,
            Err(_) => return Ok("".to_string()),
        };

        let rp_pos = stat_content.rfind(')').unwrap_or(0);
        if rp_pos == 0 || rp_pos + 2 >= stat_content.len() {
            return Ok("".to_string());
        }

        let rest = &stat_content[rp_pos + 2..];
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 6 {
            return Ok("".to_string());
        }

        let pgrp: u32 = parts[2].parse().unwrap_or(0);
        let tpgid: u32 = parts[5].parse().unwrap_or(0);

        if tpgid == 0 || tpgid == pgrp {
            return Ok("".to_string());
        }

        let tpgid_stat_path = format!("/proc/{}/stat", tpgid);
        if let Ok(tpgid_stat) = std::fs::read_to_string(&tpgid_stat_path) {
            let lp_pos = tpgid_stat.find('(');
            let rp_pos = tpgid_stat.rfind(')');
            if let (Some(lp), Some(rp)) = (lp_pos, rp_pos) {
                if rp > lp {
                    return Ok(tpgid_stat[lp + 1..rp].to_string());
                }
            }
        }

        Ok("".to_string())
    }
}

#[interface(name = "dev.boxxy.BoxxyTerminal.Agent.Pty")]
impl PtySubsystem {
    async fn get_preferred_shell(&self) -> fdo::Result<String> {
        if let Ok(Some(user)) = User::from_uid(getuid()) {
            let shell = user.shell.to_string_lossy().to_string();
            if !shell.is_empty() {
                return Ok(shell);
            }
        }
        Ok(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()))
    }

    async fn create_pty(&self) -> fdo::Result<OwnedFd> {
        let master_fd =
            posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK)
                .map_err(|e| fdo::Error::Failed(format!("posix_openpt failed: {}", e)))?;

        grantpt(&master_fd).map_err(|e| fdo::Error::Failed(format!("grantpt failed: {}", e)))?;
        unlockpt(&master_fd).map_err(|e| fdo::Error::Failed(format!("unlockpt failed: {}", e)))?;

        let raw_fd = master_fd.into_raw_fd();
        let std_fd = unsafe { std::os::unix::io::OwnedFd::from_raw_fd(raw_fd) };
        Ok(OwnedFd::from(std_fd))
    }

    async fn set_foreground_tracking(&self, pid: u32, enabled: bool) -> fdo::Result<()> {
        let mut lock = self.state.tracked_pids.write().await;
        if enabled {
            lock.insert(pid);
        } else {
            lock.remove(&pid);
        }
        Ok(())
    }

    async fn spawn(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        pty_master: OwnedFd,
        options: SpawnOptions,
    ) -> fdo::Result<u32> {
        let std_fd: std::os::unix::io::OwnedFd = pty_master.into();

        // Dup the master *before* consuming it into PtyMaster. This dup
        // goes into the registry and sits idle until `detach()` activates
        // it, so persistence works without the UI having to hand an FD
        // over at detach time.
        let registry_fd_raw = unsafe { libc::dup(std_fd.as_raw_fd()) };
        let registry_fd = if registry_fd_raw >= 0 {
            Some(unsafe { std::os::unix::io::OwnedFd::from_raw_fd(registry_fd_raw) })
        } else {
            log::warn!(
                "spawn: dup of master FD failed: {}; session won't be eligible for persistence",
                std::io::Error::last_os_error()
            );
            None
        };

        let master_fd = unsafe { PtyMaster::from_owned_fd(std_fd) };

        unsafe {
            let ws = libc::winsize {
                ws_row: options.rows,
                ws_col: options.cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            libc::ioctl(master_fd.as_raw_fd(), libc::TIOCSWINSZ, &ws);
        }

        let slave_name = unsafe {
            ptsname(&master_fd).map_err(|e| {
                log::error!("ptsname failed: {}", e);
                fdo::Error::Failed(format!("ptsname failed: {}", e))
            })?
        };

        let mut cmd = tokio::process::Command::new(&options.argv[0]);
        if options.argv.len() > 1 {
            cmd.args(&options.argv[1..]);
        }

        // Surgically remove leaked variables from the daemon's inherited environment.
        // We do NOT use `env_clear()` because in Flatpak mode, the UI relies on the
        // daemon to provide critical vars like PATH and DISPLAY.
        for key in LEAKED_MULTIPLEXER_EXACT {
            cmd.env_remove(key);
        }
        for (k, _) in std::env::vars() {
            if is_leaked_multiplexer_prefix(&k) {
                cmd.env_remove(&k);
            }
        }

        if !options.cwd.is_empty() {
            cmd.current_dir(&options.cwd);
        }

        if options.env.is_empty() {
            if let Ok(Some(user)) = User::from_uid(getuid()) {
                cmd.env("HOME", user.dir.to_string_lossy().as_ref());
                cmd.env("USER", &user.name);
                cmd.env("LOGNAME", &user.name);
                cmd.env("SHELL", user.shell.to_string_lossy().as_ref());
            }
        }

        for (key, value) in options.env {
            if !is_leaked_multiplexer_var(&key) {
                cmd.env(key, value);
            }
        }

        let master_raw_fd = master_fd.as_raw_fd();

        unsafe {
            cmd.pre_exec(move || {
                libc::setsid();

                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGHUP) != 0 {
                    return Err(std::io::Error::last_os_error());
                }

                let slave_fd = libc::open(
                    std::ffi::CString::new(slave_name.clone()).unwrap().as_ptr(),
                    libc::O_RDWR,
                );
                if slave_fd == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }

                libc::dup2(slave_fd, libc::STDIN_FILENO);
                libc::dup2(slave_fd, libc::STDOUT_FILENO);
                libc::dup2(slave_fd, libc::STDERR_FILENO);

                libc::close(master_raw_fd);
                if slave_fd > 2 {
                    libc::close(slave_fd);
                }

                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| {
            log::error!("Agent spawn failed: {}", e);
            fdo::Error::Failed(format!("spawn failed: {}", e))
        })?;

        let pid = child
            .id()
            .ok_or_else(|| fdo::Error::Failed("Failed to get PID".to_string()))?;

        // Register the PTY session so detach/reattach/list-detached can
        // find it later. viewer_count starts at 1 (the spawning UI).
        // The daemon's FD dup sits idle until detach activates it.
        self.state
            .pty_registry
            .register(pid, options.pane_id.clone(), registry_fd)
            .await;

        let emitter = emitter.to_owned();
        let tracked_pids = self.state.tracked_pids.clone();
        let pty_registry = self.state.pty_registry.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));
            let mut last_process_name = String::new();

            loop {
                tokio::select! {
                    status = child.wait() => {
                        let exit_code = match status {
                            Ok(s) => s.code().unwrap_or(0),
                            Err(_) => -1,
                        };
                        let _ = PtySubsystem::exited(&emitter, pid, exit_code).await;
                        tracked_pids.write().await.remove(&pid);
                        // Drop the session record; this also aborts any
                        // active detached reader task.
                        pty_registry.remove(pid).await;
                        break;
                    }
                    _ = interval.tick() => {
                        let is_tracked = tracked_pids.read().await.contains(&pid);
                        if is_tracked {
                            if let Ok(current_process) = Self::get_foreground_process_internal(pid) {
                                if current_process != last_process_name {
                                    let _ = PtySubsystem::foreground_process_changed(&emitter, pid, current_process.clone()).await;
                                    last_process_name = current_process;
                                }
                            }
                        } else if !last_process_name.is_empty() {
                            last_process_name = String::new();
                            let _ = PtySubsystem::foreground_process_changed(&emitter, pid, String::new()).await;
                        }
                    }
                }
            }
        });

        Ok(pid)
    }

    async fn set_persistence(&self, pid: u32, enabled: bool) -> fdo::Result<()> {
        self.state.pty_registry.set_persistence(pid, enabled).await;
        Ok(())
    }

    /// Decrements the viewer count. If the last viewer detached:
    ///   * persistence off → SIGTERM the process group (returns 0).
    ///   * persistence on  → daemon starts buffering via its stored
    ///     FD dup (returns 1). The UI doesn't need to hand an FD over;
    ///     the daemon has had one since `spawn()`.
    /// Numeric codes match `DetachOutcome`; the UI only has to check 1
    /// for "successfully detached and alive".
    async fn detach(&self, pid: u32) -> fdo::Result<u32> {
        let outcome = self.state.pty_registry.detach(pid).await;
        Ok(match outcome {
            registry::DetachOutcome::Detached => 1,
            registry::DetachOutcome::Terminated => 0,
            registry::DetachOutcome::StillViewed => 2,
            registry::DetachOutcome::DetachedUnbuffered => 3,
            registry::DetachOutcome::Unknown => u32::MAX,
        })
    }

    async fn list_detached_sessions(&self) -> fdo::Result<Vec<(u32, String, u64)>> {
        Ok(self.state.pty_registry.list_detached().await)
    }

    async fn reattach(&self, pid: u32) -> fdo::Result<(Vec<u8>, OwnedFd)> {
        match self.state.pty_registry.reattach(pid).await {
            Some((bytes, fd)) => Ok((bytes, OwnedFd::from(fd))),
            None => Err(fdo::Error::Failed(format!(
                "No detached session for pid {pid}"
            ))),
        }
    }

    async fn get_cwd(&self, pid: u32) -> fdo::Result<String> {
        let link_path = format!("/proc/{}/cwd", pid);
        match std::fs::read_link(link_path) {
            Ok(path) => Ok(path.to_string_lossy().to_string()),
            Err(e) => Err(fdo::Error::Failed(format!("Failed to read CWD: {}", e))),
        }
    }

    async fn get_foreground_process(&self, pid: u32) -> fdo::Result<String> {
        Self::get_foreground_process_internal(pid)
    }

    async fn get_running_processes(&self, pid: u32) -> fdo::Result<Vec<(u32, String)>> {
        let mut all_procs = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                if let Ok(file_name) = entry.file_name().into_string() {
                    if let Ok(p) = file_name.parse::<u32>() {
                        let stat_path = format!("/proc/{}/stat", p);
                        if let Ok(stat_content) = std::fs::read_to_string(&stat_path) {
                            let lp_pos = stat_content.find('(');
                            let rp_pos = stat_content.rfind(')');
                            if let (Some(lp), Some(rp)) = (lp_pos, rp_pos) {
                                if rp > lp {
                                    let name = stat_content[lp + 1..rp].to_string();
                                    let rest = &stat_content[rp + 2..];
                                    let parts: Vec<&str> = rest.split_whitespace().collect();
                                    if parts.len() > 1 {
                                        if let Ok(ppid) = parts[1].parse::<u32>() {
                                            let cmdline_path = format!("/proc/{}/cmdline", p);
                                            let full_name =
                                                if let Ok(cmdline) = std::fs::read(&cmdline_path) {
                                                    let cmd = String::from_utf8_lossy(&cmdline)
                                                        .replace('\0', " ")
                                                        .trim()
                                                        .to_string();
                                                    if cmd.is_empty() { name } else { cmd }
                                                } else {
                                                    name
                                                };
                                            all_procs.push((p, ppid, full_name));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut descendants = Vec::new();
        let mut to_visit = vec![pid];
        let mut visited = std::collections::HashSet::new();

        while let Some(current) = to_visit.pop() {
            if !visited.insert(current) {
                continue;
            }
            for (p, ppid, name) in &all_procs {
                if *ppid == current {
                    descendants.push((*p, name.clone()));
                    to_visit.push(*p);
                }
            }
        }

        Ok(descendants)
    }

    async fn signal_process_group(&self, pid: u32, signal: i32) -> fdo::Result<()> {
        let mut tpgid = None;
        let stat_path = format!("/proc/{}/stat", pid);
        if let Ok(stat_content) = std::fs::read_to_string(&stat_path) {
            if let Some(rp_pos) = stat_content.rfind(')') {
                if rp_pos + 2 < stat_content.len() {
                    let rest = &stat_content[rp_pos + 2..];
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if parts.len() >= 6 {
                        let pgrp: u32 = parts[2].parse().unwrap_or(0);
                        let parsed_tpgid: u32 = parts[5].parse().unwrap_or(0);
                        if parsed_tpgid != 0 && parsed_tpgid != pgrp {
                            tpgid = Some(parsed_tpgid);
                        }
                    }
                }
            }
        }

        unsafe {
            if let Some(tg) = tpgid {
                log::debug!(
                    "Signaling foreground process group {} with signal {}",
                    tg,
                    signal
                );
                libc::kill(-(tg as i32), signal);
            }
            log::debug!(
                "Signaling shell process group {} with signal {}",
                pid,
                signal
            );
            if libc::kill(-(pid as i32), signal) != 0 {
                // Ignore error if shell is already dead
            }
        }
        Ok(())
    }

    async fn get_environment_variable(&self, name: String) -> fdo::Result<String> {
        Ok(std::env::var(name).unwrap_or_default())
    }

    #[zbus(signal)]
    async fn exited(emitter: &SignalEmitter<'_>, pid: u32, exit_code: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn foreground_process_changed(
        emitter: &SignalEmitter<'_>,
        pid: u32,
        process_name: String,
    ) -> zbus::Result<()>;
}
