use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeminiModel {
    #[serde(rename = "gemini-3.1-pro-preview")]
    Pro,
    #[serde(rename = "gemini-3.1-flash-lite-preview")]
    Flash,
}

impl fmt::Display for GeminiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeminiModel::Pro => write!(f, "Gemini 3.1 Pro"),
            GeminiModel::Flash => write!(f, "Gemini 3.1 Flash Lite"),
        }
    }
}

impl GeminiModel {
    pub fn all() -> Vec<GeminiModel> {
        vec![GeminiModel::Pro, GeminiModel::Flash]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            GeminiModel::Pro => "gemini-3.1-pro-preview",
            GeminiModel::Flash => "gemini-3.1-flash-lite-preview",
        }
    }

    pub fn supports_thinking(&self) -> bool {
        true
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        match self {
            GeminiModel::Pro => vec![
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
            ],
            GeminiModel::Flash => vec![
                ThinkingLevel::Minimal,
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
            ],
        }
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
    #[serde(rename = "claude-opus-4-6")]
    ClaudeOpus,
    #[serde(rename = "claude-opus-4-7")]
    ClaudeOpus47,
    #[serde(rename = "claude-sonnet-4-6")]
    ClaudeSonnet,
}

impl fmt::Display for AnthropicModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnthropicModel::ClaudeOpus => write!(f, "Claude Opus 4.6"),
            AnthropicModel::ClaudeOpus47 => write!(f, "Claude Opus 4.7"),
            AnthropicModel::ClaudeSonnet => write!(f, "Claude Sonnet 4.6"),
        }
    }
}

impl AnthropicModel {
    pub fn all() -> Vec<AnthropicModel> {
        vec![AnthropicModel::ClaudeSonnet, AnthropicModel::ClaudeOpus, AnthropicModel::ClaudeOpus47]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            AnthropicModel::ClaudeOpus => "claude-opus-4-6",
            AnthropicModel::ClaudeOpus47 => "claude-opus-4-7",
            AnthropicModel::ClaudeSonnet => "claude-sonnet-4-6",
        }
    }

    pub fn supports_extended_thinking(&self) -> bool {
        match self {
            AnthropicModel::ClaudeOpus47 => false, // Adaptive only, no budget param
            _ => true,
        }
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
    #[serde(rename = "gpt-5.4")]
    Gpt5_4,
    #[serde(rename = "gpt-5.4-mini")]
    Gpt5_4Mini,
    #[serde(rename = "gpt-5.4-nano")]
    Gpt5_4Nano,
}

impl fmt::Display for OpenAiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenAiModel::Gpt5_4 => write!(f, "GPT-5.4"),
            OpenAiModel::Gpt5_4Mini => write!(f, "GPT-5.4 Mini"),
            OpenAiModel::Gpt5_4Nano => write!(f, "GPT-5.4 Nano"),
        }
    }
}

impl OpenAiModel {
    pub fn all() -> Vec<OpenAiModel> {
        vec![
            OpenAiModel::Gpt5_4,
            OpenAiModel::Gpt5_4Mini,
            OpenAiModel::Gpt5_4Nano,
        ]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            OpenAiModel::Gpt5_4 => "gpt-5.4",
            OpenAiModel::Gpt5_4Mini => "gpt-5.4-mini",
            OpenAiModel::Gpt5_4Nano => "gpt-5.4-nano",
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
    #[serde(rename = "deepseek-v4-pro")]
    Pro,
    #[serde(rename = "deepseek-v4-flash")]
    Flash,
}

impl fmt::Display for DeepSeekModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeepSeekModel::Pro => write!(f, "DeepSeek-V4-Pro"),
            DeepSeekModel::Flash => write!(f, "DeepSeek-V4-Flash"),
        }
    }
}

impl DeepSeekModel {
    pub fn all() -> Vec<DeepSeekModel> {
        vec![DeepSeekModel::Pro, DeepSeekModel::Flash]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            DeepSeekModel::Pro => "deepseek-v4-pro",
            DeepSeekModel::Flash => "deepseek-v4-flash",
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
        ModelProvider::Gemini(GeminiModel::Flash, Some(ThinkingLevel::Low))
    }
}
