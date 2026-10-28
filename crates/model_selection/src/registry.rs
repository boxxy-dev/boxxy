use crate::models::{
    AnthropicModel, CustomModelConfig, DeepSeekModel, GeminiModel, GroqModel, MiniMaxModel,
    MistralModel, ModelProvider, MoonshotModel, OpenAiCompatibleEndpoint, OpenAiModel,
    ThinkingLevel, XaiModel, ZaiModel, catalog_thinking_levels_for_any_slug,
};
use gtk4 as gtk;
use gtk4::prelude::*;

// --- Provider Abstraction Trait ---

pub trait AiProvider {
    fn name(&self) -> &str;

    /// Returns true if this provider requires an API key in settings.
    fn requires_api_key(&self) -> bool {
        true
    }

    /// Returns true if this provider uses a free-form text entry instead of a fixed dropdown.
    fn uses_custom_entry(&self) -> bool {
        false
    }

    /// Returns a list of model display names.
    fn get_models(&self) -> Vec<String>;

    /// Returns the ModelProvider enum for a given selection.
    fn create_model_provider(
        &self,
        model_idx: u32,
        model_name: Option<String>,
        thinking_idx: Option<u32>,
    ) -> ModelProvider;

    /// Returns thinking levels if supported for the model.
    fn get_thinking_levels(&self, _model_idx: u32) -> Vec<String> {
        vec![]
    }

    /// Returns the recommended default index in `get_thinking_levels(model_idx)`.
    fn default_thinking_idx(&self, _model_idx: u32) -> u32 {
        0
    }

    /// Whether to show the thinking options for this model.
    fn supports_thinking(&self, _model_idx: u32) -> bool {
        false
    }

    /// Update the UI dropdowns to match a specific ModelProvider instance.
    fn sync_ui(
        &self,
        provider: &ModelProvider,
        model_dropdown: &gtk::DropDown,
        thinking_dropdown: &gtk::DropDown,
        model_list: &gtk::StringList,
        thinking_list: &gtk::StringList,
    );
}

macro_rules! impl_catalog_provider {
    ($struct_name:ident, $provider_name:literal, $model_type:ident, $variant:ident, $fallback_id:literal) => {
        struct $struct_name;
        impl AiProvider for $struct_name {
            fn name(&self) -> &str {
                $provider_name
            }
            fn get_models(&self) -> Vec<String> {
                $model_type::all()
                    .into_iter()
                    .map(|m| m.to_string())
                    .collect()
            }
            fn supports_thinking(&self, model_idx: u32) -> bool {
                let am = $model_type::all();
                am.get(model_idx as usize)
                    .is_some_and($model_type::supports_thinking)
            }
            fn get_thinking_levels(&self, model_idx: u32) -> Vec<String> {
                let am = $model_type::all();
                if let Some(m) = am.get(model_idx as usize) {
                    return m
                        .available_thinking_levels()
                        .into_iter()
                        .map(|l| l.to_string())
                        .collect();
                }
                vec![]
            }
            fn default_thinking_idx(&self, model_idx: u32) -> u32 {
                let am = $model_type::all();
                if let Some(m) = am.get(model_idx as usize) {
                    let levels = m.available_thinking_levels();
                    if let Some(def) = m.default_thinking_level()
                        && let Some(pos) = levels.iter().position(|l| *l == def)
                    {
                        return pos as u32;
                    }
                }
                0
            }
            fn create_model_provider(
                &self,
                model_idx: u32,
                _name: Option<String>,
                thinking_idx: Option<u32>,
            ) -> ModelProvider {
                let am = $model_type::all();
                let model = am
                    .get(model_idx as usize)
                    .cloned()
                    .unwrap_or_else(|| $model_type::new($fallback_id));
                let levels = model.available_thinking_levels();
                let thinking = thinking_idx
                    .and_then(|idx| levels.get(idx as usize))
                    .copied();
                ModelProvider::$variant(model, thinking)
            }
            fn sync_ui(
                &self,
                provider: &ModelProvider,
                model_dropdown: &gtk::DropDown,
                thinking_dropdown: &gtk::DropDown,
                _model_list: &gtk::StringList,
                thinking_list: &gtk::StringList,
            ) {
                if let ModelProvider::$variant(m, t) = provider {
                    let am = $model_type::all();
                    if let Some(pos) = am.iter().position(|x| x.api_name() == m.api_name()) {
                        model_dropdown.set_selected(pos as u32);
                        thinking_list.splice(0, thinking_list.n_items(), &[]);
                        let levels = am[pos].available_thinking_levels();
                        for l in &levels {
                            thinking_list.append(&l.to_string());
                        }
                        if let Some(think) = t
                            && let Some(t_pos) = levels.iter().position(|l| l == think)
                        {
                            thinking_dropdown.set_selected(t_pos as u32);
                        } else if !levels.is_empty() {
                            thinking_dropdown.set_selected(self.default_thinking_idx(pos as u32));
                        }
                    }
                }
            }
        }
    };
}

impl_catalog_provider!(
    GeminiProviderImpl,
    "Gemini",
    GeminiModel,
    Gemini,
    "gemini-3.8-flash"
);
impl_catalog_provider!(
    AnthropicProviderImpl,
    "Anthropic",
    AnthropicModel,
    Anthropic,
    "claude-sonnet-5-5"
);
impl_catalog_provider!(
    OpenAiProviderImpl,
    "OpenAI",
    OpenAiModel,
    OpenAi,
    "gpt-6.1-sol"
);
impl_catalog_provider!(
    MoonshotProviderImpl,
    "Moonshot",
    MoonshotModel,
    Moonshot,
    "kimi-k3"
);
impl_catalog_provider!(XaiProviderImpl, "xAI", XaiModel, Xai, "grok-4.7");
impl_catalog_provider!(ZaiProviderImpl, "Z.ai", ZaiModel, Zai, "glm-5.3");
impl_catalog_provider!(
    MistralProviderImpl,
    "Mistral",
    MistralModel,
    Mistral,
    "mistral-large-latest"
);
impl_catalog_provider!(
    MiniMaxProviderImpl,
    "MiniMax",
    MiniMaxModel,
    MiniMax,
    "MiniMax-M3"
);
impl_catalog_provider!(
    GroqProviderImpl,
    "Groq",
    GroqModel,
    Groq,
    "openai/gpt-oss-120b"
);

struct OllamaProviderImpl;
impl AiProvider for OllamaProviderImpl {
    fn name(&self) -> &str {
        "Ollama"
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    fn get_models(&self) -> Vec<String> {
        vec!["Loading...".to_string()]
    }
    fn create_model_provider(
        &self,
        _idx: u32,
        name: Option<String>,
        _think: Option<u32>,
    ) -> ModelProvider {
        ModelProvider::Ollama(name.unwrap_or_default())
    }
    fn sync_ui(
        &self,
        provider: &ModelProvider,
        model_dropdown: &gtk::DropDown,
        _thinking_dropdown: &gtk::DropDown,
        model_list: &gtk::StringList,
        _thinking_list: &gtk::StringList,
    ) {
        if let ModelProvider::Ollama(m) = provider {
            let mut found_pos = None;
            for i in 0..model_list.n_items() {
                if let Some(item) = model_list
                    .item(i)
                    .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                    && item.string().as_str() == m
                {
                    found_pos = Some(i);
                    break;
                }
            }
            if let Some(pos) = found_pos {
                model_dropdown.set_selected(pos);
            } else if !m.is_empty() && m != "Loading..." && m != "Ollama Offline" {
                model_list.append(m);
                model_dropdown.set_selected(model_list.n_items() - 1);
            }
        }
    }
}

struct OpenRouterProviderImpl;
impl AiProvider for OpenRouterProviderImpl {
    fn name(&self) -> &str {
        "OpenRouter"
    }
    fn uses_custom_entry(&self) -> bool {
        true
    }
    fn get_models(&self) -> Vec<String> {
        vec![]
    }
    fn create_model_provider(
        &self,
        _idx: u32,
        name: Option<String>,
        _think: Option<u32>,
    ) -> ModelProvider {
        ModelProvider::OpenRouter(name.unwrap_or_default())
    }
    fn sync_ui(
        &self,
        _provider: &ModelProvider,
        _model_dropdown: &gtk::DropDown,
        _thinking_dropdown: &gtk::DropDown,
        _model_list: &gtk::StringList,
        _thinking_list: &gtk::StringList,
    ) {
        // UI sync is handled specially in selector.rs for custom text entry
    }
}

struct DeepSeekProviderImpl;
impl AiProvider for DeepSeekProviderImpl {
    fn name(&self) -> &str {
        "DeepSeek"
    }
    fn get_models(&self) -> Vec<String> {
        DeepSeekModel::all()
            .into_iter()
            .map(|m| m.to_string())
            .collect()
    }
    fn create_model_provider(
        &self,
        model_idx: u32,
        _name: Option<String>,
        _thinking: Option<u32>,
    ) -> ModelProvider {
        let am = DeepSeekModel::all();
        let model = am
            .get(model_idx as usize)
            .cloned()
            .unwrap_or_else(|| DeepSeekModel::new("deepseek-flash"));
        ModelProvider::DeepSeek(model)
    }
    fn sync_ui(
        &self,
        provider: &ModelProvider,
        model_dropdown: &gtk::DropDown,
        _thinking_dropdown: &gtk::DropDown,
        _model_list: &gtk::StringList,
        _thinking_list: &gtk::StringList,
    ) {
        if let ModelProvider::DeepSeek(m) = provider {
            let am = DeepSeekModel::all();
            if let Some(pos) = am.iter().position(|x| x.api_name() == m.api_name()) {
                model_dropdown.set_selected(pos as u32);
            }
        }
    }
}

struct OpenAiCompatibleProviderImpl {
    endpoint: OpenAiCompatibleEndpoint,
}

impl AiProvider for OpenAiCompatibleProviderImpl {
    fn name(&self) -> &str {
        self.endpoint.name.as_str()
    }

    fn requires_api_key(&self) -> bool {
        self.endpoint.requires_api_key
    }

    fn uses_custom_entry(&self) -> bool {
        self.endpoint.models.is_empty()
    }

    fn get_models(&self) -> Vec<String> {
        self.endpoint
            .models
            .iter()
            .map(CustomModelConfig::effective_display_name)
            .collect()
    }

    fn supports_thinking(&self, model_idx: u32) -> bool {
        self.endpoint
            .models
            .get(model_idx as usize)
            .is_some_and(CustomModelConfig::supports_thinking)
    }

    fn get_thinking_levels(&self, model_idx: u32) -> Vec<String> {
        if let Some(m) = self.endpoint.models.get(model_idx as usize) {
            m.effective_thinking_levels()
                .into_iter()
                .map(|l| l.to_string())
                .collect()
        } else {
            vec![]
        }
    }

    fn default_thinking_idx(&self, model_idx: u32) -> u32 {
        if let Some(m) = self.endpoint.models.get(model_idx as usize) {
            let levels = m.effective_thinking_levels();
            if let Some(def) = m.default_thinking_level()
                && let Some(pos) = levels.iter().position(|l| *l == def)
            {
                return pos as u32;
            }
        }
        0
    }

    fn create_model_provider(
        &self,
        model_idx: u32,
        model_name: Option<String>,
        thinking_idx: Option<u32>,
    ) -> ModelProvider {
        if let Some(m) = self.endpoint.models.get(model_idx as usize) {
            let levels = m.effective_thinking_levels();
            let thinking = thinking_idx
                .and_then(|idx| levels.get(idx as usize))
                .copied();
            ModelProvider::OpenAiCompatible {
                provider_name: self.endpoint.name.clone(),
                model_id: m.id.clone(),
                thinking,
            }
        } else {
            let slug = model_name.unwrap_or_default();
            let levels: Vec<ThinkingLevel> = catalog_thinking_levels_for_any_slug(&slug);
            let thinking = thinking_idx
                .and_then(|idx| levels.get(idx as usize))
                .copied();
            ModelProvider::OpenAiCompatible {
                provider_name: self.endpoint.name.clone(),
                model_id: slug,
                thinking,
            }
        }
    }

    fn sync_ui(
        &self,
        provider: &ModelProvider,
        model_dropdown: &gtk::DropDown,
        thinking_dropdown: &gtk::DropDown,
        _model_list: &gtk::StringList,
        thinking_list: &gtk::StringList,
    ) {
        if let ModelProvider::OpenAiCompatible {
            model_id, thinking, ..
        } = provider
            && let Some(pos) = self
                .endpoint
                .models
                .iter()
                .position(|x| x.id.trim() == model_id.trim())
        {
            model_dropdown.set_selected(pos as u32);
            thinking_list.splice(0, thinking_list.n_items(), &[]);
            let levels = self.endpoint.models[pos].effective_thinking_levels();
            for l in &levels {
                thinking_list.append(&l.to_string());
            }
            if let Some(think) = thinking
                && let Some(t_pos) = levels.iter().position(|l| l == think)
            {
                thinking_dropdown.set_selected(t_pos as u32);
            } else if !levels.is_empty() {
                thinking_dropdown.set_selected(self.default_thinking_idx(pos as u32));
            }
        }
    }
}

pub fn get_providers() -> Vec<Box<dyn AiProvider>> {
    get_providers_with_custom(&[])
}

pub fn get_providers_with_custom(
    custom_endpoints: &[OpenAiCompatibleEndpoint],
) -> Vec<Box<dyn AiProvider>> {
    let mut providers: Vec<Box<dyn AiProvider>> = vec![
        Box::new(GeminiProviderImpl),
        Box::new(OllamaProviderImpl),
        Box::new(AnthropicProviderImpl),
        Box::new(OpenAiProviderImpl),
        Box::new(OpenRouterProviderImpl),
        Box::new(DeepSeekProviderImpl),
        Box::new(MoonshotProviderImpl),
        Box::new(XaiProviderImpl),
        Box::new(ZaiProviderImpl),
        Box::new(MistralProviderImpl),
        Box::new(MiniMaxProviderImpl),
        Box::new(GroqProviderImpl),
    ];

    for ep in custom_endpoints {
        if ep.enabled && !ep.name.trim().is_empty() && !ep.base_url.trim().is_empty() {
            providers.push(Box::new(OpenAiCompatibleProviderImpl {
                endpoint: ep.clone(),
            }));
        }
    }

    providers
}
