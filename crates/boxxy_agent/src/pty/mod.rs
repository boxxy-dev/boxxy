pub mod registry;
pub mod shell_integration;

use crate::core::state::AgentState;
use crate::ipc::pty::{DetachedSessionInfo, SpawnOptions};
use nix::fcntl::OFlag;
use nix::pty::{PtyMaster, grantpt, posix_openpt, ptsname, unlockpt};
use nix::unistd::{User, getuid};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use zbus::OwnedFd;
use zbus::fdo;
use zbus::interface;
use zbus::object_server::SignalEmitter;

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

    /// Inspects the foreground process group of `shell_pid` and checks whether
    /// any process in that foreground group is blocked on a TTY read (`wchan` in
    /// `n_tty_read` / `tty_read`). Never flags the shell prompt itself.
    fn inspect_foreground_state(shell_pid: u32) -> (String, bool) {
        let stat_path = format!("/proc/{shell_pid}/stat");
        let Ok(stat_content) = std::fs::read_to_string(&stat_path) else {
            return (String::new(), false);
        };

        let rp_pos = stat_content.rfind(')').unwrap_or(0);
        if rp_pos == 0 || rp_pos + 2 >= stat_content.len() {
            return (String::new(), false);
        }

        let rest = &stat_content[rp_pos + 2..];
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 6 {
            return (String::new(), false);
        }

        let pgrp: u32 = parts[2].parse().unwrap_or(0);
        let tpgid: u32 = parts[5].parse().unwrap_or(0);

        // If tpgid == pgrp, the shell itself owns the foreground (idle at prompt).
        if tpgid == 0 || tpgid == pgrp {
            return (String::new(), false);
        }

        let mut fg_name = String::new();
        let tpgid_stat_path = format!("/proc/{tpgid}/stat");
        if let Ok(tpgid_stat) = std::fs::read_to_string(&tpgid_stat_path)
            && let (Some(lp), Some(rp)) = (tpgid_stat.find('('), tpgid_stat.rfind(')'))
            && rp > lp
        {
            fg_name = tpgid_stat[lp + 1..rp].to_string();
        }

        if fg_name.is_empty() {
            return (String::new(), false);
        }

        let mut pids_to_check = vec![tpgid];
        if let Some(children) = read_direct_children_procfs(tpgid) {
            pids_to_check.extend(children);
        }

        let waiting_on_tty = pids_to_check.into_iter().any(is_pid_wchan_tty_read);
        (fg_name, waiting_on_tty)
    }
}

pub(crate) fn prettify_character_name(raw: &str) -> String {
    raw.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) async fn query_detached_sessions_detailed(
    pty_registry: &registry::PtyRegistry,
) -> Vec<DetachedSessionInfo> {
    let detached = pty_registry.list_detached_with_cwd().await;
    let workspace = boxxy_claw::registry::workspace::global_workspace().await;
    let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
    let catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();

    let mut out = Vec::with_capacity(detached.len());
    for (pid, pane_id, orig_pane_id, idle_secs, initial_cwd) in detached {
        let pending_tasks = workspace.get_pane_pending_tasks(&orig_pane_id).await;
        let pending_task_count = pending_tasks.len() as u32;
        let claim_opt = claims.iter().find(|c| c.holder_id == orig_pane_id);
        let has_claw = pending_task_count > 0 || claim_opt.is_some();

        // If a detached shell has no active foreground/child process anymore
        // AND no detached AI agent / pending reminder, reap it instead of prompting
        // the user to restore an empty interactive shell.
        if !shell_has_active_process(pid) && !has_claw {
            log::info!(
                "pty-registry: reaping idle detached shell pid={} (no active child processes)",
                pid
            );
            pty_registry.set_persistence(pid, false).await;
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            continue;
        }

        let (character_id, character_name, character_slug, session_id) =
            if let Some(claim) = claim_opt {
                let char_info = catalog.iter().find(|c| c.config.id == claim.character_id);
                let name = char_info
                    .map(|c| prettify_character_name(&c.config.display_name))
                    .unwrap_or_default();
                let slug = char_info.map(|c| c.config.name.clone()).unwrap_or_default();
                (
                    claim.character_id.clone(),
                    name,
                    slug,
                    claim.session_id.clone(),
                )
            } else {
                (String::new(), String::new(), String::new(), String::new())
            };

        let live_cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(initial_cwd);
        let (mut fg_proc, wchan_tty) = PtySubsystem::inspect_foreground_state(pid);
        if fg_proc.is_empty()
            && let Some((_, child_name)) = collect_descendant_processes(pid).into_iter().next()
        {
            fg_proc = child_name;
        }
        if fg_proc.is_empty() {
            if let Some(first_task) = pending_tasks.first() {
                let msg = first_task.payload.trim();
                let extra = if pending_task_count > 1 {
                    format!(" (+{} more)", pending_task_count - 1)
                } else {
                    String::new()
                };
                if !character_name.is_empty() {
                    fg_proc = format!("{character_name}: {msg}{extra}");
                } else {
                    fg_proc = format!("Reminder: {msg}{extra}");
                }
            } else if !character_name.is_empty() {
                fg_proc = format!("{character_name} (AI Agent)");
            }
        }
        let waiting_for_input = wchan_tty && idle_secs >= 2;
        out.push(DetachedSessionInfo {
            pid,
            pane_id,
            idle_secs,
            cwd: live_cwd,
            foreground_process: fg_proc,
            waiting_for_input,
            character_id,
            character_name,
            character_slug,
            session_id,
            pending_task_count,
        });
    }
    out
}

/// Returns `true` if `shell_pid` is currently running a foreground command
/// (`tpgid != pgrp`), has at least one descendant child process, or was
/// launched as a non-interactive command (e.g. `sh -c ...` or `exec <cmd>`).
/// Returns `true` if `/proc/<shell_pid>` does not exist (synthetic PIDs in unit tests).
pub(crate) fn shell_has_active_process(shell_pid: u32) -> bool {
    let proc_path = format!("/proc/{shell_pid}");
    if !std::path::Path::new(&proc_path).exists() {
        return true;
    }

    // If the root process was spawned with `-c` (non-interactive command script)
    // or exec'd into a non-shell binary (e.g. `top`, `nvim`), treat it as active.
    if let Ok(cmdline) = std::fs::read(format!("/proc/{shell_pid}/cmdline")) {
        let args: Vec<&[u8]> = cmdline
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .collect();
        if args.iter().skip(1).any(|&arg| arg == b"-c") {
            return true;
        }
    }
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{shell_pid}/comm")) {
        let name = comm.trim();
        let is_shell = matches!(
            name,
            "sh" | "bash"
                | "zsh"
                | "fish"
                | "dash"
                | "nu"
                | "ksh"
                | "tcsh"
                | "csh"
                | "elvish"
                | "xonsh"
        );
        if !name.is_empty() && !is_shell {
            return true;
        }
    }

    let has_fg = PtySubsystem::get_foreground_process_internal(shell_pid)
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_fg {
        return true;
    }
    !collect_descendant_processes(shell_pid).is_empty()
}

fn is_pid_wchan_tty_read(pid: u32) -> bool {
    let wchan_path = format!("/proc/{pid}/wchan");
    if let Ok(wchan) = std::fs::read_to_string(&wchan_path) {
        let trimmed = wchan.trim();
        if matches!(trimmed, "n_tty_read" | "tty_read") {
            return true;
        }
    }
    false
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
        #[zbus(header)] header: zbus::message::Header<'_>,
        pty_master: OwnedFd,
        options: SpawnOptions,
    ) -> fdo::Result<u32> {
        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
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

        let mut argv = options.argv;
        let mut env = options.env;
        if let Some(integration_root) = shell_integration::ensure_integration_scripts() {
            shell_integration::apply_shell_integration(&mut argv, &mut env, &integration_root);
        }

        let mut cmd = tokio::process::Command::new(&argv[0]);
        if argv.len() > 1 {
            cmd.args(&argv[1..]);
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

        if env.is_empty() {
            if let Ok(Some(user)) = User::from_uid(getuid()) {
                cmd.env("HOME", user.dir.to_string_lossy().as_ref());
                cmd.env("USER", &user.name);
                cmd.env("LOGNAME", &user.name);
                cmd.env("SHELL", user.shell.to_string_lossy().as_ref());
            }
        }

        for (key, value) in env {
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
        let default_persist = boxxy_preferences::Settings::load().pty_persistence;
        self.state
            .pty_registry
            .register_with_owner(
                pid,
                options.pane_id.clone(),
                owner_bus_name,
                options.cwd.clone(),
                default_persist,
                registry_fd,
            )
            .await;

        let emitter = emitter.to_owned();
        let tracked_pids = self.state.tracked_pids.clone();
        let pty_registry = self.state.pty_registry.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
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
                        // Transition the session record to `Exited` (retaining exit_code
                        // for `EXITED_TTL` while immediately dropping master_fd and any
                        // detached reader task).
                        pty_registry.mark_exited(pid, exit_code).await;
                        break;
                    }
                    _ = interval.tick() => {
                        let is_tracked = tracked_pids.read().await.contains(&pid);
                        if is_tracked {
                            if let Ok(current_process) = Self::get_foreground_process_internal(pid) {
                                if current_process != last_process_name {
                                    if let Some(orig_pane_id) = pty_registry.get_original_pane_id(pid).await {
                                        let status_opt = (!current_process.is_empty())
                                            .then(|| format!("Running: {current_process}"));
                                        let workspace = boxxy_claw::registry::workspace::global_workspace().await;
                                        workspace.set_pane_status(orig_pane_id, status_opt).await;
                                    }
                                    let _ = PtySubsystem::foreground_process_changed(&emitter, pid, current_process.clone()).await;
                                    last_process_name = current_process;
                                }
                            }
                        } else if !last_process_name.is_empty() {
                            last_process_name = String::new();
                            if let Some(orig_pane_id) = pty_registry.get_original_pane_id(pid).await {
                                let workspace = boxxy_claw::registry::workspace::global_workspace().await;
                                workspace.set_pane_status(orig_pane_id, None).await;
                            }
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

    async fn set_pane_id(&self, pid: u32, pane_id: String) -> fdo::Result<()> {
        self.state.pty_registry.set_pane_id(pid, pane_id).await;
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

    async fn detach_with_snapshot(&self, pid: u32, initial_snapshot: Vec<u8>) -> fdo::Result<u32> {
        let outcome = self
            .state
            .pty_registry
            .detach_with_snapshot(pid, &initial_snapshot)
            .await;
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

    async fn list_detached_sessions_detailed(&self) -> fdo::Result<Vec<DetachedSessionInfo>> {
        Ok(query_detached_sessions_detailed(&self.state.pty_registry).await)
    }

    async fn reattach(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        pid: u32,
    ) -> fdo::Result<(Vec<u8>, OwnedFd)> {
        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        match self.state.pty_registry.reattach(pid).await {
            Some((bytes, fd)) => {
                self.state.pty_registry.set_owner(pid, owner_bus_name).await;
                // Nudge TIOCSWINSZ (+1 col then restore) so the kernel unconditionally
                // delivers SIGWINCH to the foreground process group, forcing idle shell
                // prompts (zle/readline/fish) and TUIs (nvim/htop) to redraw on reattach.
                unsafe {
                    let raw = fd.as_raw_fd();
                    let mut ws: libc::winsize = std::mem::zeroed();
                    if libc::ioctl(raw, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
                        let mut nudged = ws;
                        nudged.ws_col = ws.ws_col.saturating_add(1);
                        let _ = libc::ioctl(raw, libc::TIOCSWINSZ, &nudged);
                        let _ = libc::ioctl(raw, libc::TIOCSWINSZ, &ws);
                    }
                }
                Ok((bytes, OwnedFd::from(fd)))
            }
            None => Err(fdo::Error::Failed(format!(
                "No detached session for pid {pid}"
            ))),
        }
    }

    async fn get_exit_status(&self, pid: u32) -> fdo::Result<(bool, i32)> {
        match self.state.pty_registry.get_exit_status(pid).await {
            Some(code) => Ok((true, code)),
            None => Ok((false, 0)),
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
        Ok(collect_descendant_processes(pid))
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

fn read_process_display_name(pid: u32) -> Option<String> {
    let cmdline_path = format!("/proc/{pid}/cmdline");
    if let Ok(cmdline) = std::fs::read(&cmdline_path) {
        let cmd = String::from_utf8_lossy(&cmdline)
            .replace('\0', " ")
            .trim()
            .to_string();
        if !cmd.is_empty() {
            return Some(cmd);
        }
    }

    let stat_path = format!("/proc/{pid}/stat");
    let stat_content = std::fs::read_to_string(&stat_path).ok()?;
    let lp = stat_content.find('(')?;
    let rp = stat_content.rfind(')')?;
    (rp > lp).then(|| stat_content[lp + 1..rp].to_string())
}

fn read_direct_children_procfs(pid: u32) -> Option<Vec<u32>> {
    let primary_path = format!("/proc/{pid}/task/{pid}/children");
    let mut children = Vec::new();

    let task_dir = format!("/proc/{pid}/task");
    if let Ok(entries) = std::fs::read_dir(&task_dir) {
        let mut read_any = false;
        for entry in entries.flatten() {
            let children_path = entry.path().join("children");
            if let Ok(content) = std::fs::read_to_string(&children_path) {
                read_any = true;
                for token in content.split_whitespace() {
                    if let Ok(child_pid) = token.parse::<u32>() {
                        children.push(child_pid);
                    }
                }
            }
        }
        if read_any {
            return Some(children);
        }
    }

    if let Ok(content) = std::fs::read_to_string(&primary_path) {
        for token in content.split_whitespace() {
            if let Ok(child_pid) = token.parse::<u32>() {
                children.push(child_pid);
            }
        }
        return Some(children);
    }

    None
}

/// Collects all descendant processes `(pid, display_name)` of `root_pid`.
///
/// Uses the direct `/proc/<pid>/task/<tid>/children` kernel interface ($O(k)$
/// file reads where $k$ is the number of descendant processes), falling back
/// to a `/proc` parent-PID scan only if `children` is unavailable.
pub(crate) fn collect_descendant_processes(root_pid: u32) -> Vec<(u32, String)> {
    // Fast path: traverse `/proc/<pid>/task/*/children` directly.
    if let Some(initial_children) = read_direct_children_procfs(root_pid) {
        let mut descendants = Vec::new();
        let mut to_visit = initial_children;
        let mut visited = std::collections::HashSet::new();
        visited.insert(root_pid);

        while let Some(child_pid) = to_visit.pop() {
            if !visited.insert(child_pid) {
                continue;
            }
            if let Some(name) = read_process_display_name(child_pid) {
                descendants.push((child_pid, name));
            }
            if let Some(grandchildren) = read_direct_children_procfs(child_pid) {
                to_visit.extend(grandchildren);
            }
        }
        return descendants;
    }

    // If the root process doesn't exist at all, return empty immediately.
    if !std::path::Path::new(&format!("/proc/{root_pid}")).exists() {
        return Vec::new();
    }

    // Fallback for kernels compiled without CONFIG_PROC_CHILDREN: scan `/proc/*/stat`
    // for `(pid, ppid)`, and only read `cmdline` for matched descendants.
    let mut proc_edges = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(file_name) = entry.file_name().into_string()
                && let Ok(p) = file_name.parse::<u32>()
            {
                let stat_path = format!("/proc/{p}/stat");
                if let Ok(stat_content) = std::fs::read_to_string(&stat_path)
                    && let Some(rp) = stat_content.rfind(')')
                    && rp + 2 < stat_content.len()
                {
                    let rest = &stat_content[rp + 2..];
                    let mut parts = rest.split_whitespace();
                    let _state = parts.next();
                    if let Some(ppid_str) = parts.next()
                        && let Ok(ppid) = ppid_str.parse::<u32>()
                    {
                        proc_edges.push((p, ppid));
                    }
                }
            }
        }
    }

    let mut descendants = Vec::new();
    let mut to_visit = vec![root_pid];
    let mut visited = std::collections::HashSet::new();

    while let Some(current) = to_visit.pop() {
        if !visited.insert(current) {
            continue;
        }
        for &(p, ppid) in &proc_edges {
            if ppid == current {
                if let Some(name) = read_process_display_name(p) {
                    descendants.push((p, name));
                }
                to_visit.push(p);
            }
        }
    }

    descendants
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_descendant_processes_finds_spawned_child() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let child_pid = child.id();
        let my_pid = std::process::id();

        let descendants = collect_descendant_processes(my_pid);
        assert!(
            descendants.iter().any(|(pid, name)| *pid == child_pid && name.contains("sleep")),
            "expected child pid {child_pid} in {descendants:?}"
        );

        let _ = child.kill();
        let _ = child.wait();

        let after_exit = collect_descendant_processes(my_pid);
        assert!(!after_exit.iter().any(|(pid, _)| *pid == child_pid));
    }

    #[tokio::test]
    async fn idle_shell_without_children_terminates_on_detach_even_with_persistence_on() {
        // Spawn a real interactive-style shell process that has NO child processes
        // and is simply waiting on stdin.
        let mut idle_proc = std::process::Command::new("/bin/sh")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn /bin/sh");
        let pid = idle_proc.id();

        assert!(
            !shell_has_active_process(pid),
            "idle /bin/sh with no children and no separate tpgid should be detected as idle"
        );

        let reg = registry::PtyRegistry::new();
        reg.register(pid, "pane-idle-test".into(), None).await;
        reg.set_persistence(pid, true).await;

        let outcome = reg.detach(pid).await;
        assert_eq!(outcome, registry::DetachOutcome::Terminated);
        assert!(reg.list_detached().await.is_empty());

        let _ = idle_proc.kill();
        let _ = idle_proc.wait();
    }

    #[tokio::test]
    async fn idle_shell_with_pending_claw_task_stays_alive_on_detach() {
        use nix::fcntl::OFlag;
        use nix::pty::{grantpt, posix_openpt, unlockpt};
        use std::os::fd::{FromRawFd, IntoRawFd};

        let mut idle_proc = std::process::Command::new("/bin/sh")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn /bin/sh");
        let pid = idle_proc.id();

        let master = posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_NONBLOCK)
            .expect("posix_openpt");
        grantpt(&master).expect("grantpt");
        unlockpt(&master).expect("unlockpt");
        let master_owned = unsafe { std::os::unix::io::OwnedFd::from_raw_fd(master.into_raw_fd()) };

        let pane_id = format!("pane-reminder-test-{}", uuid::Uuid::new_v4());
        let workspace = boxxy_claw::registry::workspace::global_workspace().await;
        workspace
            .update_pane_tasks(
                pane_id.clone(),
                vec![boxxy_claw_protocol::ScheduledTask {
                    id: uuid::Uuid::new_v4(),
                    task_type: boxxy_claw_protocol::TaskType::Notification,
                    payload: "Wake me up".to_string(),
                    due_at: chrono::Utc::now() + chrono::Duration::seconds(120),
                    status: boxxy_claw_protocol::TaskStatus::Pending,
                }],
            )
            .await;

        let reg = registry::PtyRegistry::new();
        reg.register(pid, pane_id.clone(), Some(master_owned)).await;
        reg.set_persistence(pid, true).await;

        let outcome = reg.detach(pid).await;
        assert_eq!(outcome, registry::DetachOutcome::Detached);
        assert_eq!(reg.list_detached().await.len(), 1);

        workspace.update_pane_tasks(pane_id, Vec::new()).await;
        reg.remove(pid).await;
        let _ = idle_proc.kill();
        let _ = idle_proc.wait();
    }
}
