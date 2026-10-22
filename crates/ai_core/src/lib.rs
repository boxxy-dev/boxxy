use boxxy_model_selection::{ModelProvider, ThinkingLevel};
use rig::agent::hook::{
    AgentHook, CompletionCallAction, CompletionCallEvent, DispatchAction, DispatchEvent,
    HookContext,
};
use rig::completion::{CacheRetention, Effort, OnUnsupported, Reasoning};
use rig::message::Message;
use rig::wasm_compat::WasmCompatSend;
use std::future::Future;

pub fn map_thinking_level(level: ThinkingLevel) -> Reasoning {
    match level {
        ThinkingLevel::None => Reasoning::Off,
        ThinkingLevel::Minimal => Reasoning::Effort(Effort::Minimal),
        ThinkingLevel::Low => Reasoning::Effort(Effort::Low),
        ThinkingLevel::Medium => Reasoning::Effort(Effort::Medium),
        ThinkingLevel::High => Reasoning::Effort(Effort::High),
        ThinkingLevel::XHigh => Reasoning::Effort(Effort::XHigh),
    }
}

/// Returns the provider-specific `additional_params` JSON payload to enable
/// built-in server-side web search in `rig-core 0.44.0`, or `None` if the
/// provider does not support native search.
///
/// Each supported wire encoder in `rig-core 0.44.0` merges `additional_params.tools`
/// alongside standard function tools rather than overwriting them.
pub fn native_search_additional_params(provider: &ModelProvider) -> Option<serde_json::Value> {
    match provider {
        ModelProvider::Gemini(_, _) => Some(serde_json::json!({
            "tools": [{ "googleSearch": {} }],
            "toolConfig": {
                "includeServerSideToolInvocations": true
            }
        })),
        ModelProvider::OpenAi(_, _) => Some(serde_json::json!({
            "tools": [{ "type": "web_search" }]
        })),
        ModelProvider::Anthropic(_, _) => Some(serde_json::json!({
            "tools": [{ "type": "web_search_20250305", "name": "web_search" }]
        })),
        ModelProvider::Ollama(_) | ModelProvider::OpenRouter(_) | ModelProvider::DeepSeek(_) => {
            None
        }
    }
}

/// Collects unique `(title, url)` pairs from `Text::citations()` across assistant turns in `messages`.
pub fn collect_url_citations(messages: &[Message]) -> Vec<(Option<String>, String)> {
    let mut seen_urls = std::collections::HashSet::new();
    let mut sources = Vec::new();

    for msg in messages {
        if let Message::Assistant(turn) = msg {
            for item in turn.content.iter() {
                if let rig::message::AssistantContent::Text(text) = item {
                    for citation in text.citations() {
                        for source in &citation.sources {
                            if let rig::message::SourceLocation::Url { url } = &source.location
                                && !url.is_empty()
                                && seen_urls.insert(url.clone())
                            {
                                sources.push((source.title.clone(), url.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    sources
}

/// Appends a Markdown `**Sources:**` footer to `output` for any URL citations in `messages`
/// whose URLs are not already linked inline in `output`.
pub fn append_citations_to_output(messages: &[Message], output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed == "[SILENT_ACK]" {
        return output.to_string();
    }

    let missing_sources: Vec<(Option<String>, String)> = collect_url_citations(messages)
        .into_iter()
        .filter(|(_, url)| !output.contains(url.as_str()))
        .collect();

    if missing_sources.is_empty() {
        return output.to_string();
    }

    let mut result = output.to_string();
    result.push_str("\n\n**Sources:**\n");
    for (title, url) in missing_sources {
        let label = title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(url.as_str());
        result.push_str(&format!("- [{label}]({url})\n"));
    }
    result.trim_end().to_string()
}

/// Inspects the committed `messages` from a turn for server-side web search items
/// (`web_search_call` on OpenAI, `server_tool_use` / `web_search_tool_result` on Anthropic,
/// or URL grounding citations on Gemini/OpenAI/Anthropic) and returns a human-readable
/// summary when native web search was used.
pub fn extract_native_search_summary(messages: &[Message]) -> Option<String> {
    let mut queries = Vec::new();
    let mut search_invoked = false;

    for msg in messages {
        if let Message::Assistant(turn) = msg {
            for item in turn.content.iter() {
                if let rig::message::AssistantContent::Opaque(opaque) = item {
                    match opaque.kind() {
                        Some("web_search_call") => {
                            search_invoked = true;
                            if let Some(q) = opaque
                                .item
                                .get("action")
                                .and_then(|a| a.get("query"))
                                .and_then(|q| q.as_str())
                                && !q.is_empty()
                                && !queries.contains(&q.to_string())
                            {
                                queries.push(q.to_string());
                            }
                        }
                        Some("server_tool_use")
                            if opaque.item.get("name").and_then(|n| n.as_str())
                                == Some("web_search") =>
                        {
                            search_invoked = true;
                            if let Some(q) = opaque
                                .item
                                .get("input")
                                .and_then(|i| i.get("query"))
                                .and_then(|q| q.as_str())
                                && !q.is_empty()
                                && !queries.contains(&q.to_string())
                            {
                                queries.push(q.to_string());
                            }
                        }
                        Some("web_search_tool_result") => {
                            search_invoked = true;
                        }
                        _ => {
                            // Gemini server-side tool invocation part (`toolCall` / `toolResponse`)
                            if let Some(tool_call) = opaque.item.get("toolCall") {
                                search_invoked = true;
                                if let Some(args) = tool_call.get("args") {
                                    if let Some(q) = args.get("query").and_then(|q| q.as_str())
                                        && !q.is_empty()
                                        && !queries.contains(&q.to_string())
                                    {
                                        queries.push(q.to_string());
                                    }
                                    if let Some(arr) =
                                        args.get("queries").and_then(|q| q.as_array())
                                    {
                                        for q in arr.iter().filter_map(|v| v.as_str()) {
                                            if !q.is_empty() && !queries.contains(&q.to_string()) {
                                                queries.push(q.to_string());
                                            }
                                        }
                                    }
                                }
                            } else if opaque.item.get("toolResponse").is_some() {
                                search_invoked = true;
                            }
                        }
                    }
                }
            }
        }
    }

    let citations = collect_url_citations(messages);
    if !search_invoked && citations.is_empty() {
        return None;
    }

    let mut summary = String::new();
    if !queries.is_empty() {
        summary.push_str(&format!("Queries: {}\n", queries.join(", ")));
    }
    if !citations.is_empty() {
        summary.push_str("Sources:\n");
        for (title, url) in &citations {
            let label = title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or(url.as_str());
            summary.push_str(&format!("- {label}: {url}\n"));
        }
    }
    if summary.is_empty() {
        summary.push_str("Executed provider-native web search.");
    }

    Some(summary.trim_end().to_string())
}

/// Resolves a [`ModelProvider`]'s [`rig::catalog::ModelSpec`] from [`rig::catalog::Catalog::builtin`].
///
/// For `OpenRouter(slug)` (e.g. `"anthropic/claude-sonnet-5"`), this first checks the
/// `"openrouter"` catalog vendor and then falls back to resolving `slug` directly as a
/// `"vendor/model"` reference so custom OpenRouter slugs inherit the base vendor's
/// context window and pricing when not listed separately under OpenRouter.
#[must_use]
pub fn resolve_catalog_spec(provider: &ModelProvider) -> Option<&'static rig::catalog::ModelSpec> {
    let catalog = rig::catalog::Catalog::builtin();
    match provider {
        ModelProvider::Gemini(model, _) => catalog
            .resolve(&format!("gcp.gemini/{}", model.api_name()))
            .ok()
            .map(|r| r.spec),
        ModelProvider::Anthropic(model, _) => catalog
            .resolve(&format!("anthropic/{}", model.api_name()))
            .ok()
            .map(|r| r.spec),
        ModelProvider::OpenAi(model, _) => catalog
            .resolve(&format!("openai/{}", model.api_name()))
            .ok()
            .map(|r| r.spec),
        ModelProvider::DeepSeek(model) => catalog
            .resolve(&format!("deepseek/{}", model.api_name()))
            .ok()
            .map(|r| r.spec),
        ModelProvider::OpenRouter(slug) => {
            let trimmed = slug.trim();
            if trimmed.is_empty() {
                return None;
            }
            catalog
                .resolve(&format!("openrouter/{trimmed}"))
                .or_else(|_| catalog.resolve(trimmed))
                .ok()
                .map(|r| r.spec)
        }
        ModelProvider::Ollama(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return None;
            }
            catalog
                .resolve(&format!("ollama/{trimmed}"))
                .ok()
                .map(|r| r.spec)
        }
    }
}

/// Returns the model's maximum context window in tokens from [`rig::catalog::Catalog::builtin`], if known.
#[must_use]
pub fn model_context_window(provider: &Option<ModelProvider>) -> Option<u64> {
    let p = provider.as_ref()?;
    resolve_catalog_spec(p)
        .and_then(|spec| spec.context_window)
        .map(u64::from)
}

/// Ensures `usage.cost` is populated when the model's pricing is known in [`rig::catalog::Catalog::builtin`]
/// (e.g., for OpenRouter models resolved via base-vendor fallback). Local `Ollama` models are never priced.
pub fn enrich_usage_with_catalog_cost(
    mut usage: rig::completion::Usage,
    provider: &Option<ModelProvider>,
) -> rig::completion::Usage {
    if usage.cost.is_some() {
        return usage;
    }
    let Some(p) = provider.as_ref() else {
        return usage;
    };
    if matches!(p, ModelProvider::Ollama(_)) {
        return usage;
    }
    if let Some(spec) = resolve_catalog_spec(p)
        && let Some(pricing) = &spec.pricing
        && let Some(cost) = pricing.cost(&usage)
    {
        usage.cost = Some(cost);
    }
    usage
}

/// Extracts the context window token count from the final completion request of a `PromptResponse`,
/// falling back to the aggregated `res.usage` when `completion_calls` is empty.
#[must_use]
pub fn extract_response_context_tokens(res: &rig::agent::PromptResponse) -> u64 {
    if let Some(last_call) = res.completion_calls.last() {
        let u = &last_call.usage;
        let total = u
            .total_tokens
            .unwrap_or_else(|| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0));
        if total > 0 {
            return total;
        }
    }
    let u = &res.usage;
    u.total_tokens
        .unwrap_or_else(|| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
}

fn resolve_model_reasoning(
    provider: &ModelProvider,
    thinking: Option<ThinkingLevel>,
) -> Option<Reasoning> {
    let level = thinking?;
    let spec = resolve_catalog_spec(provider);

    if level == ThinkingLevel::None {
        if spec.is_none_or(|s| s.reasoning.can_disable().unwrap_or(true)) {
            return Some(Reasoning::Off);
        }
        return None;
    }

    if let Some(s) = spec {
        if !s.reasoning.supported() {
            return None;
        }
        if s.reasoning.levels().is_none_or(<[_]>::is_empty)
            && let Some(range) = s.reasoning.budget()
        {
            let tokens = level
                .anthropic_budget_tokens()
                .clamp(*range.start(), *range.end());
            return Some(Reasoning::Budget { tokens });
        }
    }

    Some(map_thinking_level(level))
}

pub fn build_provider_model(
    provider: &ModelProvider,
    creds: &AiCredentials,
) -> (
    rig::DynModel<rig::operation::Completion>,
    String,
    String,
    Option<Reasoning>,
) {
    match provider {
        ModelProvider::Gemini(model, thinking) => {
            let key = creds.api_keys.get("Gemini").cloned().unwrap_or_default();
            let client = rig::providers::gemini::Gemini::new(key.trim());
            let gemini_model = client.completion(model.api_name());
            let reasoning = resolve_model_reasoning(provider, *thinking);
            (
                gemini_model.into(),
                "Gemini".to_string(),
                model.api_name().to_string(),
                reasoning,
            )
        }
        ModelProvider::Ollama(model_name) => {
            let client = rig::providers::ollama::OllamaConfig::new()
                .with_base_url(creds.ollama_url.as_str())
                .client();
            let ollama_model = client.native_completion(model_name.as_str());
            (
                ollama_model.into(),
                "Ollama".to_string(),
                model_name.clone(),
                None,
            )
        }
        ModelProvider::Anthropic(model, thinking) => {
            let key = creds.api_keys.get("Anthropic").cloned().unwrap_or_default();
            let client = rig::providers::anthropic::Anthropic::new(key.trim());
            let anthropic_model = client.completion(model.api_name());
            let reasoning = resolve_model_reasoning(provider, *thinking);
            (
                anthropic_model.into(),
                "Anthropic".to_string(),
                model.api_name().to_string(),
                reasoning,
            )
        }
        ModelProvider::OpenAi(model, thinking) => {
            let key = creds.api_keys.get("OpenAI").cloned().unwrap_or_default();
            let client = rig::providers::openai::OpenAI::new(key.trim());
            let openai_model = client.completion(model.api_name());
            let reasoning = resolve_model_reasoning(provider, *thinking);
            (
                openai_model.into(),
                "OpenAI".to_string(),
                model.api_name().to_string(),
                reasoning,
            )
        }
        ModelProvider::OpenRouter(model_name) => {
            let key = creds
                .api_keys
                .get("OpenRouter")
                .cloned()
                .unwrap_or_default();
            let client = rig::providers::openrouter::new(key.trim());
            let openrouter_model = client.chat(model_name.as_str());
            (
                openrouter_model.into(),
                "OpenRouter".to_string(),
                model_name.clone(),
                None,
            )
        }
        ModelProvider::DeepSeek(model) => {
            let key = creds.api_keys.get("DeepSeek").cloned().unwrap_or_default();
            let client = rig::providers::deepseek::new(key.trim());
            let deepseek_model = client.chat(model.api_name());
            (
                deepseek_model.into(),
                "DeepSeek".to_string(),
                model.api_name().to_string(),
                None,
            )
        }
    }
}

#[derive(Clone)]
pub struct ModelContextHook {
    pub preamble: String,
}

impl AgentHook for ModelContextHook {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        event: CompletionCallEvent<'_>,
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

    fn on_dispatch(
        &self,
        _ctx: &HookContext,
        event: DispatchEvent<'_>,
    ) -> impl Future<Output = DispatchAction> + WasmCompatSend {
        let tool_call = event
            .tool_name()
            .zip(event.tool_args())
            .map(|(n, a)| (n.to_string(), a.to_string()));

        let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
            .map(|v| v == "1")
            .unwrap_or(false);

        async move {
            if is_explicit && let Some((tool_name, args)) = tool_call {
                log::info!(
                    target: "model-context",
                    "\n=== MODEL TOOL CALL ===\nTOOL: {}\nARGS: {}\n=======================\n",
                    tool_name,
                    args
                );
            }
            DispatchAction::Proceed
        }
    }
}

#[derive(Clone)]
pub struct BoxxyAgent {
    inner: BoxxyAgentInner,
    preamble: String,
    provider: Option<ModelProvider>,
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
        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let msg = prompt.into();

        let agent = match &self.inner {
            BoxxyAgentInner::Ready(agent) => agent,
            BoxxyAgentInner::Error(e) => {
                return Err(rig::completion::PromptError::Provider(
                    rig::error::ProviderError::Provider(e.clone()),
                ));
            }
        };

        let res_result = agent
            .prompt(msg)
            .history(history)
            .add_hook(hook)
            .await;

        let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
            .map(|v| v == "1")
            .unwrap_or(false);

        match res_result {
            Ok(res) => {
                let raw_output = res.output();
                let output = append_citations_to_output(&res.messages, &raw_output);
                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL RESPONSE ===\n{}\n======================\n",
                        output
                    );
                }
                let usage = enrich_usage_with_catalog_cost(res.usage, &self.provider);
                Ok((output, Some(usage), Some(res.messages)))
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
        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let agent = match &self.inner {
            BoxxyAgentInner::Ready(agent) => agent,
            BoxxyAgentInner::Error(e) => {
                return Err(rig::completion::PromptError::Provider(
                    rig::error::ProviderError::Provider(e.clone()),
                ));
            }
        };

        let res_result = agent
            .prompt(prompt)
            .add_hook(hook)
            .await;

        let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
            .map(|v| v == "1")
            .unwrap_or(false);

        match res_result {
            Ok(res) => {
                let output = res.output();
                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL RESPONSE ===\n{}\n======================\n",
                        output
                    );
                }
                let usage = enrich_usage_with_catalog_cost(res.usage, &self.provider);
                Ok((output, Some(usage), Some(res.messages)))
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
    create_agent_with_options(provider, creds, system_prompt, false)
}

pub fn create_agent_with_options(
    provider: &Option<ModelProvider>,
    creds: &AiCredentials,
    system_prompt: &str,
    enable_native_search: bool,
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
                provider: None,
            }
        }
    };

    let (model, _provider_name, _model_name, reasoning) = build_provider_model(provider, creds);

    let mut builder = rig::agent::AgentBuilder::new(model)
        .preamble(system_prompt)
        .cache(CacheRetention::Short)
        .on_unsupported(OnUnsupported::Ignore);

    if let Some(reasoning) = reasoning {
        builder = builder.reasoning(reasoning);
    }

    if enable_native_search
        && let Some(params) = native_search_additional_params(provider)
    {
        builder = builder.additional_params(params);
    }

    BoxxyAgent {
        inner: BoxxyAgentInner::Ready(builder.build()),
        preamble: system_prompt.to_string(),
        provider: Some(provider.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_model_selection::{AnthropicModel, DeepSeekModel, GeminiModel, OpenAiModel};

    #[test]
    fn test_map_thinking_level() {
        assert_eq!(map_thinking_level(ThinkingLevel::None), Reasoning::Off);
        assert_eq!(
            map_thinking_level(ThinkingLevel::Minimal),
            Reasoning::Effort(Effort::Minimal)
        );
        assert_eq!(
            map_thinking_level(ThinkingLevel::Low),
            Reasoning::Effort(Effort::Low)
        );
        assert_eq!(
            map_thinking_level(ThinkingLevel::Medium),
            Reasoning::Effort(Effort::Medium)
        );
        assert_eq!(
            map_thinking_level(ThinkingLevel::High),
            Reasoning::Effort(Effort::High)
        );
        assert_eq!(
            map_thinking_level(ThinkingLevel::XHigh),
            Reasoning::Effort(Effort::XHigh)
        );
    }

    #[test]
    fn test_build_provider_model_reasoning() {
        let creds = AiCredentials::default();

        // Gemini with Low thinking -> Effort(Low)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Gemini(GeminiModel::Flash3_8, Some(ThinkingLevel::Low)),
            &creds,
        );
        assert_eq!(provider, "Gemini");
        assert_eq!(model, "gemini-3.8-flash");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::Low)));

        // Gemini with None thinking -> None (Gemini 3 cannot disable thinking)
        let (_, _, _, reasoning_none) = build_provider_model(
            &ModelProvider::Gemini(GeminiModel::Flash3_8, Some(ThinkingLevel::None)),
            &creds,
        );
        assert_eq!(reasoning_none, None);

        // Anthropic Claude 5.5 with Medium -> adaptive Effort(Medium)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Anthropic(AnthropicModel::ClaudeSonnet5, Some(ThinkingLevel::Medium)),
            &creds,
        );
        assert_eq!(provider, "Anthropic");
        assert_eq!(model, "claude-sonnet-5-5");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::Medium)));

        // OpenAI with High -> Effort(High)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, Some(ThinkingLevel::High)),
            &creds,
        );
        assert_eq!(provider, "OpenAI");
        assert_eq!(model, "gpt-6-astra");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::High)));

        // DeepSeek
        let (_, provider, model, reasoning) =
            build_provider_model(&ModelProvider::DeepSeek(DeepSeekModel::V4_1Flash), &creds);
        assert_eq!(provider, "DeepSeek");
        assert_eq!(model, "deepseek-flash");
        assert_eq!(reasoning, None);
    }

    #[test]
    fn test_native_search_additional_params() {
        assert_eq!(
            native_search_additional_params(&ModelProvider::Gemini(GeminiModel::Flash3_8, None)),
            Some(serde_json::json!({
                "tools": [{ "googleSearch": {} }],
                "toolConfig": {
                    "includeServerSideToolInvocations": true
                }
            }))
        );
        assert_eq!(
            native_search_additional_params(&ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, None)),
            Some(serde_json::json!({ "tools": [{ "type": "web_search" }] }))
        );
        assert_eq!(
            native_search_additional_params(&ModelProvider::Anthropic(
                AnthropicModel::ClaudeSonnet5,
                None
            )),
            Some(serde_json::json!({
                "tools": [{ "type": "web_search_20250305", "name": "web_search" }]
            }))
        );
        assert_eq!(
            native_search_additional_params(&ModelProvider::Ollama("llama3.2".to_string())),
            None
        );
        assert_eq!(
            native_search_additional_params(&ModelProvider::OpenRouter(
                "anthropic/claude-sonnet-4".to_string()
            )),
            None
        );
        assert_eq!(
            native_search_additional_params(&ModelProvider::DeepSeek(DeepSeekModel::V4_1Flash)),
            None
        );
    }

    #[test]
    fn test_citations_and_native_search_summary() {
        use rig::message::{
            AssistantContent, AssistantMessage, Citation, Opaque, Source, SourceLocation, Text,
        };

        let text = Text::new("Rust 2024 is supported.").with_citations([Citation::new([
            Source::new(SourceLocation::Url {
                url: "https://example.com/rust-2024".to_string(),
            })
            .title("Rust 2024 Edition Guide"),
        ])]);

        let search_opaque = Opaque {
            item: serde_json::json!({
                "type": "web_search_call",
                "id": "ws_1",
                "status": "completed",
                "action": { "type": "search", "query": "rust 2024 edition" }
            }),
            replay: true,
        };

        let turn = AssistantMessage::new(vec![
            AssistantContent::Opaque(search_opaque),
            AssistantContent::Text(text),
        ]);
        let messages = vec![Message::user("What edition?"), Message::Assistant(turn)];

        let formatted = append_citations_to_output(&messages, "Rust 2024 is supported.");
        assert!(formatted.contains("**Sources:**"));
        assert!(
            formatted.contains("- [Rust 2024 Edition Guide](https://example.com/rust-2024)")
        );

        // If URL is already inline in the output, don't duplicate it
        let inline =
            "Rust 2024 is supported ([guide](https://example.com/rust-2024)).";
        assert_eq!(append_citations_to_output(&messages, inline), inline);

        let summary = extract_native_search_summary(&messages).expect("should detect search");
        assert!(summary.contains("Queries: rust 2024 edition"));
        assert!(summary.contains("https://example.com/rust-2024"));
    }

    #[test]
    fn test_catalog_resolution_and_cost() {
        let gemini = Some(ModelProvider::Gemini(GeminiModel::Flash3_8, None));
        assert_eq!(model_context_window(&gemini), Some(1_048_576));

        let anthropic = Some(ModelProvider::Anthropic(AnthropicModel::ClaudeSonnet5, None));
        assert_eq!(model_context_window(&anthropic), Some(1_000_000));

        let openai = Some(ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, None));
        assert_eq!(model_context_window(&openai), Some(1_050_000));

        let deepseek = Some(ModelProvider::DeepSeek(DeepSeekModel::V4_1Flash));
        assert_eq!(model_context_window(&deepseek), Some(1_000_000));

        // OpenRouter with a base vendor/model slug resolves via fallback
        let openrouter = Some(ModelProvider::OpenRouter(
            "anthropic/claude-sonnet-5".to_string(),
        ));
        assert_eq!(model_context_window(&openrouter), Some(1_000_000));

        let usage = rig::completion::Usage::new()
            .input_tokens(100_000)
            .output_tokens(10_000);
        let priced = enrich_usage_with_catalog_cost(usage, &openrouter);
        assert!(priced.cost.is_some());
        // Claude Sonnet 5: $2/1M input ($0.20) + $10/1M output ($0.10) = $0.30
        assert!((priced.cost.unwrap().total - 0.30).abs() < 1e-6);

        // Local Ollama models are never priced
        let ollama = Some(ModelProvider::Ollama("llama3.2:latest".to_string()));
        let ollama_priced = enrich_usage_with_catalog_cost(usage, &ollama);
        assert!(ollama_priced.cost.is_none());
    }
}
