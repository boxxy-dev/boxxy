use crate::engine::agent_config::AgentConfig;
use rig::message::Message;
use std::borrow::Cow;

pub fn provider_family(model: &Option<boxxy_model_selection::ModelProvider>) -> &'static str {
    match model {
        Some(boxxy_model_selection::ModelProvider::Gemini(_, _)) => "gemini",
        Some(boxxy_model_selection::ModelProvider::Ollama(_)) => "ollama",
        Some(boxxy_model_selection::ModelProvider::Anthropic(_)) => "anthropic",
        Some(boxxy_model_selection::ModelProvider::OpenAi(_, _)) => "openai",
        Some(boxxy_model_selection::ModelProvider::OpenRouter(_)) => "openrouter",
        Some(boxxy_model_selection::ModelProvider::DeepSeek(_)) => "deepseek",
        None => "unknown",
    }
}

pub fn maybe_sanitize_history<'a>(
    history: &'a [Message],
    old_config: Option<&AgentConfig>,
    new_config: &AgentConfig,
) -> Cow<'a, [Message]> {
    let family_changed = old_config
        .map(|old| provider_family(&old.model) != provider_family(&new_config.model))
        .unwrap_or(false);

    if family_changed {
        // TODO: implement stripping of tool blocks
        Cow::Owned(history.iter().cloned().collect())
    } else {
        Cow::Borrowed(history)
    }
}
