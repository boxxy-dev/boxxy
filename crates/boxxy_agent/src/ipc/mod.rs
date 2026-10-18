pub mod claw;
pub mod client_tracker;
pub mod maintenance;
pub mod pty;

use crate::claw::notifier;
use crate::daemon::DaemonCore;
use anyhow::Result;
use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, watch};
use zbus::{Connection, connection::Builder, object_server::SignalEmitter};

pub async fn start_services(
    core: Arc<DaemonCore>,
    client_count_tx: watch::Sender<usize>,
) -> Result<(Connection, Arc<Mutex<ClawSessionManager>>)> {
    let claw_subsystem = Arc::new(crate::claw::ClawSubsystem::new(core.state.clone()));
    let session_manager = Arc::new(Mutex::new(ClawSessionManager::new(claw_subsystem.clone())));

    let conn = Builder::session()
        .name("dev.boxxy.BoxxyAgent")
        .serve_at(
            "/dev/boxxy/Agent",
            AgentInterface {
                core: core.clone(),
                client_count_tx: client_count_tx.clone(),
                session_manager: session_manager.clone(),
            },
        )
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Pty",
            crate::pty::PtySubsystem::new(core.state.clone()),
        )
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Claw",
            crate::claw::ClawSubsystem::new(core.state.clone()),
        )
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Maintenance",
            crate::maintenance::MaintenanceSubsystem::new(
                core.state.clone(),
                core.dream_status.clone(),
                core.power.clone(),
            ),
        )
        .build()
        .await?;

    // Spawn a background task to broadcast registry changes to all D-Bus clients.
    // This centralizes emission so that API calls (try_claim), background cleanup
    // (owner_tracker), and startup seeding all use the same path.
    let mut rx_registry = core.registry.subscribe();
    let conn_for_signals = conn.clone();
    tokio::spawn(async move {
        while let Ok(snapshot) = rx_registry.recv().await {
            if let Ok(snapshot_json) = serde_json::to_string(&snapshot) {
                let _ = conn_for_signals
                    .emit_signal(
                        Option::<zbus::names::BusName>::None,
                        "/dev/boxxy/Agent",
                        "dev.boxxy.BoxxyTerminal.Agent",
                        "ClaimsChanged",
                        &(snapshot_json,),
                    )
                    .await;
            }
        }
    });

    // Spawn a background task to broadcast detached PTY session changes to all D-Bus
    // clients (GNOME Shell extension, Waybar, KDE widgets, and Boxxy windows).
    let mut rx_pty = core.state.pty_registry.subscribe();
    let pty_reg_for_signals = core.state.pty_registry.clone();
    let conn_for_pty_signals = conn.clone();
    tokio::spawn(async move {
        while rx_pty.recv().await.is_ok() {
            let list = crate::pty::query_detached_sessions_detailed(&pty_reg_for_signals).await;
            if let Ok(json) = serde_json::to_string(&list) {
                let _ = conn_for_pty_signals
                    .emit_signal(
                        Option::<zbus::names::BusName>::None,
                        "/dev/boxxy/Agent",
                        "dev.boxxy.BoxxyTerminal.Agent",
                        "DetachedSessionsChanged",
                        &(json,),
                    )
                    .await;
            }
        }
    });

    Ok((conn, session_manager))
}

/// Bookkeeping for one hosted `ClawSession`. We keep the pane id and agent
/// name so the UI can reattach by querying `list_sessions()`.
struct HostedSession {
    pane_id: String,
    agent_name: String,
    tx: async_channel::Sender<ClawMessage>,
}

pub struct ClawSessionManager {
    sessions: HashMap<String, HostedSession>,
    env: Arc<crate::claw::ClawSubsystem>,
}

impl ClawSessionManager {
    fn new(env: Arc<crate::claw::ClawSubsystem>) -> Self {
        Self {
            sessions: HashMap::new(),
            env,
        }
    }

    /// Drops any sessions whose sender is closed — the actor has exited on
    /// its own, or the channel was dropped. Keeps the map from growing
    /// unboundedly over the daemon's lifetime.
    fn prune_dead(&mut self) {
        self.sessions.retain(|_, s| !s.tx.is_closed());
    }

    /// Shuts down all hosted `ClawSession` actors belonging to `pane_id`.
    /// Sends `ClawMessage::Deactivate` (aborting any in-flight turn, draining
    /// pending approvals, and flushing unflushed history to SQLite) and closes
    /// the channel so the actor loop and D-Bus forwarder task exit cleanly.
    pub fn shutdown_pane(&mut self, pane_id: &str) {
        self.prune_dead();
        let to_close: Vec<HostedSession> = self
            .sessions
            .extract_if(|_, s| s.pane_id == pane_id)
            .map(|(_, s)| s)
            .collect();
        for session in to_close {
            let _ = session.tx.try_send(ClawMessage::Deactivate);
            session.tx.close();
        }
    }
}

#[zbus::proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/Agent"
)]
pub trait Agent {
    async fn get_version(&self) -> zbus::Result<String>;
    async fn get_config_dir(&self) -> zbus::Result<String>;
    async fn update_credentials(
        &self,
        api_keys: std::collections::HashMap<String, String>,
        ollama_url: String,
    ) -> zbus::Result<()>;
    async fn notify_settings_invalidated(&self) -> zbus::Result<()>;
    async fn request_reload(&self) -> zbus::Result<()>;
    async fn request_stop(&self) -> zbus::Result<()>;

    // Claw Session Management (Using JSON for complex types)
    async fn claim_startup_token(&self) -> zbus::Result<String>;
    async fn get_registry_snapshot(&self) -> zbus::Result<String>;
    async fn try_claim_character(
        &self,
        holder_id: String,
        holder_kind: u8,
        character_id: String,
    ) -> zbus::Result<String>;
    async fn release_holder(&self, holder_id: String) -> zbus::Result<()>;
    async fn resolve_peer(&self, query_json: String) -> zbus::Result<String>;

    async fn post_claw_message(&self, session_id: String, message_json: String)
    -> zbus::Result<()>;
    /// Returns `[(session_id, pane_id, agent_name), ...]` for every session
    /// the daemon is currently hosting. Used by a freshly launched UI to
    /// discover sessions it can reattach to.
    async fn list_claw_sessions(&self) -> zbus::Result<Vec<(String, String, String)>>;

    /// Requests that the UI raises the window and focuses the specified pane.
    async fn request_focus_pane(&self, pane_id: String) -> zbus::Result<()>;

    /// Returns a JSON array of `DetachedSessionInfo` objects for all detached
    /// background shells. Used by desktop companion extensions (GNOME Shell, KDE, Waybar).
    async fn get_detached_sessions(&self) -> zbus::Result<String>;

    /// Requests that a detached background shell (`pid`, or `0` for all) be restored
    /// into a live Boxxy Terminal tab. If no Boxxy window is open, queues the PID so
    /// the next window launched automatically restores it.
    async fn request_restore_detached(&self, pid: u32) -> zbus::Result<()>;

    /// Drains any PIDs queued via `request_restore_detached` while no UI window was open.
    async fn take_pending_restores(&self) -> zbus::Result<Vec<(u32, String)>>;

    /// Terminates a detached background shell (`pid`, or `0` for all).
    async fn terminate_detached_session(&self, pid: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn claw_event(&self, session_id: String, event_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn claims_changed(&self, snapshot_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn detached_sessions_changed(&self, sessions_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn restore_detached_requested(&self, pid: u32, cwd: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn desktop_notification(&self, title: String, message: String) -> zbus::Result<()>;
}

pub struct AgentInterface {
    pub core: Arc<DaemonCore>,
    pub client_count_tx: watch::Sender<usize>,
    session_manager: Arc<Mutex<ClawSessionManager>>,
}

#[zbus::interface(name = "dev.boxxy.BoxxyTerminal.Agent")]
impl AgentInterface {
    async fn get_version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    async fn get_config_dir(&self) -> String {
        boxxy_sys_utils::get_config_dir().to_string_lossy().into_owned()
    }

    async fn update_credentials(
        &self,
        api_keys: std::collections::HashMap<String, String>,
        ollama_url: String,
    ) {
        {
            let keys_guard = self.core.state.api_keys.read().await;
            let url_guard = self.core.state.ollama_url.read().await;
            if *keys_guard == api_keys && *url_guard == ollama_url {
                return;
            }
        }

        log::info!("Updating credentials for {} providers", api_keys.len());

        let mut keys_guard = self.core.state.api_keys.write().await;
        *keys_guard = api_keys;
        let mut url_guard = self.core.state.ollama_url.write().await;
        *url_guard = ollama_url;
    }

    async fn notify_settings_invalidated(&self) {
        log::debug!("Settings invalidated signal received from UI. Reloading cache.");
        boxxy_preferences::Settings::reload();
        let persist = boxxy_preferences::Settings::load().pty_persistence;
        self.core
            .state
            .pty_registry
            .sync_default_persistence(persist)
            .await;
    }

    #[zbus(signal)]
    async fn desktop_notification(
        emitter: &SignalEmitter<'_>,
        title: String,
        message: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn claims_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        snapshot_json: String,
    ) -> zbus::Result<()>;

    async fn claim_startup_token(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> String {
        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        crate::ipc::client_tracker::register_client(owner_bus_name).await;

        let current = *self.client_count_tx.borrow();
        let _ = self.client_count_tx.send(current + 1);
        log::info!("Client connected. Total clients: {}", current + 1);

        let db_was_reset =
            boxxy_db::DATABASE_WAS_RESET.swap(false, std::sync::atomic::Ordering::SeqCst);
        let snapshot = self.core.registry.snapshot().await;

        let token = boxxy_claw_protocol::characters::StartupToken {
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            db_was_reset,
            initial_revision: snapshot.revision,
        };
        serde_json::to_string(&token).unwrap_or_default()
    }

    async fn get_registry_snapshot(&self) -> String {
        let snapshot = self.core.registry.snapshot().await;
        serde_json::to_string(&snapshot).unwrap_or_default()
    }

    async fn try_claim_character(
        &self,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
        holder_id: String,
        holder_kind: u8,
        character_id: String,
    ) -> String {
        let kind = match boxxy_claw_protocol::characters::HolderKind::try_from(holder_kind) {
            Ok(k) => k,
            Err(_) => {
                return serde_json::to_string(&Result::<
                    boxxy_claw_protocol::characters::ClaimedSession,
                    boxxy_claw_protocol::characters::ClaimError,
                >::Err(
                    boxxy_claw_protocol::characters::ClaimError::UnknownCharacter { character_id },
                ))
                .unwrap_or_default();
            }
        };

        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let result = self
            .do_claim(emitter, conn, holder_id, kind, character_id, owner_bus_name)
            .await;
        serde_json::to_string(&result).unwrap_or_default()
    }

    async fn release_holder(&self, holder_id: String) {
        self.session_manager.lock().await.shutdown_pane(&holder_id);

        if let Some(claim) = self.core.registry.release_holder(&holder_id).await {
            log::info!(
                "Released holder {} (kind {:?})",
                holder_id,
                claim.holder_kind
            );

            if claim.holder_kind == boxxy_claw_protocol::characters::HolderKind::Pane {
                let workspace = boxxy_claw::registry::workspace::global_workspace().await;
                workspace.release_all_locks(&claim.holder_id).await;
                workspace.unregister_pane(claim.holder_id).await;
            }
        }
    }

    async fn resolve_peer(&self, query_json: String) -> String {
        if let Ok(query) =
            serde_json::from_str::<boxxy_claw_protocol::characters::PeerQuery>(&query_json)
        {
            let snapshot = self.core.registry.snapshot().await;
            let match_opt = match query {
                boxxy_claw_protocol::characters::PeerQuery::ByCharacterId(id) => {
                    snapshot.claims.into_iter().find(|c| c.character_id == id)
                }
                boxxy_claw_protocol::characters::PeerQuery::ByCharacterDisplayName(name) => {
                    let mut found_id = None;
                    if let Some(info) = snapshot
                        .catalog
                        .iter()
                        .find(|c| c.config.display_name.eq_ignore_ascii_case(&name))
                    {
                        found_id = Some(info.config.id.clone());
                    }
                    if let Some(id) = found_id {
                        snapshot.claims.into_iter().find(|c| c.character_id == id)
                    } else {
                        None
                    }
                }
                boxxy_claw_protocol::characters::PeerQuery::ByPetname(petname) => snapshot
                    .claims
                    .into_iter()
                    .find(|c| c.petname.eq_ignore_ascii_case(&petname)),
                boxxy_claw_protocol::characters::PeerQuery::ByHolderId(id) => {
                    snapshot.claims.into_iter().find(|c| c.holder_id == id)
                }
            };

            if let Some(claim) = match_opt {
                let display_name = snapshot
                    .catalog
                    .iter()
                    .find(|c| c.config.id == claim.character_id)
                    .map(|c| c.config.display_name.clone())
                    .unwrap_or_else(|| "Unknown".to_string());

                let info = boxxy_claw_protocol::characters::PeerInfo {
                    holder_id: claim.holder_id,
                    holder_kind: claim.holder_kind,
                    session_id: claim.session_id,
                    character_id: claim.character_id,
                    character_display_name: display_name,
                    petname: claim.petname,
                };
                return serde_json::to_string(&Some(info)).unwrap_or_default();
            }
        }
        serde_json::to_string(&Option::<boxxy_claw_protocol::characters::PeerInfo>::None)
            .unwrap_or_default()
    }

    async fn request_reload(&self) {
        log::info!("Self-reload requested. Executing new binary...");

        let exe_raw = std::env::current_exe()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // If the binary on disk was atomically swapped via rename(), Linux's
        // `/proc/self/exe` symlink appends `" (deleted)"`. Strip that suffix so
        // `execvp` launches the newly installed binary at the canonical path.
        let exe_clean = exe_raw
            .strip_suffix(" (deleted)")
            .unwrap_or(&exe_raw)
            .to_string();

        // Filter out `--background` so the reloaded process replaces the current
        // daemon PID in-place instead of calling `daemonize()` (fork + exit) a
        // second time, which would trigger PR_SET_PDEATHSIG (SIGHUP) on child PTYs.
        let args: Vec<String> = std::env::args()
            .filter(|a| a != "--background")
            .collect();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Ok(c_exe) = std::ffi::CString::new(exe_clean) {
                let c_args: Vec<std::ffi::CString> = args
                    .iter()
                    .filter_map(|s| std::ffi::CString::new(s.as_str()).ok())
                    .collect();
                let _ = nix::unistd::execvp(&c_exe, &c_args);
            }
        });
    }

    async fn request_stop(&self) {
        log::info!("Stop requested. Flushing telemetry and exiting...");
        boxxy_telemetry::flush_journal().await;
        std::process::exit(0);
    }

    async fn post_claw_message(&self, session_id: String, message_json: String) {
        if let Ok(message) = serde_json::from_str::<ClawMessage>(&message_json) {
            let pane_info = {
                let manager = self.session_manager.lock().await;
                manager
                    .sessions
                    .get(&session_id)
                    .map(|s| (s.pane_id.clone(), s.tx.clone()))
            };

            if let Some((pane_id, tx)) = pane_info {
                if let ClawMessage::ResumeSession {
                    session_id: ref resume_sid,
                } = message
                {
                    let store = boxxy_db::store::Store::new(self.core.db.pool());
                    if let Ok(Some(db_session)) = store.get_session(resume_sid).await
                        && let Some((_, display_name)) = self
                            .core
                            .registry
                            .resolve_and_update_holder_character(
                                &pane_id,
                                &db_session.character_id,
                                &db_session.character_display_name,
                                db_session.agent_name.as_deref(),
                            )
                            .await
                    {
                        let mut manager = self.session_manager.lock().await;
                        if let Some(hosted) = manager.sessions.get_mut(&session_id) {
                            hosted.agent_name = display_name;
                        }
                    }
                }
                let _ = tx.send(message).await;
            }
        }
    }

    async fn list_claw_sessions(&self) -> Vec<(String, String, String)> {
        let mut manager = self.session_manager.lock().await;
        manager.prune_dead();
        manager
            .sessions
            .iter()
            .map(|(sid, s)| (sid.clone(), s.pane_id.clone(), s.agent_name.clone()))
            .collect()
    }

    async fn request_focus_pane(
        &self,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
        pane_id: String,
    ) {
        log::info!("Focus requested for pane {}", pane_id);
        let manager = self.session_manager.lock().await;
        // Find any session tied to this pane. Usually there is only one.
        for (sid, session) in manager.sessions.iter() {
            if session.pane_id == pane_id {
                let event = ClawEngineEvent::FocusPane;
                if let Ok(event_json) = serde_json::to_string(&event) {
                    let _ = Self::claw_event(&emitter, sid.clone(), event_json).await;
                }
            }
        }
    }

    #[zbus(signal)]
    async fn claw_event(
        emitter: &SignalEmitter<'_>,
        session_id: String,
        event_json: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn detached_sessions_changed(
        emitter: &SignalEmitter<'_>,
        sessions_json: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn restore_detached_requested(
        emitter: &SignalEmitter<'_>,
        pid: u32,
        cwd: String,
    ) -> zbus::Result<()>;

    async fn get_detached_sessions(&self) -> String {
        let list = crate::pty::query_detached_sessions_detailed(&self.core.state.pty_registry).await;
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }

    async fn request_restore_detached(
        &self,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
        pid: u32,
    ) {
        let list = crate::pty::query_detached_sessions_detailed(&self.core.state.pty_registry).await;
        let targets: Vec<_> = if pid == 0 {
            list
        } else {
            list.into_iter().filter(|s| s.pid == pid).collect()
        };

        if self.core.ghost.is_ghost() {
            let mut pending = self.core.state.pending_restores.write().await;
            for t in targets {
                if !pending.contains(&t.pid) {
                    pending.push(t.pid);
                }
            }
        } else {
            for t in targets {
                let _ = Self::restore_detached_requested(&emitter, t.pid, t.cwd).await;
            }
        }
    }

    async fn take_pending_restores(&self) -> Vec<(u32, String)> {
        let queued: Vec<u32> = {
            let mut pending = self.core.state.pending_restores.write().await;
            std::mem::take(&mut *pending)
        };
        if queued.is_empty() {
            return Vec::new();
        }
        let list = crate::pty::query_detached_sessions_detailed(&self.core.state.pty_registry).await;
        list.into_iter()
            .filter(|s| queued.contains(&s.pid))
            .map(|s| (s.pid, s.cwd))
            .collect()
    }

    async fn terminate_detached_session(&self, pid: u32) {
        let list = crate::pty::query_detached_sessions_detailed(&self.core.state.pty_registry).await;
        let targets: Vec<u32> = if pid == 0 {
            list.into_iter().map(|s| s.pid).collect()
        } else {
            list.into_iter()
                .filter(|s| s.pid == pid)
                .map(|s| s.pid)
                .collect()
        };

        for target_pid in targets {
            self.core
                .state
                .pty_registry
                .set_persistence(target_pid, false)
                .await;
            unsafe {
                libc::kill(-(target_pid as i32), libc::SIGTERM);
            }
        }
        self.core.state.pty_registry.notify_changed();
    }
}

/// Converts select `ClawEngineEvent`s into host desktop notifications when in Ghost Mode.
///
/// When at least one UI window is attached (`!ghost.is_ghost()`), `boxxy-window`
/// handles `TaskCompleted`, `LongTaskCompleted`, and `PushGlobalNotification` via
/// `gio::Notification` (with character avatars and focus-suppression). When no UI is
/// attached (`ghost.is_ghost()`), the daemon emits desktop notifications directly over
/// `org.freedesktop.Notifications` so background reminders and long tasks are never lost.
async fn maybe_send_desktop_notification(
    conn: &Connection,
    ghost: &crate::daemon::lifecycle::GhostMode,
    agent_name: &str,
    event: &ClawEngineEvent,
) {
    if !ghost.is_ghost() {
        return;
    }

    let first_name = agent_name.split_whitespace().next().unwrap_or(agent_name);
    match event {
        ClawEngineEvent::PushGlobalNotification { title, message } => {
            notifier::send(conn, title, message).await;
        }
        ClawEngineEvent::TaskCompleted { message, .. } => {
            let title = format!("{first_name} sends a reminder");
            let body = message
                .clone()
                .unwrap_or_else(|| format!("{first_name} has finished a timer."));
            notifier::send(conn, &title, &body).await;
        }
        ClawEngineEvent::LongTaskCompleted { message, .. } => {
            let title = format!("{first_name} completed a task");
            notifier::send(conn, &title, message).await;
        }
        _ => {}
    }
}

impl AgentInterface {
    async fn do_claim(
        &self,
        emitter: zbus::object_server::SignalEmitter<'_>,
        conn: &zbus::Connection,
        holder_id: String,
        holder_kind: boxxy_claw_protocol::characters::HolderKind,
        character_id: String,
        owner_bus_name: String,
    ) -> Result<
        boxxy_claw_protocol::characters::ClaimedSession,
        boxxy_claw_protocol::characters::ClaimError,
    > {
        let claim_res = self
            .core
            .registry
            .try_claim(
                holder_id.clone(),
                holder_kind,
                character_id.clone(),
                owner_bus_name.clone(),
            )
            .await;

        match claim_res {
            Ok(claimed_session) => {
                let session_id = claimed_session.session_id.clone();
                let character_display_name = {
                    let snapshot = self.core.registry.snapshot().await;
                    snapshot
                        .catalog
                        .iter()
                        .find(|c| c.config.id == character_id)
                        .map(|c| c.config.display_name.clone())
                        .unwrap_or_else(|| "Unknown".to_string())
                };

                let (session, tx, rx_ui) = boxxy_claw::engine::ClawSession::new(holder_id.clone());
                let session = session.with_identity(
                    character_id.clone(),
                    character_display_name.clone(),
                    session_id.clone(),
                );

                // Register with workspace before emitting signal
                if holder_kind == boxxy_claw_protocol::characters::HolderKind::Pane {
                    let workspace = boxxy_claw::registry::workspace::global_workspace().await;
                    workspace
                        .register_pane_tx(holder_id.clone(), tx.clone())
                        .await;
                }

                {
                    // Initialize session with core state
                    let mut state_lock = session.state.lock().await;
                    state_lock.tracked_pids = self.core.state.tracked_pids.clone();
                    state_lock.api_keys = self.core.state.api_keys.clone();
                    state_lock.ollama_url = self.core.state.ollama_url.clone();
                    state_lock.tx_self = tx.clone();
                }

                {
                    let mut manager = self.session_manager.lock().await;
                    manager.prune_dead();
                    manager.sessions.insert(
                        session_id.clone(),
                        HostedSession {
                            pane_id: holder_id.clone(),
                            agent_name: character_display_name.clone(),
                            tx,
                        },
                    );
                }

                let env = {
                    let manager = self.session_manager.lock().await;
                    manager.env.clone()
                };
                session.start(env);

                let session_id_clone = session_id.clone();
                let emitter = emitter.to_owned();
                let conn_clone = conn.clone();
                let ghost_clone = self.core.ghost.clone();
                let character_display_name_clone = character_display_name.clone();
                tokio::spawn(async move {
                    while let Ok(event) = rx_ui.recv().await {
                        maybe_send_desktop_notification(
                            &conn_clone,
                            &ghost_clone,
                            &character_display_name_clone,
                            &event,
                        )
                        .await;

                        if let Ok(event_json) = serde_json::to_string(&event) {
                            let _ =
                                Self::claw_event(&emitter, session_id_clone.clone(), event_json)
                                    .await;
                        }
                    }
                });

                Ok(claimed_session)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claw::ClawSubsystem;
    use crate::core::state::AgentState;
    use crate::daemon::dreaming::{DreamStatus, DreamStatusCell};
    use crate::daemon::power;
    use crate::ipc::claw::AgentClawProxy;
    use crate::ipc::maintenance::AgentMaintenanceProxy;
    use crate::ipc::pty::AgentPtyProxy;
    use crate::maintenance::MaintenanceSubsystem;
    use crate::pty::PtySubsystem;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;
    use zbus::Guid;

    #[tokio::test]
    async fn p2p_subsystems_roundtrip() {
        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();
        let dream_status = DreamStatusCell::new(DreamStatus::Idle);
        let power_monitor = power::ac_only();

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Pty",
                    PtySubsystem::new(state.clone()),
                )
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Claw",
                    ClawSubsystem::new(state.clone()),
                )
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Maintenance",
                    MaintenanceSubsystem::new(state, dream_status, power_monitor),
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let pty_proxy = AgentPtyProxy::new(&client_conn).await.expect("pty proxy");
        let master_fd = pty_proxy.create_pty().await.expect("create_pty over p2p");
        assert!(master_fd.as_raw_fd() >= 0);

        let maint_proxy = AgentMaintenanceProxy::new(&client_conn)
            .await
            .expect("maintenance proxy");
        assert_eq!(
            maint_proxy
                .get_maintenance_status()
                .await
                .expect("maintenance status"),
            "idle"
        );
        assert!(!maint_proxy.is_on_battery().await.expect("is_on_battery"));

        let claw_proxy = AgentClawProxy::new(&client_conn).await.expect("claw proxy");
        let (code, stdout, _stderr) = claw_proxy
            .exec_shell("printf boxxy_zbus6".to_string())
            .await
            .expect("exec_shell over p2p");
        assert_eq!(code, 0);
        assert_eq!(stdout, "boxxy_zbus6");

        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("zbus6_test.txt");
        let file_path_str = file_path.to_string_lossy().into_owned();
        claw_proxy
            .write_file(file_path_str.clone(), "line1\nline2\n".to_string())
            .await
            .expect("write_file over p2p");
        let content = claw_proxy
            .read_file(file_path_str.clone(), 1, 10)
            .await
            .expect("read_file over p2p");
        assert!(content.contains("line1"));
        claw_proxy
            .delete_file(file_path_str)
            .await
            .expect("delete_file over p2p");
    }

    #[tokio::test]
    async fn p2p_pty_spawn_and_exited_signal_stream() {
        use crate::ipc::pty::SpawnOptions;
        use futures_util::StreamExt;
        use std::os::unix::io::FromRawFd;

        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Pty",
                    PtySubsystem::new(state),
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let pty_proxy = AgentPtyProxy::new(&client_conn).await.expect("pty proxy");
        let mut exited_stream = pty_proxy
            .receive_exited()
            .await
            .expect("subscribe exited signal");

        let master_fd = pty_proxy.create_pty().await.expect("create_pty");
        let dup_raw = unsafe { libc::dup(master_fd.as_raw_fd()) };
        assert!(dup_raw >= 0);
        let fd_for_spawn =
            zbus::OwnedFd::from(unsafe { std::os::unix::io::OwnedFd::from_raw_fd(dup_raw) });

        let tmp = tempfile::tempdir().expect("tempdir");
        let options = SpawnOptions {
            cwd: tmp.path().to_string_lossy().into_owned(),
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "exit 42".to_string(),
            ],
            env: vec![("TERM".to_string(), "xterm-256color".to_string())],
            cols: 80,
            rows: 24,
            pane_id: "pane-signal-test".to_string(),
        };

        let pid = pty_proxy
            .spawn(fd_for_spawn, options)
            .await
            .expect("spawn over p2p");

        let signal = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            exited_stream.next(),
        )
        .await
        .expect("timed out waiting for Exited signal")
        .expect("stream ended unexpectedly");

        let args = signal.args().expect("signal args");
        assert_eq!(args.pid, pid);
        assert_eq!(args.exit_code, 42);

        let (has_exited, recorded_code) = pty_proxy
            .get_exit_status(pid)
            .await
            .expect("get_exit_status");
        assert!(has_exited);
        assert_eq!(recorded_code, 42);
    }

    #[tokio::test]
    async fn p2p_pty_detach_and_reattach_fd_roundtrip() {
        use crate::ipc::pty::SpawnOptions;
        use std::os::unix::io::FromRawFd;

        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Pty",
                    PtySubsystem::new(state),
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let pty_proxy = AgentPtyProxy::new(&client_conn).await.expect("pty proxy");
        let master_fd = pty_proxy.create_pty().await.expect("create_pty");
        let dup_raw = unsafe { libc::dup(master_fd.as_raw_fd()) };
        let fd_for_spawn =
            zbus::OwnedFd::from(unsafe { std::os::unix::io::OwnedFd::from_raw_fd(dup_raw) });
        // Drop the client's original master_fd so only the daemon holds it once detached
        drop(master_fd);

        let tmp = tempfile::tempdir().expect("tempdir");
        let options = SpawnOptions {
            cwd: tmp.path().to_string_lossy().into_owned(),
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf 'zbus6_persist_ok'; sleep 10".to_string(),
            ],
            env: vec![],
            cols: 80,
            rows: 24,
            pane_id: "pane-persist-test".to_string(),
        };

        let pid = pty_proxy
            .spawn(fd_for_spawn, options)
            .await
            .expect("spawn over p2p");

        pty_proxy
            .set_persistence(pid, true)
            .await
            .expect("set_persistence");
        let outcome = pty_proxy.detach(pid).await.expect("detach");
        assert_eq!(outcome, 1, "expected DetachOutcome::Detached (1)");

        let detached = pty_proxy
            .list_detached_sessions()
            .await
            .expect("list_detached_sessions");
        assert!(
            detached
                .iter()
                .any(|(p, pane, _)| *p == pid && pane == "pane-persist-test")
        );

        // Wait briefly for the detached reader to capture the shell output into the ring buffer
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        let (replay_bytes, reattached_fd) = pty_proxy.reattach(pid).await.expect("reattach");
        assert!(reattached_fd.as_raw_fd() >= 0);
        assert!(
            String::from_utf8_lossy(&replay_bytes).contains("zbus6_persist_ok"),
            "unexpected replay buffer: {:?}",
            String::from_utf8_lossy(&replay_bytes)
        );

        let _ = pty_proxy
            .signal_process_group(pid, libc::SIGKILL)
            .await;
    }
}
