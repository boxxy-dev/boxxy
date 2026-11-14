//! Called once at startup (and on agent reconnect) to ensure the
//! host-side `boxxy-agent` binary is present and matches our version.

use anyhow::{Context, Result};
use boxxy_sys_utils::is_flatpak;
use log::{info, warn};
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
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
fn agent_host_path() -> Result<PathBuf> {
    if is_flatpak() {
        let home = home::home_dir().context("Could not resolve user home directory")?;
        Ok(home.join(".local/boxxy-terminal/boxxy-agent"))
    } else {
        // Native: look in the same directory as the current executable
        let mut p = std::env::current_exe().context("Could not resolve current executable path")?;
        p.pop();
        p.push("boxxy-agent");
        Ok(p)
    }
}

/// Full handshake:
/// 1. Deploy agent binary if missing or outdated (Flatpak only).
/// 2. Spawn agent if not already running with matching version & config dir.
/// 3. Wait for the agent to claim its D-Bus name.
pub async fn ensure_agent_running() -> Result<()> {
    let host_path = agent_host_path()?;
    let conn = Connection::session().await.ok();

    if is_flatpak() && needs_redeploy(conn.as_ref(), &host_path).await {
        if let Some(ref c) = conn {
            let _ = stop_ghost(c).await;
            wait_for_agent_stopped(c).await;
        }
        deploy_agent_binary(&host_path).await?;
    }

    // Step 2 — Ensure daemon is running. Returns `true` if a new daemon was spawned.
    let spawned = spawn_agent(conn.as_ref(), &host_path).await?;

    // Step 3 — Wait for D-Bus name to be ready when a new daemon was spawned.
    if spawned {
        let ready_conn = match conn {
            Some(c) => c,
            None => Connection::session().await?,
        };
        wait_for_agent_ready(&ready_conn).await?;
    }

    Ok(())
}

async fn wait_for_agent_stopped(conn: &Connection) {
    let Ok(proxy) = connect_version_proxy(conn).await else {
        return;
    };
    for _ in 0..40 {
        if proxy.get_version().await.is_err() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
}

async fn wait_for_agent_ready(conn: &Connection) -> Result<()> {
    let proxy = connect_version_proxy(conn).await?;
    let mut attempts = 0;
    let max_attempts = 200; // 3 seconds total at 15ms intervals

    loop {
        match proxy.get_version().await {
            Ok(ver) => {
                info!("Agent is ready (v{ver})");
                return Ok(());
            }
            Err(e) => {
                attempts += 1;
                if attempts >= max_attempts {
                    anyhow::bail!("Timed out waiting for agent to claim D-Bus name: {e}");
                }
                if attempts % 20 == 0 {
                    info!("Waiting for agent D-Bus name (attempt {attempts}/{max_attempts})...");
                }
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            }
        }
    }
}

/// Compares two files without allocating full binary buffers in memory.
/// Checks byte lengths via `metadata` first, then streams in 64 KiB chunks.
async fn files_identical(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (meta_a, meta_b) = tokio::try_join!(tokio::fs::metadata(a), tokio::fs::metadata(b))?;
    if meta_a.len() != meta_b.len() {
        return Ok(false);
    }

    let (file_a, file_b) = tokio::try_join!(tokio::fs::File::open(a), tokio::fs::File::open(b))?;
    let mut reader_a = tokio::io::BufReader::with_capacity(64 * 1024, file_a);
    let mut reader_b = tokio::io::BufReader::with_capacity(64 * 1024, file_b);
    let mut buf_a = [0u8; 8192];
    let mut buf_b = [0u8; 8192];

    loop {
        let n_a = reader_a.read(&mut buf_a).await?;
        if n_a == 0 {
            let n_b = reader_b.read(&mut buf_b).await?;
            return Ok(n_b == 0);
        }
        reader_b.read_exact(&mut buf_b[..n_a]).await?;
        if buf_a[..n_a] != buf_b[..n_a] {
            return Ok(false);
        }
    }
}

async fn needs_redeploy(conn: Option<&Connection>, host_path: &Path) -> bool {
    if !tokio::fs::try_exists(host_path).await.unwrap_or(false) {
        return true;
    }

    // Compare the packaged `/app/libexec/boxxy-agent` binary against the deployed
    // host binary so Flatpak rebuilds with the same CARGO_PKG_VERSION always redeploy,
    // while avoiding a redundant copy when the on-disk binary already matches.
    let src_path = Path::new("/app/libexec/boxxy-agent");
    if tokio::fs::try_exists(src_path).await.unwrap_or(false) {
        match files_identical(src_path, host_path).await {
            Ok(true) => return false,
            Ok(false) => {
                info!(
                    "Host boxxy-agent binary differs from /app/libexec/boxxy-agent — redeploying"
                );
                return true;
            }
            Err(_) => {}
        }
    }

    // Fallback when host filesystem isn't directly readable from the sandbox:
    // ask the running Ghost its version via D-Bus.
    let Some(conn) = conn else {
        return true;
    };
    match query_ghost_version(conn).await {
        Ok(ver) if ver == AGENT_VERSION => false,
        Ok(ver) => {
            warn!("Ghost version {ver} != app version {AGENT_VERSION} — redeploying");
            true
        }
        Err(_) => true,
    }
}

async fn query_ghost_version(conn: &Connection) -> Result<String> {
    let proxy = connect_version_proxy(conn).await?;
    let ver = proxy.get_version().await?;
    Ok(ver)
}

async fn query_ghost_version_and_config_dir(conn: &Connection) -> Result<(String, String)> {
    let proxy = connect_version_proxy(conn).await?;
    let (ver, dir) = tokio::try_join!(proxy.get_version(), proxy.get_config_dir())?;
    Ok((ver, dir))
}

async fn deploy_agent_binary(dest: &Path) -> Result<()> {
    info!("Deploying agent binary to {}", dest.display());

    let src_path = Path::new("/app/libexec/boxxy-agent");
    if !tokio::fs::try_exists(src_path).await.unwrap_or(false) {
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
        let file = tokio::fs::File::open(src_path).await?;
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

async fn spawn_agent(conn: Option<&Connection>, host_path: &Path) -> Result<bool> {
    let expected_config_dir = boxxy_sys_utils::get_config_dir();

    // Check if it's already running with matching version and config dir in one round-trip.
    if let Some(conn) = conn {
        match query_ghost_version_and_config_dir(conn).await {
            Ok((ver, dir)) if ver == AGENT_VERSION && Path::new(&dir) == expected_config_dir => {
                info!("Agent v{ver} with matching config dir is already running.");
                return Ok(false);
            }
            Ok((ver, dir)) => {
                if ver != AGENT_VERSION {
                    warn!(
                        "Ghost version {ver} != app version {AGENT_VERSION} — stopping old agent"
                    );
                } else {
                    warn!(
                        "Ghost config dir ({dir}) != app config dir ({expected_config_dir:?}) — stopping old agent"
                    );
                }
                let _ = stop_ghost(conn).await;
                wait_for_agent_stopped(conn).await;
            }
            Err(_) => {
                info!("Agent is not running; will spawn.");
            }
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
        cmd.spawn().context("Failed to spawn agent on host")?;
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

async fn stop_ghost(conn: &Connection) -> Result<()> {
    let proxy = connect_version_proxy(conn).await?;
    let _ = proxy.request_stop().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn files_identical_checks_length_then_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.bin");
        let b = tmp.path().join("b.bin");
        let c = tmp.path().join("c.bin");

        let payload = vec![0x42u8; 20_000];
        tokio::fs::write(&a, &payload).await.unwrap();
        tokio::fs::write(&b, &payload).await.unwrap();

        let mut modified = payload;
        modified[19_999] = 0x99;
        tokio::fs::write(&c, &modified).await.unwrap();

        assert!(files_identical(&a, &b).await.unwrap());
        assert!(!files_identical(&a, &c).await.unwrap());
    }
}
