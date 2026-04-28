use anyhow::{Context, Result};
use boxxy_agent::ipc::AgentProxy;
use boxxy_agent::ipc::claw::AgentClawProxy;
use boxxy_agent::ipc::pty::{AgentPtyProxy, SpawnOptions};
use boxxy_claw_protocol::{ClawEngineEvent, ClawEnvironment, ClawMessage};
use futures_util::StreamExt;
use serde_json;
use zbus::Connection;
use zbus::zvariant::OwnedFd;

#[derive(Clone)]
pub struct AgentManager {
    agent_proxy: AgentProxy<'static>,
    proxy: AgentPtyProxy<'static>,
    claw_proxy: AgentClawProxy<'static>,
    /// Notification produced during init (e.g. DB reset). Stored here instead
    /// of sent to TERMINAL_EVENT_BUS immediately, because the bus is a
    /// broadcast channel that drops messages when there are no subscribers —
    /// and the window hasn't subscribed yet when AgentManager is initialized.
    startup_notification: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl AgentManager {
    pub async fn new() -> Result<Self> {
        let conn = Connection::session()
            .await
            .context("Failed to connect to Session Bus")?;

        // Wait for the agent to appear on D-Bus if it's not there yet.
        // This handles cases where the UI starts faster than the agent.
        let mut attempts = 0;
        let max_attempts = 30; // 3 seconds total
        let agent_proxy = loop {
            match AgentProxy::builder(&conn)
                .destination("dev.boxxy.BoxxyAgent")?
                .build()
                .await
            {
                Ok(proxy) => break proxy,
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow::anyhow!(
                            "Timed out waiting for AgentProxy on Session Bus: {}",
                            e
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        };

        let proxy = AgentPtyProxy::builder(&conn)
            .destination("dev.boxxy.BoxxyAgent")?
            .build()
            .await
            .context("Failed to create AgentPtyProxy on Session Bus")?;

        let claw_proxy = AgentClawProxy::builder(&conn)
            .destination("dev.boxxy.BoxxyAgent")?
            .build()
            .await
            .context("Failed to create AgentClawProxy on Session Bus")?;

        // Global UI watcher for Character Claims updates (New API)
        let agent_proxy_for_claims = agent_proxy.clone();
        tokio::spawn(async move {
            if let Ok(mut stream) = agent_proxy_for_claims.receive_claims_changed().await {
                let mut last_seen_revision = 0;
                while let Some(msg) = stream.next().await {
                    let args = msg.args().expect("Failed to parse claims_changed args");
                    if let Ok(snapshot) = serde_json::from_str::<
                        boxxy_claw_protocol::characters::RegistrySnapshot,
                    >(&args.snapshot_json)
                    {
                        if snapshot.revision > last_seen_revision {
                            last_seen_revision = snapshot.revision;
                            boxxy_claw_protocol::characters::CLAIMS_CACHE
                                .store(std::sync::Arc::new(snapshot.claims));
                            boxxy_claw_protocol::characters::CHARACTER_CACHE
                                .store(std::sync::Arc::new(snapshot.catalog));
                        }
                    }
                }
            }
        });

        // Notify the daemon that a client has connected. Returns true if the DB
        // was wiped on this startup. We store the notification rather than
        // broadcasting it immediately — TERMINAL_EVENT_BUS is a broadcast
        // channel that drops messages with no receivers, and the window hasn't
        // subscribed yet at this point in the startup sequence.

        let startup_token_json = agent_proxy.claim_startup_token().await.unwrap_or_default();
        let startup_notification = if let Ok(token) = serde_json::from_str::<
            boxxy_claw_protocol::characters::StartupToken,
        >(&startup_token_json)
        {
            if token.db_was_reset {
                Some("Database reset: Conversation history was cleared for a schema update. This only happens during the Preview.".to_string())
            } else {
                None
            }
        } else {
            None
        };

        // Forward ongoing daemon-level desktop notifications (e.g. "Character in Use") as in-app toasts.
        if let Ok(mut stream) = agent_proxy.receive_desktop_notification().await {
            tokio::spawn(async move {
                while let Some(msg) = stream.next().await {
                    if let Ok(args) = msg.args() {
                        let text = if args.title.is_empty() {
                            args.message.to_string()
                        } else {
                            format!("{}: {}", args.title, args.message)
                        };
                        let event = crate::TerminalEvent {
                            id: String::new(),
                            kind: crate::TerminalEventKind::Notification(text),
                        };
                        let _ = crate::TERMINAL_EVENT_BUS.send(event);
                    }
                }
            });
        }

        // Seed the UI character cache
        if let Ok(snapshot_json) = agent_proxy.get_registry_snapshot().await {
            if let Ok(snapshot) = serde_json::from_str::<
                boxxy_claw_protocol::characters::RegistrySnapshot,
            >(&snapshot_json)
            {
                boxxy_claw_protocol::characters::CLAIMS_CACHE
                    .store(std::sync::Arc::new(snapshot.claims));
                boxxy_claw_protocol::characters::CHARACTER_CACHE
                    .store(std::sync::Arc::new(snapshot.catalog));
                log::info!("Seeded UI Character and Claims Cache on startup.");
            }
        }

        Ok(Self {
            agent_proxy,
            proxy,
            claw_proxy,
            startup_notification: std::sync::Arc::new(std::sync::Mutex::new(startup_notification)),
        })
    }

    /// Returns any notification generated during initialization (e.g. DB reset
    /// message), consuming it so it is only shown once. Call this after the
    /// window has subscribed to TERMINAL_EVENT_BUS.
    pub fn take_startup_notification(&self) -> Option<String> {
        self.startup_notification.lock().unwrap().take()
    }

    pub async fn disconnect(&self) {
        // notify_client_disconnected removed in Phase 2
    }

    pub async fn notify_settings_invalidated(&self) -> Result<()> {
        self.agent_proxy
            .notify_settings_invalidated()
            .await
            .context("Failed to notify agent of settings invalidation")
    }

    pub async fn update_credentials(
        &self,
        api_keys: std::collections::HashMap<String, String>,
        ollama_url: String,
    ) -> Result<()> {
        let _: () = self
            .agent_proxy
            .update_credentials(api_keys, ollama_url)
            .await
            .context("Failed to update agent credentials")?;
        Ok(())
    }

    pub async fn claim_startup_token(
        &self,
    ) -> Result<boxxy_claw_protocol::characters::StartupToken> {
        let token_json = self
            .agent_proxy
            .claim_startup_token()
            .await
            .context("Failed to get startup token")?;
        let token = serde_json::from_str(&token_json).context("Failed to parse StartupToken")?;
        Ok(token)
    }

    pub async fn get_registry_snapshot(
        &self,
    ) -> Result<boxxy_claw_protocol::characters::RegistrySnapshot> {
        let snapshot_json = self
            .agent_proxy
            .get_registry_snapshot()
            .await
            .context("Failed to get registry snapshot")?;
        let snapshot =
            serde_json::from_str(&snapshot_json).context("Failed to parse RegistrySnapshot")?;
        Ok(snapshot)
    }

    pub async fn try_claim_character(
        &self,
        pane_id: String,
        holder_kind: u8,
        character_id: String,
    ) -> Result<(String, async_channel::Receiver<ClawEngineEvent>)> {
        let result_json = self
            .agent_proxy
            .try_claim_character(pane_id, holder_kind, character_id)
            .await
            .context("Failed to try claim character via Agent")?;

        let result: Result<
            boxxy_claw_protocol::characters::ClaimedSession,
            boxxy_claw_protocol::characters::ClaimError,
        > = serde_json::from_str(&result_json).context("Failed to parse claim result")?;

        let claimed_session = result.map_err(|e| anyhow::anyhow!("Claim failed: {:?}", e))?;
        let session_id = claimed_session.session_id;

        let (tx, rx) = async_channel::unbounded();

        // Subscribe to events for this session
        let mut stream = self
            .agent_proxy
            .receive_claw_event()
            .await
            .context("Failed to subscribe to Claw events")?;

        let session_id_filter = session_id.clone();
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                if let Ok(args) = msg.args() {
                    if args.session_id == session_id_filter {
                        if let Ok(event) = serde_json::from_str::<ClawEngineEvent>(&args.event_json)
                        {
                            let _ = tx.send(event).await;
                        }
                    }
                }
            }
        });

        Ok((session_id, rx))
    }

    pub async fn release_holder(&self, holder_id: String) -> Result<()> {
        self.agent_proxy
            .release_holder(holder_id)
            .await
            .context("Failed to release holder via Agent")
    }

    pub async fn resolve_peer(
        &self,
        query: boxxy_claw_protocol::characters::PeerQuery,
    ) -> Result<Option<boxxy_claw_protocol::characters::PeerInfo>> {
        let query_json = serde_json::to_string(&query).context("Failed to serialize PeerQuery")?;
        let result_json = self
            .agent_proxy
            .resolve_peer(query_json)
            .await
            .context("Failed to resolve peer")?;
        let info = serde_json::from_str(&result_json).context("Failed to parse PeerInfo")?;
        Ok(info)
    }

    pub async fn create_claw_session(
        &self,
        pane_id: String,
        character_id: String,
    ) -> Result<(String, async_channel::Receiver<ClawEngineEvent>)> {
        // Shim mapping old UI code to new try_claim API
        self.try_claim_character(pane_id, 0, character_id).await
    }

    pub async fn post_claw_message(&self, session_id: String, message: ClawMessage) -> Result<()> {
        let message_json =
            serde_json::to_string(&message).context("Failed to serialize ClawMessage")?;
        self.agent_proxy
            .post_claw_message(session_id, message_json)
            .await
            .context("Failed to post Claw message via Agent")
    }

    pub fn proxy(&self) -> &AgentPtyProxy<'static> {
        &self.proxy
    }

    pub fn claw_proxy(&self) -> &AgentClawProxy<'static> {
        &self.claw_proxy
    }

    pub async fn get_preferred_shell(&self) -> Result<String> {
        self.proxy
            .get_preferred_shell()
            .await
            .context("Agent get_preferred_shell failed")
    }

    pub async fn create_pty(&self) -> Result<OwnedFd> {
        self.proxy
            .create_pty()
            .await
            .context("Agent create_pty failed")
    }

    pub async fn spawn_process(&self, pty_master: OwnedFd, options: SpawnOptions) -> Result<u32> {
        self.proxy
            .spawn(pty_master, options)
            .await
            .context("Agent spawn failed")
    }

    pub async fn get_cwd(&self, pid: u32) -> Result<String> {
        self.proxy
            .get_cwd(pid)
            .await
            .context("Agent get_cwd failed")
    }

    pub async fn get_foreground_process(&self, pid: u32) -> Result<String> {
        self.proxy
            .get_foreground_process(pid)
            .await
            .context("Agent get_foreground_process failed")
    }

    pub async fn get_running_processes(&self, pid: u32) -> Result<Vec<(u32, String)>> {
        self.proxy
            .get_running_processes(pid)
            .await
            .context("Agent get_running_processes failed")
    }

    pub async fn set_foreground_tracking(&self, pid: u32, enabled: bool) -> Result<()> {
        self.proxy
            .set_foreground_tracking(pid, enabled)
            .await
            .context("Agent set_foreground_tracking failed")
    }

    pub async fn signal_process_group(&self, pid: u32, signal: i32) -> Result<()> {
        self.proxy
            .signal_process_group(pid, signal)
            .await
            .context("Agent signal_process_group failed")
    }

    pub async fn set_persistence(&self, pid: u32, enabled: bool) -> Result<()> {
        self.proxy
            .set_persistence(pid, enabled)
            .await
            .context("Agent set_persistence failed")
    }

    /// Returns the raw `DetachOutcome` code: 0=Terminated, 1=Detached,
    /// 2=StillViewed, 3=DetachedUnbuffered, u32::MAX=Unknown.
    pub async fn detach(&self, pid: u32) -> Result<u32> {
        self.proxy.detach(pid).await.context("Agent detach failed")
    }

    pub async fn list_detached_sessions(&self) -> Result<Vec<(u32, String, u64)>> {
        self.proxy
            .list_detached_sessions()
            .await
            .context("Agent list_detached_sessions failed")
    }

    pub async fn reattach(&self, pid: u32) -> Result<(Vec<u8>, OwnedFd)> {
        self.proxy
            .reattach(pid)
            .await
            .context("Agent reattach failed")
    }
}

/// Implementation of ClawEnvironment that forwards calls over D-Bus.
/// Used by scenarios or UI-side code that still needs to run Claw sessions locally.
pub struct DbusClawEnvironment {
    proxy: AgentClawProxy<'static>,
}

impl DbusClawEnvironment {
    pub fn new(proxy: AgentClawProxy<'static>) -> Self {
        Self { proxy }
    }
}

#[async_trait::async_trait]
impl ClawEnvironment for DbusClawEnvironment {
    async fn exec_shell(&self, command: String) -> Result<(i32, String, String)> {
        self.proxy
            .exec_shell(command)
            .await
            .context("D-Bus exec_shell failed")
    }

    async fn read_file(&self, path: String, start_line: u32, end_line: u32) -> Result<String> {
        self.proxy
            .read_file(path, start_line, end_line)
            .await
            .context("D-Bus read_file failed")
    }

    async fn write_file(&self, path: String, content: String) -> Result<()> {
        self.proxy
            .write_file(path, content)
            .await
            .context("D-Bus write_file failed")
    }

    async fn list_directory(&self, path: String) -> Result<Vec<(String, bool, u64)>> {
        self.proxy
            .list_directory(path)
            .await
            .context("D-Bus list_directory failed")
    }

    async fn delete_file(&self, path: String) -> Result<()> {
        self.proxy
            .delete_file(path)
            .await
            .context("D-Bus delete_file failed")
    }

    async fn get_system_info(&self) -> Result<String> {
        self.proxy
            .get_system_info()
            .await
            .context("D-Bus get_system_info failed")
    }

    async fn list_processes(&self) -> Result<Vec<(u32, String, f64, u64, u64, u64)>> {
        self.proxy
            .list_processes()
            .await
            .context("D-Bus list_processes failed")
    }

    async fn kill_process(&self, pid: u32, signal: i32) -> Result<()> {
        self.proxy
            .kill_process(pid, signal)
            .await
            .context("D-Bus kill_process failed")
    }

    async fn get_clipboard(&self) -> Result<String> {
        self.proxy
            .get_clipboard()
            .await
            .context("D-Bus get_clipboard failed")
    }

    async fn set_clipboard(&self, text: String) -> Result<()> {
        self.proxy
            .set_clipboard(text)
            .await
            .context("D-Bus set_clipboard failed")
    }
}
