pub mod procfs;
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

pub(crate) use procfs::{collect_descendant_processes, shell_has_active_process};

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
            let _ = registry::kill_process_group_safe(pid, libc::SIGTERM);
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
        let (mut fg_proc, wchan_tty) = procfs::inspect_foreground_state(pid);
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

fn exit_status_to_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        code
    } else if let Some(sig) = status.signal() {
        128 + sig
    } else {
        -1
    }
}

fn ensure_cloexec(fd: std::os::unix::io::RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 && (flags & libc::FD_CLOEXEC) == 0 {
            let _ = libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
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
        #[zbus(header)] header: zbus::message::Header<'_>,
        pty_master: OwnedFd,
        options: SpawnOptions,
    ) -> fdo::Result<u32> {
        if options.argv.is_empty() || options.argv[0].is_empty() {
            return Err(fdo::Error::InvalidArgs(
                "spawn argv must contain a non-empty executable".to_string(),
            ));
        }

        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let std_fd: std::os::unix::io::OwnedFd = pty_master.into();
        ensure_cloexec(std_fd.as_raw_fd());

        // Clone the master FD with FD_CLOEXEC (`F_DUPFD_CLOEXEC`) *before*
        // consuming `std_fd` into `PtyMaster`. Using `try_clone()` (instead of
        // `libc::dup`, which strips `FD_CLOEXEC`) guarantees the parked master FD
        // never leaks into sibling shells, `exec_shell`, `spawn_detached`, or MCP servers.
        let registry_fd = match std_fd.try_clone() {
            Ok(fd) => Some(fd),
            Err(e) => {
                log::warn!(
                    "spawn: try_clone of master FD failed: {e}; session won't be eligible for persistence"
                );
                None
            }
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
        // Pre-allocate CString on the parent side so `pre_exec` performs zero heap
        // allocations between `fork()` and `exec()` (async-signal-safety requirement).
        let slave_cstr = std::ffi::CString::new(slave_name)
            .map_err(|e| fdo::Error::Failed(format!("invalid slave PTY path: {e}")))?;

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

        if env.is_empty()
            && let Ok(Some(user)) = User::from_uid(getuid())
        {
            cmd.env("HOME", user.dir.to_string_lossy().as_ref());
            cmd.env("USER", &user.name);
            cmd.env("LOGNAME", &user.name);
            cmd.env("SHELL", user.shell.to_string_lossy().as_ref());
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

                let slave_fd = libc::open(slave_cstr.as_ptr(), libc::O_RDWR);
                if slave_fd == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }

                libc::dup2(slave_fd, libc::STDIN_FILENO);
                libc::dup2(slave_fd, libc::STDOUT_FILENO);
                libc::dup2(slave_fd, libc::STDERR_FILENO);

                if slave_fd > 2 {
                    libc::close(slave_fd);
                }
                if master_raw_fd > 2 {
                    libc::close(master_raw_fd);
                }
                // Mark every remaining descriptor >= 3 as FD_CLOEXEC in O(1) kernel time
                // (without breaking std::process::Command's internal CLOEXEC exec-error pipe)
                // so C libraries (NVML/Vulkan) or D-Bus sockets never leak FDs into user shells.
                let _ = libc::syscall(
                    libc::SYS_close_range,
                    3 as libc::c_uint,
                    libc::c_uint::MAX,
                    libc::CLOSE_RANGE_CLOEXEC as libc::c_uint,
                );

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
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(150));
            let mut last_process_name = String::new();

            loop {
                tokio::select! {
                    status = child.wait() => {
                        let exit_code = match status {
                            Ok(s) => exit_status_to_code(s),
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
                            if let Ok(current_process) = procfs::get_foreground_process(pid)
                                && current_process != last_process_name
                            {
                                if let Some(orig_pane_id) = pty_registry.get_original_pane_id(pid).await {
                                    let status_opt = (!current_process.is_empty())
                                        .then(|| format!("Running: {current_process}"));
                                    let workspace = boxxy_claw::registry::workspace::global_workspace().await;
                                    workspace.set_pane_status(orig_pane_id, status_opt).await;
                                }
                                let _ = PtySubsystem::foreground_process_changed(&emitter, pid, current_process.clone()).await;
                                last_process_name = current_process;
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

    async fn detach_with_snapshot(&self, pid: u32, initial_snapshot: &[u8]) -> fdo::Result<u32> {
        let outcome = self
            .state
            .pty_registry
            .detach_with_snapshot(pid, initial_snapshot)
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
        procfs::get_foreground_process(pid)
    }

    async fn get_running_processes(&self, pid: u32) -> fdo::Result<Vec<(u32, String)>> {
        Ok(collect_descendant_processes(pid))
    }

    async fn signal_process_group(&self, pid: u32, signal: i32) -> fdo::Result<()> {
        let raw_pid = i32::try_from(pid).map_err(|_| {
            fdo::Error::InvalidArgs(format!("Invalid PID {pid}: exceeds maximum positive pid_t"))
        })?;
        if raw_pid <= 1 || pid == std::process::id() {
            return Err(fdo::Error::InvalidArgs(format!(
                "Refusing to signal protected or invalid PID {pid}"
            )));
        }
        if !(1..=64).contains(&signal) {
            return Err(fdo::Error::InvalidArgs(format!(
                "Invalid signal number: {signal}"
            )));
        }

        match self.state.pty_registry.process_state(pid).await {
            Some(registry::ProcessState::Running) => {}
            Some(registry::ProcessState::Exited { .. }) => {
                // Process already exited; ignore cleanly without risking PID-reuse signaling.
                return Ok(());
            }
            None => {
                return Err(fdo::Error::InvalidArgs(format!(
                    "Refusing to signal untracked PTY PID {pid}"
                )));
            }
        }

        if let Some(tg) = procfs::read_foreground_tpgid(pid)
            .filter(|&tg| tg > 1 && tg != std::process::id())
        {
            log::debug!("Signaling foreground process group {tg} with signal {signal}");
            let _ = registry::kill_process_group_safe(tg, signal);
        }
        log::debug!("Signaling shell process group {pid} with signal {signal}");
        let _ = registry::kill_process_group_safe(pid, signal);
        Ok(())
    }

    async fn get_environment_variable(&self, name: &str) -> fdo::Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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

        let master = posix_openpt(
            OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        )
        .expect("posix_openpt");
        grantpt(&master).expect("grantpt");
        unlockpt(&master).expect("unlockpt");
        let master_owned =
            unsafe { std::os::unix::io::OwnedFd::from_raw_fd(master.into_raw_fd()) };

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
