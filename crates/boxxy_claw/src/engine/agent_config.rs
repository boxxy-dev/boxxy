use boxxy_model_selection::ModelProvider;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentConfig {
    pub model: Option<ModelProvider>,
    pub file_tools_enabled: bool,
    pub character_creator_enabled: bool,
    pub system_tools_enabled: bool,
    pub dangerous_tools_enabled: bool,
    pub web_tools_enabled: bool,
    pub clipboard_tools_enabled: bool,
    pub web_search_master_enabled: bool,
    pub web_search_local_enabled: bool,
    pub use_native_search: bool,
    pub mcp_servers: Vec<boxxy_mcp::config::McpServerConfig>,
    pub openai_compatible_endpoints: Vec<boxxy_model_selection::OpenAiCompatibleEndpoint>,
    /// Peer characters `(display_name, slug, provider_name)` in the active catalog whose
    /// resolved `ModelProvider` is ready and supports native image generation.
    pub image_capable_peers: Vec<(String, String, String)>,
    pub preamble: String,
    pub user_name: Option<String>,
}

/// Returns `(display_name, slug, provider_name)` for every character in `catalog` (excluding
/// `exclude_character_id` and `/incognito`) whose resolved model is `Ready` and supports image generation.
#[must_use]
pub fn find_image_capable_characters_in_catalog(
    settings: &boxxy_preferences::Settings,
    exclude_character_id: &str,
    catalog: &[boxxy_claw_protocol::characters::CharacterInfo],
) -> Vec<(String, String, String)> {
    let exclude = exclude_character_id.trim();
    catalog
        .iter()
        .filter(|c| {
            !c.config.id.is_empty()
                && c.config.id != exclude
                && !boxxy_claw_protocol::characters::is_incognito_character(&c.config.id)
        })
        .filter_map(|c| {
            let resolved = settings.resolve_character_model(&c.config.id);
            match settings.resolve_model_state(&resolved) {
                boxxy_preferences::config::ModelConfigState::Ready(mp)
                    if mp.supports_image_generation() =>
                {
                    let display =
                        crate::engine::session::roundtable::speaker_from_info(c, "", "")
                            .display_name;
                    Some((display, c.config.name.clone(), mp.provider_name().to_string()))
                }
                _ => None,
            }
        })
        .collect()
}

/// Returns `(display_name, slug, provider_name)` for all image-capable peer characters in the active catalog.
#[must_use]
pub fn find_image_capable_characters(
    settings: &boxxy_preferences::Settings,
    exclude_character_id: &str,
) -> Vec<(String, String, String)> {
    let catalog = crate::engine::session::roundtable::get_active_catalog();
    find_image_capable_characters_in_catalog(settings, exclude_character_id, &catalog)
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
        let self_supports_images = resolved_model
            .as_ref()
            .is_some_and(ModelProvider::supports_image_generation);
        let image_capable_peers = if self_supports_images {
            Vec::new()
        } else {
            find_image_capable_characters(settings, character_id.unwrap_or_default())
        };

        let character_creator_enabled = settings.enable_file_tools
            && !boxxy_claw_protocol::skill_loader::is_skill_disabled(
                &settings.disabled_skills,
                boxxy_preferences::config::BOXXY_CHARACTER_CREATOR_SLUG,
                "Boxxy Character Creator",
            );

        Self {
            model: resolved_model,
            file_tools_enabled: settings.enable_file_tools,
            character_creator_enabled,
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
            openai_compatible_endpoints: settings.openai_compatible_endpoints.clone(),
            image_capable_peers,
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

/// Merges on-disk/environment API keys (`Settings::get_effective_api_keys`) with any
/// non-empty keys pushed to the session over IPC (`state.api_keys`), so per-character
/// model overrides and guest roundtable speakers always see keys for every configured provider.
#[must_use]
pub fn merge_session_credentials<S: std::hash::BuildHasher>(
    settings: &boxxy_preferences::Settings,
    ipc_keys: &std::collections::HashMap<String, String, S>,
    ipc_ollama_url: &str,
) -> boxxy_ai_core::AiCredentials {
    let mut keys = settings.get_effective_api_keys();
    for (provider, key) in ipc_keys {
        if !key.trim().is_empty() {
            keys.insert(provider.clone(), key.clone());
        }
    }
    let ollama_url = if ipc_ollama_url.trim().is_empty() {
        settings.ollama_base_url.clone()
    } else {
        ipc_ollama_url.to_string()
    };
    boxxy_ai_core::AiCredentials::new(keys, ollama_url)
        .with_custom_endpoints(settings.openai_compatible_endpoints.clone())
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

    #[test]
    fn find_image_capable_characters_filters_ready_keys_and_excludes_self() {
        use boxxy_claw_protocol::characters::{CharacterConfig, CharacterInfo};
        use boxxy_model_selection::{AnthropicModel, OpenAiModel};

        let catalog = vec![
            CharacterInfo::new(
                CharacterConfig {
                    id: "char-claude".to_string(),
                    name: "niko-una".to_string(),
                    display_name: "Niko Una".to_string(),
                    color: "#ff6699".to_string(),
                    duties: "Engineer".to_string(),
                    personality: "Direct".to_string(),
                    greetings: Vec::new(),
                },
                false,
            ),
            CharacterInfo::new(
                CharacterConfig {
                    id: "char-gemini".to_string(),
                    name: "levi-kujo".to_string(),
                    display_name: "Levi Kujo".to_string(),
                    color: "#3399ff".to_string(),
                    duties: "Architect".to_string(),
                    personality: "Calm".to_string(),
                    greetings: Vec::new(),
                },
                false,
            ),
            CharacterInfo::new(
                CharacterConfig {
                    id: "char-openai-nokey".to_string(),
                    name: "kuro".to_string(),
                    display_name: "Kuro".to_string(),
                    color: "#9966ff".to_string(),
                    duties: "Security".to_string(),
                    personality: "Dry".to_string(),
                    greetings: Vec::new(),
                },
                false,
            ),
        ];

        let mut settings = boxxy_preferences::Settings::default();
        settings.claw_model = Some(ModelProvider::Anthropic(
            AnthropicModel::ClaudeSonnet5,
            None,
        ));
        settings
            .api_keys
            .insert("Anthropic".to_string(), "sk-ant-synthetic".to_string());
        settings
            .api_keys
            .insert("Gemini".to_string(), "gem-synthetic".to_string());
        settings.character_models.insert(
            "char-gemini".to_string(),
            ModelProvider::Gemini(GeminiModel::Flash3_8, Some(ThinkingLevel::Low)),
        );
        // Kuro is assigned OpenAI, but no OpenAI API key is configured -> must NOT be advertised as Ready
        settings.character_models.insert(
            "char-openai-nokey".to_string(),
            ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, None),
        );

        let peers = find_image_capable_characters_in_catalog(&settings, "char-claude", &catalog);
        assert_eq!(
            peers,
            vec![(
                "Levi Kujo".to_string(),
                "levi-kujo".to_string(),
                "Gemini".to_string()
            )]
        );

        // Excluded when querying for Levi himself
        let self_peers =
            find_image_capable_characters_in_catalog(&settings, "char-gemini", &catalog);
        assert!(self_peers.is_empty());
    }

    #[test]
    fn character_creator_tool_flag_tracks_default_off_skill_toggle_and_file_tools() {
        let default_settings = boxxy_preferences::Settings::default();
        let cfg_default =
            AgentConfig::from_env_for_character(&default_settings, None, false, String::new());
        assert!(
            !cfg_default.character_creator_enabled,
            "boxxy-character-creator must be off by default"
        );

        let enabled_settings = boxxy_preferences::Settings {
            disabled_skills: vec!["boxxy-theme-creator".to_string()],
            enable_file_tools: true,
            ..boxxy_preferences::Settings::default()
        };
        let cfg_enabled =
            AgentConfig::from_env_for_character(&enabled_settings, None, false, String::new());
        assert!(cfg_enabled.character_creator_enabled);

        let no_file_tools = boxxy_preferences::Settings {
            disabled_skills: Vec::new(),
            enable_file_tools: false,
            ..boxxy_preferences::Settings::default()
        };
        let cfg_no_files =
            AgentConfig::from_env_for_character(&no_file_tools, None, false, String::new());
        assert!(!cfg_no_files.character_creator_enabled);
    }

    #[test]
    fn merge_session_credentials_preserves_settings_keys_across_providers_and_skips_blanks() {
        let mut settings = boxxy_preferences::Settings::default();
        settings
            .api_keys
            .insert("Gemini".to_string(), "gem-from-settings".to_string());
        settings
            .api_keys
            .insert("Anthropic".to_string(), "ant-from-settings".to_string());
        settings.ollama_base_url = "http://127.0.0.1:11434".to_string();

        let mut ipc_keys = std::collections::HashMap::new();
        ipc_keys.insert("Anthropic".to_string(), "ant-from-ipc".to_string());
        ipc_keys.insert("Gemini".to_string(), "   ".to_string());

        let merged = merge_session_credentials(&settings, &ipc_keys, "");
        assert_eq!(
            merged.api_keys.get("Anthropic").map(String::as_str),
            Some("ant-from-ipc")
        );
        assert_eq!(
            merged.api_keys.get("Gemini").map(String::as_str),
            Some("gem-from-settings")
        );
        assert_eq!(merged.ollama_url, "http://127.0.0.1:11434");
    }
}
