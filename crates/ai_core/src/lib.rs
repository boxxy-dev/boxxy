use boxxy_model_selection::ModelProvider;
use rig::agent::{HookAction, PromptHook, ToolCallHookAction};
use rig::client::CompletionClient;
use rig::message::Message;
use rig::providers::openai::responses_api::ResponsesCompletionModel;
use rig::wasm_compat::WasmCompatSend;
use serde_json::json;
use std::future::Future;

pub mod utils;

#[derive(Clone)]
pub struct ModelContextHook {
    pub preamble: String,
}

impl<M: rig::completion::CompletionModel> PromptHook<M> for ModelContextHook {
    fn on_completion_call(
        &self,
        prompt: &Message,
        history: &[Message],
    ) -> impl Future<Output = HookAction> + WasmCompatSend {
        let preamble = self.preamble.clone();
        let prompt = prompt.clone();
        let history = history.to_vec();

        // Check if model-context debugging is explicitly enabled via dedicated env var
        let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
            .map(|v| v == "1")
            .unwrap_or(false);

        async move {
            if is_explicit {
                log::info!(
                    target: "model-context",
                    "\n=== MODEL CONTEXT SEND ===\nSYSTEM PROMPT:\n{}\n\nHISTORY:\n{:#?}\n\nUSER PROMPT:\n{:#?}\n==========================\n",
                    preamble,
                    history,
                    prompt
                );
            }
            HookAction::cont()
        }
    }

    fn on_tool_call(
        &self,
        tool_name: &str,
        _tool_call_id: Option<String>,
        _internal_call_id: &str,
        args: &str,
    ) -> impl Future<Output = ToolCallHookAction> + WasmCompatSend {
        let tool_name = tool_name.to_string();
        let args = args.to_string();

        let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
            .map(|v| v == "1")
            .unwrap_or(false);

        async move {
            if is_explicit {
                log::info!(
                    target: "model-context",
                    "\n=== MODEL TOOL CALL ===\nTOOL: {}\nARGS: {}\n=======================\n",
                    tool_name,
                    args
                );
            }
            ToolCallHookAction::cont()
        }
    }
}

#[derive(Clone)]
pub struct BoxxyAgent {
    inner: BoxxyAgentInner,
    preamble: String,
}

#[derive(Clone)]
enum BoxxyAgentInner {
    // We use the concrete CompletionModel type from each provider since Agent is generic over the model.
    Gemini(rig::agent::Agent<rig::providers::gemini::CompletionModel>),
    Ollama(rig::agent::Agent<rig::providers::ollama::CompletionModel>),
    Anthropic(rig::agent::Agent<rig::providers::anthropic::completion::CompletionModel>),
    OpenAi(rig::agent::Agent<ResponsesCompletionModel>),
    OpenRouter(rig::agent::Agent<ResponsesCompletionModel>),
    DeepSeek(rig::agent::Agent<rig::providers::deepseek::CompletionModel>),
    Error(String),
}

impl BoxxyAgent {
    pub async fn chat(
        &self,
        prompt: &str,
        history: Vec<Message>,
    ) -> Result<(String, Option<rig::completion::Usage>), rig::completion::PromptError> {
        use rig::completion::Prompt;

        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let res_result = match &self.inner {
            BoxxyAgentInner::Gemini(agent) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::Ollama(agent) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::Anthropic(agent) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::OpenAi(agent) => {
                agent
                    .prompt(prompt)
                    .with_history(history.clone())
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::OpenRouter(agent) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::DeepSeek(agent) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::Error(e) => {
                return Err(rig::completion::PromptError::CompletionError(
                    rig::completion::CompletionError::ProviderError(e.clone()),
                ));
            }
        };

        let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
            .map(|v| v == "1")
            .unwrap_or(false);

        match res_result {
            Ok(res) => {
                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL RESPONSE ===\n{}\n======================\n",
                        res.output
                    );
                }
                Ok((res.output.clone(), Some(res.usage)))
            }
            Err(e) => {
                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL ERROR ===\n{:?}\n===================\n",
                        e
                    );
                }
                Err(e)
            }
        }
    }

    pub async fn prompt(
        &self,
        prompt: &str,
    ) -> Result<(String, Option<rig::completion::Usage>), rig::completion::PromptError> {
        use rig::completion::Prompt;

        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let res_result = match &self.inner {
            BoxxyAgentInner::Gemini(agent) => {
                agent
                    .prompt(prompt)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::Ollama(agent) => {
                agent
                    .prompt(prompt)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::Anthropic(agent) => {
                agent
                    .prompt(prompt)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::OpenAi(agent) => {
                agent
                    .prompt(prompt)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::OpenRouter(agent) => {
                agent
                    .prompt(prompt)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::DeepSeek(agent) => {
                agent
                    .prompt(prompt)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            BoxxyAgentInner::Error(e) => {
                return Err(rig::completion::PromptError::CompletionError(
                    rig::completion::CompletionError::ProviderError(e.clone()),
                ));
            }
        };

        let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
            .map(|v| v == "1")
            .unwrap_or(false);

        match res_result {
            Ok(res) => {
                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL RESPONSE ===\n{}\n======================\n",
                        res.output
                    );
                }
                Ok((res.output.clone(), Some(res.usage)))
            }
            Err(e) => {
                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL ERROR ===\n{:?}\n===================\n",
                        e
                    );
                }
                Err(e)
            }
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
            return BoxxyAgent {
                inner: BoxxyAgentInner::Error(
                    "No AI model selected. Please configure your models in Preferences -> APIs -> Models Selection."
                        .to_string(),
                ),
                preamble: system_prompt.to_string(),
            }
        }
    };

    let inner = match provider {
        ModelProvider::Gemini(model, _thinking) => {
            let key = creds.api_keys.get("Gemini").cloned().unwrap_or_default();
            let client = rig::providers::gemini::Client::new(key.trim()).unwrap();
            let gemini_model = client.completion_model(model.api_name());

            let agent = rig::agent::AgentBuilder::new(gemini_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgentInner::Gemini(agent)
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
            BoxxyAgentInner::Ollama(agent)
        }
        ModelProvider::Anthropic(model) => {
            let key = creds.api_keys.get("Anthropic").cloned().unwrap_or_default();
            let client = rig::providers::anthropic::Client::new(key.trim()).unwrap();
            let anthropic_model = client.completion_model(model.api_name());

            let agent = rig::agent::AgentBuilder::new(anthropic_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgentInner::Anthropic(agent)
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

            BoxxyAgentInner::OpenAi(builder.build())
        }
        ModelProvider::OpenRouter(model_name) => {
            let key = creds
                .api_keys
                .get("OpenRouter")
                .cloned()
                .unwrap_or_default();
            let client = rig::providers::openai::Client::builder()
                .api_key(key.trim())
                .base_url("https://openrouter.ai/api/v1")
                .build()
                .unwrap();
            let openrouter_model = client.completion_model(model_name.as_str());

            let agent = rig::agent::AgentBuilder::new(openrouter_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgentInner::OpenRouter(agent)
        }
        ModelProvider::DeepSeek(model) => {
            let key = creds.api_keys.get("DeepSeek").cloned().unwrap_or_default();
            let client = rig::providers::deepseek::Client::new(key.trim()).unwrap();
            let deepseek_model = client.completion_model(model.api_name());

            let agent = rig::agent::AgentBuilder::new(deepseek_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgentInner::DeepSeek(agent)
        }
    };

    BoxxyAgent {
        inner,
        preamble: system_prompt.to_string(),
    }
}
