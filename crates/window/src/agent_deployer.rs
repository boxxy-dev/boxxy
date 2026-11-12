//! Called once at startup (and on every app foreground) to ensure the
//! host-side boxxy-agent binary is present and matches our version.

use anyhow::{Context, Result};
use boxxy_sys_utils::is_flatpak;
use log::{info, warn};
use std::path::PathBuf;
use tokio::process::Command;
use zbus::{Connection, proxy};

const AGENT_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("GIT_HASH"));

/// Proxy pointing at the running Ghost's version interface.
#[proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/Agent"
)]
trait AgentVersion {
    async fn get_version(&self) -> zbus::Result<String>;
    async fn get_config_dir(&self) -> zbus::Result<String>;
    async fn request_stop(&self) -> zbus::Result<()>;
}

async fn connect_version_proxy(conn: &Connection) -> zbus::Result<AgentVersionProxy<'static>> {
    AgentVersionProxy::builder(conn)
        .destination(boxxy_sys_utils::agent_bus_name())
        .build()
        .await
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
            let _ = stop_ghost().await;
            wait_for_agent_stopped().await;
            deploy_agent_binary(&host_path).await?;
        }
    }

    // Step 2 — Ensure daemon is running. Returns `true` if a new daemon was spawned.
    let spawned = spawn_agent(&host_path).await?;

    // Step 3 — Wait for D-Bus name to be ready when a new daemon was spawned.
    if spawned {
        wait_for_agent_ready().await?;
    }

    Ok(())
}

async fn wait_for_agent_stopped() {
    let Ok(conn) = Connection::session().await else {
        return;
    };
    let Ok(proxy) = connect_version_proxy(&conn).await else {
        return;
    };
    for _ in 0..40 {
        if proxy.get_version().await.is_err() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
}

async fn wait_for_agent_ready() -> Result<()> {
    let conn = Connection::session().await?;
    let proxy = connect_version_proxy(&conn).await?;
    let mut attempts = 0;
    let max_attempts = 200; // 3 seconds total at 15ms intervals

    loop {
        match proxy.get_version().await {
            Ok(ver) => {
                info!("Agent is ready (v{})", ver);
                return Ok(());
            }
            Err(e) => {
                attempts += 1;
                if attempts >= max_attempts {
                    anyhow::bail!("Timed out waiting for agent to claim D-Bus name: {}", e);
                }
                if attempts % 20 == 0 {
                    info!(
                        "Waiting for agent D-Bus name (attempt {}/{})...",
                        attempts, max_attempts
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            }
        }
    }
}

async fn needs_redeploy(host_path: &PathBuf) -> bool {
    if !host_path.exists() {
        return true;
    }

    // Compare the packaged `/app/libexec/boxxy-agent` binary against the deployed
    // host binary so Flatpak rebuilds with the same CARGO_PKG_VERSION always redeploy.
    let src_path = PathBuf::from("/app/libexec/boxxy-agent");
    if src_path.exists()
        && let (Ok(src_bytes), Ok(dst_bytes)) = (
            tokio::fs::read(&src_path).await,
            tokio::fs::read(host_path).await,
        )
        && src_bytes != dst_bytes
    {
        info!("Host boxxy-agent binary differs from /app/libexec/boxxy-agent — redeploying");
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
    let proxy = connect_version_proxy(&conn).await?;
    let ver = proxy.get_version().await?;
    Ok(ver)
}

async fn query_ghost_version_and_config_dir() -> Result<(String, String)> {
    let conn = Connection::session().await?;
    let proxy = connect_version_proxy(&conn).await?;
    let (ver, dir) = tokio::try_join!(proxy.get_version(), proxy.get_config_dir())?;
    Ok((ver, dir))
}

async fn deploy_agent_binary(dest: &PathBuf) -> Result<()> {
    info!("Deploying agent binary to {}", dest.display());

    let src_path = PathBuf::from("/app/libexec/boxxy-agent");
    if !src_path.exists() {
        anyhow::bail!("Source agent binary not found at {}", src_path.display());
    }

    // Create the destination directory on the host if it doesn't exist.
    let parent = dest.parent().context("no parent dir")?;

    let mkdir_status = Command::new("flatpak-spawn")
        .args(["--host", "mkdir", "-p", &parent.to_string_lossy()])
        .status()
        .await?;
    if !mkdir_status.success() {
        anyhow::bail!(
            "Failed to create host directory {} (exit: {})",
            parent.display(),
            mkdir_status
        );
    }

    // Write via flatpak-spawn --host to escape the sandbox.
    let tmp = dest.with_extension("tmp");
    let mut child = Command::new("flatpak-spawn")
        .args(["--host", "tee", &tmp.to_string_lossy()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let file = tokio::fs::File::open(&src_path).await?;
        let mut reader = tokio::io::BufReader::with_capacity(64 * 1024, file);
        tokio::io::copy_buf(&mut reader, &mut stdin).await?;
        stdin.shutdown().await?;
    }

    let tee_status = child.wait().await?;
    if !tee_status.success() {
        anyhow::bail!(
            "Failed to write temporary agent binary {} (exit: {})",
            tmp.display(),
            tee_status
        );
    }

    let chmod_status = Command::new("flatpak-spawn")
        .args(["--host", "chmod", "+x", &tmp.to_string_lossy()])
        .status()
        .await?;
    if !chmod_status.success() {
        anyhow::bail!(
            "Failed to chmod agent binary {} (exit: {})",
            tmp.display(),
            chmod_status
        );
    }

    let mv_status = Command::new("flatpak-spawn")
        .args([
            "--host",
            "mv",
            "-f",
            &tmp.to_string_lossy(),
            &dest.to_string_lossy(),
        ])
        .status()
        .await?;
    if !mv_status.success() {
        anyhow::bail!(
            "Failed to move agent binary into {} (exit: {})",
            dest.display(),
            mv_status
        );
    }

    Ok(())
}

async fn spawn_agent(host_path: &PathBuf) -> Result<bool> {
    let expected_config_dir = boxxy_sys_utils::get_config_dir();

    // Check if it's already running with matching version and config dir in one round-trip.
    match query_ghost_version_and_config_dir().await {
        Ok((ver, dir)) if ver == AGENT_VERSION && PathBuf::from(&dir) == expected_config_dir => {
            info!("Agent v{} with matching config dir is already running.", ver);
            return Ok(false);
        }
        Ok((ver, dir)) => {
            if ver != AGENT_VERSION {
                warn!(
                    "Ghost version {} != app version {} — stopping old agent",
                    ver, AGENT_VERSION
                );
            } else {
                warn!(
                    "Ghost config dir ({}) != app config dir ({:?}) — stopping old agent",
                    dir, expected_config_dir
                );
            }
            let _ = stop_ghost().await;
            wait_for_agent_stopped().await;
        }
        Err(_) => {
            info!("Agent is not running; will spawn.");
        }
    }

    let daemon_env = boxxy_sys_utils::env::daemon_spawn_env(&expected_config_dir);

    if is_flatpak() {
        info!("Spawning agent on host: {}", host_path.display());
        let mut cmd = Command::new("flatpak-spawn");
        for (k, v) in &daemon_env {
            cmd.arg(format!("--env={k}={v}"));
        }
        cmd.args([
            "--host".to_string(),
            host_path.to_string_lossy().into_owned(),
            "--background".to_string(),
        ]);
        cmd.spawn()
            .context("Failed to spawn agent on host")?;
    } else {
        info!(
            "Spawning native agent ({}): {}",
            boxxy_sys_utils::agent_bus_name(),
            host_path.display()
        );
        let mut cmd = Command::new(host_path);
        for (k, v) in &daemon_env {
            cmd.env(k, v);
        }
        cmd.arg("--background")
            .spawn()
            .context("Failed to spawn native agent")?;
    }
    Ok(true)
}

async fn stop_ghost() -> Result<()> {
    let conn = Connection::session().await?;
    let proxy = connect_version_proxy(&conn).await?;
    let _ = proxy.request_stop().await;
    Ok(())
}
