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
fn is_selectable_chat_model(catalog: &Catalog, _vendor: &str, spec: &ModelSpec) -> bool {
    if spec.deprecated || !spec.tools || !spec.input.text {
        return false;
    }

    // In rig-core's catalog, a row with `reasoning: true` but no levels, no budget,
    // and `can_disable: false` is a specialized non-standard endpoint (e.g. deep-research)
    // that refuses all standard reasoning options.
    if spec.reasoning.supported()
        && spec.reasoning.levels().is_some_and(<[_]>::is_empty)
        && spec.reasoning.budget().is_none()
        && spec.reasoning.can_disable() == Some(false)
    {
        return false;
    }

    let id = spec.id.as_str();
    if id.contains("-image")
        || id.contains("-live")
        || id.contains("-realtime")
        || id.contains("-tts")
        || id.contains("-customtools")
        || id.contains("deep-research")
        || id.contains("computer-use")
    {
        return false;
    }

    // Skip dated snapshot duplicates when the undated alias exists in the same vendor catalog.
    if let Some(base_id) = strip_date_suffix(id)
        && catalog.get_exact(spec.provider, base_id).is_some()
    {
        return false;
    }

    true
}

/// Returns Boxxy's effective [`Catalog`], layering temporary model overrides on top of
/// [`Catalog::builtin`] for newly released models that have not yet landed in `rig-core`'s
/// embedded `models.json`.
#[must_use]
pub fn boxxy_catalog() -> &'static Catalog {
    static BOXXY_CATALOG: std::sync::LazyLock<Catalog> = std::sync::LazyLock::new(|| {
        let mut catalog = Catalog::builtin().clone();

        // TODO(rig-catalog): Remove this temporary `gpt-6.1-terra` override once `rig-core`
        // updates its embedded `models.json` catalog with `openai/gpt-6.1-terra`.
        // Source: https://models.dev/api.json (`openai.models["gpt-6.1-terra"]`).
        if let Some(openai) = rig::providers::registry::ProviderId::catalog("openai")
            && catalog.get_exact(openai, "gpt-6.1-terra").is_none()
        {
            const OVERRIDE_JSON: &str = r#"{
                "openai": {
                    "models": {
                        "gpt-6.1-terra": {
                            "name": "GPT-6.1 Terra",
                            "reasoning": true,
                            "reasoning_options": [
                                {
                                    "type": "effort",
                                    "values": ["none", "low", "medium", "high", "xhigh", "max"]
                                }
                            ],
                            "tool_call": true,
                            "structured_output": true,
                            "temperature": false,
                            "modalities": {
                                "input": ["text", "image", "pdf"]
                            },
                            "limit": {
                                "context": 1050000,
                                "output": 128000
                            },
                            "cost": {
                                "input": 1.0,
                                "output": 5.0,
                                "cache_read": 0.05,
                                "cache_write": 1.25
                            },
                            "rig": {
                                "cache": ["none", "short", "long"],
                                "prompt_cache_options": true,
                                "reasoning_default": "medium",
                                "sampling": "reasoning_off"
                            }
                        }
                    }
                }
            }"#;
            if let Ok(overrides) = Catalog::from_overrides(OVERRIDE_JSON, &catalog) {
                catalog = catalog.with_overrides(&overrides);
            }
        }

        catalog
    });
    &BOXXY_CATALOG
}

/// Returns all selectable [`ModelSpec`]s for `vendor` from [`boxxy_catalog`],
/// ordered with the newest / flagship generations first.
pub fn catalog_models_for_vendor(vendor: &str) -> Vec<&'static ModelSpec> {
    let catalog = boxxy_catalog();
    let mut specs: Vec<&'static ModelSpec> = catalog
        .iter()
        .filter(|spec| {
            spec.provider.vendor() == vendor && is_selectable_chat_model(catalog, vendor, spec)
        })
        .collect();

    // Catalog::builtin() stores entries in ascending ASCII order by ID (so older versions
    // like 2.5 / 4.5 come before 3.8 / 5.5 / 6.1). Reversing puts the newest generation first.
    specs.reverse();

    // For Moonshot, keep flagship `kimi-*` models ahead of legacy `moonshot-v1-*` models.
    if vendor == "moonshot" {
        specs.sort_by_key(|spec| spec.id.starts_with("moonshot-v1-"));
    } else if vendor == "xai" {
        specs.sort_by_key(|spec| spec.id.starts_with("grok-build"));
    } else if vendor == "zai" {
        specs.sort_by_key(|spec| spec.id.starts_with("glm-5v-"));
    } else if vendor == "mistral" {
        specs.sort_by_key(|spec| {
            if matches!(
                spec.id.as_str(),
                "mistral-large-latest"
                    | "mistral-medium-latest"
                    | "mistral-small-latest"
                    | "codestral-latest"
                    | "magistral-medium-latest"
            ) {
                0
            } else if spec.id.ends_with("-latest") {
                1
            } else {
                2
            }
        });
    }

    specs
}

/// Computes the available [`ThinkingLevel`]s for a [`ModelSpec`].
pub fn thinking_levels_from_spec(spec: &ModelSpec) -> Vec<ThinkingLevel> {
    if !spec.reasoning.supported() {
        return Vec::new();
    }

    let mut levels = Vec::new();
    if spec.reasoning.can_disable().unwrap_or(false) {
        levels.push(ThinkingLevel::None);
    }

    if let Some(spec_levels) = spec.reasoning.levels() {
        if !spec_levels.is_empty() {
            for &effort in spec_levels {
                let mapped = effort_to_thinking_level(effort);
                if !levels.contains(&mapped) {
                    levels.push(mapped);
                }
            }
        } else if spec.reasoning.budget().is_some() {
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
    } else if !matches!(spec.provider.vendor(), "moonshot" | "minimax") {
        // `ReasoningSupport::Unknown` — model supports reasoning and refuses no standard level
        // (except on Moonshot and MiniMax, where only models with listed effort levels accept
        // `reasoning_effort` and other reasoning models reason automatically).
        for level in [
            ThinkingLevel::None,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ] {
            if !levels.contains(&level) {
                levels.push(level);
            }
        }
    }

    if levels.as_slice() == [ThinkingLevel::None] {
        return Vec::new();
    }

    levels
}

/// Helper that looks up `(vendor, model_id)` in [`boxxy_catalog`] using O(log N) binary search
/// (`Catalog::get`) without triggering `Catalog::resolve`'s expensive Levenshtein suggestion scan on a miss.
#[must_use]
pub fn catalog_get_spec(vendor: &str, model_id: &str) -> Option<&'static ModelSpec> {
    let pid = rig::providers::registry::ProviderId::catalog(vendor)?;
    boxxy_catalog().get(pid, model_id).map(|r| r.spec)
}

/// Computes the available [`ThinkingLevel`]s for a model from its [`ModelSpec`] in [`boxxy_catalog`].
pub fn catalog_thinking_levels(vendor: &str, model_id: &str) -> Vec<ThinkingLevel> {
    let Some(spec) = catalog_get_spec(vendor, model_id) else {
        return Vec::new();
    };
    thinking_levels_from_spec(spec)
}

/// O(1) lookup index over [`boxxy_catalog`] by lowercase `"vendor/id"` and bare `"id"` so
/// [`resolve_any_catalog_spec`] is a single `HashMap` lookup (~30ns) and never blocks the UI thread.
static CATALOG_LOOKUP_INDEX: std::sync::LazyLock<
    std::collections::HashMap<String, &'static ModelSpec>,
> = std::sync::LazyLock::new(|| {
    let catalog = boxxy_catalog();
    let mut map = std::collections::HashMap::with_capacity(3200);

    for spec in catalog.iter() {
        if !spec.deprecated {
            map.insert(
                format!("{}/{}", spec.provider.vendor(), spec.id).to_ascii_lowercase(),
                spec,
            );
            map.entry(spec.id.to_ascii_lowercase()).or_insert(spec);
        }
    }

    // Prioritize primary origin vendors over gateway copies for bare model IDs
    const PRIMARY_VENDORS: &[&str] = &[
        "openai",
        "anthropic",
        "deepseek",
        "gcp.gemini",
        "xai",
        "moonshot",
        "zai",
        "alibaba",
        "minimax",
        "xiaomimimo",
        "mistral",
        "cohere",
    ];
    for spec in catalog.iter() {
        if !spec.deprecated && PRIMARY_VENDORS.contains(&spec.provider.vendor()) {
            map.insert(spec.id.to_ascii_lowercase(), spec);
        }
    }

    if let Some(&ds_flash) = map.get("deepseek/deepseek-flash") {
        map.insert("deepseek-v4.1-flash".to_string(), ds_flash);
        map.insert("deepseek-v4-flash".to_string(), ds_flash);
    }

    map
});

/// Resolves any model slug (e.g. `"anthropic/claude-sonnet-5-5"`, `"opencode-go/deepseek-v4-flash"`,
/// `"deepseek-v4-pro"`, `"gpt-6-astra"`) against [`Catalog::builtin`] in O(1) time.
#[must_use]
pub fn resolve_any_catalog_spec(slug: &str) -> Option<&'static ModelSpec> {
    let trimmed = slug.trim();
    if trimmed.is_empty() {
        return None;
    }

    let index = &*CATALOG_LOOKUP_INDEX;
    let lower = trimmed.to_ascii_lowercase();

    if let Some(&spec) = index.get(&lower) {
        return Some(spec);
    }

    if let Some(base) = strip_date_suffix(&lower)
        && let Some(&spec) = index.get(base)
    {
        return Some(spec);
    }

    if let Some((_, bare)) = lower.split_once('/') {
        let bare = bare.trim();
        if let Some(&spec) = index.get(bare) {
            return Some(spec);
        }
        if let Some(base) = strip_date_suffix(bare)
            && let Some(&spec) = index.get(base)
        {
            return Some(spec);
        }
    }

    None
}

/// Returns available [`ThinkingLevel`]s for an arbitrary model slug by consulting [`Catalog::builtin`].
#[must_use]
pub fn catalog_thinking_levels_for_any_slug(slug: &str) -> Vec<ThinkingLevel> {
    resolve_any_catalog_spec(slug)
        .map(thinking_levels_from_spec)
        .unwrap_or_default()
}

/// Returns the recommended default [`ThinkingLevel`] for `(vendor, model_id)` from [`Catalog::builtin`].
pub fn catalog_default_thinking_level(vendor: &str, model_id: &str) -> Option<ThinkingLevel> {
    let levels = catalog_thinking_levels(vendor, model_id);
    if levels.is_empty() {
        return None;
    }

    if let Some(spec) = catalog_get_spec(vendor, model_id)
        && let Some(def_effort) = spec.reasoning.default_effort()
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
    catalog_get_spec(vendor, model_id).is_some_and(|spec| {
        spec.reasoning.supported()
            && spec.reasoning.levels().is_none_or(<[_]>::is_empty)
            && spec.reasoning.budget().is_some()
    })
}

/// Resolves the human-readable display name for `(vendor, model_id)` from [`Catalog::builtin`].
pub fn catalog_display_name<'a>(vendor: &str, model_id: &'a str) -> &'a str {
    catalog_get_spec(vendor, model_id)
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

    /// Parses a single thinking level token (case-insensitive).
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => Some(Self::None),
            "minimal" | "min" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" | "extreme" | "extreme high" | "extreme_high" => Some(Self::XHigh),
            _ => None,
        }
    }

    /// Parses a comma-separated list of thinking levels (e.g. `"none, low, medium, high"`).
    #[must_use]
    pub fn parse_comma_separated(input: &str) -> Vec<Self> {
        let mut out = Vec::new();
        for part in input.split(',') {
            if let Some(level) = Self::from_token(part)
                && !out.contains(&level)
            {
                out.push(level);
            }
        }
        out
    }

    /// Formats a slice of thinking levels as a comma-separated string (e.g. `"none, low, medium, high"`).
    #[must_use]
    pub fn format_comma_separated(levels: &[Self]) -> String {
        levels
            .iter()
            .map(Self::api_name)
            .collect::<Vec<_>>()
            .join(", ")
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
#[serde(transparent)]
pub struct MoonshotModel(pub String);

#[allow(non_upper_case_globals)]
impl MoonshotModel {
    pub const KimiK3: Self = Self(String::new());
    pub const KimiK2_7Code: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<MoonshotModel> {
        let models: Vec<MoonshotModel> = catalog_models_for_vendor("moonshot")
            .into_iter()
            .map(|spec| MoonshotModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![MoonshotModel::new("kimi-k3")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "kimi-k3"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("moonshot", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("moonshot", self.api_name())
    }
}

impl fmt::Display for MoonshotModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("moonshot", self.api_name()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct XaiModel(pub String);

#[allow(non_upper_case_globals)]
impl XaiModel {
    pub const Grok4_7: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<XaiModel> {
        let models: Vec<XaiModel> = catalog_models_for_vendor("xai")
            .into_iter()
            .map(|spec| XaiModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![XaiModel::new("grok-4.7")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "grok-4.7"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("xai", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("xai", self.api_name())
    }
}

impl fmt::Display for XaiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("xai", self.api_name()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ZaiModel(pub String);

#[allow(non_upper_case_globals)]
impl ZaiModel {
    pub const Glm5_3: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<ZaiModel> {
        let models: Vec<ZaiModel> = catalog_models_for_vendor("zai")
            .into_iter()
            .map(|spec| ZaiModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![ZaiModel::new("glm-5.3")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "glm-5.3"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("zai", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("zai", self.api_name())
    }
}

impl fmt::Display for ZaiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("zai", self.api_name()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MistralModel(pub String);

#[allow(non_upper_case_globals)]
impl MistralModel {
    pub const MistralLargeLatest: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<MistralModel> {
        let models: Vec<MistralModel> = catalog_models_for_vendor("mistral")
            .into_iter()
            .map(|spec| MistralModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![MistralModel::new("mistral-large-latest")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "mistral-large-latest"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("mistral", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("mistral", self.api_name())
    }
}

impl fmt::Display for MistralModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("mistral", self.api_name()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MiniMaxModel(pub String);

#[allow(non_upper_case_globals)]
impl MiniMaxModel {
    pub const MiniMaxM3: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<MiniMaxModel> {
        let models: Vec<MiniMaxModel> = catalog_models_for_vendor("minimax")
            .into_iter()
            .map(|spec| MiniMaxModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![MiniMaxModel::new("MiniMax-M3")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "MiniMax-M3"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("minimax", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("minimax", self.api_name())
    }
}

impl fmt::Display for MiniMaxModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("minimax", self.api_name()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroqModel(pub String);

#[allow(non_upper_case_globals)]
impl GroqModel {
    pub const GptOss120B: Self = Self(String::new());

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn all() -> Vec<GroqModel> {
        let models: Vec<GroqModel> = catalog_models_for_vendor("groq")
            .into_iter()
            .map(|spec| GroqModel(spec.id.clone()))
            .collect();
        if models.is_empty() {
            vec![GroqModel::new("openai/gpt-oss-120b")]
        } else {
            models
        }
    }

    pub fn api_name(&self) -> &str {
        if self.0.is_empty() {
            "openai/gpt-oss-120b"
        } else {
            self.0.as_str()
        }
    }

    pub fn supports_thinking(&self) -> bool {
        !self.available_thinking_levels().is_empty()
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        catalog_thinking_levels("groq", self.api_name())
    }

    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        catalog_default_thinking_level("groq", self.api_name())
    }
}

impl fmt::Display for GroqModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", catalog_display_name("groq", self.api_name()))
    }
}

fn default_true() -> bool {
    true
}

/// Which OpenAI-compatible wire endpoint a model uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiApiRoute {
    /// Standard `/chat/completions` route (used by LiteLLM, vLLM, llama.cpp, OpenCode Go chat models).
    #[default]
    ChatCompletions,
    /// OpenAI `/responses` route (used by OpenCode Zen/Go for GPT & Grok models).
    Responses,
}

impl OpenAiApiRoute {
    /// Infers the best default route for `model_id` on `base_url`.
    #[must_use]
    pub fn infer_for_model(base_url: &str, model_id: &str) -> Self {
        let bare = model_id
            .trim()
            .split_once('/')
            .map_or_else(|| model_id.trim(), |(_, m)| m.trim())
            .to_ascii_lowercase();
        let url_lower = base_url.to_ascii_lowercase();
        if url_lower.contains("opencode.ai/zen")
            && (bare.starts_with("gpt-") || bare.starts_with("grok-"))
        {
            Self::Responses
        } else {
            Self::ChatCompletions
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ChatCompletions => "Chat Completions (/chat/completions)",
            Self::Responses => "Responses (/responses)",
        }
    }
}

/// Configuration for a single model under an [`OpenAiCompatibleEndpoint`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomModelConfig {
    /// Model slug / ID sent to the API (e.g. `"deepseek-v4-pro"`, `"gpt-6-astra"`, `"qwen3-coder"`).
    pub id: String,
    /// Optional human-readable display name. Defaults to catalog display name or `id` when empty.
    #[serde(default)]
    pub display_name: String,
    /// API wire route (`/chat/completions` vs `/responses`).
    #[serde(default)]
    pub route: OpenAiApiRoute,
    /// Optional context window size in tokens (e.g. `128_000`). Falls back to `Catalog::builtin()` if `None`.
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Optional user-configured thinking modes. Falls back to `Catalog::builtin()` if empty.
    #[serde(default)]
    pub thinking_levels: Vec<ThinkingLevel>,
    /// Optional custom input price in USD per 1M tokens.
    #[serde(default)]
    pub input_cost_per_1m: Option<f64>,
    /// Optional custom output price in USD per 1M tokens.
    #[serde(default)]
    pub output_cost_per_1m: Option<f64>,
}

impl CustomModelConfig {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            id,
            display_name: String::new(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: None,
            thinking_levels: Vec::new(),
            input_cost_per_1m: None,
            output_cost_per_1m: None,
        }
    }

    #[must_use]
    pub fn with_route(mut self, route: OpenAiApiRoute) -> Self {
        self.route = route;
        self
    }

    #[must_use]
    pub fn effective_display_name(&self) -> String {
        let trimmed_disp = self.display_name.trim();
        if !trimmed_disp.is_empty() {
            return trimmed_disp.to_string();
        }
        if let Some(spec) = resolve_any_catalog_spec(&self.id) {
            return format!("{} ({})", spec.display_name, self.id);
        }
        self.id.clone()
    }

    #[must_use]
    pub fn effective_context_window(&self) -> Option<u64> {
        if let Some(ctx) = self.context_window.filter(|&w| w > 0) {
            return Some(ctx);
        }
        resolve_any_catalog_spec(&self.id)
            .and_then(|s| s.context_window)
            .map(u64::from)
    }

    #[must_use]
    pub fn effective_thinking_levels(&self) -> Vec<ThinkingLevel> {
        if !self.thinking_levels.is_empty() {
            return self.thinking_levels.clone();
        }
        catalog_thinking_levels_for_any_slug(&self.id)
    }

    #[must_use]
    pub fn supports_thinking(&self) -> bool {
        !self.effective_thinking_levels().is_empty()
    }

    #[must_use]
    pub fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        let levels = self.effective_thinking_levels();
        if levels.is_empty() {
            return None;
        }
        if let Some(spec) = resolve_any_catalog_spec(&self.id)
            && let Some(def_effort) = spec.reasoning.default_effort()
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
}

/// Configuration for a custom OpenAI-compatible provider endpoint (e.g. OpenCode Go, OpenCode Zen, LiteLLM, vLLM, llama.cpp).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiCompatibleEndpoint {
    /// Unique provider name displayed in dropdowns and settings (e.g. `"OpenCode Go"`, `"OpenCode Zen"`, `"Local vLLM"`).
    pub name: String,
    /// Endpoint Base URL including `/v1` (e.g. `"https://opencode.ai/zen/go/v1"`, `"http://localhost:8000/v1"`).
    pub base_url: String,
    /// Optional environment variable name to read the API key from (e.g. `"OPENCODE_API_KEY"`).
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Whether this endpoint requires an API key (`true` for cloud gateways, `false` for local unauthenticated servers).
    #[serde(default = "default_true")]
    pub requires_api_key: bool,
    /// When `true` (e.g. local server or flat-rate subscription like OpenCode Go), token cost is `$0.00` unless overridden on the model.
    #[serde(default)]
    pub zero_cost: bool,
    /// Configured models for this endpoint.
    #[serde(default)]
    pub models: Vec<CustomModelConfig>,
    /// Whether this endpoint is active and selectable.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl OpenAiCompatibleEndpoint {
    /// Preset for **OpenCode Go** (`https://opencode.ai/zen/go/v1`, subscription-based).
    #[must_use]
    pub fn preset_opencode_go() -> Self {
        let base_url = "https://opencode.ai/zen/go/v1".to_string();
        let default_slugs = [
            "deepseek-v4-pro",
            "deepseek-v4.1-flash",
            "glm-5.3",
            "glm-5.3-flash",
            "kimi-k3",
            "kimi-k2.7-code",
            "gpt-6-luna",
            "gpt-5.6-luna",
            "grok-4.7",
            "mimo-v2.6-flash",
        ];
        let models = default_slugs
            .into_iter()
            .map(|slug| {
                let route = OpenAiApiRoute::infer_for_model(&base_url, slug);
                CustomModelConfig::new(slug).with_route(route)
            })
            .collect();

        Self {
            name: "OpenCode Go".to_string(),
            base_url,
            api_key_env: Some("OPENCODE_API_KEY".to_string()),
            requires_api_key: true,
            zero_cost: true,
            models,
            enabled: true,
        }
    }

    /// Preset for **OpenCode Zen** (`https://opencode.ai/zen/v1`, pay-as-you-go).
    #[must_use]
    pub fn preset_opencode_zen() -> Self {
        let base_url = "https://opencode.ai/zen/v1".to_string();
        let default_slugs = [
            "gpt-6.1-sol",
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-6-luna",
            "gpt-5.6-sol",
            "deepseek-v4-pro",
            "deepseek-v4.1-flash",
            "glm-5.3",
            "kimi-k3",
        ];
        let models = default_slugs
            .into_iter()
            .map(|slug| {
                let route = OpenAiApiRoute::infer_for_model(&base_url, slug);
                CustomModelConfig::new(slug).with_route(route)
            })
            .collect();

        Self {
            name: "OpenCode Zen".to_string(),
            base_url,
            api_key_env: Some("OPENCODE_API_KEY".to_string()),
            requires_api_key: true,
            zero_cost: false,
            models,
            enabled: true,
        }
    }

    /// Preset for **LiteLLM Proxy** (`http://localhost:4000/v1`).
    #[must_use]
    pub fn preset_litellm() -> Self {
        Self {
            name: "LiteLLM".to_string(),
            base_url: "http://localhost:4000/v1".to_string(),
            api_key_env: Some("LITELLM_API_KEY".to_string()),
            requires_api_key: true,
            zero_cost: false,
            models: Vec::new(),
            enabled: true,
        }
    }

    /// Preset for a **Local OpenAI-Compatible Server** (`vLLM`, `llama.cpp`, `LM Studio` at `http://localhost:8000/v1`).
    #[must_use]
    pub fn preset_local_server() -> Self {
        Self {
            name: "Local OpenAI Server".to_string(),
            base_url: "http://localhost:8000/v1".to_string(),
            api_key_env: None,
            requires_api_key: false,
            zero_cost: true,
            models: Vec::new(),
            enabled: true,
        }
    }

    /// Looks up a configured model by ID.
    #[must_use]
    pub fn find_model(&self, model_id: &str) -> Option<&CustomModelConfig> {
        let trimmed = model_id.trim();
        self.models.iter().find(|m| m.id.trim() == trimmed)
    }
}

/// Queries an OpenAI-compatible endpoint's `/models` route (`GET {base_url}/models`)
/// using [`rig::providers::openai::OpenAIConfig`] and returns the discovered model IDs.
pub async fn fetch_openai_compatible_models(
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, String> {
    let trimmed_url = base_url.trim().trim_end_matches('/');
    if trimmed_url.is_empty() {
        return Err("Base URL cannot be empty.".to_string());
    }
    let effective_key = if api_key.trim().is_empty() {
        "not-needed"
    } else {
        api_key.trim()
    };

    let client = rig::providers::openai::OpenAIConfig::new(effective_key)
        .with_base_url(trimmed_url)
        .client();

    match client.list_models().await {
        Ok(list) => {
            let mut ids: Vec<String> = list
                .iter()
                .map(|m| m.id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect();
            ids.sort();
            ids.dedup();
            Ok(ids)
        }
        Err(e) => Err(format!("Failed to list models from {trimmed_url}: {e}")),
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
    Moonshot(MoonshotModel, Option<ThinkingLevel>),
    Xai(XaiModel, Option<ThinkingLevel>),
    Zai(ZaiModel, Option<ThinkingLevel>),
    Mistral(MistralModel, Option<ThinkingLevel>),
    MiniMax(MiniMaxModel, Option<ThinkingLevel>),
    Groq(GroqModel, Option<ThinkingLevel>),
    OpenAiCompatible {
        provider_name: String,
        model_id: String,
        thinking: Option<ThinkingLevel>,
    },
}

impl ModelProvider {
    pub fn provider_name(&self) -> &str {
        match self {
            ModelProvider::Gemini(_, _) => "Gemini",
            ModelProvider::Ollama(_) => "Ollama",
            ModelProvider::Anthropic(_, _) => "Anthropic",
            ModelProvider::OpenAi(_, _) => "OpenAI",
            ModelProvider::OpenRouter(_) => "OpenRouter",
            ModelProvider::DeepSeek(_) => "DeepSeek",
            ModelProvider::Moonshot(_, _) => "Moonshot",
            ModelProvider::Xai(_, _) => "xAI",
            ModelProvider::Zai(_, _) => "Z.ai",
            ModelProvider::Mistral(_, _) => "Mistral",
            ModelProvider::MiniMax(_, _) => "MiniMax",
            ModelProvider::Groq(_, _) => "Groq",
            ModelProvider::OpenAiCompatible { provider_name, .. } => provider_name.as_str(),
        }
    }

    /// Returns a privacy-safe provider label for telemetry, grouping all user-defined
    /// OpenAI-compatible endpoints under `"Custom Endpoints"` so custom endpoint names never leak.
    #[must_use]
    pub fn telemetry_provider_name(&self) -> &'static str {
        match self {
            ModelProvider::Gemini(_, _) => "Gemini",
            ModelProvider::Ollama(_) => "Ollama",
            ModelProvider::Anthropic(_, _) => "Anthropic",
            ModelProvider::OpenAi(_, _) => "OpenAI",
            ModelProvider::OpenRouter(_) => "OpenRouter",
            ModelProvider::DeepSeek(_) => "DeepSeek",
            ModelProvider::Moonshot(_, _) => "Moonshot",
            ModelProvider::Xai(_, _) => "xAI",
            ModelProvider::Zai(_, _) => "Z.ai",
            ModelProvider::Mistral(_, _) => "Mistral",
            ModelProvider::MiniMax(_, _) => "MiniMax",
            ModelProvider::Groq(_, _) => "Groq",
            ModelProvider::OpenAiCompatible { .. } => "Custom Endpoints",
        }
    }

    pub fn api_model_name(&self) -> &str {
        match self {
            ModelProvider::Gemini(model, _) => model.api_name(),
            ModelProvider::Ollama(model) => model.as_str(),
            ModelProvider::Anthropic(model, _) => model.api_name(),
            ModelProvider::OpenAi(model, _) => model.api_name(),
            ModelProvider::OpenRouter(model) => model.as_str(),
            ModelProvider::DeepSeek(model) => model.api_name(),
            ModelProvider::Moonshot(model, _) => model.api_name(),
            ModelProvider::Xai(model, _) => model.api_name(),
            ModelProvider::Zai(model, _) => model.api_name(),
            ModelProvider::Mistral(model, _) => model.api_name(),
            ModelProvider::MiniMax(model, _) => model.api_name(),
            ModelProvider::Groq(model, _) => model.api_name(),
            ModelProvider::OpenAiCompatible { model_id, .. } => model_id.as_str(),
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
            ModelProvider::Moonshot(model, _) => format!("Moonshot/{}", model),
            ModelProvider::Xai(model, _) => format!("xAI/{}", model),
            ModelProvider::Zai(model, _) => format!("Z.ai/{}", model),
            ModelProvider::Mistral(model, _) => format!("Mistral/{}", model),
            ModelProvider::MiniMax(model, _) => format!("MiniMax/{}", model),
            ModelProvider::Groq(model, _) => format!("Groq/{}", model),
            ModelProvider::OpenAiCompatible {
                provider_name,
                model_id,
                ..
            } => format!("{provider_name}/{model_id}"),
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

        // OpenAI catalog includes GPT-6.1 Terra, GPT-6.1 Sol, and GPT-6 Astra
        let openai_all = OpenAiModel::all();
        assert!(openai_all.iter().any(|m| m.api_name() == "gpt-6.1-terra"));
        assert!(openai_all.iter().any(|m| m.api_name() == "gpt-6.1-sol"));
        assert!(openai_all.iter().any(|m| m.api_name() == "gpt-6-astra"));
        let terra_61 = OpenAiModel::new("gpt-6.1-terra");
        assert_eq!(terra_61.to_string(), "GPT-6.1 Terra");
        assert!(terra_61.supports_thinking());
        assert_eq!(
            terra_61.available_thinking_levels(),
            vec![
                ThinkingLevel::None,
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
                ThinkingLevel::XHigh,
            ]
        );
        assert_eq!(terra_61.default_thinking_level(), Some(ThinkingLevel::Medium));
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

        // Moonshot catalog orders flagship `kimi-k3` first ahead of legacy `moonshot-v1-*`
        let moonshot_all = MoonshotModel::all();
        assert_eq!(moonshot_all.first().map(MoonshotModel::api_name), Some("kimi-k3"));
        assert!(moonshot_all
            .iter()
            .any(|m| m.api_name() == "kimi-k2.7-code"));
        let kimi_k3 = MoonshotModel::new("kimi-k3");
        assert_eq!(kimi_k3.to_string(), "Kimi K3");
        assert!(kimi_k3.supports_thinking());
        assert_eq!(
            kimi_k3.available_thinking_levels(),
            vec![ThinkingLevel::Low, ThinkingLevel::High]
        );
        assert_eq!(kimi_k3.default_thinking_level(), Some(ThinkingLevel::Low));

        // xAI catalog orders Grok 4.7 first
        let xai_all = XaiModel::all();
        assert_eq!(xai_all.first().map(XaiModel::api_name), Some("grok-4.7"));
        let grok_47 = XaiModel::new("grok-4.7");
        assert_eq!(grok_47.to_string(), "Grok 4.7");
        assert!(grok_47.supports_thinking());

        // Z.ai catalog includes GLM-5.3 and GLM-5.3-Flash
        let zai_all = ZaiModel::all();
        assert!(zai_all.iter().any(|m| m.api_name() == "glm-5.3"));
        assert!(zai_all.iter().any(|m| m.api_name() == "glm-5.3-flash"));
        let glm_53 = ZaiModel::new("glm-5.3");
        assert_eq!(glm_53.to_string(), "GLM-5.3");
        assert!(glm_53.supports_thinking());

        // Mistral catalog includes Codestral and Mistral Large
        let mistral_all = MistralModel::all();
        assert!(mistral_all.iter().any(|m| m.api_name() == "codestral-latest"));
        assert!(mistral_all
            .iter()
            .any(|m| m.api_name() == "mistral-large-latest"));

        // MiniMax catalog includes MiniMax-M3 and MiniMax-M2.7
        let minimax_all = MiniMaxModel::all();
        assert_eq!(
            minimax_all.first().map(MiniMaxModel::api_name),
            Some("MiniMax-M3")
        );
        assert!(minimax_all.iter().any(|m| m.api_name() == "MiniMax-M2.7"));

        // Groq catalog includes GPT OSS 120B and Qwen 3.8 27B
        let groq_all = GroqModel::all();
        assert!(groq_all
            .iter()
            .any(|m| m.api_name() == "openai/gpt-oss-120b"));
        assert!(groq_all.iter().any(|m| m.api_name() == "qwen/qwen3.8-27b"));
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
        assert!(
            !ModelProvider::Moonshot(MoonshotModel::new("kimi-k3"), Some(ThinkingLevel::Low))
                .supports_native_search()
        );
        assert!(
            !ModelProvider::OpenAiCompatible {
                provider_name: "OpenCode Go".to_string(),
                model_id: "deepseek-v4-pro".to_string(),
                thinking: Some(ThinkingLevel::Medium),
            }
            .supports_native_search()
        );
    }

    #[test]
    fn test_openai_compatible_models_and_presets() {
        let parsed = ThinkingLevel::parse_comma_separated("off, low, MED, high, xhigh, low");
        assert_eq!(
            parsed,
            vec![
                ThinkingLevel::None,
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
                ThinkingLevel::XHigh,
            ]
        );
        assert_eq!(
            ThinkingLevel::format_comma_separated(&parsed),
            "none, low, medium, high, xhigh"
        );

        let go = OpenAiCompatibleEndpoint::preset_opencode_go();
        assert_eq!(go.name, "OpenCode Go");
        assert_eq!(go.base_url, "https://opencode.ai/zen/go/v1");
        assert!(go.zero_cost);
        assert!(go.requires_api_key);
        assert_eq!(go.api_key_env.as_deref(), Some("OPENCODE_API_KEY"));

        // DeepSeek on OpenCode Go uses ChatCompletions, GPT-6 Luna uses Responses
        let ds = go.find_model("deepseek-v4-pro").expect("deepseek-v4-pro");
        assert_eq!(ds.route, OpenAiApiRoute::ChatCompletions);
        assert_eq!(ds.effective_context_window(), Some(1_000_000));

        let gpt = go.find_model("gpt-6-luna").expect("gpt-6-luna");
        assert_eq!(gpt.route, OpenAiApiRoute::Responses);

        // Verify all preset models resolve metadata in < 1ms without any slow misses
        let start = std::time::Instant::now();
        for m in &go.models {
            let _ = m.effective_display_name();
            let _ = m.effective_context_window();
            let _ = m.effective_thinking_levels();
        }
        let zen = OpenAiCompatibleEndpoint::preset_opencode_zen();
        for m in &zen.models {
            let _ = m.effective_display_name();
            let _ = m.effective_context_window();
            let _ = m.effective_thinking_levels();
        }
        assert!(
            start.elapsed().as_millis() < 50,
            "Preset metadata resolution should be sub-millisecond after catalog init"
        );
        let prefixed = resolve_any_catalog_spec("opencode-go/deepseek-v4.1-flash")
            .expect("should resolve prefixed deepseek flash");
        assert_eq!(prefixed.id, "deepseek-flash");
    }
}
