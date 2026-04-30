use crate::engine::agent_config::AgentConfig;
use rig::message::Message;
use std::borrow::Cow;

pub fn provider_family(model: &Option<boxxy_model_selection::ModelProvider>) -> &'static str {
    match model {
        Some(boxxy_model_selection::ModelProvider::Gemini(_, _)) => "gemini",
        Some(boxxy_model_selection::ModelProvider::Ollama(_)) => "ollama",
        Some(boxxy_model_selection::ModelProvider::Anthropic(_, _)) => "anthropic",
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

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_model_selection::{AnthropicModel, GeminiModel, ModelProvider};

    fn gemini_config() -> AgentConfig {
        AgentConfig {
            model: Some(ModelProvider::Gemini(GeminiModel::Flash, None)),
            ..Default::default()
        }
    }

    fn anthropic_config() -> AgentConfig {
        AgentConfig {
            model: Some(ModelProvider::Anthropic(AnthropicModel::ClaudeSonnet, None)),
            ..Default::default()
        }
    }

    #[test]
    fn same_provider_family_borrows() {
        let history = vec![rig::message::Message::user("hi")];
        let result = maybe_sanitize_history(&history, Some(&gemini_config()), &gemini_config());
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn different_provider_family_returns_owned() {
        let history = vec![rig::message::Message::user("hi")];
        let result = maybe_sanitize_history(&history, Some(&gemini_config()), &anthropic_config());
        assert!(matches!(result, std::borrow::Cow::Owned(_)));
    }

    #[test]
    fn no_old_config_borrows() {
        let history = vec![rig::message::Message::user("hi")];
        let result = maybe_sanitize_history(&history, None, &anthropic_config());
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
    }
}
