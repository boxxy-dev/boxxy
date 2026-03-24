use nix::fcntl::OFlag;
use nix::pty::{PtyMaster, grantpt, posix_openpt, ptsname, unlockpt};
use nix::unistd::{User, getuid};
use serde::{Deserialize, Serialize};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use zbus::fdo;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::proxy;
use zbus::zvariant::{OwnedFd, Type};

#[proxy(
    interface = "play.mii.Boxxy.Agent",
    default_path = "/play/mii/Boxxy/Agent",
    gen_blocking = false
)]
pub trait Agent {
    async fn get_preferred_shell(&self) -> zbus::Result<String>;
    async fn create_pty(&self) -> zbus::Result<OwnedFd>;
    async fn spawn(&self, pty_master: OwnedFd, options: SpawnOptions) -> zbus::Result<u32>;
    async fn get_cwd(&self, pid: u32) -> zbus::Result<String>;
    async fn get_foreground_process(&self, pid: u32) -> zbus::Result<String>;
    async fn get_running_processes(&self, pid: u32) -> zbus::Result<Vec<(u32, String)>>;
    async fn signal_process_group(&self, pid: u32, signal: i32) -> zbus::Result<()>;
    async fn set_foreground_tracking(&self, pid: u32, enabled: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn exited(&self, pid: u32, exit_code: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn foreground_process_changed(&self, pid: u32, process_name: String) -> zbus::Result<()>;
}

#[proxy(
    interface = "play.mii.Boxxy.AgentClaw",
    default_path = "/play/mii/Boxxy/AgentClaw",
    gen_blocking = false
)]
pub trait AgentClaw {
    async fn exec_shell(&self, command: String) -> zbus::Result<(i32, String, String)>;
    async fn read_file(&self, path: String, start_line: u32, end_line: u32)
    -> zbus::Result<String>;
    async fn write_file(&self, path: String, content: String) -> zbus::Result<()>;
    async fn list_directory(&self, path: String) -> zbus::Result<Vec<(String, bool, u64)>>;
    async fn delete_file(&self, path: String) -> zbus::Result<()>;
    async fn get_system_info(&self) -> zbus::Result<String>;
    async fn list_processes(&self) -> zbus::Result<Vec<(u32, String, f64, u64, u64, u64)>>;
    async fn kill_process(&self, pid: u32, signal: i32) -> zbus::Result<()>;
    async fn get_clipboard(&self) -> zbus::Result<String>;
    async fn set_clipboard(&self, text: String) -> zbus::Result<()>;
}

#[derive(Debug, Serialize, Deserialize, Type)]
pub struct SpawnOptions {
    pub cwd: String, // Use empty string for None
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Default)]
pub struct BoxxyAgent {
    pub tracked_pids: std::sync::Arc<tokio::sync::RwLock<std::collections::HashSet<u32>>>,
}

impl BoxxyAgent {
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
            if let (Some(lp), Some(rp)) = (lp_pos, rp_pos)
                && rp > lp
            {
                return Ok(tpgid_stat[lp + 1..rp].to_string());
            }
        }

        Ok("".to_string())
    }
}

#[interface(name = "play.mii.Boxxy.Agent")]
impl BoxxyAgent {
    /// Return the preferred login shell for the current user.
    async fn get_preferred_shell(&self) -> fdo::Result<String> {
        if let Ok(Some(user)) = User::from_uid(getuid()) {
            let shell = user.shell.to_string_lossy().to_string();
            if !shell.is_empty() {
                return Ok(shell);
            }
        }
        // Fallback to $SHELL or /bin/sh
        Ok(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()))
    }

    /// Create a new PTY master/slave pair and return the master FD.
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
        let mut lock = self.tracked_pids.write().await;
        if enabled {
            lock.insert(pid);
        } else {
            lock.remove(&pid);
        }
        Ok(())
    }

    /// Spawn a process on the host using the provided master PTY FD.
    async fn spawn(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        pty_master: OwnedFd,
        options: SpawnOptions,
    ) -> fdo::Result<u32> {
        // Convert zbus OwnedFd to std OwnedFd
        let std_fd: std::os::unix::io::OwnedFd = pty_master.into();
        let master_fd = unsafe { PtyMaster::from_owned_fd(std_fd) };

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

        if !options.cwd.is_empty() {
            cmd.current_dir(&options.cwd);
        }

        // When the UI sends an empty env vector (Flatpak mode) we populate a
        // minimal baseline from the host passwd entry so the shell has at least
        // HOME, USER, LOGNAME, and SHELL set correctly before sourcing its own
        // startup files (e.g. ~/.config/fish/config.fish, ~/.bashrc, etc.).
        if options.env.is_empty()
            && let Ok(Some(user)) = User::from_uid(getuid()) {
                cmd.env("HOME", user.dir.to_string_lossy().as_ref());
                cmd.env("USER", &user.name);
                cmd.env("LOGNAME", &user.name);
                cmd.env("SHELL", user.shell.to_string_lossy().as_ref());
            }

        for (key, value) in options.env {
            cmd.env(key, value);
        }

        let master_raw_fd = master_fd.as_raw_fd();

        unsafe {
            cmd.pre_exec(move || {
                // 1. Create a new session
                libc::setsid();

                // 2. Open the slave PTY
                let slave_fd = libc::open(
                    std::ffi::CString::new(slave_name.clone()).unwrap().as_ptr(),
                    libc::O_RDWR,
                );
                if slave_fd == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                // 3. Set the controlling terminal
                if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }

                // 4. Dup slave to stdin, stdout, stderr
                libc::dup2(slave_fd, libc::STDIN_FILENO);
                libc::dup2(slave_fd, libc::STDOUT_FILENO);
                libc::dup2(slave_fd, libc::STDERR_FILENO);

                // Close original master and slave FDs
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

        let emitter = emitter.to_owned();
        let tracked_pids = self.tracked_pids.clone();

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
                        let _ = BoxxyAgent::exited(&emitter, pid, exit_code).await;
                        tracked_pids.write().await.remove(&pid);
                        break;
                    }
                    _ = interval.tick() => {
                        let is_tracked = tracked_pids.read().await.contains(&pid);
                        if is_tracked {
                            if let Ok(current_process) = Self::get_foreground_process_internal(pid)
                                && current_process != last_process_name {
                                    let _ = BoxxyAgent::foreground_process_changed(&emitter, pid, current_process.clone()).await;
                                    last_process_name = current_process;
                                }
                        } else if !last_process_name.is_empty() {
                            // If tracking is disabled, reset the last known state so UI clears it
                            last_process_name = String::new();
                            let _ = BoxxyAgent::foreground_process_changed(&emitter, pid, String::new()).await;
                        }
                    }
                }
            }
        });

        Ok(pid)
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
                if let Ok(file_name) = entry.file_name().into_string()
                    && let Ok(p) = file_name.parse::<u32>() {
                        let stat_path = format!("/proc/{}/stat", p);
                        if let Ok(stat_content) = std::fs::read_to_string(&stat_path) {
                            let lp_pos = stat_content.find('(');
                            let rp_pos = stat_content.rfind(')');
                            if let (Some(lp), Some(rp)) = (lp_pos, rp_pos)
                                && rp > lp {
                                    let name = stat_content[lp + 1..rp].to_string();
                                    let rest = &stat_content[rp + 2..];
                                    let parts: Vec<&str> = rest.split_whitespace().collect();
                                    if parts.len() > 1
                                        && let Ok(ppid) = parts[1].parse::<u32>() {
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

        let mut descendants = Vec::new();
        let mut to_visit = vec![pid];

        while let Some(current) = to_visit.pop() {
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
        unsafe {
            if libc::kill(-(pid as i32), signal) != 0 {
                return Err(fdo::Error::Failed("kill failed".to_string()));
            }
        }
        Ok(())
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
