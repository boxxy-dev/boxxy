//! Provider model factory mapping [`ModelProvider`] + [`AiCredentials`] into `rig::DynModel`.

use crate::credentials::AiCredentials;
use crate::reasoning::resolve_model_reasoning_with_endpoints;
pub use boxxy_model_selection::fetch_openai_compatible_models;
use boxxy_model_selection::{ModelProvider, OpenAiApiRoute, ThinkingLevel};
use rig::completion::Reasoning;

/// Type-erased completion model paired with its telemetry provider label, model identifier,
/// and resolved [`Reasoning`] option.
pub type BuiltProviderModel = (
    rig::DynModel<rig::operation::Completion>,
    String,
    String,
    Option<Reasoning>,
);

const DEFAULT_CUSTOM_BASE_URL: &str = "http://localhost:8000/v1";

fn finish_provider_model(
    dyn_model: rig::DynModel<rig::operation::Completion>,
    provider_label: &str,
    model_id: &str,
    provider: &ModelProvider,
    thinking: Option<ThinkingLevel>,
    creds: &AiCredentials,
) -> BuiltProviderModel {
    let reasoning = resolve_model_reasoning_with_endpoints(
        provider,
        thinking,
        &creds.openai_compatible_endpoints,
    );
    (
        dyn_model,
        provider_label.to_owned(),
        model_id.to_owned(),
        reasoning,
    )
}

fn build_opencode_gateway_model(
    provider_label: &str,
    catalog_vendor: &str,
    model_id: &str,
    is_go: bool,
    provider: &ModelProvider,
    thinking: Option<ThinkingLevel>,
    creds: &AiCredentials,
) -> BuiltProviderModel {
    let key = creds.resolve_api_key(provider_label);
    let reference = format!("{catalog_vendor}/{model_id}");
    let dyn_model = rig::catalog::Catalog::builtin()
        .connect_with(
            &reference,
            rig::providers::registry::ConnectOptions::new().api_key(key.as_ref()),
        )
        .unwrap_or_else(|_| {
            let client = if is_go {
                rig::providers::opencode::go_new(key.as_ref())
            } else {
                rig::providers::opencode::new(key.as_ref())
            };
            client.completion(model_id).into()
        });
    finish_provider_model(
        dyn_model,
        provider_label,
        model_id,
        provider,
        thinking,
        creds,
    )
}

fn build_custom_openai_model(
    provider: &ModelProvider,
    provider_name: &str,
    model_id: &str,
    thinking: Option<ThinkingLevel>,
    creds: &AiCredentials,
) -> BuiltProviderModel {
    let endpoint = creds.find_custom_endpoint(provider_name);
    let trimmed_model_id = model_id.trim();

    let base_url = endpoint
        .map(|e| e.base_url.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .unwrap_or(DEFAULT_CUSTOM_BASE_URL);

    let key = creds.resolve_custom_api_key(provider_name, endpoint);

    let route = endpoint
        .and_then(|e| e.find_model(trimmed_model_id))
        .map_or_else(
            || OpenAiApiRoute::infer_for_model(base_url, trimmed_model_id),
            |m| m.route,
        );

    let mut config =
        rig::providers::openai::OpenAIConfig::new(key.as_ref()).with_base_url(base_url);
    config.dialect.quirks.done_without_finish_reason = true;
    let client = config.client();

    let dyn_model: rig::DynModel<rig::operation::Completion> = match route {
        OpenAiApiRoute::ChatCompletions => client.chat(trimmed_model_id).into(),
        OpenAiApiRoute::Responses => client.responses(trimmed_model_id).into(),
    };

    finish_provider_model(
        dyn_model,
        "Custom Endpoints",
        trimmed_model_id,
        provider,
        thinking,
        creds,
    )
}

/// Resolves a [`ModelProvider`] and [`AiCredentials`] into a type-erased [`rig::DynModel`].
///
/// Returns `(dyn_model, telemetry_provider_label, api_model_name, resolved_reasoning)`.
#[must_use]
pub fn build_provider_model(provider: &ModelProvider, creds: &AiCredentials) -> BuiltProviderModel {
    match provider {
        ModelProvider::Gemini(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("Gemini");
            let model = rig::providers::gemini::Gemini::new(key.as_ref()).completion(id);
            finish_provider_model(model.into(), "Gemini", id, provider, *t, creds)
        }
        ModelProvider::Ollama(name) => {
            let mut cfg = rig::providers::ollama::OllamaConfig::new();
            if let Some(base_url) = creds.effective_ollama_url() {
                cfg = cfg.with_base_url(base_url);
            }
            let id = name.trim();
            let model = cfg.client().native_completion(id);
            (model.into(), "Ollama".to_owned(), id.to_owned(), None)
        }
        ModelProvider::Anthropic(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("Anthropic");
            let model = rig::providers::anthropic::Anthropic::new(key.as_ref()).completion(id);
            finish_provider_model(model.into(), "Anthropic", id, provider, *t, creds)
        }
        ModelProvider::OpenAi(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("OpenAI");
            let model = rig::providers::openai::OpenAI::new(key.as_ref()).completion(id);
            finish_provider_model(model.into(), "OpenAI", id, provider, *t, creds)
        }
        ModelProvider::OpenRouter(name) => {
            let id = name.trim();
            let key = creds.resolve_api_key("OpenRouter");
            let model = rig::providers::openrouter::new(key.as_ref()).chat(id);
            (model.into(), "OpenRouter".to_owned(), id.to_owned(), None)
        }
        ModelProvider::DeepSeek(m) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("DeepSeek");
            let model = rig::providers::deepseek::new(key.as_ref()).chat(id);
            (model.into(), "DeepSeek".to_owned(), id.to_owned(), None)
        }
        ModelProvider::Moonshot(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("Moonshot");
            let model = rig::providers::moonshot::new(key.as_ref()).chat(id);
            finish_provider_model(model.into(), "Moonshot", id, provider, *t, creds)
        }
        ModelProvider::Xai(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("xAI");
            let model = rig::providers::xai::new(key.as_ref()).completion(id);
            finish_provider_model(model.into(), "xAI", id, provider, *t, creds)
        }
        ModelProvider::Zai(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("Z.ai");
            let model = rig::providers::zai::new(key.as_ref()).chat(id);
            finish_provider_model(model.into(), "Z.ai", id, provider, *t, creds)
        }
        ModelProvider::Mistral(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("Mistral");
            let model = rig::providers::mistral::new(key.as_ref()).chat(id);
            finish_provider_model(model.into(), "Mistral", id, provider, *t, creds)
        }
        ModelProvider::MiniMax(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("MiniMax");
            let model = rig::providers::minimax::new(key.as_ref()).chat(id);
            finish_provider_model(model.into(), "MiniMax", id, provider, *t, creds)
        }
        ModelProvider::Groq(m, t) => {
            let id = m.api_name().trim();
            let key = creds.resolve_api_key("Groq");
            let model = rig::providers::groq::new(key.as_ref()).chat(id);
            finish_provider_model(model.into(), "Groq", id, provider, *t, creds)
        }
        ModelProvider::OpenCodeZen(m, t) => build_opencode_gateway_model(
            "OpenCode Zen",
            "opencode",
            m.api_name().trim(),
            false,
            provider,
            *t,
            creds,
        ),
        ModelProvider::OpenCodeGo(m, t) => build_opencode_gateway_model(
            "OpenCode Go",
            "opencode-go",
            m.api_name().trim(),
            true,
            provider,
            *t,
            creds,
        ),
        ModelProvider::OpenAiCompatible {
            provider_name,
            model_id,
            thinking,
        } => build_custom_openai_model(provider, provider_name, model_id, *thinking, creds),
    }
}
