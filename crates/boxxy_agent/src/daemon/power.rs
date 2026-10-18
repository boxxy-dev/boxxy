//! UPower client: daemon-wide "am I on battery?" signal.
//!
//! Background work (Dreaming, indexing, anything expensive) pauses
//! while the laptop is on battery so we don't trade kibibytes of
//! memory consolidation for unwanted battery drain.
//!
//! Fallback: if UPower isn't running (some minimal desktops, most
//! containers, servers), we default to `on_battery = false` — the safe
//! behaviour is "let work run", not "refuse to work because we can't
//! tell". The user can still disable auto-dreaming in Preferences.

use anyhow::Result;
use std::collections::HashMap;
use tokio::sync::watch;
use zbus::{Connection, OwnedVariant};

#[zbus::proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.DBus.Properties",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait Properties {
    #[zbus(signal)]
    fn properties_changed(
        &self,
        interface_name: String,
        changed_properties: HashMap<String, OwnedVariant>,
        invalidated_properties: Vec<String>,
    ) -> zbus::Result<()>;
}

/// Observer handle exposing a cheap "are we on battery?" check.
///
/// Cloning is cheap (just an Arc-backed `watch::Receiver`). All clones
/// see the same underlying value, so `DaemonCore` builds one at startup,
/// and any subsystem that later clones it observes live updates.
#[derive(Clone)]
pub struct PowerMonitor {
    rx: watch::Receiver<bool>,
}

impl PowerMonitor {
    pub fn is_on_battery(&self) -> bool {
        *self.rx.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }
}

/// Constructs an AC-defaulted monitor + its companion sender. The
/// sender is passed to `start()` (or left dormant in test / no-UPower
/// environments). Receivers cloned from the monitor see whatever the
/// sender publishes, no matter when they were cloned.
pub fn channel() -> (watch::Sender<bool>, PowerMonitor) {
    let (tx, rx) = watch::channel(false);
    (tx, PowerMonitor { rx })
}

/// Fallback: a monitor whose sender is immediately dropped, so the
/// `on_battery` value is permanently `false`. Tests and non-UPower
/// environments use this.
pub fn ac_only() -> PowerMonitor {
    let (_tx, rx) = watch::channel(false);
    PowerMonitor { rx }
}

/// Subscribes to UPower and pushes `OnBattery` changes into `tx`. If
/// UPower is unreachable (container, minimal desktop) this returns
/// silently; the monitor stays at `false` and we let work proceed.
pub async fn start(conn: &Connection, tx: watch::Sender<bool>) -> Result<()> {
    let upower = match UPowerProxy::new(conn).await {
        Ok(p) => p,
        Err(e) => {
            log::info!(
                "power: UPower not reachable ({}); assuming AC always — auto-dreaming stays enabled",
                e
            );
            return Ok(());
        }
    };

    match upower.on_battery().await {
        Ok(b) => {
            let _ = tx.send(b);
            log::info!("power: initial state on_battery={}", b);
        }
        Err(e) => log::warn!("power: OnBattery read failed: {}", e),
    }

    let props = match PropertiesProxy::new(conn).await {
        Ok(p) => p,
        Err(e) => {
            log::warn!("power: couldn't subscribe to PropertiesChanged: {}", e);
            return Ok(());
        }
    };

    let upower_for_task = upower;
    tokio::spawn(async move {
        use futures_util::StreamExt;
        let mut stream = match props.receive_properties_changed().await {
            Ok(s) => s,
            Err(e) => {
                log::warn!("power: receive_properties_changed failed: {}", e);
                return;
            }
        };

        while let Some(signal) = stream.next().await {
            let args = match signal.args() {
                Ok(a) => a,
                Err(_) => continue,
            };
            if args.interface_name != "org.freedesktop.UPower" {
                continue;
            }
            // Re-read authoritatively; the changed-props map may or may
            // not include `OnBattery`, and the property is cheap to fetch.
            if let Ok(b) = upower_for_task.on_battery().await {
                if *tx.borrow() != b {
                    log::info!("power: transition on_battery={}", b);
                    let _ = tx.send(b);
                }
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_only_reports_not_on_battery() {
        assert!(!ac_only().is_on_battery());
    }

    #[tokio::test]
    async fn ac_only_receiver_stays_false() {
        let monitor = ac_only();
        let mut rx = monitor.subscribe();
        // No sender is pushing anything; the initial value must be
        // `false` and remain so.
        assert!(!*rx.borrow_and_update());
    }
}
