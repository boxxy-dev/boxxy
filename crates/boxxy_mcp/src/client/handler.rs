use std::sync::Weak;

use rmcp::{
    ClientHandler,
    model::{ClientCapabilities, ClientConfig, Implementation},
    service::{NotificationContext, RoleClient},
};

use crate::manager::McpClientManager;

/// Custom `rmcp::ClientHandler` for Boxxy-Terminal.
///
/// Advertises Boxxy's client identity during MCP initialization/discovery and
/// invalidates the local tool cache when a server emits `notifications/tools/list_changed`.
#[derive(Clone)]
pub struct BoxxyClientHandler {
    server_name: String,
    manager: Weak<McpClientManager>,
}

impl BoxxyClientHandler {
    pub fn new(server_name: impl Into<String>, manager: Weak<McpClientManager>) -> Self {
        Self {
            server_name: server_name.into(),
            manager,
        }
    }
}

impl ClientHandler for BoxxyClientHandler {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("boxxy-terminal", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        log::info!(
            "MCP server '{}' notified tool list changed; invalidating local tool cache",
            self.server_name
        );
        if let Some(manager) = self.manager.upgrade() {
            manager.invalidate_tool_cache(&self.server_name).await;
        }
    }
}
