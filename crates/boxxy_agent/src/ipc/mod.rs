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
) -> Result<Connection> {
    let claw_subsystem = Arc::new(crate::claw::ClawSubsystem::new(core.state.clone()));
    let session_manager = Arc::new(Mutex::new(ClawSessionManager::new(claw_subsystem.clone())));

    let conn = Builder::session()?
        .name("dev.boxxy.BoxxyAgent")?
        .serve_at(
            "/dev/boxxy/Agent",
            AgentInterface {
                core: core.clone(),
                client_count_tx: client_count_tx.clone(),
                session_manager,
            },
        )?
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Pty",
            crate::pty::PtySubsystem::new(core.state.clone()),
        )?
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Claw",
            crate::claw::ClawSubsystem::new(core.state.clone()),
        )?
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Maintenance",
            crate::maintenance::MaintenanceSubsystem::new(
                core.state.clone(),
                core.dream_status.clone(),
                core.power.clone(),
            ),
        )?
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

    Ok(conn)
}

/// Bookkeeping for one hosted `ClawSession`. We keep the pane id and agent
/// name so the UI can reattach by querying `list_sessions()`.
struct HostedSession {
    pane_id: String,
    agent_name: String,
    tx: async_channel::Sender<ClawMessage>,
}

struct ClawSessionManager {
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
}

#[zbus::proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/Agent"
)]
pub trait Agent {
    async fn get_version(&self) -> zbus::Result<String>;
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

    #[zbus(signal)]
    async fn claw_event(&self, session_id: String, event_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn claims_changed(&self, snapshot_json: String) -> zbus::Result<()>;

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

        let exe = std::env::current_exe().unwrap_or_default();
        let args: Vec<String> = std::env::args().collect();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let c_exe = std::ffi::CString::new(exe.to_string_lossy().to_string()).unwrap();
            let c_args: Vec<std::ffi::CString> = args
                .iter()
                .map(|s| std::ffi::CString::new(s.as_str()).unwrap())
                .collect();
            let _ = nix::unistd::execvp(&c_exe, &c_args);
        });
    }

    async fn request_stop(&self) {
        log::info!("Stop requested. Exiting...");
        std::process::exit(0);
    }

    async fn post_claw_message(&self, session_id: String, message_json: String) {
        if let Ok(message) = serde_json::from_str::<ClawMessage>(&message_json) {
            let manager = self.session_manager.lock().await;
            if let Some(session) = manager.sessions.get(&session_id) {
                let _ = session.tx.send(message).await;
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

    #[zbus(signal)]
    async fn claw_event(
        emitter: &SignalEmitter<'_>,
        session_id: String,
        event_json: String,
    ) -> zbus::Result<()>;
}

/// Converts select `ClawEngineEvent`s into host desktop notifications.
///
/// We intentionally do NOT gate on "UI is detached". The notifications daemon
/// on most desktop environments suppresses toasts when the emitting app is
/// focused, so this is already the right behaviour; and when a background
/// task completes while the user is *looking* at another window, the toast
/// is still the signal they need.
async fn maybe_send_desktop_notification(
    conn: &Connection,
    agent_name: &str,
    event: &ClawEngineEvent,
) {
    match event {
        ClawEngineEvent::PushGlobalNotification { title, message } => {
            notifier::send(conn, title, message).await;
        }
        ClawEngineEvent::TaskCompleted {
            agent_name: task_agent,
            ..
        } => {
            let title = format!("{} finished a task", task_agent);
            notifier::send(
                conn,
                &title,
                &format!("Agent {} completed a background task.", agent_name),
            )
            .await;
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
                let character_display_name_clone = character_display_name.clone();
                tokio::spawn(async move {
                    while let Ok(event) = rx_ui.recv().await {
                        maybe_send_desktop_notification(
                            &conn_clone,
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
