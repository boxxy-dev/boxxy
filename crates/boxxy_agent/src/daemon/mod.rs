pub mod dreaming;
pub mod lifecycle;
pub mod power;
pub mod priority;
pub mod singleton;

use anyhow::Result;
use log::info;
use std::sync::Arc;
use tokio::sync::watch;

use crate::claw::CharacterRegistry;
use crate::core::state::AgentState;

/// The single owner of every subsystem in the daemon.
pub struct DaemonCore {
    pub state: AgentState,
    /// Persistent `pane_id → (agent_name, session_id)` mapping. Makes
    /// agent identity survive UI restarts.
    pub registry: Arc<CharacterRegistry>,
    /// "On battery / on AC" watcher. Constructed early and passed into
    /// subsystems; UPower updates flow through the same backing channel
    /// regardless of when clones were made.
    pub power: power::PowerMonitor,
    /// "No UIs connected" signal.
    pub ghost: lifecycle::GhostMode,
    /// What the maintenance subsystem is doing right now, readable via
    /// `MaintenanceSubsystem::get_maintenance_status()`.
    pub dream_status: dreaming::DreamStatusCell,
    pub db: boxxy_db::Db,
}

impl DaemonCore {
    pub async fn run() -> Result<()> {
        let state = AgentState::new();
        let db = boxxy_db::Db::new().await.unwrap_or_else(|e| {
            panic!("Fatal error: failed to initialize database: {}", e);
        });

        // The daemon is the single source of truth. Character assignments are
        // now purely in-memory and restart-safe by virtue of the client-tracking
        // logic. We no longer persist them to SQLite.
        let registry = Arc::new(CharacterRegistry::load_or_default(&db).await);

        // Build the watchers up front so every subsystem that later
        // clones them sees live updates as soon as the owning task
        // publishes the first value.
        let (client_tx, client_rx) = watch::channel(0usize);
        let (power_tx, power) = power::channel();
        let ghost = lifecycle::start(client_rx);
        let dream_status = dreaming::DreamStatusCell::default();

        let core = Arc::new(Self {
            state,
            registry: registry.clone(),
            power: power.clone(),
            ghost: ghost.clone(),
            dream_status: dream_status.clone(),
            db: db.clone(),
        });

        // Start D-Bus services on a fresh session-bus connection.
        let conn = crate::ipc::start_services(core.clone(), client_tx.clone()).await?;
        info!("D-Bus services registered");

        // Spawn the client owner tracker. This handles automatic cleanup of
        // character assignments and swarm locks when a UI process disconnects.
        let workspace = boxxy_claw::registry::workspace::global_workspace().await;
        crate::ipc::client_tracker::spawn_owner_tracker(
            &conn,
            registry.clone(),
            workspace,
            client_tx.clone(),
        )
        .await?;

        // Wire up UPower now that we have a connection. Failures are
        // logged and non-fatal — `power` stays at the AC default.
        if let Err(e) = power::start(&conn, power_tx).await {
            log::warn!("power: start() failed: {}; assuming AC", e);
        }

        // Dream cycle: niceness-19, battery-gated, ghost-gated,
        // setting-gated. Owns its own status cell; we hand it ours so
        // `MaintenanceSubsystem::get_maintenance_status()` reads the same state.
        dreaming::spawn_with_status(power.clone(), ghost.clone(), dream_status.clone());

        // Telemetry subsystem: Initialize and periodically flush in the background
        // so it never delays daemon startup or UI responsiveness.
        tokio::spawn(async move {
            boxxy_telemetry::init_db().await;
            boxxy_telemetry::init().await;

            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30 * 60));
            loop {
                tick.tick().await;
                boxxy_telemetry::flush_journal().await;
            }
        });

        // Zombie-guard sweeper: runs at niceness 19 and only sweeps
        // while in ghost mode — the TTL is 4 h, so a one-session delay
        // doesn't matter.
        let pty_registry = core.state.pty_registry.clone();
        let ghost_for_sweep = ghost.clone();
        tokio::spawn(async move {
            if let Err(e) = priority::set_current_thread_nice(priority::MAINTENANCE_NICE) {
                log::warn!("sweeper: set_current_thread_nice failed: {}", e);
            }
            let mut tick = tokio::time::interval(crate::pty::registry::SWEEP_INTERVAL);
            loop {
                tick.tick().await;
                if ghost_for_sweep.is_ghost() {
                    pty_registry.sweep_zombies().await;
                }
            }
        });

        // The D-Bus connection + spawned tasks keep the daemon alive;
        // this loop is just the "don't return" anchor.
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }
}
