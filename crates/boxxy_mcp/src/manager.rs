use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

use log::{error, info, warn};
use rmcp::model::Tool as McpToolDefinition;
use rmcp::{RoleClient, service::RunningService};
use tokio::sync::{OnceCell, RwLock};

use crate::client::handler::BoxxyClientHandler;
use crate::config::{McpServerConfig, McpTransport};
use crate::rig_bridge::tool::{DynamicMcpTool, McpToolObserver};

pub type McpClientWrapper = RunningService<RoleClient, BoxxyClientHandler>;

pub struct McpClientManager {
    self_weak: Weak<Self>,
    configs: RwLock<HashMap<String, McpServerConfig>>,
    active_clients: RwLock<HashMap<String, Arc<McpClientWrapper>>>,
    tool_cache: RwLock<HashMap<String, Vec<McpToolDefinition>>>,
}

static MANAGER: OnceCell<Arc<McpClientManager>> = OnceCell::const_new();

pub async fn global_manager() -> Arc<McpClientManager> {
    MANAGER
        .get_or_init(|| async { McpClientManager::new() })
        .await
        .clone()
}

impl McpClientManager {
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            self_weak: weak.clone(),
            configs: RwLock::new(HashMap::new()),
            active_clients: RwLock::new(HashMap::new()),
            tool_cache: RwLock::new(HashMap::new()),
        })
    }

    /// Synchronizes the active server configurations with the user's preferences.
    ///
    /// Disconnects and invalidates cached tools for any server that was removed,
    /// disabled, or had its transport/timeout configuration modified.
    pub async fn update_configs(&self, servers: Vec<McpServerConfig>) {
        let mut to_disconnect: Vec<String> = Vec::new();

        {
            let mut configs = self.configs.write().await;
            let mut new_configs = HashMap::new();

            for server in servers {
                if server.enabled {
                    new_configs.insert(server.name.clone(), server);
                }
            }

            for (old_name, old_cfg) in configs.iter() {
                match new_configs.get(old_name) {
                    None => to_disconnect.push(old_name.clone()),
                    Some(new_cfg) if new_cfg != old_cfg => {
                        to_disconnect.push(old_name.clone());
                    }
                    Some(_) => {}
                }
            }

            *configs = new_configs;
        }

        if to_disconnect.is_empty() {
            return;
        }

        let mut removed_clients = Vec::new();
        {
            let mut active_clients = self.active_clients.write().await;
            let mut cache = self.tool_cache.write().await;
            for name in to_disconnect {
                cache.remove(&name);
                if let Some(client) = active_clients.remove(&name) {
                    removed_clients.push((name, client));
                }
            }
        }

        for (name, client) in removed_clients {
            info!("Disconnecting MCP server: {name}");
            // Cancel the service cancellation token immediately in case DynamicMcpTool holds an Arc.
            client.cancellation_token().cancel();
            if let Ok(mut owned_client) = Arc::try_unwrap(client) {
                tokio::spawn(async move {
                    let _ = owned_client
                        .close_with_timeout(Duration::from_secs(3))
                        .await;
                });
            }
        }
    }

    /// Invalidates the local tool cache for a specific server (called when
    /// `notifications/tools/list_changed` is received from the peer).
    pub async fn invalidate_tool_cache(&self, server_name: &str) {
        let mut cache = self.tool_cache.write().await;
        cache.remove(server_name);
    }

    /// Ensures a live client connection exists for `server_name`, reconnecting with
    /// exponential backoff if the previous session was closed or never started.
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

        let config = {
            let configs = self.configs.read().await;
            configs
                .get(server_name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("MCP server '{server_name}' is not configured"))?
        };

        info!("MCP: Establishing connection to '{server_name}'...");

        let attempts = config.max_retries.max(1);
        let mut delay_ms = config.backoff.initial_ms.max(100);
        let max_delay_ms = config.backoff.max_ms.max(delay_ms);
        let multiplier = if config.backoff.multiplier > 1.0 {
            config.backoff.multiplier
        } else {
            2.0
        };

        let mut last_err = None;
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
                    let mut clients = self.active_clients.write().await;
                    clients.insert(server_name.to_string(), client_arc.clone());
                    return Ok(client_arc);
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt < attempts {
                        warn!(
                            "MCP connection attempt {attempt}/{attempts} for '{server_name}' failed; retrying in {delay_ms}ms..."
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        delay_ms = ((delay_ms as f64 * f64::from(multiplier)) as u64)
                            .min(max_delay_ms);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!("Failed to connect to MCP server '{server_name}'")
        }))
    }

    /// Returns the cached tools for a server. If not cached (or if the underlying client
    /// connection has closed), connects and fetches all tool pages via `list_all_tools()`.
    pub async fn get_tools(&self, server_name: &str) -> Option<Vec<McpToolDefinition>> {
        let client = match self.ensure_client(server_name).await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to MCP server '{server_name}': {e}");
                return None;
            }
        };

        {
            let cache = self.tool_cache.read().await;
            if let Some(tools) = cache.get(server_name) {
                return Some(tools.clone());
            }
        }

        let timeout_ms = {
            let configs = self.configs.read().await;
            configs
                .get(server_name)
                .map(|c| c.timeout_ms.max(1_000))
                .unwrap_or(60_000)
        };

        let tools = match tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            client.list_all_tools(),
        )
        .await
        {
            Ok(Ok(tools)) => tools,
            Ok(Err(e)) => {
                error!("Failed to list tools for MCP server '{server_name}': {e}");
                return None;
            }
            Err(_) => {
                error!(
                    "Timed out after {timeout_ms}ms listing tools for MCP server '{server_name}'"
                );
                return None;
            }
        };

        let mut cache = self.tool_cache.write().await;
        cache.insert(server_name.to_string(), tools.clone());

        Some(tools)
    }

    /// Builds `rig::tool::DynamicTool` instances for all enabled MCP servers.
    ///
    /// Pass `observer` so MCP tool invocations report start/result events to the
    /// Claw indicator badge, read-only sidebar debug log, and telemetry.
    pub async fn build_rig_tools(
        &self,
        observer: Option<Arc<dyn McpToolObserver>>,
    ) -> Vec<rig::tool::DynamicTool> {
        let mut rig_tools: Vec<rig::tool::DynamicTool> = Vec::new();
        let mut seen_names = HashSet::new();

        let server_configs: Vec<(String, u64)> = {
            let configs = self.configs.read().await;
            configs
                .values()
                .map(|c| (c.name.clone(), c.timeout_ms))
                .collect()
        };

        let manager_arc = self.self_weak.upgrade();

        for (name, timeout_ms) in server_configs {
            if let Some(tools) = self.get_tools(&name).await {
                let client_opt = {
                    let clients = self.active_clients.read().await;
                    clients.get(&name).cloned()
                };

                if let Some(client) = client_opt {
                    for mcp_tool in tools {
                        let dynamic_tool = DynamicMcpTool {
                            manager: manager_arc.clone(),
                            client: client.clone(),
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
            }
        }

        rig_tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BackoffConfig;

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

        {
            let configs = manager.configs.read().await;
            assert!(configs.contains_key("alpha"));
            assert!(!configs.contains_key("beta"));
        }

        // Seed a synthetic tool cache entry for "alpha"
        {
            let mut cache = manager.tool_cache.write().await;
            cache.insert("alpha".to_string(), vec![]);
        }

        // Updating with identical config preserves the tool cache
        manager.update_configs(vec![cfg1.clone()]).await;
        assert!(manager.tool_cache.read().await.contains_key("alpha"));

        // Modifying transport command invalidates the cache
        let modified = sample_stdio_config("alpha", "cat");
        manager.update_configs(vec![modified]).await;
        assert!(!manager.tool_cache.read().await.contains_key("alpha"));
    }

    #[tokio::test]
    async fn tool_list_changed_notification_invalidates_manager_tool_cache() {
        use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
        use serde_json::json;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let manager = McpClientManager::new();
        {
            let mut cache = manager.tool_cache.write().await;
            cache.insert("live-srv".to_string(), vec![]);
        }

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

            // Emit `notifications/tools/list_changed`
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

        let handler = BoxxyClientHandler::new("live-srv", Arc::downgrade(&manager));
        let client = serve_client_with_lifecycle(
            handler,
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover succeeded");

        server_task.await.unwrap();

        // Give the background notification loop a moment to process the notification
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
        let _ = client.cancel().await;
    }
}
