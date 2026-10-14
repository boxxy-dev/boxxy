use anyhow::{Context, Result};
use boxxy_agent::ipc::AgentProxy;
use boxxy_agent::ipc::claw::AgentClawProxy;
use boxxy_agent::ipc::pty::{AgentPtyProxy, SpawnOptions};
use boxxy_claw_protocol::{ClawEngineEvent, ClawEnvironment, ClawMessage};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use zbus::Connection;
use zbus::zvariant::OwnedFd;

static NEXT_SUBSCRIPTION_ID: AtomicU64 = AtomicU64::new(1);

/// RAII guard that removes a registered listener from its `SignalRouter`
/// as soon as the owning pane or subscriber is dropped.
pub struct SubscriptionGuard {
    cleanup: Option<Box<dyn FnOnce() + Send>>,
}

impl SubscriptionGuard {
    pub fn new(cleanup: impl FnOnce() + Send + 'static) -> Self {
        Self {
            cleanup: Some(Box::new(cleanup)),
        }
    }
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

/// Thread-safe, lock-free-await signal router that multiplexes a single D-Bus
/// signal stream to keyed per-pane channels.
///
/// Uses `std::sync::Mutex` (never held across `.await`) + unbounded `try_send`
/// so `SubscriptionGuard::drop` can deterministically remove subscribers
/// synchronously, dropping the channel `Sender` and immediately waking any
/// awaiting `Receiver::recv().await` task on pane close.
pub struct SignalRouter<K, V> {
    subscribers: Arc<Mutex<HashMap<K, Vec<(u64, async_channel::Sender<V>)>>>>,
}

impl<K, V> Clone for SignalRouter<K, V> {
    fn clone(&self) -> Self {
        Self {
            subscribers: self.subscribers.clone(),
        }
    }
}

impl<K, V> Default for SignalRouter<K, V>
where
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Send + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> SignalRouter<K, V>
where
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Send + 'static,
{
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers an existing `Sender<V>` under `key` and returns an RAII
    /// `SubscriptionGuard` that removes it on drop.
    pub fn subscribe_with_sender(
        &self,
        key: K,
        tx: async_channel::Sender<V>,
    ) -> SubscriptionGuard {
        let sub_id = NEXT_SUBSCRIPTION_ID.fetch_add(1, Ordering::Relaxed);
        {
            let mut map = self.subscribers.lock().unwrap();
            map.entry(key.clone()).or_default().push((sub_id, tx));
        }

        let subscribers = self.subscribers.clone();
        SubscriptionGuard::new(move || {
            let mut map = subscribers.lock().unwrap();
            if let Some(list) = map.get_mut(&key) {
                list.retain(|(id, _)| *id != sub_id);
                if list.is_empty() {
                    map.remove(&key);
                }
            }
        })
    }

    /// Creates a new unbounded channel, registers its `Sender` under `key`,
    /// and returns `(Receiver<V>, SubscriptionGuard)`. When the guard drops,
    /// the stored `Sender` is dropped so `rx.recv().await` terminates immediately.
    pub fn subscribe(&self, key: K) -> (async_channel::Receiver<V>, SubscriptionGuard) {
        let (tx, rx) = async_channel::unbounded();
        let guard = self.subscribe_with_sender(key, tx);
        (rx, guard)
    }

    /// Dispatches `value` to all active subscribers for `key`, automatically
    /// purging any closed channels.
    pub fn dispatch(&self, key: &K, value: V) {
        let mut map = self.subscribers.lock().unwrap();
        if let Some(list) = map.get_mut(key) {
            list.retain(|(_, tx)| !tx.is_closed() && tx.try_send(value.clone()).is_ok());
            if list.is_empty() {
                map.remove(key);
            }
        }
    }

    /// Dispatches `value` once and removes all subscribers for `key` (used for
    /// terminal one-shot events like process `Exited`).
    pub fn dispatch_once(&self, key: &K, value: V) {
        let removed = {
            let mut map = self.subscribers.lock().unwrap();
            map.remove(key)
        };
        if let Some(list) = removed {
            for (_, tx) in list {
                let _ = tx.try_send(value.clone());
            }
        }
    }

    /// Removes all subscribers for `key` (dropping their `Sender`s).
    pub fn remove_key(&self, key: &K) {
        self.subscribers.lock().unwrap().remove(key);
    }

    /// Returns a snapshot of keys that currently have at least one live subscriber.
    pub fn active_keys(&self) -> Vec<K> {
        let mut map = self.subscribers.lock().unwrap();
        map.retain(|_, list| {
            list.retain(|(_, tx)| !tx.is_closed());
            !list.is_empty()
        });
        map.keys().cloned().collect()
    }

    #[cfg(test)]
    pub fn subscriber_count(&self, key: &K) -> usize {
        self.subscribers
            .lock()
            .unwrap()
            .get(key)
            .map(|l| l.len())
            .unwrap_or(0)
    }
}

/// Centralized D-Bus signal multiplexer and connection supervisor.
/// Maintains $O(1)$ session-bus match rules regardless of open pane count.
#[derive(Clone)]
pub struct AgentConnection {
    pub exited_router: SignalRouter<u32, i32>,
    pub fg_router: SignalRouter<u32, String>,
    pub claw_router: SignalRouter<String, ClawEngineEvent>,
}

impl AgentConnection {
    fn spawn_supervisors(
        agent_proxy: AgentProxy<'static>,
        pty_proxy: AgentPtyProxy<'static>,
    ) -> Self {
        let exited_router = SignalRouter::<u32, i32>::new();
        let fg_router = SignalRouter::<u32, String>::new();
        let claw_router = SignalRouter::<String, ClawEngineEvent>::new();

        // 1. Character Claims & Catalog Cache Supervisor
        {
            let proxy = agent_proxy.clone();
            tokio::spawn(async move {
                let mut last_seen_revision = 0u64;
                loop {
                    if let Ok(mut stream) = proxy.receive_claims_changed().await {
                        while let Some(msg) = stream.next().await {
                            if let Ok(args) = msg.args()
                                && let Ok(snapshot) = serde_json::from_str::<
                                    boxxy_claw_protocol::characters::RegistrySnapshot,
                                >(&args.snapshot_json)
                                && snapshot.revision > last_seen_revision
                            {
                                last_seen_revision = snapshot.revision;
                                boxxy_claw_protocol::characters::CLAIMS_CACHE
                                    .store(Arc::new(snapshot.claims));
                                boxxy_claw_protocol::characters::CHARACTER_CACHE
                                    .store(Arc::new(snapshot.catalog));
                            }
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    // Reconcile snapshot on stream reconnect
                    if let Ok(snapshot_json) = proxy.get_registry_snapshot().await
                        && let Ok(snapshot) = serde_json::from_str::<
                            boxxy_claw_protocol::characters::RegistrySnapshot,
                        >(&snapshot_json)
                    {
                        last_seen_revision = snapshot.revision;
                        boxxy_claw_protocol::characters::CLAIMS_CACHE
                            .store(Arc::new(snapshot.claims));
                        boxxy_claw_protocol::characters::CHARACTER_CACHE
                            .store(Arc::new(snapshot.catalog));
                    }
                }
            });
        }

        // 2. Desktop Notification Supervisor
        {
            let proxy = agent_proxy.clone();
            tokio::spawn(async move {
                loop {
                    if let Ok(mut stream) = proxy.receive_desktop_notification().await {
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
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
            });
        }

        // 3. Claw Session Event Multiplexer
        {
            let proxy = agent_proxy;
            let router = claw_router.clone();
            tokio::spawn(async move {
                loop {
                    if let Ok(mut stream) = proxy.receive_claw_event().await {
                        while let Some(msg) = stream.next().await {
                            if let Ok(args) = msg.args()
                                && let Ok(event) =
                                    serde_json::from_str::<ClawEngineEvent>(&args.event_json)
                            {
                                router.dispatch(&args.session_id.to_string(), event);
                            }
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
            });
        }

        // 4. PTY Process Exit Multiplexer + Reconnect State Reconciler
        {
            let proxy = pty_proxy.clone();
            let exit_router = exited_router.clone();
            let fg_cleanup = fg_router.clone();
            tokio::spawn(async move {
                loop {
                    if let Ok(mut stream) = proxy.receive_exited().await {
                        while let Some(signal) = stream.next().await {
                            if let Ok(args) = signal.args() {
                                fg_cleanup.remove_key(&args.pid);
                                exit_router.dispatch_once(&args.pid, args.exit_code);
                            }
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    // Reconcile any processes that exited while the stream was reconnecting
                    for pid in exit_router.active_keys() {
                        if let Ok((true, exit_code)) = proxy.get_exit_status(pid).await {
                            fg_cleanup.remove_key(&pid);
                            exit_router.dispatch_once(&pid, exit_code);
                        }
                    }
                }
            });
        }

        // 5. Foreground Process Changed Multiplexer
        {
            let proxy = pty_proxy;
            let router = fg_router.clone();
            tokio::spawn(async move {
                loop {
                    if let Ok(mut stream) = proxy.receive_foreground_process_changed().await {
                        while let Some(signal) = stream.next().await {
                            if let Ok(args) = signal.args() {
                                router.dispatch(&args.pid, args.process_name.to_string());
                            }
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
            });
        }

        Self {
            exited_router,
            fg_router,
            claw_router,
        }
    }
}

#[derive(Clone)]
pub struct AgentManager {
    agent_proxy: AgentProxy<'static>,
    proxy: AgentPtyProxy<'static>,
    claw_proxy: AgentClawProxy<'static>,
    connection: AgentConnection,
    /// Notification produced during init (e.g. DB reset). Stored here instead
    /// of sent to TERMINAL_EVENT_BUS immediately, because the bus is a
    /// broadcast channel that drops messages when there are no subscribers —
    /// and the window hasn't subscribed yet when AgentManager is initialized.
    startup_notification: Arc<Mutex<Option<String>>>,
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

        let connection = AgentConnection::spawn_supervisors(agent_proxy.clone(), proxy.clone());

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

        // Seed the UI character cache
        if let Ok(snapshot_json) = agent_proxy.get_registry_snapshot().await
            && let Ok(snapshot) = serde_json::from_str::<
                boxxy_claw_protocol::characters::RegistrySnapshot,
            >(&snapshot_json)
        {
            boxxy_claw_protocol::characters::CLAIMS_CACHE
                .store(Arc::new(snapshot.claims));
            boxxy_claw_protocol::characters::CHARACTER_CACHE
                .store(Arc::new(snapshot.catalog));
            log::info!("Seeded UI Character and Claims Cache on startup.");
        }

        Ok(Self {
            agent_proxy,
            proxy,
            claw_proxy,
            connection,
            startup_notification: Arc::new(Mutex::new(startup_notification)),
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
        api_keys: HashMap<String, String>,
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

    /// Subscribes to exit notifications for `pid` via the centralized
    /// `AgentConnection` router. If the daemon reports that `pid` has already
    /// exited, the exit code is dispatched immediately.
    pub async fn subscribe_process_exit(
        &self,
        pid: u32,
    ) -> (async_channel::Receiver<i32>, SubscriptionGuard) {
        // Register in router *before* querying exit status so we cannot miss
        // an `Exited` signal emitted concurrently with the query.
        let (rx, guard) = self.connection.exited_router.subscribe(pid);
        if let Ok((true, exit_code)) = self.proxy.get_exit_status(pid).await {
            self.connection.fg_router.remove_key(&pid);
            self.connection.exited_router.dispatch_once(&pid, exit_code);
        }
        (rx, guard)
    }

    /// Subscribes to foreground process changes for `pid` via the centralized
    /// `AgentConnection` router.
    pub fn subscribe_foreground_process(
        &self,
        pid: u32,
    ) -> (async_channel::Receiver<String>, SubscriptionGuard) {
        self.connection.fg_router.subscribe(pid)
    }

    /// Claims a character for `pane_id` and routes its `ClawEngineEvent`s
    /// directly into the provided `event_tx` via `AgentConnection::claw_router`.
    pub async fn try_claim_character_with_sender(
        &self,
        pane_id: String,
        holder_kind: u8,
        character_id: String,
        event_tx: async_channel::Sender<ClawEngineEvent>,
    ) -> Result<(String, SubscriptionGuard)> {
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

        let guard = self
            .connection
            .claw_router
            .subscribe_with_sender(session_id.clone(), event_tx);

        Ok((session_id, guard))
    }

    pub async fn create_claw_session_with_sender(
        &self,
        pane_id: String,
        character_id: String,
        event_tx: async_channel::Sender<ClawEngineEvent>,
    ) -> Result<(String, SubscriptionGuard)> {
        self.try_claim_character_with_sender(pane_id, 0, character_id, event_tx)
            .await
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

    async fn spawn_detached(&self, command: String, cwd: String) -> Result<u32> {
        self.proxy
            .spawn_detached(command, cwd)
            .await
            .context("D-Bus spawn_detached failed")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signal_router_guard_drop_removes_subscriber_and_closes_receiver() {
        let router = SignalRouter::<u32, String>::new();
        let (rx, guard) = router.subscribe(1001);
        assert_eq!(router.subscriber_count(&1001), 1);

        router.dispatch(&1001, "nvim".to_string());
        assert_eq!(rx.recv().await.unwrap(), "nvim");

        // Dropping the guard must remove the subscriber and drop its Sender
        // so `rx.recv().await` immediately resolves with Err.
        drop(guard);
        assert_eq!(router.subscriber_count(&1001), 0);
        assert!(rx.recv().await.is_err());
    }

    #[tokio::test]
    async fn signal_router_purges_closed_receiver_on_dispatch() {
        let router = SignalRouter::<String, i32>::new();
        let (tx, rx) = async_channel::unbounded();
        let _guard = router.subscribe_with_sender("session-1".to_string(), tx);
        assert_eq!(router.subscriber_count(&"session-1".to_string()), 1);

        rx.close();
        router.dispatch(&"session-1".to_string(), 42);
        assert_eq!(router.subscriber_count(&"session-1".to_string()), 0);
    }

    #[tokio::test]
    async fn signal_router_dispatch_once_delivers_and_clears_key() {
        let router = SignalRouter::<u32, i32>::new();
        let (rx1, _g1) = router.subscribe(77);
        let (rx2, _g2) = router.subscribe(77);
        assert_eq!(router.subscriber_count(&77), 2);

        router.dispatch_once(&77, 0);
        assert_eq!(router.subscriber_count(&77), 0);
        assert_eq!(rx1.recv().await.unwrap(), 0);
        assert_eq!(rx2.recv().await.unwrap(), 0);
    }
}
