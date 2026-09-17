use boxxy_model_selection::ModelProvider;
use rig::agent::hook::{
    AgentHook, CompletionCall, CompletionCallAction, HookContext, ToolCall, ToolCallAction,
};
use rig::message::Message;
use rig::prelude::*;
use rig::wasm_compat::WasmCompatSend;
use serde_json::json;
use std::future::Future;

#[derive(Clone)]
pub struct ModelContextHook {
    pub preamble: String,
}

impl AgentHook for ModelContextHook {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        event: CompletionCall<'_>,
    ) -> impl Future<Output = CompletionCallAction> + WasmCompatSend {
        let preamble = self.preamble.clone();
        let prompt = event.prompt.clone();
        let history = event.history.to_vec();

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
            CompletionCallAction::Continue
        }
    }

    fn on_tool_call(
        &self,
        _ctx: &HookContext,
        event: ToolCall<'_>,
    ) -> impl Future<Output = ToolCallAction> + WasmCompatSend {
        let tool_name = event.tool_name.to_string();
        let args = event.args.to_string();

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
            ToolCallAction::Run
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
    Ready(rig::agent::Agent),
    Error(String),
}

impl BoxxyAgent {
    pub async fn chat<P: Into<Message> + Send>(
        &self,
        prompt: P,
        history: Vec<Message>,
    ) -> Result<(String, Option<rig::completion::Usage>, Option<Vec<Message>>), rig::completion::PromptError> {
        use rig::completion::Prompt;

        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let msg = prompt.into();

        let agent = match &self.inner {
            BoxxyAgentInner::Ready(agent) => agent,
            BoxxyAgentInner::Error(e) => {
                return Err(rig::completion::PromptError::CompletionError(
                    rig::completion::CompletionError::ProviderError(e.clone()),
                ));
            }
        };

        let res_result = agent
            .prompt(msg)
            .history(history)
            .add_hook(hook)
            .extended_details()
            .await;

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
                Ok((res.output, Some(res.usage), res.messages))
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
    ) -> Result<(String, Option<rig::completion::Usage>, Option<Vec<Message>>), rig::completion::PromptError> {
        use rig::completion::Prompt;

        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let agent = match &self.inner {
            BoxxyAgentInner::Ready(agent) => agent,
            BoxxyAgentInner::Error(e) => {
                return Err(rig::completion::PromptError::CompletionError(
                    rig::completion::CompletionError::ProviderError(e.clone()),
                ));
            }
        };

        let res_result = agent
            .prompt(prompt)
            .add_hook(hook)
            .extended_details()
            .await;

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
                Ok((res.output, Some(res.usage), res.messages))
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
        ModelProvider::Gemini(model, thinking) => {
            let key = creds.api_keys.get("Gemini").cloned().unwrap_or_default();
            let client = rig::providers::gemini::Client::new(key.trim()).unwrap();
            let gemini_model = client.completion_model(model.api_name());

            let mut builder = rig::agent::AgentBuilder::new(gemini_model)
                .preamble(system_prompt);

            if let Some(level) = thinking {
                if *level != boxxy_model_selection::ThinkingLevel::None {
                    builder = builder.additional_params(serde_json::json!({
                        "generationConfig": {
                            "thinkingConfig": {
                                "thinkingLevel": level.api_name()
                            }
                        }
                    }));
                }
            }

            let agent = builder.build();
            BoxxyAgentInner::Ready(agent)
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
            BoxxyAgentInner::Ready(agent)
        }
        ModelProvider::Anthropic(model, thinking) => {
            let key = creds.api_keys.get("Anthropic").cloned().unwrap_or_default();
            let client = rig::providers::anthropic::Client::new(key.trim()).unwrap();
            let anthropic_model = client.completion_model(model.api_name());

            let mut builder =
                rig::agent::AgentBuilder::new(anthropic_model).preamble(system_prompt);

            if let Some(level) = thinking {
                if *level != boxxy_model_selection::ThinkingLevel::None
                    && model.supports_extended_thinking()
                {
                    builder = builder.additional_params(serde_json::json!({
                        "thinking": {
                            "type": "enabled",
                            "budget_tokens": level.anthropic_budget_tokens()
                        }
                    }));
                }
            }

            let agent = builder.build();
            BoxxyAgentInner::Ready(agent)
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

            BoxxyAgentInner::Ready(builder.build())
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
            BoxxyAgentInner::Ready(agent)
        }
        ModelProvider::DeepSeek(model) => {
            let key = creds.api_keys.get("DeepSeek").cloned().unwrap_or_default();
            let client = rig::providers::deepseek::Client::new(key.trim()).unwrap();
            let deepseek_model = client.completion_model(model.api_name());

            let agent = rig::agent::AgentBuilder::new(deepseek_model)
                .preamble(system_prompt)
                .build();
            BoxxyAgentInner::Ready(agent)
        }
    };

    BoxxyAgent {
        inner,
        preamble: system_prompt.to_string(),
    }
}
