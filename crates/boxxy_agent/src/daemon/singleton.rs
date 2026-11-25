use crate::ipc::{
    connect_agent_proxy, connect_control_session, is_service_unavailable,
    wait_for_bus_name_released,
};
use anyhow::Result;
use log::{info, warn};
use std::path::Path;
use std::time::Duration;

pub const AGENT_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("GIT_HASH"));

#[derive(Debug, PartialEq, Eq)]
pub enum ClaimResult {
    /// No live daemon owns the bus name — proceed to start the daemon loop
    /// and atomically claim the name once `ObjectServer` interfaces are ready.
    Claimed,
    /// An existing daemon with the same version and config directory is running.
    HandedOff,
    /// We triggered the existing daemon to `exec()`-reload itself from disk.
    Upgraded,
}

/// Probes whether an existing `boxxy-agent` already owns [`boxxy_sys_utils::agent_bus_name()`]
/// without prematurely claiming and dropping the D-Bus name on an empty connection.
pub async fn try_claim_or_handoff() -> Result<ClaimResult> {
    let conn = connect_control_session().await?;
    let proxy = connect_agent_proxy(&conn).await?;

    match tokio::try_join!(proxy.get_version(), proxy.get_config_dir()) {
        Ok((running_ver, running_dir)) => {
            let expected_dir = boxxy_sys_utils::get_config_dir();
            if Path::new(&running_dir) != expected_dir {
                warn!(
                    "Ghost config dir ({running_dir}) != expected ({expected_dir:?}) — stopping old daemon"
                );
                let _ = proxy.request_stop().await;
                wait_for_bus_name_released(
                    &conn,
                    boxxy_sys_utils::agent_bus_name(),
                    Duration::from_secs(2),
                )
                .await;
                Ok(ClaimResult::Claimed)
            } else if running_ver == AGENT_VERSION {
                info!("Ghost version and config dir match ({running_ver}) — standing down");
                Ok(ClaimResult::HandedOff)
            } else {
                warn!("Ghost version mismatch: running={running_ver} new={AGENT_VERSION}");
                let _ = proxy.request_reload().await;
                Ok(ClaimResult::Upgraded)
            }
        }
        Err(e) if is_service_unavailable(&e) => Ok(ClaimResult::Claimed),
        Err(e) => {
            warn!("Could not reach existing Ghost for version check: {e}");
            Ok(ClaimResult::HandedOff)
        }
    }
}
