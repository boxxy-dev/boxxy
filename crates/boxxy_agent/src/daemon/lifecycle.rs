//! Ghost Mode tracker: derives a "nobody is watching" bool from the
//! running UI client count and publishes it to any subsystem that
//! wants to throttle on it.
//!
//! When `client_count` hits zero the daemon enters Ghost Mode: no UI
//! is attached, so:
//!   * interactive tools are paused,
//!   * idle shells with persistence enabled are detached,
//!   * background maintenance resumes,
//!   * active agent tasks keep running and notify on completion.
//!
//! This module implements the signal; individual subsystems decide how
//! to react. The zombie sweeper and Dreaming both consult it.

use tokio::sync::watch;

/// Observer handle. `true` means "no UIs connected, it's safe to do
/// bulk maintenance"; `false` means "at least one UI is attached,
/// prefer responsive-ness over throughput".
#[derive(Clone)]
pub struct GhostMode {
    rx: watch::Receiver<bool>,
}

impl GhostMode {
    pub fn is_ghost(&self) -> bool {
        *self.rx.borrow()
    }

    pub fn is_closed(&self) -> bool {
        self.rx.is_closed()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }
}

/// Starts a watcher that maps `client_count_rx` → `ghost_mode` and
/// logs transitions. Returns a `GhostMode` handle that any subsystem
/// can clone.
pub fn start(mut client_count_rx: watch::Receiver<usize>) -> GhostMode {
    let initial_ghost = *client_count_rx.borrow() == 0;
    let (tx, rx) = watch::channel(initial_ghost);

    tokio::spawn(async move {
        while client_count_rx.changed().await.is_ok() {
            let count = *client_count_rx.borrow();
            let next = count == 0;
            let prev = *tx.borrow();
            if prev != next {
                log::info!(
                    "lifecycle: ghost_mode {} (client_count={})",
                    if next { "ENTERED" } else { "EXITED" },
                    count
                );
                let _ = tx.send(next);
            }
        }
    });

    GhostMode { rx }
}

/// Test helper: a `GhostMode` that always reports `false` (i.e. a UI
/// is attached). Use this in unit tests that don't want to wire a
/// whole watch channel.
pub fn ui_attached() -> GhostMode {
    let (_tx, rx) = watch::channel(false);
    GhostMode { rx }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initial_zero_count_enters_ghost_mode() {
        let (tx, rx) = watch::channel(0usize);
        let g = start(rx);
        // initial derivation happens synchronously in `start()`
        assert!(g.is_ghost());
        drop(tx);
    }

    #[tokio::test]
    async fn transitions_track_client_count() {
        let (tx, rx) = watch::channel(0usize);
        let g = start(rx);
        let mut watch_rx = g.subscribe();

        // 0 → 1 should exit ghost
        tx.send(1).unwrap();
        // give the watcher task a tick
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), watch_rx.changed())
            .await
            .unwrap();
        assert!(!g.is_ghost());

        // 1 → 0 should re-enter ghost
        tx.send(0).unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), watch_rx.changed())
            .await
            .unwrap();
        assert!(g.is_ghost());
    }

    #[test]
    fn ui_attached_helper_is_not_ghost() {
        let gm = ui_attached();
        assert!(!gm.is_ghost());
        assert!(gm.is_closed());
    }
}
