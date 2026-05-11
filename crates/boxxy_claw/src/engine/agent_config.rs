use boxxy_model_selection::ModelProvider;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentConfig {
    pub model: Option<ModelProvider>,
    pub file_tools_enabled: bool,
    pub system_tools_enabled: bool,
    pub dangerous_tools_enabled: bool,
    pub web_tools_enabled: bool,
    pub clipboard_tools_enabled: bool,
    pub web_search_master_enabled: bool,
    pub web_search_local_enabled: bool,
    pub mcp_servers: Vec<boxxy_mcp::config::McpServerConfig>,
    pub preamble: String,
    pub user_name: Option<String>,
}

impl AgentConfig {
    pub fn from_env(
        settings: &boxxy_preferences::Settings,
        web_search_local_enabled: bool,
        preamble: String,
    ) -> Self {
        let user_name = if settings.user_profile.display_name.trim().is_empty() {
            None
        } else {
            Some(settings.user_profile.display_name.trim().to_string())
        };

        Self {
            model: settings.claw_model.clone(),
            file_tools_enabled: settings.enable_file_tools,
            system_tools_enabled: settings.enable_system_tools,
            dangerous_tools_enabled: settings.enable_dangerous_tools,
            web_tools_enabled: settings.enable_web_tools,
            clipboard_tools_enabled: settings.enable_clipboard_tools,
            web_search_master_enabled: settings.enable_web_search,
            web_search_local_enabled,
            mcp_servers: settings
                .mcp_servers
                .iter()
                .filter(|s| s.enabled)
                .cloned()
                .collect(),
            preamble,
            user_name,
        }
    }

    pub fn model_label(&self) -> String {
        self.model
            .as_ref()
            .map(|p| p.format_label())
            .unwrap_or_else(|| "default".into())
    }
}
