use rig::completion::{Chat, Prompt};
use rig::message::Message;
use rig::client::CompletionClient;
use boxxy_model_selection::ModelProvider;

pub mod utils;

#[derive(Clone)]
pub enum BoxxyAgent {
    // We use the concrete CompletionModel type from each provider since Agent is generic over the model.
    Gemini(rig::agent::Agent<rig::providers::gemini::CompletionModel>),
    Ollama(rig::agent::Agent<rig::providers::ollama::CompletionModel>),
}

impl BoxxyAgent {
    pub async fn chat(&self, prompt: &str, history: Vec<Message>) -> Result<String, rig::completion::PromptError> {
        match self {
            Self::Gemini(agent) => agent.chat(prompt, history).await,
            Self::Ollama(agent) => agent.chat(prompt, history).await,
        }
    }

    pub async fn prompt(&self, prompt: &str) -> Result<String, rig::completion::PromptError> {
        match self {
            Self::Gemini(agent) => agent.prompt(prompt).await,
            Self::Ollama(agent) => agent.prompt(prompt).await,
        }
    }
}

pub fn create_agent(
    provider: &ModelProvider, 
    api_key: &str, 
    ollama_url: &str,
    system_prompt: &str
) -> BoxxyAgent {
    match provider {
        ModelProvider::Gemini(model, _thinking) => {
            let client = rig::providers::gemini::Client::new(api_key.trim()).unwrap();
            let gemini_model = client.completion_model(model.api_name());
            
            let agent = rig::agent::AgentBuilder::new(gemini_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgent::Gemini(agent)
        },
        ModelProvider::Ollama(model_name) => {
            let client: rig::providers::ollama::Client = rig::providers::ollama::Client::builder()
                .api_key(rig::client::Nothing)
                .base_url(ollama_url)
                .build()
                .unwrap();
            let ollama_model = client.completion_model(model_name.as_str());
            
            let agent = rig::agent::AgentBuilder::new(ollama_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgent::Ollama(agent)
        }
    }
}
