use crate::models::{AnthropicModel, DeepSeekModel, GeminiModel, ModelProvider, OpenAiModel};
use gtk4 as gtk;
use gtk4::prelude::*;

// --- Provider Abstraction Trait ---

pub trait AiProvider {
    fn name(&self) -> &'static str;

    /// Returns true if this provider requires an API key in settings.
    fn requires_api_key(&self) -> bool {
        true
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

struct GeminiProviderImpl;
impl AiProvider for GeminiProviderImpl {
    fn name(&self) -> &'static str {
        "Gemini"
    }
    fn get_models(&self) -> Vec<String> {
        GeminiModel::all()
            .into_iter()
            .map(|m| m.to_string())
            .collect()
    }
    fn supports_thinking(&self, model_idx: u32) -> bool {
        let am = GeminiModel::all();
        if let Some(m) = am.get(model_idx as usize) {
            return m.supports_thinking();
        }
        false
    }
    fn get_thinking_levels(&self, model_idx: u32) -> Vec<String> {
        let am = GeminiModel::all();
        if let Some(m) = am.get(model_idx as usize) {
            return m
                .available_thinking_levels()
                .into_iter()
                .map(|l| l.to_string())
                .collect();
        }
        vec![]
    }
    fn create_model_provider(
        &self,
        model_idx: u32,
        _name: Option<String>,
        thinking_idx: Option<u32>,
    ) -> ModelProvider {
        let am = GeminiModel::all();
        let model = am
            .get(model_idx as usize)
            .cloned()
            .unwrap_or(GeminiModel::Flash);
        let levels = model.available_thinking_levels();
        let thinking = thinking_idx
            .and_then(|idx| levels.get(idx as usize))
            .cloned();
        ModelProvider::Gemini(model, thinking)
    }
    fn sync_ui(
        &self,
        provider: &ModelProvider,
        model_dropdown: &gtk::DropDown,
        thinking_dropdown: &gtk::DropDown,
        _model_list: &gtk::StringList,
        thinking_list: &gtk::StringList,
    ) {
        if let ModelProvider::Gemini(m, t) = provider {
            let am = GeminiModel::all();
            if let Some(pos) = am.iter().position(|x| x == m) {
                model_dropdown.set_selected(pos as u32);
                thinking_list.splice(0, thinking_list.n_items(), &[]);
                let levels = m.available_thinking_levels();
                for l in &levels {
                    thinking_list.append(&l.to_string());
                }
                if let Some(think) = t
                    && let Some(t_pos) = levels.iter().position(|l| l == think)
                {
                    thinking_dropdown.set_selected(t_pos as u32);
                }
            }
        }
    }
}

struct AnthropicProviderImpl;
impl AiProvider for AnthropicProviderImpl {
    fn name(&self) -> &'static str {
        "Anthropic"
    }
    fn get_models(&self) -> Vec<String> {
        AnthropicModel::all()
            .into_iter()
            .map(|m| m.to_string())
            .collect()
    }
    fn supports_thinking(&self, model_idx: u32) -> bool {
        let am = AnthropicModel::all();
        if let Some(m) = am.get(model_idx as usize) {
            m.supports_extended_thinking()
        } else {
            false
        }
    }
    fn get_thinking_levels(&self, model_idx: u32) -> Vec<String> {
        let am = AnthropicModel::all();
        if let Some(m) = am.get(model_idx as usize) {
            m.available_thinking_levels()
                .into_iter()
                .map(|l| l.to_string())
                .collect()
        } else {
            vec![]
        }
    }
    fn create_model_provider(
        &self,
        model_idx: u32,
        _name: Option<String>,
        thinking_idx: Option<u32>,
    ) -> ModelProvider {
        let am = AnthropicModel::all();
        let model = am
            .get(model_idx as usize)
            .cloned()
            .unwrap_or(AnthropicModel::ClaudeSonnet);

        let thinking = if model.supports_extended_thinking() {
            if let Some(idx) = thinking_idx {
                model.available_thinking_levels().get(idx as usize).cloned()
            } else {
                None
            }
        } else {
            None
        };

        ModelProvider::Anthropic(model, thinking)
    }
    fn sync_ui(
        &self,
        provider: &ModelProvider,
        model_dropdown: &gtk::DropDown,
        thinking_dropdown: &gtk::DropDown,
        _model_list: &gtk::StringList,
        thinking_list: &gtk::StringList,
    ) {
        if let ModelProvider::Anthropic(m, t) = provider {
            let am = AnthropicModel::all();
            if let Some(pos) = am.iter().position(|x| x == m) {
                model_dropdown.set_selected(pos as u32);
                thinking_list.splice(0, thinking_list.n_items(), &[]);
                let levels = m.available_thinking_levels();
                for l in &levels {
                    thinking_list.append(&l.to_string());
                }
                if let Some(think) = t
                    && let Some(t_pos) = levels.iter().position(|l| l == think)
                {
                    thinking_dropdown.set_selected(t_pos as u32);
                }
            }
        }
    }
}

struct OpenAiProviderImpl;
impl AiProvider for OpenAiProviderImpl {
    fn name(&self) -> &'static str {
        "OpenAI"
    }
    fn get_models(&self) -> Vec<String> {
        OpenAiModel::all()
            .into_iter()
            .map(|m| m.to_string())
            .collect()
    }
    fn supports_thinking(&self, _model_idx: u32) -> bool {
        true
    }
    fn get_thinking_levels(&self, model_idx: u32) -> Vec<String> {
        let am = OpenAiModel::all();
        if let Some(m) = am.get(model_idx as usize) {
            return m
                .available_thinking_levels()
                .into_iter()
                .map(|l| l.to_string())
                .collect();
        }
        vec![]
    }
    fn create_model_provider(
        &self,
        model_idx: u32,
        _name: Option<String>,
        thinking_idx: Option<u32>,
    ) -> ModelProvider {
        let am = OpenAiModel::all();
        let model = am
            .get(model_idx as usize)
            .cloned()
            .unwrap_or(OpenAiModel::Gpt5_4);
        let levels = model.available_thinking_levels();
        let thinking = thinking_idx
            .and_then(|idx| levels.get(idx as usize))
            .cloned();
        ModelProvider::OpenAi(model, thinking)
    }
    fn sync_ui(
        &self,
        provider: &ModelProvider,
        model_dropdown: &gtk::DropDown,
        thinking_dropdown: &gtk::DropDown,
        _model_list: &gtk::StringList,
        thinking_list: &gtk::StringList,
    ) {
        if let ModelProvider::OpenAi(m, t) = provider {
            let am = OpenAiModel::all();
            if let Some(pos) = am.iter().position(|x| x == m) {
                model_dropdown.set_selected(pos as u32);
                thinking_list.splice(0, thinking_list.n_items(), &[]);
                let levels = m.available_thinking_levels();
                for l in &levels {
                    thinking_list.append(&l.to_string());
                }
                if let Some(think) = t
                    && let Some(t_pos) = levels.iter().position(|l| l == think)
                {
                    thinking_dropdown.set_selected(t_pos as u32);
                }
            }
        }
    }
}

struct OllamaProviderImpl;
impl AiProvider for OllamaProviderImpl {
    fn name(&self) -> &'static str {
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
    fn name(&self) -> &'static str {
        "OpenRouter"
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
    fn name(&self) -> &'static str {
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
            .unwrap_or(DeepSeekModel::Flash);
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
            if let Some(pos) = am.iter().position(|x| x == m) {
                model_dropdown.set_selected(pos as u32);
            }
        }
    }
}

pub fn get_providers() -> Vec<Box<dyn AiProvider>> {
    vec![
        Box::new(GeminiProviderImpl),
        Box::new(OllamaProviderImpl),
        Box::new(AnthropicProviderImpl),
        Box::new(OpenAiProviderImpl),
        Box::new(OpenRouterProviderImpl),
        Box::new(DeepSeekProviderImpl),
    ]
}
