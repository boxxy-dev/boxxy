//! Shared D-Bus client operations used by both `boxxy-terminal +agent`,
//! `boxxy-terminal +doctor`, and the low-level `boxxy-agent` daemon binary.

use crate::ipc::pty::DetachedSessionInfo;
use crate::ipc::{connect_agent_proxy, connect_maintenance_proxy, connect_pty_proxy};
use anyhow::{Context, Result};
use serde::Serialize;
use zbus::Connection;

/// Snapshot of a running `boxxy-agent` daemon's status over session D-Bus.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatusReport {
    pub bus_name: String,
    pub version: String,
    pub config_dir: String,
    pub on_battery: bool,
    pub power_source: &'static str,
    pub dream_cycle: String,
    pub active_claw_panes: usize,
    pub detached_shells: usize,
}

/// Queries the active `boxxy-agent` instance on the session D-Bus.
/// Returns an error if the session bus is unreachable or no daemon owns `agent_bus_name()`.
pub async fn query_status() -> Result<DaemonStatusReport> {
    let conn = Connection::session()
        .await
        .context("Failed to connect to the session D-Bus")?;
    let bus_name = boxxy_sys_utils::agent_bus_name().to_string();
    let agent_proxy = connect_agent_proxy(&conn).await?;
    let maint_proxy = connect_maintenance_proxy(&conn).await?;
    let pty_proxy = connect_pty_proxy(&conn).await?;

    let version = agent_proxy
        .get_version()
        .await
        .with_context(|| format!("No running boxxy-agent responded on '{bus_name}'"))?;
    let config_dir = agent_proxy.get_config_dir().await.unwrap_or_default();
    let dream_cycle = maint_proxy
        .get_maintenance_status()
        .await
        .unwrap_or_else(|_| "unknown".to_string());
    let on_battery = maint_proxy.is_on_battery().await.unwrap_or(false);
    let claw_sessions = agent_proxy.list_claw_sessions().await.unwrap_or_default();
    let detached_ptys = pty_proxy
        .list_detached_sessions_detailed()
        .await
        .unwrap_or_default();

    Ok(DaemonStatusReport {
        bus_name,
        version,
        config_dir,
        on_battery,
        power_source: if on_battery { "Battery" } else { "AC" },
        dream_cycle,
        active_claw_panes: claw_sessions.len(),
        detached_shells: detached_ptys.len(),
    })
}

/// Lists all detached PTY sessions currently kept alive by `boxxy-agent`.
pub async fn query_detached_sessions() -> Result<Vec<DetachedSessionInfo>> {
    let conn = Connection::session()
        .await
        .context("Failed to connect to the session D-Bus")?;
    let proxy = connect_pty_proxy(&conn).await?;
    let bus_name = boxxy_sys_utils::agent_bus_name();
    proxy
        .list_detached_sessions_detailed()
        .await
        .with_context(|| format!("No running boxxy-agent responded on '{bus_name}'"))
}

/// Requests a graceful stop of the running `boxxy-agent` daemon (flushes telemetry first).
pub async fn request_stop() -> Result<()> {
    let conn = Connection::session()
        .await
        .context("Failed to connect to the session D-Bus")?;
    let proxy = connect_agent_proxy(&conn).await?;
    let bus_name = boxxy_sys_utils::agent_bus_name();
    proxy
        .request_stop()
        .await
        .with_context(|| format!("No running boxxy-agent responded on '{bus_name}'"))?;
    Ok(())
}

/// Requests an in-place `exec()` reload of the running `boxxy-agent` daemon.
pub async fn request_restart() -> Result<()> {
    let conn = Connection::session()
        .await
        .context("Failed to connect to the session D-Bus")?;
    let proxy = connect_agent_proxy(&conn).await?;
    let bus_name = boxxy_sys_utils::agent_bus_name();
    proxy
        .request_reload()
        .await
        .with_context(|| format!("No running boxxy-agent responded on '{bus_name}'"))?;
    Ok(())
}

/// Formats and prints [`DaemonStatusReport`] for human terminal output.
pub fn print_status_human(report: &DaemonStatusReport) {
    println!("Boxxy Agent v{} ({})", report.version, report.bus_name);
    println!("  Config Dir:        {}", report.config_dir);
    println!("  Power Source:      {}", report.power_source);
    println!("  Dream Cycle:       {}", report.dream_cycle);
    println!("  Active Claw Panes: {}", report.active_claw_panes);
    println!("  Detached Shells:   {}", report.detached_shells);
}

/// Formats and prints detached PTY sessions for human terminal output.
pub fn print_sessions_human(sessions: &[DetachedSessionInfo]) {
    if sessions.is_empty() {
        println!("No detached PTY sessions.");
        return;
    }
    println!(
        "{:>8}  {:>8}  {:<16}  {:<10}  {:<30}  PANE_ID",
        "PID", "IDLE_S", "PROCESS", "STATE", "CWD"
    );
    for s in sessions {
        let proc_label = if s.foreground_process.is_empty() {
            "(shell)"
        } else {
            s.foreground_process.as_str()
        };
        let state_label = if s.waiting_for_input {
            "INPUT_WAIT"
        } else {
            "DETACHED"
        };
        println!(
            "{:>8}  {:>8}  {:<16}  {:<10}  {:<30}  {}",
            s.pid, s.idle_secs, proc_label, state_label, s.cwd, s.pane_id
        );
    }
}
