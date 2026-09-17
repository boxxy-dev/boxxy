use crate::config::{McpServerConfig, McpTransport};
use crate::rig_bridge::tool::DynamicMcpTool;
use log::{error, info};
use rmcp::model::Tool as McpToolDefinition;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};

use rmcp::{RoleClient, service::RunningService};

pub type McpClientWrapper = RunningService<RoleClient, ()>;

pub struct McpClientManager {
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
        Arc::new(Self {
            configs: RwLock::new(HashMap::new()),
            active_clients: RwLock::new(HashMap::new()),
            tool_cache: RwLock::new(HashMap::new()),
        })
    }

    pub async fn update_configs(&self, servers: Vec<McpServerConfig>) {
        let mut configs = self.configs.write().await;
        let old_server_names: std::collections::HashSet<String> = configs.keys().cloned().collect();

        configs.clear();
        let mut new_server_names = std::collections::HashSet::new();
        for server in servers {
            if server.enabled {
                new_server_names.insert(server.name.clone());
                configs.insert(server.name.clone(), server);
            }
        }

        // Disconnect clients that are no longer in the config or were disabled.
        let mut active_clients = self.active_clients.write().await;
        for removed_name in old_server_names.difference(&new_server_names) {
            if let Some(client) = active_clients.remove(removed_name) {
                info!("Disconnecting MCP server: {}", removed_name);
                // Extract from Arc if it's the only reference to gracefully close it.
                // Otherwise, the drop implementation (or OS process termination) will handle it.
                if let Ok(mut c) = Arc::try_unwrap(client) {
                    tokio::spawn(async move {
                        let _ = c.close().await;
                    });
                }
            }

            // Also clear from tool cache
            let mut cache = self.tool_cache.write().await;
            cache.remove(removed_name);
        }
    }

    /// Returns the cached tools for a server. If not cached, it attempts to initialize
    /// the connection (lazy initialization) and fetch the tools.
    pub async fn get_tools(&self, server_name: &str) -> Option<Vec<McpToolDefinition>> {
        let cache = self.tool_cache.read().await;
        if let Some(tools) = cache.get(server_name) {
            return Some(tools.clone());
        }
        drop(cache);

        // Not in cache, we must connect and fetch.
        let config = {
            let configs = self.configs.read().await;
            configs.get(server_name).cloned()?
        };

        info!("MCP Lazy Boot: Initializing connection for {}", server_name);

        let client_res = match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                crate::client::stdio::build_stdio_client(command, args, env).await
            }
            McpTransport::Http {
                url,
                headers,
                streamable: true,
            } => crate::client::http::build_streamable_http_client(url, headers).await,
            _ => Err(anyhow::anyhow!("Transport not yet supported")),
        };

        let client = match client_res {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to MCP server {}: {}", server_name, e);
                return None;
            }
        };

        let tools = match client
            .list_tools(Some(rmcp::model::PaginatedRequestParams::default()))
            .await
        {
            Ok(result) => result.tools,
            Err(e) => {
                error!("Failed to list tools for MCP server {}: {}", server_name, e);
                return None;
            }
        };

        let mut cache = self.tool_cache.write().await;
        cache.insert(server_name.to_string(), tools.clone());

        // Store active client to prevent it from dropping
        let mut clients = self.active_clients.write().await;
        clients.insert(server_name.to_string(), Arc::new(client));

        Some(tools)
    }

    pub async fn build_rig_tools(&self) -> Vec<rig::tool::DynamicTool> {
        let mut rig_tools: Vec<rig::tool::DynamicTool> = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        let configs = self.configs.read().await;
        let server_names: Vec<String> = configs.keys().cloned().collect();
        drop(configs);

        for name in server_names {
            if let Some(tools) = self.get_tools(&name).await {
                let clients = self.active_clients.read().await;
                if let Some(client) = clients.get(&name) {
                    for mcp_tool in tools {
                        let dynamic_tool = DynamicMcpTool {
                            client: client.clone(),
                            mcp_tool,
                            server_name: name.clone(),
                        };

                        let tool_name = dynamic_tool.name();

                        if seen_names.contains(&tool_name) {
                            log::warn!(
                                "MCP: Skipping duplicate tool name '{}' from server '{}'",
                                tool_name,
                                name
                            );
                            continue;
                        }
                        seen_names.insert(tool_name);

                        rig_tools.push(dynamic_tool.to_dynamic_tool());
                    }
                }
            }
        }

        rig_tools
    }
}
