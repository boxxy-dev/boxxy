//! Called once at startup (and on every app foreground) to ensure the
//! host-side boxxy-agent binary is present and matches our version.

use anyhow::{Context, Result};
use boxxy_sys_utils::is_flatpak;
use log::{info, warn};
use std::path::PathBuf;
use tokio::process::Command;
use zbus::{Connection, proxy};

const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const WELL_KNOWN_NAME: &str = "dev.boxxy.BoxxyAgent";

/// Proxy pointing at the running Ghost's version interface.
#[proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/Agent"
)]
trait AgentVersion {
    async fn get_version(&self) -> zbus::Result<String>;
    async fn request_stop(&self) -> zbus::Result<()>;
}

/// Returns the path where the agent binary lives on the host.
fn agent_host_path() -> PathBuf {
    if is_flatpak() {
        home::home_dir()
            .expect("no home dir")
            .join(".local/boxxy-terminal/boxxy-agent")
    } else {
        // Native: look in the same directory as the current executable
        let mut p = std::env::current_exe().expect("no current exe");
        p.pop();
        p.push("boxxy-agent");
        p
    }
}

/// Full handshake:
/// 1. Deploy agent binary if missing or outdated (Flatpak only).
/// 2. Spawn agent if not already running.
/// 3. Wait for the agent to claim its D-Bus name.
pub async fn ensure_agent_running() -> Result<()> {
    let host_path = agent_host_path();

    if is_flatpak() {
        // Step 1 — Re-deploy binary if needed (only in Flatpak).
        if needs_redeploy(&host_path).await {
            deploy_agent_binary(&host_path).await?;
        }
    }

    // Step 2 — Ensure daemon is running.
    spawn_agent(&host_path).await?;

    // Step 3 — Wait for D-Bus name to be ready.
    wait_for_agent_ready().await?;

    Ok(())
}

async fn wait_for_agent_ready() -> Result<()> {
    let mut attempts = 0;
    let max_attempts = 10;

    loop {
        match query_ghost_version().await {
            Ok(ver) => {
                info!("Agent is ready (v{})", ver);
                return Ok(());
            }
            Err(e) => {
                attempts += 1;
                if attempts >= max_attempts {
                    anyhow::bail!("Timed out waiting for agent to claim D-Bus name: {}", e);
                }
                info!(
                    "Waiting for agent D-Bus name (attempt {}/{})...",
                    attempts, max_attempts
                );
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }
    }
}

async fn needs_redeploy(host_path: &PathBuf) -> bool {
    if !host_path.exists() {
        return true;
    }

    // Ask the running Ghost its version via D-Bus.
    match query_ghost_version().await {
        Ok(ver) if ver == AGENT_VERSION => false,
        Ok(ver) => {
            warn!(
                "Ghost version {} != app version {} — redeploying",
                ver, AGENT_VERSION
            );
            true
        }
        Err(_) => true,
    }
}

async fn query_ghost_version() -> Result<String> {
    let conn = Connection::session().await?;
    let proxy = AgentVersionProxy::new(&conn).await?;
    let ver = proxy.get_version().await?;
    Ok(ver)
}

async fn deploy_agent_binary(dest: &PathBuf) -> Result<()> {
    info!("Deploying agent binary to {}", dest.display());

    let src_path = PathBuf::from("/app/libexec/boxxy-agent");
    if !src_path.exists() {
        anyhow::bail!("Source agent binary not found at {}", src_path.display());
    }

    // Create the destination directory on the host if it doesn't exist.
    let parent = dest.parent().context("no parent dir")?;

    Command::new("flatpak-spawn")
        .args(["--host", "mkdir", "-p", &parent.to_string_lossy()])
        .status()
        .await?;

    // Write via flatpak-spawn --host to escape the sandbox.
    let tmp = dest.with_extension("tmp");
    let mut child = Command::new("flatpak-spawn")
        .args(["--host", "tee", &tmp.to_string_lossy()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        let mut file = tokio::fs::File::open(&src_path).await?;
        tokio::io::copy(&mut file, &mut stdin).await?;
    }

    child.wait().await?;

    Command::new("flatpak-spawn")
        .args(["--host", "chmod", "+x", &tmp.to_string_lossy()])
        .status()
        .await?;

    Command::new("flatpak-spawn")
        .args([
            "--host",
            "mv",
            "-f",
            &tmp.to_string_lossy(),
            &dest.to_string_lossy(),
        ])
        .status()
        .await?;

    Ok(())
}

async fn spawn_agent(host_path: &PathBuf) -> Result<()> {
    // Check if it's already running.
    match query_ghost_version().await {
        Ok(ver) if ver == AGENT_VERSION => {
            info!("Agent v{} is already running.", ver);
            return Ok(());
        }
        Ok(ver) => {
            warn!(
                "Ghost version {} != app version {} — stopping old agent",
                ver, AGENT_VERSION
            );
            let _ = stop_ghost().await;
            // Give it a moment to exit
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(_) => {
            info!("Agent is not running; will spawn.");
        }
    }

    if is_flatpak() {
        info!("Spawning agent on host: {}", host_path.display());
        let mut cmd = Command::new("flatpak-spawn");
        if let Ok(val) = std::env::var("BOXXY_DEBUG_CONTEXT") {
            cmd.args(["--env=BOXXY_DEBUG_CONTEXT=".to_string() + &val]);
        }
        cmd.args(["--host", &host_path.to_string_lossy(), "--background"])
            .spawn()
            .context("Failed to spawn agent on host")?;
    } else {
        info!("Spawning native agent: {}", host_path.display());
        let mut cmd = Command::new(host_path);
        if let Ok(val) = std::env::var("BOXXY_DEBUG_CONTEXT") {
            cmd.env("BOXXY_DEBUG_CONTEXT", val);
        }
        cmd.arg("--background")
            .spawn()
            .context("Failed to spawn native agent")?;
    }
    Ok(())
}

async fn stop_ghost() -> Result<()> {
    let conn = Connection::session().await?;
    let _proxy = AgentVersionProxy::new(&conn).await?;
    let _ = _proxy.request_stop().await;
    Ok(())
}
