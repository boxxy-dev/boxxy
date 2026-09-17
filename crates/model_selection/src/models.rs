use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeminiModel {
    #[serde(rename = "gemini-3.8-flash")]
    Flash3_8,
}

impl fmt::Display for GeminiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeminiModel::Flash3_8 => write!(f, "Gemini 3.8 Flash"),
        }
    }
}

impl GeminiModel {
    pub fn all() -> Vec<GeminiModel> {
        vec![GeminiModel::Flash3_8]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            GeminiModel::Flash3_8 => "gemini-3.8-flash",
        }
    }

    pub fn supports_thinking(&self) -> bool {
        true
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        vec![
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ]
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
            _ => 2_000, // Fallback for Minimal/XHigh/None if mistakenly applied
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnthropicModel {
    #[serde(rename = "claude-fable-5-1")]
    ClaudeFable51,
    #[serde(rename = "claude-sonnet-5")]
    ClaudeSonnet5,
    #[serde(rename = "claude-opus-5")]
    ClaudeOpus5,
}

impl fmt::Display for AnthropicModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnthropicModel::ClaudeFable51 => write!(f, "Claude Fable 5.1"),
            AnthropicModel::ClaudeSonnet5 => write!(f, "Claude Sonnet 5"),
            AnthropicModel::ClaudeOpus5 => write!(f, "Claude Opus 5"),
        }
    }
}

impl AnthropicModel {
    pub fn all() -> Vec<AnthropicModel> {
        vec![
            AnthropicModel::ClaudeFable51,
            AnthropicModel::ClaudeSonnet5,
            AnthropicModel::ClaudeOpus5,
        ]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            AnthropicModel::ClaudeFable51 => "claude-fable-5-1",
            AnthropicModel::ClaudeSonnet5 => "claude-sonnet-5",
            AnthropicModel::ClaudeOpus5 => "claude-opus-5",
        }
    }

    pub fn supports_extended_thinking(&self) -> bool {
        false // Claude 5 series uses native adaptive thinking
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        vec![
            ThinkingLevel::None,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenAiModel {
    #[serde(rename = "gpt-6-astra")]
    Gpt6Astra,
    #[serde(rename = "gpt-5.6-sol")]
    Gpt5_6Sol,
}

impl fmt::Display for OpenAiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenAiModel::Gpt6Astra => write!(f, "GPT-6 Astra"),
            OpenAiModel::Gpt5_6Sol => write!(f, "GPT-5.6 Sol"),
        }
    }
}

impl OpenAiModel {
    pub fn all() -> Vec<OpenAiModel> {
        vec![
            OpenAiModel::Gpt6Astra,
            OpenAiModel::Gpt5_6Sol,
        ]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            OpenAiModel::Gpt6Astra => "gpt-6-astra",
            OpenAiModel::Gpt5_6Sol => "gpt-5.6-sol",
        }
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        vec![
            ThinkingLevel::None,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::XHigh,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeepSeekModel {
    #[serde(rename = "deepseek-flash")]
    V4_1Flash,
}

impl fmt::Display for DeepSeekModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeepSeekModel::V4_1Flash => write!(f, "DeepSeek-V4.1-Flash"),
        }
    }
}

impl DeepSeekModel {
    pub fn all() -> Vec<DeepSeekModel> {
        vec![DeepSeekModel::V4_1Flash]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            DeepSeekModel::V4_1Flash => "deepseek-flash",
        }
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
}

impl Default for ModelProvider {
    fn default() -> Self {
        ModelProvider::Gemini(GeminiModel::Flash3_8, Some(ThinkingLevel::Low))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_model_deserialization() {
        let model_38: GeminiModel = serde_json::from_str("\"gemini-3.8-flash\"").unwrap();
        assert_eq!(model_38, GeminiModel::Flash3_8);
    }

    #[test]
    fn test_gemini_model_api_names() {
        assert_eq!(GeminiModel::Flash3_8.api_name(), "gemini-3.8-flash");
    }

    #[test]
    fn test_gemini_model_thinking_levels() {
        let levels_3_8 = GeminiModel::Flash3_8.available_thinking_levels();
        assert!(!levels_3_8.contains(&ThinkingLevel::Minimal));
        assert!(levels_3_8.contains(&ThinkingLevel::Low));
        assert!(levels_3_8.contains(&ThinkingLevel::Medium));
        assert!(levels_3_8.contains(&ThinkingLevel::High));
    }

    #[test]
    fn test_anthropic_models() {
        assert_eq!(AnthropicModel::ClaudeFable51.api_name(), "claude-fable-5-1");
        assert_eq!(AnthropicModel::ClaudeSonnet5.api_name(), "claude-sonnet-5");
        assert_eq!(AnthropicModel::ClaudeOpus5.api_name(), "claude-opus-5");
        assert!(!AnthropicModel::ClaudeFable51.supports_extended_thinking());
        assert!(!AnthropicModel::ClaudeSonnet5.supports_extended_thinking());
        assert!(!AnthropicModel::ClaudeOpus5.supports_extended_thinking());
    }

    #[test]
    fn test_openai_models() {
        assert_eq!(OpenAiModel::Gpt6Astra.api_name(), "gpt-6-astra");
        assert_eq!(OpenAiModel::Gpt5_6Sol.api_name(), "gpt-5.6-sol");
    }

    #[test]
    fn test_deepseek_models() {
        assert_eq!(DeepSeekModel::V4_1Flash.api_name(), "deepseek-flash");
    }
}
