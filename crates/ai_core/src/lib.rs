use boxxy_model_selection::ModelProvider;
use rig::client::CompletionClient;
use rig::completion::{Chat, Prompt};
use rig::message::Message;

pub mod utils;

#[derive(Clone)]
pub enum BoxxyAgent {
    // We use the concrete CompletionModel type from each provider since Agent is generic over the model.
    Gemini(rig::agent::Agent<rig::providers::gemini::CompletionModel>),
    Ollama(rig::agent::Agent<rig::providers::ollama::CompletionModel>),
    Anthropic(rig::agent::Agent<rig::providers::anthropic::completion::CompletionModel>),
    Error(String),
}

impl BoxxyAgent {
    pub async fn chat(
        &self,
        prompt: &str,
        history: Vec<Message>,
    ) -> Result<String, rig::completion::PromptError> {
        match self {
            Self::Gemini(agent) => agent.chat(prompt, history).await,
            Self::Ollama(agent) => agent.chat(prompt, history).await,
            Self::Anthropic(agent) => agent.chat(prompt, history).await,
            Self::Error(e) => Err(rig::completion::PromptError::CompletionError(
                rig::completion::CompletionError::ProviderError(e.clone()),
            )),
        }
    }

    pub async fn prompt(&self, prompt: &str) -> Result<String, rig::completion::PromptError> {
        match self {
            Self::Gemini(agent) => agent.prompt(prompt).await,
            Self::Ollama(agent) => agent.prompt(prompt).await,
            Self::Anthropic(agent) => agent.prompt(prompt).await,
            Self::Error(e) => Err(rig::completion::PromptError::CompletionError(
                rig::completion::CompletionError::ProviderError(e.clone()),
            )),
        }
    }
}

#[derive(Clone, Default)]
pub struct AiCredentials {
    pub api_keys: std::collections::HashMap<String, String>,
    pub ollama_url: String,
}

impl AiCredentials {
    pub fn new(api_keys: std::collections::HashMap<String, String>, ollama_url: String) -> Self {
        Self {
            api_keys,
            ollama_url,
        }
    }
}

pub fn create_agent(
    provider: &Option<ModelProvider>,
    creds: &AiCredentials,
    system_prompt: &str,
) -> BoxxyAgent {
    let provider = match provider {
        Some(p) => p,
        None => {
            return BoxxyAgent::Error(
                "No AI model selected. Please configure your models in Settings -> APIs -> Models Selection."
                    .to_string(),
            )
        }
    };

    match provider {
        ModelProvider::Gemini(model, _thinking) => {
            let key = creds.api_keys.get("Gemini").cloned().unwrap_or_default();
            let client = rig::providers::gemini::Client::new(key.trim()).unwrap();
            let gemini_model = client.completion_model(model.api_name());

            let agent = rig::agent::AgentBuilder::new(gemini_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgent::Gemini(agent)
        }
        ModelProvider::Ollama(model_name) => {
            let client: rig::providers::ollama::Client = rig::providers::ollama::Client::builder()
                .api_key(rig::client::Nothing)
                .base_url(creds.ollama_url.as_str())
                .build()
                .unwrap();
            let ollama_model = client.completion_model(model_name.as_str());

            let agent = rig::agent::AgentBuilder::new(ollama_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgent::Ollama(agent)
        }
        ModelProvider::Anthropic(model) => {
            let key = creds.api_keys.get("Anthropic").cloned().unwrap_or_default();
            let client = rig::providers::anthropic::Client::new(key.trim()).unwrap();
            let anthropic_model = client.completion_model(model.api_name());

            let agent = rig::agent::AgentBuilder::new(anthropic_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgent::Anthropic(agent)
        }
    }
}
