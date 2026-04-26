pub mod claw;
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
    async fn notify_client_connected(&self) -> zbus::Result<bool>;
    async fn notify_client_disconnected(&self) -> zbus::Result<()>;
    async fn notify_settings_invalidated(&self) -> zbus::Result<()>;
    async fn request_reload(&self) -> zbus::Result<()>;
    async fn request_stop(&self) -> zbus::Result<()>;

    // Claw Session Management (Using JSON for complex types)
    async fn get_character_registry(&self) -> zbus::Result<String>;
    async fn create_claw_session(
        &self,
        pane_id: String,
        character_id: String,
    ) -> zbus::Result<String>;
    async fn post_claw_message(&self, session_id: String, message_json: String)
    -> zbus::Result<()>;
    /// Returns `[(session_id, pane_id, agent_name), ...]` for every session
    /// the daemon is currently hosting. Used by a freshly launched UI to
    /// discover sessions it can reattach to.
    async fn list_claw_sessions(&self) -> zbus::Result<Vec<(String, String, String)>>;
    /// Tears down a session actor and forgets its identity in the registry.
    /// Called when a pane is permanently closed (not just detached).
    async fn end_claw_session(&self, session_id: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn claw_event(&self, session_id: String, event_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn character_registry_updated(&self, registry_json: String) -> zbus::Result<()>;

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
        log::info!("Updating credentials for {} providers", api_keys.len());

        let mut keys_guard = self.core.state.api_keys.write().await;
        *keys_guard = api_keys;
        let mut url_guard = self.core.state.ollama_url.write().await;
        *url_guard = ollama_url;
    }

    async fn notify_settings_invalidated(&self) {
        log::info!("Settings invalidated signal received from UI. Reloading cache.");
        boxxy_preferences::Settings::reload();
    }

    #[zbus(signal)]
    async fn desktop_notification(
        emitter: &SignalEmitter<'_>,
        title: String,
        message: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn character_registry_updated(
        emitter: &SignalEmitter<'_>,
        registry_json: String,
    ) -> zbus::Result<()>;

    async fn get_character_registry(&self) -> String {
        let registry = self.core.registry.get_full_registry().await;
        serde_json::to_string(&registry).unwrap_or_default()
    }

    async fn notify_client_connected(&self) -> bool {
        let current = *self.client_count_tx.borrow();
        let _ = self.client_count_tx.send(current + 1);
        log::info!("Client connected. Total clients: {}", current + 1);

        // Return whether the DB was reset — the client shows the toast directly.
        boxxy_db::DATABASE_WAS_RESET.swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    async fn notify_client_disconnected(&self) {
        let current = *self.client_count_tx.borrow();
        let new_count = current.saturating_sub(1);
        let _ = self.client_count_tx.send(new_count);
        log::info!("Client disconnected. Total clients: {}", new_count);
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

    async fn create_claw_session(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(connection)] conn: &Connection,
        pane_id: String,
        character_id: String,
    ) -> String {
        // Look up the character in the catalog by UUID.
        let registry = self.core.registry.get_full_registry().await;
        let char_info = registry
            .iter()
            .find(|c| c.config.id == character_id)
            .cloned();
        let Some(char_info) = char_info else {
            log::warn!(
                "create_claw_session: no character with id '{}' found in catalog",
                character_id
            );
            return String::new();
        };
        let character_display_name = char_info.config.display_name.clone();

        // Enforce exclusivity: if this character is active in another pane, reject it.
        if let Some(active_pane) = self
            .core
            .registry
            .get_active_pane_for_character(&character_id, &pane_id)
            .await
        {
            let is_alive = {
                let manager = self.session_manager.lock().await;
                manager.sessions.values().any(|s| s.pane_id == active_pane)
            };

            if is_alive {
                log::warn!(
                    "Rejected session creation: Character '{}' is already active in pane '{}'",
                    character_display_name,
                    active_pane
                );
                let _ = Self::desktop_notification(
                    &emitter,
                    "Character in Use".to_string(),
                    format!(
                        "{} is already active in another pane.",
                        character_display_name
                    ),
                )
                .await;
                return String::new();
            } else {
                log::info!(
                    "Reclaiming character '{}' from dead pane '{}'",
                    character_display_name,
                    active_pane
                );
                self.core
                    .registry
                    .remove_assignment(&self.core.db, &active_pane)
                    .await;
            }
        }

        log::info!(
            "Creating Claw session for pane {} as '{}' ({})",
            pane_id,
            character_display_name,
            character_id
        );

        // If this pane already had a session, end it first to prevent orphans.
        {
            let mut to_remove = None;
            let manager = self.session_manager.lock().await;
            for (sid, s) in manager.sessions.iter() {
                if s.pane_id == pane_id {
                    to_remove = Some(sid.clone());
                    break;
                }
            }
            drop(manager);
            if let Some(sid) = to_remove {
                log::info!(
                    "Pane {} is swapping characters; ending old session {}",
                    pane_id,
                    sid
                );
                let mut manager = self.session_manager.lock().await;
                manager.sessions.remove(&sid);
            }
        }

        let (session, tx, rx_ui) = boxxy_claw::engine::ClawSession::new(pane_id.clone());
        let fresh_session_id = session.session_id.clone();
        let session = session.with_identity(
            character_id.clone(),
            character_display_name.clone(),
            fresh_session_id.clone(),
        );
        let session_id = fresh_session_id;

        // Persist the assignment so the next call resolves to the same agent.
        self.core
            .registry
            .set_assignment(
                &self.core.db,
                pane_id.clone(),
                crate::claw::CharacterAssignment {
                    character_id: character_id.clone(),
                    session_id: session_id.clone(),
                },
            )
            .await;

        // Broadcast the updated registry to all clients
        let registry_json = self.get_character_registry().await;
        let _ = Self::character_registry_updated(&emitter, registry_json).await;

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
                    pane_id: pane_id.clone(),
                    agent_name: character_display_name.clone(),
                    tx,
                },
            );
        }

        // Spawn the session worker with the agent's subsystem (which implements ClawEnvironment)
        let env = {
            let manager = self.session_manager.lock().await;
            manager.env.clone()
        };
        session.start(env);

        // Forward UI events from the session to the D-Bus signal. Certain
        // events (task completion, global notifications) also trigger a
        // host-level desktop toast via the freedesktop notifications portal
        // so the user hears about work that finishes while the UI is
        // detached.
        let session_id_clone = session_id.clone();
        let emitter = emitter.to_owned();
        let conn_clone = conn.clone();
        let character_display_name_clone = character_display_name.clone();
        tokio::spawn(async move {
            while let Ok(event) = rx_ui.recv().await {
                maybe_send_desktop_notification(&conn_clone, &character_display_name_clone, &event)
                    .await;

                if let Ok(event_json) = serde_json::to_string(&event) {
                    let _ = Self::claw_event(&emitter, session_id_clone.clone(), event_json).await;
                }
            }
        });

        session_id
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

    async fn end_claw_session(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        session_id: String,
    ) {
        let pane_id = {
            let mut manager = self.session_manager.lock().await;
            manager.sessions.remove(&session_id).map(|s| s.pane_id)
        };
        if let Some(pane_id) = pane_id {
            // Forget the agent's identity too — this pane is being disposed.
            self.core
                .registry
                .remove_assignment(&self.core.db, &pane_id)
                .await;

            // Broadcast the updated registry to all clients
            let registry_json = self.get_character_registry().await;
            let _ = Self::character_registry_updated(&emitter, registry_json).await;

            log::info!(
                "Ended Claw session {} (pane {} removed from registry)",
                session_id,
                pane_id
            );
        }
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
