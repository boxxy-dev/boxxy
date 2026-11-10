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
        let (conn, session_manager) =
            crate::ipc::start_services(core.clone(), client_tx.clone()).await?;
        info!("D-Bus services registered");

        // Spawn the client owner tracker. This handles automatic cleanup of
        // character assignments, PTY sessions, and swarm locks when a UI process disconnects.
        let workspace = boxxy_claw::registry::workspace::global_workspace().await;
        let session_manager_for_sig = session_manager.clone();
        let session_manager_for_reload = session_manager.clone();
        let pty_registry_for_sig = core.state.pty_registry.clone();
        crate::ipc::client_tracker::spawn_owner_tracker(
            &conn,
            registry.clone(),
            core.state.pty_registry.clone(),
            session_manager,
            workspace.clone(),
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

        // MCP background health monitor: warms up configured MCP servers at startup,
        // periodically checks local/active MCP server liveness while any UI window is attached,
        // and pushes `McpStatusChanged` over D-Bus whenever a server connects or disconnects.
        let conn_for_mcp = conn.clone();
        let ghost_for_mcp = ghost.clone();
        tokio::spawn(async move {
            let mut last_statuses: Option<Vec<(String, bool)>> = None;
            loop {
                let settings = boxxy_preferences::Settings::load();
                let has_enabled = settings.mcp_servers.iter().any(|s| s.enabled);
                if has_enabled && (!ghost_for_mcp.is_ghost() || last_statuses.is_none()) {
                    let statuses =
                        boxxy_claw::sync_and_probe_mcp_servers(settings.mcp_servers.clone()).await;
                    if last_statuses.as_ref() != Some(&statuses) {
                        last_statuses = Some(statuses.clone());
                        boxxy_claw_protocol::characters::update_mcp_status_cache(statuses.clone());
                        crate::ipc::emit_mcp_status_changed(&conn_for_mcp, &statuses).await;
                    }
                } else if !has_enabled && last_statuses.as_ref().is_none_or(|s| !s.is_empty()) {
                    let empty = Vec::new();
                    last_statuses = Some(empty.clone());
                    boxxy_claw_protocol::characters::update_mcp_status_cache(empty.clone());
                    crate::ipc::emit_mcp_status_changed(&conn_for_mcp, &empty).await;
                }
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        });

        tokio::spawn(async move {
            boxxy_telemetry::init_db().await;
            boxxy_telemetry::init().await;

            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30 * 60));
            // Consume the immediate first tick so periodic flushes occur every 30 minutes
            // (and immediately when a UI client disconnects).
            tick.tick().await;
            loop {
                tick.tick().await;
                boxxy_telemetry::flush_journal().await;
            }
        });

        // Graceful shutdown on SIGTERM / SIGINT: flush telemetry before exiting.
        #[cfg(unix)]
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut sigint = match signal(SignalKind::interrupt()) {
                Ok(s) => s,
                Err(_) => return,
            };
            tokio::select! {
                _ = sigterm.recv() => log::info!("SIGTERM received; terminating sessions and flushing telemetry before exit..."),
                _ = sigint.recv() => log::info!("SIGINT received; terminating sessions and flushing telemetry before exit..."),
            }
            session_manager_for_sig.lock().await.shutdown_all();
            pty_registry_for_sig.terminate_all().await;
            boxxy_telemetry::flush_journal().await;
            std::process::exit(0);
        });

        // Zombie-guard sweeper: sweeps detached idle sessions (> 4h TTL, viewer_count == 0)
        // and prunes expired `Exited` records even while other UI windows remain open.
        let pty_registry = core.state.pty_registry.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(crate::pty::registry::SWEEP_INTERVAL);
            loop {
                tick.tick().await;
                pty_registry.sweep_zombies().await;
            }
        });

        // Spawn character watcher for hot-reloading
        if let Ok(characters_dir) = boxxy_claw_protocol::character_loader::get_characters_dir() {
            let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel::<()>(4);
            crate::character_watcher::spawn_character_watcher(characters_dir, watcher_tx);

            let registry_for_reload = registry.clone();
            let workspace_for_reload = workspace.clone();
            tokio::spawn(async move {
                while watcher_rx.recv().await.is_some() {
                    log::info!("Character catalog change detected, reloading...");
                    match registry_for_reload.reload_catalog().await {
                        Ok((changed_char_ids, migrated_holder_ids)) => {
                            let snapshot = registry_for_reload.snapshot().await;
                            // Sync live agent display names across hosted sessions (handles renames & migrations)
                            {
                                let mut mgr = session_manager_for_reload.lock().await;
                                for claim in &snapshot.claims {
                                    if let Some(info) = snapshot
                                        .catalog
                                        .iter()
                                        .find(|c| c.config.id == claim.character_id)
                                    {
                                        mgr.sync_pane_agent_name(
                                            &claim.holder_id,
                                            &info.config.display_name,
                                        );
                                    }
                                }
                            }

                            if !changed_char_ids.is_empty() || !migrated_holder_ids.is_empty() {
                                for claim in snapshot.claims {
                                    let personality_changed =
                                        changed_char_ids.contains(&claim.character_id);
                                    // Use holder_id for migrated panes to avoid invalidating
                                    // unrelated sessions that happen to share the fallback character.
                                    let was_migrated =
                                        migrated_holder_ids.contains(&claim.holder_id);
                                    if personality_changed || was_migrated {
                                        log::info!(
                                            "Sending SettingsInvalidated to pane {} (personality_changed={}, migrated={})",
                                            claim.holder_id,
                                            personality_changed,
                                            was_migrated
                                        );
                                        if let Some(tx) = workspace_for_reload
                                            .get_pane_tx_by_id(&claim.holder_id)
                                            .await
                                        {
                                            let _ = tx
                                                .send(boxxy_claw_protocol::ClawMessage::SettingsInvalidated)
                                                .await;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Character hot-reload aborted (parse error): {e}");
                        }
                    }
                }
            });
        }

        // The D-Bus connection + spawned tasks keep the daemon alive;
        // this loop is just the "don't return" anchor.
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }
}
