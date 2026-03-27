use boxxy_model_selection::ModelProvider;
use rig::client::CompletionClient;
use rig::message::Message;
use rig::providers::openai::responses_api::ResponsesCompletionModel;
use serde_json::json;

pub mod utils;

#[derive(Clone)]
pub enum BoxxyAgent {
    // We use the concrete CompletionModel type from each provider since Agent is generic over the model.
    Gemini(rig::agent::Agent<rig::providers::gemini::CompletionModel>),
    Ollama(rig::agent::Agent<rig::providers::ollama::CompletionModel>),
    Anthropic(rig::agent::Agent<rig::providers::anthropic::completion::CompletionModel>),
    OpenAi(rig::agent::Agent<ResponsesCompletionModel>),
    Error(String),
}

impl BoxxyAgent {
    pub async fn chat(
        &self,
        prompt: &str,
        history: Vec<Message>,
    ) -> Result<(String, Option<rig::completion::Usage>), rig::completion::PromptError> {
        use rig::completion::Completion;

        match self {
            Self::Gemini(agent) => {
                let res = agent.completion(prompt, history).await?.send().await?;
                let text = res
                    .choice
                    .into_iter()
                    .next()
                    .and_then(|c| match c {
                        rig::completion::AssistantContent::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok((text, Some(res.usage)))
            }
            Self::Ollama(agent) => {
                let res = agent.completion(prompt, history).await?.send().await?;
                let text = res
                    .choice
                    .into_iter()
                    .next()
                    .and_then(|c| match c {
                        rig::completion::AssistantContent::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok((text, Some(res.usage)))
            }
            Self::Anthropic(agent) => {
                let res = agent.completion(prompt, history).await?.send().await?;
                let text = res
                    .choice
                    .into_iter()
                    .next()
                    .and_then(|c| match c {
                        rig::completion::AssistantContent::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok((text, Some(res.usage)))
            }
            Self::OpenAi(agent) => {
                let res = agent.completion(prompt, history).await?.send().await?;
                let text = res
                    .choice
                    .into_iter()
                    .next()
                    .and_then(|c| match c {
                        rig::completion::AssistantContent::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok((text, Some(res.usage)))
            }
            Self::Error(e) => Err(rig::completion::PromptError::CompletionError(
                rig::completion::CompletionError::ProviderError(e.clone()),
            )),
        }
    }

    pub async fn prompt(
        &self,
        prompt: &str,
    ) -> Result<(String, Option<rig::completion::Usage>), rig::completion::PromptError> {
        use rig::completion::Completion;

        match self {
            Self::Gemini(agent) => {
                let res = agent.completion(prompt, vec![]).await?.send().await?;
                let text = res
                    .choice
                    .into_iter()
                    .next()
                    .and_then(|c| match c {
                        rig::completion::AssistantContent::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok((text, Some(res.usage)))
            }
            Self::Ollama(agent) => {
                let res = agent.completion(prompt, vec![]).await?.send().await?;
                let text = res
                    .choice
                    .into_iter()
                    .next()
                    .and_then(|c| match c {
                        rig::completion::AssistantContent::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok((text, Some(res.usage)))
            }
            Self::Anthropic(agent) => {
                let res = agent.completion(prompt, vec![]).await?.send().await?;
                let text = res
                    .choice
                    .into_iter()
                    .next()
                    .and_then(|c| match c {
                        rig::completion::AssistantContent::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok((text, Some(res.usage)))
            }
            Self::OpenAi(agent) => {
                let res = agent.completion(prompt, vec![]).await?.send().await?;
                let text = res
                    .choice
                    .into_iter()
                    .next()
                    .and_then(|c| match c {
                        rig::completion::AssistantContent::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok((text, Some(res.usage)))
            }
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
        ModelProvider::OpenAi(model, thinking) => {
            let key = creds.api_keys.get("OpenAI").cloned().unwrap_or_default();
            let client = rig::providers::openai::Client::new(key.trim()).unwrap();
            let openai_model = client.completion_model(model.api_name());

            let mut builder = rig::agent::AgentBuilder::new(openai_model).preamble(system_prompt);

            if let Some(level) = thinking {
                builder = builder.additional_params(json!({
                    "reasoning": { "effort": level.api_name() }
                }));
            }

            BoxxyAgent::OpenAi(builder.build())
        }
    }
}
