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
use tokio::sync::watch;

#[zbus::proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
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

    pub fn is_closed(&self) -> bool {
        self.rx.is_closed()
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

/// Subscribes to UPower on the **system** D-Bus and pushes `OnBattery` changes
/// into `tx`. If the system bus or UPower is unreachable (container, minimal
/// desktop, CI) this returns silently; the monitor stays at `false` and lets
/// background work proceed.
pub async fn start(tx: watch::Sender<bool>) -> Result<()> {
    let sys_conn = match zbus::connection::Builder::system()
        .method_timeout(crate::ipc::CONTROL_METHOD_TIMEOUT)
        .build()
        .await
    {
        Ok(c) => c,
        Err(e) => {
            log::info!(
                "power: system D-Bus not reachable ({}); assuming AC always — auto-dreaming stays enabled",
                e
            );
            return Ok(());
        }
    };

    let upower = match UPowerProxy::new(&sys_conn).await {
        Ok(p) => p,
        Err(e) => {
            log::info!(
                "power: UPower proxy init failed ({}); assuming AC always — auto-dreaming stays enabled",
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
        Err(e) if crate::ipc::is_service_unavailable(&e) => {
            log::info!(
                "power: UPower service not present on system bus; assuming AC always — auto-dreaming stays enabled"
            );
            return Ok(());
        }
        Err(e) => log::warn!("power: OnBattery read failed: {}", e),
    }

    let upower_for_task = upower;
    tokio::spawn(async move {
        use futures_util::StreamExt;
        let _keep_conn = &sys_conn;
        let mut stream = upower_for_task.receive_on_battery_changed().await;

        while let Some(change) = stream.next().await {
            if sys_conn.is_closed() || tx.is_closed() {
                break;
            }
            if let Ok(b) = change.get().await
                && *tx.borrow() != b
            {
                log::info!("power: transition on_battery={}", b);
                let _ = tx.send(b);
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
        let m = ac_only();
        assert!(!m.is_on_battery());
        assert!(m.is_closed());
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
