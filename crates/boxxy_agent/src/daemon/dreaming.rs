//! Daemon-side driver for the Dream Cycle.
//!
//! Wraps `boxxy_claw::memories::DreamOrchestrator` (which owns the
//! three-phase LLM consolidation) and adds the daemon-side safety
//! envelope:
//!   * pause while on battery so we don't drain power for consolidation,
//!   * pause while a UI is attached (ghost mode) so token spend happens
//!     when the user isn't actively interacting,
//!   * honour the `enable_auto_dreaming` setting,
//!   * an initial 10-second warm-up so the UI renders before we start
//!     grinding,
//!   * exponential backoff on consecutive LLM failures.
//!
//! A record of what's happening at any given moment is exposed via
//! `DreamStatus` so `MaintenanceSubsystem::get_maintenance_status()`
//! stops being a stub.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

use super::lifecycle::GhostMode;
use super::power::PowerMonitor;

/// One of these snapshots what the daemon is currently doing with its
/// spare cycles. `MaintenanceSubsystem` just formats this into a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamStatus {
    /// Nobody has asked us to dream yet (or the 10 s warm-up is still
    /// running).
    Idle,
    /// Feature disabled in Preferences.
    Disabled,
    /// UPower says we're on battery; standing down until AC.
    PausedOnBattery,
    /// A UI is attached; we'd rather stay out of the way.
    PausedUIAttached,
    /// LLM call in flight: extracting facts + patterns from interactions.
    Running,
}

impl DreamStatus {
    pub fn label(&self) -> &'static str {
        match self {
            DreamStatus::Idle => "idle",
            DreamStatus::Disabled => "disabled",
            DreamStatus::PausedOnBattery => "paused_on_battery",
            DreamStatus::PausedUIAttached => "paused_ui_attached",
            DreamStatus::Running => "running",
        }
    }
}

/// Shared, cheap-to-clone status cell.
#[derive(Clone, Default)]
pub struct DreamStatusCell {
    inner: Arc<RwLock<DreamStatus>>,
}

impl DreamStatusCell {
    pub fn new(initial: DreamStatus) -> Self {
        Self {
            inner: Arc::new(RwLock::new(initial)),
        }
    }

    pub async fn get(&self) -> DreamStatus {
        *self.inner.read().await
    }

    pub async fn set(&self, s: DreamStatus) {
        *self.inner.write().await = s;
    }
}

impl Default for DreamStatus {
    fn default() -> Self {
        DreamStatus::Idle
    }
}

/// Spawns the daemon's dreaming task. `status` is shared with
/// `DaemonCore` + `MaintenanceSubsystem` so the D-Bus interface can
/// read what we're doing in real time.
///
/// The task:
///   1. Sleeps 10 s so the UI finishes its first paint.
///   2. Sets its thread's nice level to `MAINTENANCE_NICE`.
///   3. Waits for ghost mode + AC + setting-enabled (checked each
///      minute) before running a cycle. After each cycle, sleeps 15
///      minutes before considering another one.
pub fn spawn_with_status(power: PowerMonitor, ghost: GhostMode, status: DreamStatusCell) {
    let status_task = status;
    let handle = tokio::runtime::Handle::current();

    // Run the dreaming loop on a dedicated OS thread niced down to
    // `MAINTENANCE_NICE` (19). Using `Handle::block_on` keeps the root future
    // pinned to this low-priority thread so we never nice down the shared
    // `boxxy-agent-worker` threads that serve latency-critical PTYs and D-Bus.
    let _ = std::thread::Builder::new()
        .name("boxxy-dream".to_string())
        .spawn(move || {
            if let Err(e) =
                super::priority::set_current_thread_nice(super::priority::MAINTENANCE_NICE)
            {
                log::warn!("dreaming: failed to set thread nice level: {e}");
            }

            handle.block_on(async move {
                // Initial warm-up: give GTK + window-mapping 10 s of quiet.
                tokio::time::sleep(Duration::from_secs(10)).await;

                // Poll-then-run loop. `tokio::time::interval` ticks on a fixed
                // cadence; we just check whether conditions are right.
                let mut tick = tokio::time::interval(Duration::from_secs(60));
                // First tick fires immediately — skip it so we respect the 10 s
                // warm-up we just did.
                tick.tick().await;

                let mut consecutive_failures: u32 = 0;

                loop {
                    tick.tick().await;

                    let settings = boxxy_preferences::Settings::load();
                    if !settings.enable_auto_dreaming {
                        status_task.set(DreamStatus::Disabled).await;
                        continue;
                    }
                    if power.is_on_battery() {
                        status_task.set(DreamStatus::PausedOnBattery).await;
                        continue;
                    }
                    if !ghost.is_ghost() {
                        // UI is attached; let the user type without a
                        // background LLM call stealing tokens.
                        status_task.set(DreamStatus::PausedUIAttached).await;
                        continue;
                    }

                    status_task.set(DreamStatus::Running).await;
                    match run_one_cycle(&settings).await {
                        Ok(()) => {
                            consecutive_failures = 0;
                            status_task.set(DreamStatus::Idle).await;
                            // Don't re-run immediately; give the kernel/LLM budget a
                            // break even if nothing else changes.
                            tokio::time::sleep(Duration::from_secs(15 * 60)).await;
                        }
                        Err(e) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            let backoff_secs = (60u64 << consecutive_failures.min(4)).min(15 * 60);
                            log::warn!(
                                "dreaming: cycle failed (attempt {}, backing off {}s): {:?}",
                                consecutive_failures,
                                backoff_secs,
                                e
                            );
                            status_task.set(DreamStatus::Idle).await;
                            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                        }
                    }
                }
            });
        });
}

async fn run_one_cycle(settings: &boxxy_preferences::Settings) -> anyhow::Result<()> {
    // Open the same on-disk DB the UI + engine use. This is SQLite;
    // multi-process access via WAL is fine.
    let db = boxxy_db::Db::new().await?;
    let db_arc = Arc::new(Mutex::new(Some(db)));

    let mut creds = boxxy_ai_core::AiCredentials::default();
    // Prefer effective keys (env overrides + settings file merge)
    // so the daemon's dream cycle works the same way a fresh UI would.
    creds.api_keys = settings.get_effective_api_keys();
    creds.ollama_url = settings.ollama_base_url.clone();

    let model = settings
        .memory_model
        .clone()
        .or_else(|| settings.claw_model.clone());

    let orchestrator = boxxy_claw::memories::DreamOrchestrator::new(db_arc, creds, model);
    orchestrator.run_cycle().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_are_stable() {
        assert_eq!(DreamStatus::Idle.label(), "idle");
        assert_eq!(DreamStatus::Running.label(), "running");
        assert_eq!(DreamStatus::PausedOnBattery.label(), "paused_on_battery");
    }

    #[tokio::test]
    async fn status_cell_read_back_roundtrip() {
        let cell = DreamStatusCell::default();
        assert_eq!(cell.get().await, DreamStatus::Idle);
        cell.set(DreamStatus::Running).await;
        assert_eq!(cell.get().await, DreamStatus::Running);
    }
}
