//! Global multi-threaded Tokio runtime management.
//!
//! Initialized lazily via [`OnceLock`] so daemon processes (`boxxy-agent --background`)
//! can `fork()`/`setsid()` cleanly before any Tokio worker threads are spawned.

use std::sync::OnceLock;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Returns a reference to the global multi-threaded Tokio runtime.
///
/// Used for background tasks (I/O, D-Bus RPC, and `spawn_blocking` CPU work)
/// to keep them off the GTK main thread.
///
/// # Panics
///
/// Panics if the OS refuses to create worker threads or the async I/O driver.
#[must_use]
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        // Bound I/O events per tick to 256 (matching the per-worker local queue
        // capacity) and cap busy ticks at 64 (Tokio 1.54+) so bursts of PTY/D-Bus
        // readiness do not overflow a busy worker's local queue into the global
        // injection queue.
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_io_events_per_tick(256)
            .max_io_events_per_busy_tick(64)
            .thread_name("boxxy-worker")
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

/// Returns the 0-based index of the current Tokio worker thread if called
/// from within a runtime worker thread (stabilized in Tokio 1.54).
#[must_use]
pub fn current_worker_index() -> Option<usize> {
    tokio::runtime::worker_index()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_and_worker_index() {
        assert_eq!(
            current_worker_index(),
            None,
            "current_worker_index must be None outside a Tokio worker thread"
        );
        let (worker_idx, blocking_idx) = runtime().block_on(async {
            let w = runtime()
                .spawn(async { current_worker_index() })
                .await
                .expect("spawned worker task");
            let b = runtime()
                .spawn_blocking(current_worker_index)
                .await
                .expect("spawned blocking task");
            (w, b)
        });
        assert!(
            worker_idx.is_some(),
            "current_worker_index must be Some(_) inside a multi-thread runtime worker"
        );
        assert_eq!(
            blocking_idx, None,
            "blocking pool threads are not async worker threads"
        );
    }
}
