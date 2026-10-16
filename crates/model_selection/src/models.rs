use rig::catalog::{Catalog, ModelSpec};
use rig::completion::Effort;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Maps a [`rig::completion::Effort`] to Boxxy's [`ThinkingLevel`].
fn effort_to_thinking_level(effort: Effort) -> ThinkingLevel {
    match effort {
        Effort::Minimal => ThinkingLevel::Minimal,
        Effort::Low => ThinkingLevel::Low,
        Effort::Medium => ThinkingLevel::Medium,
        Effort::High => ThinkingLevel::High,
        Effort::XHigh => ThinkingLevel::XHigh,
        _ => ThinkingLevel::High,
    }
}

/// Strips a trailing `-YYYYMMDD` or `-YYYY-MM-DD` date suffix from a model ID, if present.
fn strip_date_suffix(id: &str) -> Option<&str> {
    if id.len() > 9 {
        let (prefix, tail) = id.split_at(id.len() - 9);
        if tail.starts_with("-20") && tail[1..].bytes().all(|b| b.is_ascii_digit()) {
            return Some(prefix);
        }
    }
    if id.len() > 11 {
        let (prefix, tail) = id.split_at(id.len() - 11);
        let bytes = tail.as_bytes();
        if tail.starts_with("-20")
            && bytes[5] == b'-'
            && bytes[8] == b'-'
            && bytes[1..5].iter().all(u8::is_ascii_digit)
            && bytes[6..8].iter().all(u8::is_ascii_digit)
            && bytes[9..11].iter().all(u8::is_ascii_digit)
        {
            return Some(prefix);
        }
    }
    None
}

/// Returns `true` if `spec` is an active, tool-capable chat/reasoning model suitable
/// for interactive and agentic selection.
fn is_selectable_chat_model(catalog: &Catalog, vendor: &str, spec: &ModelSpec) -> bool {
    if spec.deprecated || !spec.tools || !spec.input.text {
        return false;
    }

    // In rig-core's catalog, a row with `reasoning: true` but no levels, no budget,
    // and `can_disable: false` is a specialized non-standard endpoint (e.g. deep-research)
    // that refuses all standard reasoning options.
    if spec.reasoning.supported
        && spec.reasoning.levels.is_empty()
        && spec.reasoning.budget.is_none()
        && !spec.reasoning.can_disable
    {
        return false;
    }

    let id = spec.id.as_str();
    if id.contains("-image")
        || id.contains("-live")
        || id.contains("-realtime")
        || id.contains("-tts")
        || id.contains("-customtools")
    {
        return false;
    }

    // Skip dated snapshot duplicates when the undated alias exists in the same vendor catalog.
    if let Some(base_id) = strip_date_suffix(id)
        && catalog.resolve(&format!("{vendor}/{base_id}")).is_some()
    {
        return false;
    }

    true
}

/// Returns all selectable [`ModelSpec`]s for `vendor` from [`Catalog::builtin`],
/// ordered with the newest / flagship generations first.
pub fn catalog_models_for_vendor(vendor: &str) -> Vec<&'static ModelSpec> {
    let catalog = Catalog::builtin();
    let mut specs: Vec<&'static ModelSpec> = catalog
        .iter()
        .filter(|spec| {
            spec.provider.vendor() == vendor && is_selectable_chat_model(catalog, vendor, spec)
        })
        .collect();

    // Catalog::builtin() stores entries in ascending ASCII order by ID (so older versions
    // like 2.5 / 4.5 come before 3.8 / 5.5 / 6.1). Reversing puts the newest generation first.
    specs.reverse();
    specs
}

/// Computes the available [`ThinkingLevel`]s for a model from its [`ModelSpec`] in [`Catalog::builtin`].
pub fn catalog_thinking_levels(vendor: &str, model_id: &str) -> Vec<ThinkingLevel> {
    let Some(spec) = Catalog::builtin().resolve(&format!("{vendor}/{model_id}")) else {
        return Vec::new();
    };
    if !spec.reasoning.supported {
        return Vec::new();
    }

    let mut levels = Vec::new();
    if spec.reasoning.can_disable {
        levels.push(ThinkingLevel::None);
    }

    if !spec.reasoning.levels.is_empty() {
        for &effort in &spec.reasoning.levels {
            let mapped = effort_to_thinking_level(effort);
            if !levels.contains(&mapped) {
                levels.push(mapped);
            }
        }
    } else if spec.reasoning.budget.is_some() {
        for level in [
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ] {
            if !levels.contains(&level) {
                levels.push(level);
            }
        }
    }

    levels
}

/// Returns the recommended default [`ThinkingLevel`] for `(vendor, model_id)` from [`Catalog::builtin`].
pub fn catalog_default_thinking_level(vendor: &str, model_id: &str) -> Option<ThinkingLevel> {
    let levels = catalog_thinking_levels(vendor, model_id);
    if levels.is_empty() {
        return None;
    }

    if let Some(spec) = Catalog::builtin().resolve(&format!("{vendor}/{model_id}"))
        && let Some(def_effort) = spec.reasoning.default
    {
        let mapped = effort_to_thinking_level(def_effort);
        if levels.contains(&mapped) {
            return Some(mapped);
        }
    }

    if levels.contains(&ThinkingLevel::Medium) {
        Some(ThinkingLevel::Medium)
    } else if levels.contains(&ThinkingLevel::Low) {
        Some(ThinkingLevel::Low)
    } else {
        levels.first().copied()
    }
}

/// Returns `true` when `(vendor, model_id)` uses token-budget reasoning rather than effort levels.
pub fn catalog_uses_budget_thinking(vendor: &str, model_id: &str) -> bool {
    Catalog::builtin()
        .resolve(&format!("{vendor}/{model_id}"))
        .is_some_and(|spec| {
            spec.reasoning.supported
                && spec.reasoning.levels.is_empty()
                && spec.reasoning.budget.is_some()
        })
}

/// Resolves the human-readable display name for `(vendor, model_id)` from [`Catalog::builtin`].
pub fn catalog_display_name<'a>(vendor: &str, model_id: &'a str) -> &'a str {
    Catalog::builtin()
        .resolve(&format!("{vendor}/{model_id}"))
        .map(|spec| spec.display_name.as_str())
        .unwrap_or(model_id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GeminiModel(pub String);

#[allow(non_upper_case_globals)]
impl GeminiModel {
    pub const Flash3_8: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<GeminiModel> {
        let models: Vec<GeminiModel> = catalog_models_for_vendor("gcp.gemini")
            .into_iter()
            .map(|spec| GeminiModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![GeminiModel::new("gemini-3.8-flash")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "gemini-3.8-flash"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("gcp.gemini", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("gcp.gemini", self.api_name())
    }
}

impl fmt::Display for GeminiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("gcp.gemini", self.api_name()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingLevel {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "minimal")]
    Minimal,
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "xhigh")]
    XHigh,
}

impl fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThinkingLevel::None => write!(f, "None"),
            ThinkingLevel::Minimal => write!(f, "Minimal"),
            ThinkingLevel::Low => write!(f, "Low"),
            ThinkingLevel::Medium => write!(f, "Medium"),
            ThinkingLevel::High => write!(f, "High"),
            ThinkingLevel::XHigh => write!(f, "Extreme High"),
        }
    }
}

impl ThinkingLevel {
    pub fn api_name(&self) -> &'static str {
        match self {
            ThinkingLevel::None => "none",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::XHigh => "xhigh",
        }
    }

    pub fn anthropic_budget_tokens(&self) -> u32 {
        match self {
            ThinkingLevel::Low => 2_000,
            ThinkingLevel::Medium => 8_000,
            ThinkingLevel::High => 32_000,
            _ => 2_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnthropicModel(pub String);

#[allow(non_upper_case_globals)]
impl AnthropicModel {
    pub const ClaudeFable51: Self = Self(String::new());
    pub const ClaudeSonnet5: Self = Self(String::new());
    pub const ClaudeOpus5: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<AnthropicModel> {
        let models: Vec<AnthropicModel> = catalog_models_for_vendor("anthropic")
            .into_iter()
            .map(|spec| AnthropicModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![AnthropicModel::new("claude-sonnet-5-5")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "claude-sonnet-5-5"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn supports_extended_thinking(&self) -> bool {
        catalog_uses_budget_thinking("anthropic", self.api_name())
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("anthropic", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("anthropic", self.api_name())
    }
}

impl fmt::Display for AnthropicModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("anthropic", self.api_name()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpenAiModel(pub String);

#[allow(non_upper_case_globals)]
impl OpenAiModel {
    pub const Gpt6Astra: Self = Self(String::new());
    pub const Gpt5_6Sol: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<OpenAiModel> {
        let models: Vec<OpenAiModel> = catalog_models_for_vendor("openai")
            .into_iter()
            .map(|spec| OpenAiModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![OpenAiModel::new("gpt-6.1-sol")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "gpt-6-astra"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("openai", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("openai", self.api_name())
    }
}

impl fmt::Display for OpenAiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("openai", self.api_name()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeepSeekModel(pub String);

#[allow(non_upper_case_globals)]
impl DeepSeekModel {
    pub const V4_1Flash: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<DeepSeekModel> {
        let models: Vec<DeepSeekModel> = catalog_models_for_vendor("deepseek")
            .into_iter()
            .map(|spec| DeepSeekModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![DeepSeekModel::new("deepseek-flash")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "deepseek-flash"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("deepseek", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("deepseek", self.api_name())
    }
}

impl fmt::Display for DeepSeekModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("deepseek", self.api_name()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelProvider {
    Gemini(GeminiModel, Option<ThinkingLevel>),
    Ollama(String),
    Anthropic(AnthropicModel, Option<ThinkingLevel>),
    OpenAi(OpenAiModel, Option<ThinkingLevel>),
    OpenRouter(String),
    DeepSeek(DeepSeekModel),
}

impl ModelProvider {
    pub fn provider_name(&self) -> &'static str {
        match self {
            ModelProvider::Gemini(_, _) => "Gemini",
            ModelProvider::Ollama(_) => "Ollama",
            ModelProvider::Anthropic(_, _) => "Anthropic",
            ModelProvider::OpenAi(_, _) => "OpenAI",
            ModelProvider::OpenRouter(_) => "OpenRouter",
            ModelProvider::DeepSeek(_) => "DeepSeek",
        }
    }

    pub fn format_label(&self) -> String {
        match self {
            ModelProvider::Gemini(model, _) => format!("Google/{}", model),
            ModelProvider::Ollama(model) => format!("Ollama/{}", model),
            ModelProvider::Anthropic(model, _) => format!("Anthropic/{}", model),
            ModelProvider::OpenAi(model, _) => format!("OpenAI/{}", model),
            ModelProvider::OpenRouter(model) => format!("OpenRouter/{}", model),
            ModelProvider::DeepSeek(model) => format!("DeepSeek/{}", model),
        }
    }

    /// Returns `true` if this provider supports built-in server-side web search
    /// (Gemini Google Search grounding, OpenAI Responses `web_search`, Anthropic `web_search_20250305`).
    pub fn supports_native_search(&self) -> bool {
        matches!(
            self,
            ModelProvider::Gemini(_, _)
                | ModelProvider::OpenAi(_, _)
                | ModelProvider::Anthropic(_, _)
        )
    }
}

impl Default for ModelProvider {
    fn default() -> Self {
        ModelProvider::Gemini(
            GeminiModel::new("gemini-3.8-flash"),
            Some(ThinkingLevel::Low),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_model_deserialization() {
        let model_38: GeminiModel = serde_json::from_str("\"gemini-3.8-flash\"").unwrap();
        assert_eq!(model_38.api_name(), "gemini-3.8-flash");
        assert_eq!(model_38.to_string(), "Gemini 3.8 Flash");
    }

    #[test]
    fn test_dynamic_catalog_models_and_thinking_levels() {
        let gemini_all = GeminiModel::all();
        assert!(gemini_all.iter().any(|m| m.api_name() == "gemini-3.8-flash"));
        assert!(gemini_all.iter().any(|m| m.api_name() == "gemini-3.6-flash"));

        let flash_38 = GeminiModel::new("gemini-3.8-flash");
        let levels_3_8 = flash_38.available_thinking_levels();
        assert!(!levels_3_8.contains(&ThinkingLevel::Minimal));
        assert!(levels_3_8.contains(&ThinkingLevel::Low));
        assert!(levels_3_8.contains(&ThinkingLevel::Medium));
        assert!(levels_3_8.contains(&ThinkingLevel::High));
        assert_eq!(flash_38.default_thinking_level(), Some(ThinkingLevel::Medium));

        // Gemini 3.6 Flash supports Minimal in addition to Low/Medium/High
        let flash_36 = GeminiModel::new("gemini-3.6-flash");
        assert!(flash_36
            .available_thinking_levels()
            .contains(&ThinkingLevel::Minimal));

        // OpenAI catalog includes GPT-6.1 Sol and GPT-6 Astra
        let openai_all = OpenAiModel::all();
        assert!(openai_all.iter().any(|m| m.api_name() == "gpt-6.1-sol"));
        assert!(openai_all.iter().any(|m| m.api_name() == "gpt-6-astra"));
        let sol_61 = OpenAiModel::new("gpt-6.1-sol");
        assert_eq!(sol_61.to_string(), "GPT-6.1 Sol");
        assert!(sol_61.supports_thinking());

        // Non-reasoning OpenAI model (gpt-4.1) has no thinking levels
        let gpt_41 = OpenAiModel::new("gpt-4.1");
        assert!(!gpt_41.supports_thinking());
        assert!(gpt_41.available_thinking_levels().is_empty());

        // Anthropic catalog includes Claude Opus 5.5, Claude Sonnet 5.5, and Claude Fable 5.1
        let anthropic_all = AnthropicModel::all();
        assert!(anthropic_all
            .iter()
            .any(|m| m.api_name() == "claude-opus-5-5"));
        assert!(anthropic_all
            .iter()
            .any(|m| m.api_name() == "claude-sonnet-5-5"));
        assert!(anthropic_all
            .iter()
            .any(|m| m.api_name() == "claude-fable-5-1"));

        let sonnet_55 = AnthropicModel::new("claude-sonnet-5-5");
        assert!(sonnet_55.supports_thinking());
        assert!(!sonnet_55.supports_extended_thinking());
        assert!(sonnet_55
            .available_thinking_levels()
            .contains(&ThinkingLevel::None));

        // DeepSeek catalog includes both V4.1 Flash and V4 Pro
        let deepseek_all = DeepSeekModel::all();
        assert!(deepseek_all
            .iter()
            .any(|m| m.api_name() == "deepseek-flash"));
        assert!(deepseek_all
            .iter()
            .any(|m| m.api_name() == "deepseek-v4-pro"));
    }

    #[test]
    fn test_supports_native_search() {
        assert!(
            ModelProvider::Gemini(
                GeminiModel::new("gemini-3.8-flash"),
                Some(ThinkingLevel::Low)
            )
            .supports_native_search()
        );
        assert!(
            ModelProvider::OpenAi(
                OpenAiModel::new("gpt-6.1-sol"),
                Some(ThinkingLevel::Medium)
            )
            .supports_native_search()
        );
        assert!(
            ModelProvider::Anthropic(AnthropicModel::new("claude-opus-5-5"), None)
                .supports_native_search()
        );
        assert!(!ModelProvider::Ollama("llama3.2".to_string()).supports_native_search());
        assert!(
            !ModelProvider::OpenRouter("anthropic/claude-sonnet-4".to_string())
                .supports_native_search()
        );
        assert!(
            !ModelProvider::DeepSeek(DeepSeekModel::new("deepseek-flash")).supports_native_search()
        );
    }
}
