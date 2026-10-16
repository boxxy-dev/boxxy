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
    pub use_native_search: bool,
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
            use_native_search: settings.use_native_search,
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

/// Dynamically resolves the active web search engine name for a Claw session
/// based on the current settings and pane-local web search toggle.
pub fn resolve_active_search_label(
    settings: &boxxy_preferences::Settings,
    web_search_local_enabled: bool,
) -> &'static str {
    if !settings.enable_web_search || !web_search_local_enabled {
        return "Off";
    }

    if settings.use_native_search
        && settings
            .claw_model
            .as_ref()
            .is_some_and(|m| m.supports_native_search())
    {
        return "Native";
    }

    if !settings
        .get_effective_api_key(boxxy_core_toolbox::TavilyProvider::NAME)
        .trim()
        .is_empty()
    {
        boxxy_core_toolbox::TavilyProvider::NAME
    } else {
        "Off"
    }
}

/// Formats the session status line displaying active Claw model, Dreams model, and Search engine.
pub fn format_session_models_banner(
    settings: &boxxy_preferences::Settings,
    web_search_local_enabled: bool,
) -> String {
    let claw_label = settings
        .claw_model
        .as_ref()
        .map(|m| m.format_label())
        .unwrap_or_else(|| "Default".to_string());
    let memory_label = settings
        .memory_model
        .as_ref()
        .map(|m| m.format_label())
        .unwrap_or_else(|| "Default".to_string());
    let search_label = resolve_active_search_label(settings, web_search_local_enabled);
    format!("Claw: {claw_label} ● Dreams: {memory_label} ● Search: {search_label}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_model_selection::{GeminiModel, ModelProvider, ThinkingLevel};

    #[test]
    fn test_resolve_active_search_label_and_banner() {
        let mut settings = boxxy_preferences::Settings::default();
        settings.claw_model = Some(ModelProvider::Gemini(
            GeminiModel::Flash3_8,
            Some(ThinkingLevel::Low),
        ));
        settings.memory_model = Some(ModelProvider::Gemini(
            GeminiModel::Flash3_8,
            Some(ThinkingLevel::Low),
        ));
        settings.enable_web_search = true;

        // Native search enabled on Gemini -> "Native"
        settings.use_native_search = true;
        assert_eq!(resolve_active_search_label(&settings, true), "Native");
        assert_eq!(
            format_session_models_banner(&settings, true),
            "Claw: Google/Gemini 3.8 Flash ● Dreams: Google/Gemini 3.8 Flash ● Search: Native"
        );

        // Native search on non-native provider (Ollama) falls back to Tavily if key exists
        settings.claw_model = Some(ModelProvider::Ollama("llama3.2".to_string()));
        settings
            .api_keys
            .insert("Tavily".to_string(), "tvly-test-key".to_string());
        assert_eq!(resolve_active_search_label(&settings, true), "Tavily");

        // Native search disabled with Tavily key -> "Tavily"
        settings.use_native_search = false;
        assert_eq!(resolve_active_search_label(&settings, true), "Tavily");

        // Local web search toggle off -> "Off"
        assert_eq!(resolve_active_search_label(&settings, false), "Off");
    }
}
