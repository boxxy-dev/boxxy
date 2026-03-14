use anyhow::{Context, Result};
use tokio::net::UnixStream;
use zbus::connection::Builder;
use boxxy_agent::ipc::{SpawnOptions, AgentProxy, AgentClawProxy};
use zbus::zvariant::OwnedFd;

#[derive(Clone)]
pub struct AgentManager {
    proxy: AgentProxy<'static>,
    claw_proxy: AgentClawProxy<'static>,
}

impl AgentManager {
    pub async fn new() -> Result<Self> {
        if crate::is_flatpak() {
            // Try host agent first
            let (local_stream, remote_stream) = tokio::net::UnixStream::pair()?;
            log::info!("Attempting to spawn host agent...");
            if let Err(e) = Self::spawn_agent_on_host(&remote_stream).await {
                log::warn!("Failed to spawn host agent: {:?}. Will try sandbox fallback.", e);
            } else {
                log::info!("Host agent spawned, waiting for connection...");
            }
            
            let host_proxy = match tokio::time::timeout(
                std::time::Duration::from_millis(1500),
                Self::connect_to_proxy(local_stream)
            ).await {
                Ok(Ok(proxies)) => {
                    log::info!("Host agent connected successfully.");
                    Some(proxies)
                }
                Ok(Err(e)) => {
                    log::warn!("Host agent connection failed: {:?}", e);
                    None
                }
                Err(e) => {
                    log::warn!("Host agent connection timed out: {}", e);
                    None
                }
            };

            drop(remote_stream); // Now safe to drop

            if let Some((proxy, claw_proxy)) = host_proxy {
                return Ok(Self { proxy, claw_proxy });
            }

            log::warn!("Starting sandbox fallback agent...");
            let (local_stream, remote_stream) = tokio::net::UnixStream::pair()?;
            if let Err(e) = Self::spawn_agent_in_sandbox(&remote_stream) {
                log::error!("Failed to spawn sandbox fallback agent: {:?}", e);
            }
            drop(remote_stream);

            let (proxy, claw_proxy) = tokio::time::timeout(
                std::time::Duration::from_millis(1500),
                Self::connect_to_proxy(local_stream)
            ).await
            .context("Sandbox fallback agent connection timed out")??;
            
            log::info!("Sandbox fallback agent connected.");
            Ok(Self { proxy, claw_proxy })
        } else {
            let (local_stream, remote_stream) = tokio::net::UnixStream::pair()?;
            Self::spawn_agent_native(&remote_stream)?;
            drop(remote_stream);

            let (proxy, claw_proxy) = tokio::time::timeout(
                std::time::Duration::from_millis(2000),
                Self::connect_to_proxy(local_stream)
            ).await
            .context("Native agent connection timed out")??;

            Ok(Self { proxy, claw_proxy })
        }
    }

    async fn connect_to_proxy(stream: UnixStream) -> Result<(AgentProxy<'static>, AgentClawProxy<'static>)> {
        let guid = zbus::Guid::generate();
        let connection = Builder::unix_stream(stream)
            .p2p()
            .server(guid)?
            .build()
            .await
            .context("Failed to establish P2P connection to agent")?;

        let proxy = AgentProxy::builder(&connection)
            .destination("play.mii.Boxxy.Agent")?
            .build()
            .await
            .context("Failed to create AgentProxy")?;
            
        let claw_proxy = AgentClawProxy::builder(&connection)
            .destination("play.mii.Boxxy.AgentClaw")?
            .build()
            .await
            .context("Failed to create AgentClawProxy")?;
            
        Ok((proxy, claw_proxy))
    }

    /// Resolve the on-host path to boxxy-agent from /.flatpak-info.
    fn find_agent_host_path() -> String {
        let mut path = "/app/libexec/boxxy-agent".to_string();
        if let Ok(contents) = std::fs::read_to_string("/.flatpak-info") {
            for line in contents.lines() {
                if let Some(app_path) = line.strip_prefix("app-path=") {
                    path = format!("{}/libexec/boxxy-agent", app_path);
                    break;
                }
            }
        }
        log::info!("Resolved host agent path: {}", path);
        path
    }

    /// Spawn boxxy-agent on the HOST via `flatpak-spawn --host`.
    ///
    /// The agent binary is passed the inherited socket FD.
    async fn spawn_agent_on_host(remote_stream: &UnixStream) -> Result<()> {
        use std::os::unix::io::AsRawFd;
        let fd = remote_stream.as_raw_fd();
        let agent_path = Self::find_agent_host_path();
        log::info!("Spawning host agent: {} via FD 3 (mapped from {})", agent_path, fd);

        let mut cmd = std::process::Command::new("flatpak-spawn");
        cmd.args([
            "--host",
            "--watch-bus",
            "--forward-fd=3",
            &agent_path,
            "--socket-fd=3"
        ]);
        cmd.stderr(std::process::Stdio::piped());

        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(move || {
                // Map the socket FD to 3 so flatpak-spawn/agent can find it easily.
                if libc::dup2(fd, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Important: if fd was already 3, dup2 does nothing and FD_CLOEXEC remains set!
                // We MUST explicitly clear FD_CLOEXEC on FD 3 so flatpak-spawn inherits it.
                let flags = libc::fcntl(3, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn().context("flatpak-spawn --host failed")?;

        // Capture flatpak-spawn's stderr in the background so errors are visible in logs.
        if let Some(stderr) = child.stderr.take() {
            tokio::task::spawn_blocking(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::warn!("[flatpak-spawn] {}", line);
                }
            });
        }

        Ok(())
    }

    /// Sandbox fallback: run boxxy-agent inside the Flatpak when host execution fails.
    fn spawn_agent_in_sandbox(remote_stream: &UnixStream) -> Result<()> {
        use std::os::unix::io::AsRawFd;
        let fd = remote_stream.as_raw_fd();
        let agent_path = "/app/libexec/boxxy-agent";

        if !std::path::Path::new(agent_path).exists() {
            anyhow::bail!("Sandbox agent executable not found at {}", agent_path);
        }

        let mut cmd = std::process::Command::new(agent_path);
        cmd.arg("--socket-fd=3");

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(move || {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    if libc::dup2(fd, 3) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let flags = libc::fcntl(3, libc::F_GETFD);
                    if flags >= 0 {
                        libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                    }
                    Ok(())
                });
            }
        }

        cmd.spawn().context("Failed to spawn sandbox agent")?;
        Ok(())
    }

    /// Native build: spawn agent binary from the same directory as the UI.
    fn spawn_agent_native(remote_stream: &UnixStream) -> Result<()> {
        use std::os::unix::io::AsRawFd;
        let fd = remote_stream.as_raw_fd();
        let mut agent_path = std::env::current_exe()?;
        agent_path.pop();
        agent_path.push("boxxy-agent");

        if !agent_path.exists() {
            anyhow::bail!(
                "boxxy-agent not found at {:?}. Please ensure it is built.",
                agent_path
            );
        }

        let mut cmd = std::process::Command::new(&agent_path);
        cmd.arg("--socket-fd=3");

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(move || {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    if libc::dup2(fd, 3) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let flags = libc::fcntl(3, libc::F_GETFD);
                    if flags >= 0 {
                        libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                    }
                    Ok(())
                });
            }
        }

        cmd.spawn()
            .with_context(|| format!("Failed to spawn native agent at {:?}", agent_path))?;
        Ok(())
    }

    pub fn proxy(&self) -> &AgentProxy<'static> {
        &self.proxy
    }

    pub fn claw_proxy(&self) -> &AgentClawProxy<'static> {
        &self.claw_proxy
    }

    pub async fn get_preferred_shell(&self) -> Result<String> {
        self.proxy.get_preferred_shell().await.context("Agent get_preferred_shell failed")
    }

    pub async fn create_pty(&self) -> Result<OwnedFd> {
        self.proxy.create_pty().await.context("Agent create_pty failed")
    }

    pub async fn spawn_process(&self, pty_master: OwnedFd, options: SpawnOptions) -> Result<u32> {
        self.proxy.spawn(pty_master, options).await.context("Agent spawn failed")
    }

    pub async fn get_cwd(&self, pid: u32) -> Result<String> {
        self.proxy.get_cwd(pid).await.context("Agent get_cwd failed")
    }

    pub async fn get_foreground_process(&self, pid: u32) -> Result<String> {
        self.proxy.get_foreground_process(pid).await.context("Agent get_foreground_process failed")
    }
}

