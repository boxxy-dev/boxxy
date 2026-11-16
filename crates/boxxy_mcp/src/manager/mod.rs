mod breaker;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use log::{debug, error, info, warn};
use parking_lot::Mutex as SyncMutex;
use rmcp::RoleClient;
use rmcp::model::Tool as McpToolDefinition;
use rmcp::service::RunningService;
use tokio::sync::{Mutex as AsyncMutex, OnceCell, RwLock};

pub use self::breaker::CircuitBreakerOpenError;
use self::breaker::{CircuitBreakerState, PERMANENT_CONFIG_COOLDOWN, TRANSIENT_COOLDOWN};
use crate::client::error::{
    format_service_error, is_anyhow_connection_refused, is_permanent_failure, parse_retry_after_ms,
};
use crate::client::handler::BoxxyClientHandler;
use crate::client::http::{is_loopback_http_url, probe_loopback_http_port};
use crate::config::{MIN_TIMEOUT_MS, McpServerConfig, McpTransport};
use crate::rig_bridge::tool::{DynamicMcpTool, McpToolObserver};

pub type McpClientWrapper = RunningService<RoleClient, BoxxyClientHandler>;

/// Grace period for `RunningService::close_with_timeout`: accommodates `rmcp 3.10`'s
/// 3s stdin-EOF wait + `SIGTERM` + 3s wait on Stdio and 5s `DELETE` cleanup on HTTP.
const GRACEFUL_CLOSE_TIMEOUT: Duration = Duration::from_secs(7);

pub struct McpClientManager {
    self_weak: Weak<Self>,
    configs: RwLock<HashMap<String, McpServerConfig>>,
    active_clients: RwLock<HashMap<String, Arc<McpClientWrapper>>>,
    tool_cache: RwLock<HashMap<String, Vec<McpToolDefinition>>>,
    failed_servers: RwLock<HashMap<String, CircuitBreakerState>>,
    /// Per-server single-flight mutex preventing duplicate process spawns / HTTP handshakes
    /// when concurrent callers invoke `ensure_client` for the same disconnected server.
    connect_locks: SyncMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    /// Monotonic per-server generation counter preventing an in-flight `list_all_tools`
    /// response from overwriting a concurrent `notifications/tools/list_changed` invalidation.
    cache_generations: SyncMutex<HashMap<String, u64>>,
}

static MANAGER: OnceCell<Arc<McpClientManager>> = OnceCell::const_new();

pub async fn global_manager() -> Arc<McpClientManager> {
    MANAGER
        .get_or_init(|| async { McpClientManager::new() })
        .await
        .clone()
}

impl McpClientManager {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            self_weak: weak.clone(),
            configs: RwLock::new(HashMap::new()),
            active_clients: RwLock::new(HashMap::new()),
            tool_cache: RwLock::new(HashMap::new()),
            failed_servers: RwLock::new(HashMap::new()),
            connect_locks: SyncMutex::new(HashMap::new()),
            cache_generations: SyncMutex::new(HashMap::new()),
        })
    }

    fn server_connect_lock(&self, server_name: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self.connect_locks.lock();
        if let Some(existing) = locks.get(server_name) {
            return existing.clone();
        }
        let created = Arc::new(AsyncMutex::new(()));
        locks.insert(server_name.to_string(), created.clone());
        created
    }

    fn current_generation(&self, server_name: &str) -> u64 {
        self.cache_generations
            .lock()
            .get(server_name)
            .copied()
            .unwrap_or(0)
    }

    fn bump_generation(&self, server_name: &str) {
        let mut gens = self.cache_generations.lock();
        let entry = gens.entry(server_name.to_string()).or_insert(0);
        *entry = entry.wrapping_add(1);
        drop(gens);
    }

    fn shutdown_client_gracefully(name: &str, client: Arc<McpClientWrapper>) {
        match Arc::try_unwrap(client) {
            Ok(mut owned_client) => {
                let server_name = name.to_string();
                tokio::spawn(async move {
                    if let Err(join_err) = owned_client
                        .close_with_timeout(GRACEFUL_CLOSE_TIMEOUT)
                        .await
                    {
                        debug!(
                            "MCP server '{server_name}' background task ended during close: {join_err}"
                        );
                    }
                });
            }
            Err(shared_client) => {
                // An in-flight `call_tool` temporarily holds a clone; cancel immediately so
                // the call aborts and `ChildWithCleanup::drop` reaps any child process.
                shared_client.cancellation_token().cancel();
            }
        }
    }

    /// Synchronizes the active server configurations with the user's preferences.
    ///
    /// Disconnects and invalidates cached tools for any server that was removed,
    /// disabled, or had its transport/timeout configuration modified.
    pub async fn update_configs(&self, servers: Vec<McpServerConfig>) {
        let mut to_disconnect: Vec<String> = Vec::new();
        let mut to_reset_breaker: Vec<String> = Vec::new();
        let mut removed_completely: Vec<String> = Vec::new();

        {
            let mut configs = self.configs.write().await;
            let mut new_configs = HashMap::with_capacity(servers.len());

            for server in servers {
                if server.enabled {
                    new_configs.insert(server.name.clone(), server);
                }
            }

            for (old_name, old_cfg) in configs.iter() {
                match new_configs.get(old_name) {
                    None => {
                        to_disconnect.push(old_name.clone());
                        to_reset_breaker.push(old_name.clone());
                        removed_completely.push(old_name.clone());
                    }
                    Some(new_cfg) if new_cfg != old_cfg => {
                        to_disconnect.push(old_name.clone());
                        to_reset_breaker.push(old_name.clone());
                    }
                    Some(_) => {}
                }
            }

            for new_name in new_configs.keys() {
                if !configs.contains_key(new_name) {
                    to_reset_breaker.push(new_name.clone());
                }
            }

            *configs = new_configs;
        }

        if !to_reset_breaker.is_empty() {
            let mut failed = self.failed_servers.write().await;
            for name in &to_reset_breaker {
                failed.remove(name);
            }
        }

        if to_disconnect.is_empty() {
            return;
        }

        for name in &to_disconnect {
            self.bump_generation(name);
        }

        if !removed_completely.is_empty() {
            let mut locks = self.connect_locks.lock();
            for name in &removed_completely {
                if locks.get(name).is_some_and(|arc| Arc::strong_count(arc) == 1) {
                    locks.remove(name);
                }
            }
        }

        {
            let mut cache = self.tool_cache.write().await;
            for name in &to_disconnect {
                cache.remove(name);
            }
        }

        let mut removed_clients = Vec::new();
        {
            let mut active_clients = self.active_clients.write().await;
            for name in to_disconnect {
                if let Some(client) = active_clients.remove(&name) {
                    removed_clients.push((name, client));
                }
            }
        }

        for (name, client) in removed_clients {
            info!("Disconnecting MCP server: {name}");
            Self::shutdown_client_gracefully(&name, client);
        }
    }

    /// Invalidates the local tool cache for a specific server (called when
    /// `notifications/tools/list_changed` is received from the peer).
    pub async fn invalidate_tool_cache(&self, server_name: &str) {
        self.bump_generation(server_name);
        self.tool_cache.write().await.remove(server_name);
    }

    /// Ensures a live client connection exists for `server_name`, coalescing concurrent
    /// callers onto a single connection attempt and reconnecting with exponential backoff
    /// if the previous session was closed or never started.
    ///
    /// # Errors
    ///
    /// Returns [`CircuitBreakerOpenError`] if the server is in cooldown, an error if
    /// `server_name` is unconfigured, or the underlying transport/initialization error.
    pub async fn ensure_client(&self, server_name: &str) -> anyhow::Result<Arc<McpClientWrapper>> {
        {
            let clients = self.active_clients.read().await;
            if let Some(client) = clients.get(server_name)
                && !client.is_closed()
                && !client.is_transport_closed()
            {
                return Ok(client.clone());
            }
        }

        // Single-flight gate per server name: prevents duplicate Stdio child spawns or
        // racing HTTP sessions when multiple panes or parallel tool calls hit a cold server.
        let connect_gate = self.server_connect_lock(server_name);
        let _connect_guard = connect_gate.lock().await;

        let stale_dead_client = {
            let mut clients = self.active_clients.write().await;
            if let Some(client) = clients.get(server_name) {
                if !client.is_closed() && !client.is_transport_closed() {
                    return Ok(client.clone());
                }
                clients.remove(server_name)
            } else {
                None
            }
        };

        if let Some(dead) = stale_dead_client {
            self.bump_generation(server_name);
            self.tool_cache.write().await.remove(server_name);
            Self::shutdown_client_gracefully(server_name, dead);
        }

        {
            let failed = self.failed_servers.read().await;
            if let Some(breaker) = failed.get(server_name)
                && let Some(open_err) = breaker.open_error(server_name)
            {
                return Err(anyhow::Error::new(open_err));
            }
        }

        let config = self
            .configs
            .read()
            .await
            .get(server_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("MCP server '{server_name}' is not configured"))?;

        self.connect_with_retries(server_name, &config).await
    }

    async fn connect_with_retries(
        &self,
        server_name: &str,
        config: &McpServerConfig,
    ) -> anyhow::Result<Arc<McpClientWrapper>> {
        info!("MCP: Establishing connection to '{server_name}'...");

        let attempts = config.effective_attempts();
        let mut delay_ms = config.backoff.effective_initial_ms();
        let max_delay_ms = config.backoff.effective_max_ms();

        let mut last_err = None;
        let mut permanent_failure = false;

        for attempt in 1..=attempts {
            let handler = BoxxyClientHandler::new(server_name, self.self_weak.clone());
            let res = match &config.transport {
                McpTransport::Stdio { command, args, env } => {
                    crate::client::stdio::build_stdio_client(
                        handler,
                        command,
                        args,
                        env,
                        config.timeout_ms,
                    )
                    .await
                }
                McpTransport::Http { url, headers, .. } => {
                    crate::client::http::build_streamable_http_client(
                        handler,
                        url,
                        headers,
                        config.timeout_ms,
                    )
                    .await
                }
            };

            match res {
                Ok(client) => {
                    let client_arc = Arc::new(client);
                    self.bump_generation(server_name);
                    self.failed_servers.write().await.remove(server_name);
                    self.active_clients
                        .write()
                        .await
                        .insert(server_name.to_string(), client_arc.clone());
                    return Ok(client_arc);
                }
                Err(e) => {
                    let is_perm = is_permanent_failure(&e);
                    let is_local_offline = matches!(
                        &config.transport,
                        McpTransport::Http { url, .. } if is_loopback_http_url(url)
                    ) && is_anyhow_connection_refused(&e);

                    let server_retry_ms = parse_retry_after_ms(&e, delay_ms, max_delay_ms);

                    last_err = Some(e);
                    if is_perm {
                        permanent_failure = true;
                        warn!(
                            "MCP connection to '{server_name}' failed with permanent configuration/auth error; skipping retries."
                        );
                        break;
                    }
                    if is_local_offline {
                        warn!(
                            "Local MCP server '{server_name}' is offline (Connection refused); skipping retry backoff."
                        );
                        break;
                    }
                    if attempt < attempts {
                        let wait_ms = server_retry_ms.unwrap_or(delay_ms);
                        warn!(
                            "MCP connection attempt {attempt}/{attempts} for '{server_name}' failed; retrying in {wait_ms}ms..."
                        );
                        tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                        delay_ms = config.backoff.next_delay_ms(delay_ms);
                    }
                }
            }
        }

        let final_err = last_err
            .unwrap_or_else(|| anyhow::anyhow!("Failed to connect to MCP server '{server_name}'"));

        let cooldown = if permanent_failure {
            PERMANENT_CONFIG_COOLDOWN
        } else {
            TRANSIENT_COOLDOWN
        };
        self.failed_servers.write().await.insert(
            server_name.to_string(),
            CircuitBreakerState {
                tripped_at: Instant::now(),
                cooldown,
                last_error: final_err.to_string(),
            },
        );

        Err(final_err)
    }

    /// Returns the cached tools for a server. If not cached (or if the underlying client
    /// connection has closed), connects and fetches all tool pages via `list_all_tools()`.
    pub async fn get_tools(&self, server_name: &str) -> Option<Vec<McpToolDefinition>> {
        let client = match self.ensure_client(server_name).await {
            Ok(c) => c,
            Err(e) => {
                if e.downcast_ref::<CircuitBreakerOpenError>().is_some() {
                    debug!("{e}");
                } else {
                    error!("Failed to connect to MCP server '{server_name}': {e}");
                }
                return None;
            }
        };

        let cached = self.tool_cache.read().await.get(server_name).cloned();
        if let Some(tools) = cached {
            return Some(tools);
        }

        let start_gen = self.current_generation(server_name);

        // Do not send `tools/list` to an MCP server whose handshake declared no tools capability.
        if client
            .peer_info()
            .is_some_and(|info| info.capabilities.tools.is_none())
        {
            self.commit_tools_if_current(server_name, &client, start_gen, Vec::new())
                .await;
            return Some(Vec::new());
        }

        let timeout = self
            .configs
            .read()
            .await
            .get(server_name)
            .map_or_else(|| Duration::from_millis(60_000), McpServerConfig::effective_timeout);

        let tools = match tokio::time::timeout(timeout, client.list_all_tools()).await {
            Ok(Ok(tools)) => tools,
            Ok(Err(e)) => {
                error!(
                    "Failed to list tools for MCP server '{server_name}': {}",
                    format_service_error(&e)
                );
                return None;
            }
            Err(_) => {
                error!(
                    "Timed out after {}ms listing tools for MCP server '{server_name}'",
                    timeout.as_millis()
                );
                return None;
            }
        };

        self.commit_tools_if_current(server_name, &client, start_gen, tools.clone())
            .await;

        Some(tools)
    }

    async fn commit_tools_if_current(
        &self,
        server_name: &str,
        client: &Arc<McpClientWrapper>,
        expected_gen: u64,
        tools: Vec<McpToolDefinition>,
    ) {
        let still_same_client = self
            .active_clients
            .read()
            .await
            .get(server_name)
            .is_some_and(|cur| Arc::ptr_eq(cur, client));
        if !still_same_client {
            return;
        }

        if self.current_generation(server_name) == expected_gen {
            self.tool_cache
                .write()
                .await
                .insert(server_name.to_string(), tools);
        } else {
            debug!(
                "MCP: Skipping stale tool-list cache commit for '{server_name}' after concurrent invalidation"
            );
        }
    }

    async fn check_single_server_status(&self, name: String, cfg: McpServerConfig) -> (String, bool) {
        let loopback_url = match &cfg.transport {
            McpTransport::Http { url, .. } if is_loopback_http_url(url) => Some(url.as_str()),
            _ => None,
        };

        let already_live = self
            .active_clients
            .read()
            .await
            .get(&name)
            .is_some_and(|c| !c.is_closed() && !c.is_transport_closed());

        if let Some(url) = loopback_url {
            let port_open = probe_loopback_http_port(url).await;
            if already_live && !port_open {
                info!("MCP: Local server '{name}' is no longer listening; disconnecting.");
                self.bump_generation(&name);
                let removed = self.active_clients.write().await.remove(&name);
                self.tool_cache.write().await.remove(&name);
                if let Some(client) = removed {
                    Self::shutdown_client_gracefully(&name, client);
                }
                self.failed_servers.write().await.insert(
                    name.clone(),
                    CircuitBreakerState {
                        tripped_at: Instant::now(),
                        cooldown: TRANSIENT_COOLDOWN,
                        last_error: "Local server stopped listening".to_string(),
                    },
                );
                return (name, false);
            } else if !already_live && port_open {
                self.failed_servers.write().await.remove(&name);
            } else if !already_live && !port_open {
                self.failed_servers
                    .write()
                    .await
                    .entry(name.clone())
                    .or_insert_with(|| CircuitBreakerState {
                        tripped_at: Instant::now(),
                        cooldown: TRANSIENT_COOLDOWN,
                        last_error: "Connection refused".to_string(),
                    });
                return (name, false);
            }
        }

        if already_live {
            return (name, true);
        }
        let connected = self.ensure_client(&name).await.is_ok();
        (name, connected)
    }

    /// Returns `[(server_name, is_connected)]` sorted alphabetically for all enabled MCP servers,
    /// probing all configured servers concurrently.
    pub async fn get_server_statuses(&self) -> Vec<(String, bool)> {
        let mut entries: Vec<(String, McpServerConfig)> = self
            .configs
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        if entries.is_empty() {
            return Vec::new();
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        join_all(
            entries
                .into_iter()
                .map(|(name, cfg)| self.check_single_server_status(name, cfg)),
        )
        .await
    }

    /// Builds `rig::tool::DynamicTool` instances for all enabled MCP servers concurrently
    /// while preserving deterministic `(server_name, tool)` ordering for LLM prompt prefix caching.
    ///
    /// Pass `observer` so MCP tool invocations report start/result events to the
    /// Claw indicator badge, read-only sidebar debug log, and telemetry.
    pub async fn build_rig_tools(
        &self,
        observer: Option<Arc<dyn McpToolObserver>>,
    ) -> Vec<rig::tool::DynamicTool> {
        let mut server_configs: Vec<(String, u64)> = self
            .configs
            .read()
            .await
            .values()
            .map(|c| (c.name.clone(), c.timeout_ms.max(MIN_TIMEOUT_MS)))
            .collect();

        if server_configs.is_empty() {
            return Vec::new();
        }
        server_configs.sort_by(|a, b| a.0.cmp(&b.0));

        let fetched = join_all(server_configs.into_iter().map(|(name, timeout_ms)| async move {
            let tools = self.get_tools(&name).await;
            (name, timeout_ms, tools)
        }))
        .await;

        let mut rig_tools: Vec<rig::tool::DynamicTool> = Vec::new();
        let mut seen_names = HashSet::new();
        let manager_arc = self.self_weak.upgrade();

        for (name, timeout_ms, tools_opt) in fetched {
            let Some(tools) = tools_opt else {
                continue;
            };
            let client_weak = self
                .active_clients
                .read()
                .await
                .get(&name)
                .map(Arc::downgrade);

            let Some(fallback_client) = client_weak else {
                continue;
            };

            for mcp_tool in tools {
                let dynamic_tool = DynamicMcpTool {
                    manager: manager_arc.clone(),
                    fallback_client: fallback_client.clone(),
                    mcp_tool,
                    server_name: name.clone(),
                    timeout_ms,
                    observer: observer.clone(),
                };

                let tool_name = dynamic_tool.name();
                if !seen_names.insert(tool_name.clone()) {
                    warn!(
                        "MCP: Skipping duplicate tool name '{tool_name}' from server '{name}'"
                    );
                    continue;
                }

                if let Some(tool) = dynamic_tool.to_dynamic_tool() {
                    rig_tools.push(tool);
                }
            }
        }

        rig_tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::error::PermanentConfigError;
    use crate::config::BackoffConfig;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sample_stdio_config(name: &str, cmd: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransport::Stdio {
                command: cmd.to_string(),
                args: vec![],
                env: HashMap::new(),
            },
            enabled: true,
            timeout_ms: 5_000,
            max_retries: 1,
            backoff: BackoffConfig::default(),
        }
    }

    #[tokio::test]
    async fn update_configs_filters_disabled_and_invalidates_modified() {
        let manager = McpClientManager::new();

        let cfg1 = sample_stdio_config("alpha", "echo");
        let mut cfg2 = sample_stdio_config("beta", "echo");
        cfg2.enabled = false;

        manager.update_configs(vec![cfg1.clone(), cfg2]).await;

        assert!(manager.configs.read().await.contains_key("alpha"));
        assert!(!manager.configs.read().await.contains_key("beta"));

        manager
            .tool_cache
            .write()
            .await
            .insert("alpha".to_string(), vec![]);

        // Updating with identical config preserves the tool cache.
        manager.update_configs(vec![cfg1]).await;
        assert!(manager.tool_cache.read().await.contains_key("alpha"));

        // Modifying transport command invalidates the cache and bumps generation.
        let gen_before = manager.current_generation("alpha");
        let modified = sample_stdio_config("alpha", "cat");
        manager.update_configs(vec![modified]).await;
        assert!(!manager.tool_cache.read().await.contains_key("alpha"));
        assert!(manager.current_generation("alpha") > gen_before);
    }

    #[tokio::test]
    async fn tool_list_changed_notification_invalidates_manager_tool_cache_and_blocks_stale_commit() {
        use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
        use serde_json::json;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let manager = McpClientManager::new();
        manager
            .tool_cache
            .write()
            .await
            .insert("live-srv".to_string(), vec![]);

        let (server_io, client_io) = tokio::io::duplex(8192);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let mut reader = BufReader::new(server_read).lines();

        let server_task = tokio::spawn(async move {
            let line = reader.next_line().await.unwrap().expect("discover line");
            let req: serde_json::Value = serde_json::from_str(&line).unwrap();
            let discover_resp = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {
                    "resultType": "complete",
                    "supportedVersions": ["2026-07-28"],
                    "capabilities": { "tools": { "listChanged": true } },
                    "ttlMs": 0,
                    "cacheScope": "private"
                }
            });
            server_write
                .write_all(format!("{discover_resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let notif = json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed"
            });
            server_write
                .write_all(format!("{notif}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let stale_gen = manager.current_generation("live-srv");
        let handler = BoxxyClientHandler::new("live-srv", Arc::downgrade(&manager));
        let client = Arc::new(
            serve_client_with_lifecycle(
                handler,
                client_io,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("discover succeeded"),
        );
        manager
            .active_clients
            .write()
            .await
            .insert("live-srv".to_string(), client.clone());

        server_task.await.unwrap();

        for _ in 0..20 {
            if !manager.tool_cache.read().await.contains_key("live-srv") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            !manager.tool_cache.read().await.contains_key("live-srv"),
            "tool_cache must be invalidated on notifications/tools/list_changed"
        );

        // A late in-flight `tools/list` started under `stale_gen` must NOT repopulate the cache.
        manager
            .commit_tools_if_current("live-srv", &client, stale_gen, vec![])
            .await;
        assert!(
            !manager.tool_cache.read().await.contains_key("live-srv"),
            "stale generation commit must be rejected"
        );
    }

    #[tokio::test]
    async fn single_flight_ensure_client_coalesces_concurrent_connection_attempts() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted_connections = Arc::new(AtomicUsize::new(0));
        let accepted_clone = accepted_connections.clone();

        let server_task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                accepted_clone.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    let resp = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"mcp\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        let manager = McpClientManager::new();
        manager
            .update_configs(vec![McpServerConfig {
                name: "coalesced-srv".to_string(),
                transport: McpTransport::Http {
                    url: format!("http://{addr}/mcp"),
                    headers: HashMap::new(),
                    streamable: true,
                },
                enabled: true,
                timeout_ms: 3_000,
                max_retries: 3,
                backoff: BackoffConfig::default(),
            }])
            .await;

        let results = join_all((0..6).map(|_| manager.ensure_client("coalesced-srv"))).await;
        assert!(results.iter().all(Result::is_err));
        assert_eq!(
            accepted_connections.load(Ordering::SeqCst),
            1,
            "Concurrent ensure_client calls must coalesce onto a single handshake"
        );

        server_task.abort();
    }

    #[tokio::test]
    async fn permanent_config_error_fails_fast_trips_circuit_breaker_and_resets_on_update() {
        let manager = McpClientManager::new();
        let bad_http_cfg = McpServerConfig {
            name: "bad-scheme-srv".to_string(),
            transport: McpTransport::Http {
                url: "tcp://example.com:9000".to_string(),
                headers: HashMap::new(),
                streamable: true,
            },
            enabled: true,
            timeout_ms: 5_000,
            max_retries: 3,
            backoff: BackoffConfig {
                initial_ms: 1_000,
                max_ms: 5_000,
                multiplier: 2.0,
            },
        };

        manager.update_configs(vec![bad_http_cfg.clone()]).await;

        let start = Instant::now();
        let err1 = manager
            .ensure_client("bad-scheme-srv")
            .await
            .err()
            .expect("non-http scheme must fail");
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "Permanent config error should skip retry sleeps"
        );
        assert!(err1.downcast_ref::<PermanentConfigError>().is_some());

        assert!(
            manager
                .failed_servers
                .read()
                .await
                .contains_key("bad-scheme-srv")
        );

        let err2 = manager
            .ensure_client("bad-scheme-srv")
            .await
            .err()
            .expect("circuit breaker should short-circuit");
        assert!(err2.downcast_ref::<CircuitBreakerOpenError>().is_some());

        let updated_cfg = McpServerConfig {
            name: "bad-scheme-srv".to_string(),
            transport: McpTransport::Http {
                url: "https://example.com/mcp".to_string(),
                headers: HashMap::new(),
                streamable: true,
            },
            ..bad_http_cfg
        };
        manager.update_configs(vec![updated_cfg]).await;
        assert!(
            !manager
                .failed_servers
                .read()
                .await
                .contains_key("bad-scheme-srv"),
            "Updating config must clear the circuit breaker"
        );
    }

    #[tokio::test]
    async fn offline_loopback_http_server_fails_fast_without_retry_backoff() {
        let closed_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };

        let manager = McpClientManager::new();
        let offline_local_cfg = McpServerConfig {
            name: "offline-local-srv".to_string(),
            transport: McpTransport::Http {
                url: format!("http://127.0.0.1:{closed_port}/mcp"),
                headers: HashMap::new(),
                streamable: true,
            },
            enabled: true,
            timeout_ms: 5_000,
            max_retries: 3,
            backoff: BackoffConfig {
                initial_ms: 1_000,
                max_ms: 5_000,
                multiplier: 2.0,
            },
        };

        manager.update_configs(vec![offline_local_cfg]).await;

        let start = Instant::now();
        let statuses = manager.get_server_statuses().await;
        assert_eq!(statuses, vec![("offline-local-srv".to_string(), false)]);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "Offline loopback server must skip 3,000ms retry sleeps (elapsed: {:?})",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn resource_only_server_skips_tools_list_request_and_caches_empty() {
        use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
        use serde_json::json;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let manager = McpClientManager::new();
        let (server_io, client_io) = tokio::io::duplex(8192);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let mut reader = BufReader::new(server_read).lines();

        let server_task = tokio::spawn(async move {
            let line = reader.next_line().await.unwrap().expect("discover line");
            let req: serde_json::Value = serde_json::from_str(&line).unwrap();
            let discover_resp = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {
                    "resultType": "complete",
                    "supportedVersions": ["2026-07-28"],
                    "capabilities": { "resources": {} },
                    "ttlMs": 0,
                    "cacheScope": "private"
                }
            });
            server_write
                .write_all(format!("{discover_resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
            // Server deliberately handles NO `tools/list` message; if client sent one, `get_tools` would fail!
        });

        let handler = BoxxyClientHandler::new("res-only", Arc::downgrade(&manager));
        let client = Arc::new(
            serve_client_with_lifecycle(
                handler,
                client_io,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("discover succeeded"),
        );
        server_task.await.unwrap();

        manager
            .configs
            .write()
            .await
            .insert("res-only".to_string(), sample_stdio_config("res-only", "echo"));
        manager
            .active_clients
            .write()
            .await
            .insert("res-only".to_string(), client);

        let tools = manager.get_tools("res-only").await.expect("empty tools vec");
        assert_eq!(tools, [] as [rmcp::model::Tool; 0]);
        assert_eq!(manager.tool_cache.read().await.get("res-only"), Some(&vec![]));
    }
}
