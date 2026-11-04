pub mod engine;
pub mod memories;
pub mod registry;
pub mod utils;

/// Synchronizes the global MCP client manager with `servers` and returns
/// `[(server_name, is_connected)]` for all enabled MCP servers.
pub async fn sync_and_probe_mcp_servers(
    servers: Vec<boxxy_mcp::config::McpServerConfig>,
) -> Vec<(String, bool)> {
    let manager = boxxy_mcp::manager::global_manager().await;
    manager.update_configs(servers).await;
    manager.get_server_statuses().await
}
