use crate::ipc::connect_agent_proxy;
use anyhow::Result;
use log::{info, warn};
use zbus::Connection;

pub const AGENT_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("GIT_HASH"));

#[derive(Debug)]
pub enum ClaimResult {
    /// We are the first — proceed to start the daemon loop.
    Claimed,
    /// An existing daemon with the same or newer version is running.
    HandedOff,
    /// We triggered the existing daemon to update itself; it will restart.
    Upgraded,
}

pub async fn try_claim_or_handoff() -> Result<ClaimResult> {
    let conn = Connection::session().await?;

    // Try to claim the well-known name with "do not queue" semantics.
    use zbus::fdo::DBusProxy;
    let dbus = DBusProxy::new(&conn).await?;

    let flags = zbus::fdo::RequestNameFlags::DO_NOT_QUEUE;
    let bus_name = boxxy_sys_utils::agent_bus_name();
    let name: zbus::names::WellKnownName<'_> = bus_name.try_into()?;
    let reply = dbus.request_name(name, flags).await?;

    use zbus::fdo::RequestNameReply::*;
    match reply {
        PrimaryOwner => Ok(ClaimResult::Claimed),

        Exists | AlreadyOwner | InQueue => {
            // Ghost is running — do a version check.
            match connect_agent_proxy(&conn).await {
                Ok(proxy) => {
                    let running_ver = proxy.get_version().await.unwrap_or_default();
                    if running_ver == AGENT_VERSION {
                        info!("Ghost version matches ({running_ver}) — standing down");
                        Ok(ClaimResult::HandedOff)
                    } else {
                        warn!("Ghost version mismatch: running={running_ver} new={AGENT_VERSION}");
                        // Ask the Ghost to reload itself from the newly-deployed binary.
                        let _ = proxy.request_reload().await;
                        Ok(ClaimResult::Upgraded)
                    }
                }
                Err(e) => {
                    warn!("Could not reach Ghost for version check: {e}");
                    Ok(ClaimResult::HandedOff)
                }
            }
        }
    }
}
