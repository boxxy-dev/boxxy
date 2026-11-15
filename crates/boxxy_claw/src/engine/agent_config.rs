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
    #[must_use]
    pub fn from_env(
        settings: &boxxy_preferences::Settings,
        web_search_local_enabled: bool,
        preamble: String,
    ) -> Self {
        Self::from_env_for_character(settings, None, web_search_local_enabled, preamble)
    }

    #[must_use]
    pub fn from_env_for_character(
        settings: &boxxy_preferences::Settings,
        character_id: Option<&str>,
        web_search_local_enabled: bool,
        preamble: String,
    ) -> Self {
        let user_name = if settings.user_profile.display_name.trim().is_empty() {
            None
        } else {
            Some(settings.user_profile.display_name.trim().to_string())
        };

        let resolved_model = settings.resolve_character_model(character_id.unwrap_or_default());

        Self {
            model: resolved_model,
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

    #[must_use]
    pub fn model_label(&self) -> String {
        self.model
            .as_ref().map_or_else(|| "default".into(), boxxy_model_selection::ModelProvider::format_label)
    }
}

/// Dynamically resolves the active web search engine name for a specific `model`
/// based on the current settings and pane-local web search toggle.
#[must_use]
pub fn resolve_active_search_label_for_model(
    settings: &boxxy_preferences::Settings,
    model: &Option<ModelProvider>,
    web_search_local_enabled: bool,
) -> &'static str {
    if !settings.enable_web_search || !web_search_local_enabled {
        return "Off";
    }

    if settings.use_native_search
        && model
            .as_ref()
            .is_some_and(boxxy_model_selection::ModelProvider::supports_native_search)
    {
        return "Native";
    }

    if settings
        .get_effective_api_key(boxxy_core_toolbox::TavilyProvider::NAME)
        .trim()
        .is_empty()
    {
        "Off"
    } else {
        boxxy_core_toolbox::TavilyProvider::NAME
    }
}

/// Dynamically resolves the active web search engine name for a Claw session
/// based on the global default model and pane-local web search toggle.
#[must_use]
pub fn resolve_active_search_label(
    settings: &boxxy_preferences::Settings,
    web_search_local_enabled: bool,
) -> &'static str {
    resolve_active_search_label_for_model(settings, &settings.claw_model, web_search_local_enabled)
}

/// Formats the session status line displaying active character Claw model, system Dreams model, and Search engine.
#[must_use]
pub fn format_session_models_banner_for_character(
    settings: &boxxy_preferences::Settings,
    character_id: &str,
    web_search_local_enabled: bool,
) -> String {
    let effective_claw = settings.resolve_character_model(character_id);
    let effective_memory = settings.effective_memory_model();
    let claw_label = effective_claw
        .as_ref()
        .map_or_else(|| "Default".to_string(), boxxy_model_selection::ModelProvider::format_label);
    let memory_label = effective_memory
        .as_ref()
        .map_or_else(|| "Default".to_string(), boxxy_model_selection::ModelProvider::format_label);
    let search_label =
        resolve_active_search_label_for_model(settings, &effective_claw, web_search_local_enabled);
    format!("Claw: {claw_label} ● Dreams: {memory_label} ● Search: {search_label}")
}

/// Formats the session status line displaying global Claw model, Dreams model, and Search engine.
#[must_use]
pub fn format_session_models_banner(
    settings: &boxxy_preferences::Settings,
    web_search_local_enabled: bool,
) -> String {
    format_session_models_banner_for_character(settings, "", web_search_local_enabled)
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

        // Per-character override uses Gemini (Native search) even while global default is Ollama (Tavily)
        settings.use_native_search = true;
        settings.character_models.insert(
            "char-custom-1".to_string(),
            ModelProvider::Gemini(GeminiModel::Flash3_8, Some(ThinkingLevel::Medium)),
        );
        let custom_cfg = AgentConfig::from_env_for_character(
            &settings,
            Some("char-custom-1"),
            true,
            "preamble".into(),
        );
        assert_eq!(
            custom_cfg.model,
            Some(ModelProvider::Gemini(
                GeminiModel::Flash3_8,
                Some(ThinkingLevel::Medium)
            ))
        );
        assert_eq!(
            format_session_models_banner_for_character(&settings, "char-custom-1", true),
            "Claw: Google/Gemini 3.8 Flash ● Dreams: Google/Gemini 3.8 Flash ● Search: Native"
        );
        assert_eq!(
            format_session_models_banner_for_character(&settings, "char-unlisted", true),
            "Claw: Ollama/llama3.2 ● Dreams: Google/Gemini 3.8 Flash ● Search: Tavily"
        );
    }
}
