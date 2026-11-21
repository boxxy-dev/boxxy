//! Catalog specification lookup, context/input limits, and 3-tier token cost accounting.

use boxxy_model_selection::{
    CustomModelConfig, ModelProvider, OpenAiCompatibleEndpoint, catalog_get_spec,
    resolve_any_catalog_spec,
};
use rig::catalog::{ModelSpec, Pricing};
use rig::completion::Usage;

/// Finds a custom endpoint's model configuration by trimmed provider name and model ID.
#[expect(
    clippy::redundant_pub_crate,
    reason = "workspace unreachable_pub = deny forbids pub items in private modules unless re-exported"
)]
pub(crate) fn find_custom_model<'a>(
    endpoints: &'a [OpenAiCompatibleEndpoint],
    provider_name: &str,
    model_id: &str,
) -> Option<(&'a OpenAiCompatibleEndpoint, Option<&'a CustomModelConfig>)> {
    let target = provider_name.trim();
    let ep = endpoints.iter().find(|e| e.name.trim() == target)?;
    let custom_model = ep.find_model(model_id);
    Some((ep, custom_model))
}

/// Resolves a [`ModelProvider`]'s [`ModelSpec`] from [`rig::catalog::Catalog::builtin`].
///
/// For `OpenRouter(slug)` (e.g. `"anthropic/claude-sonnet-5"`), this first checks the
/// `"openrouter"` catalog vendor and then falls back to resolving `slug` directly as a
/// `"vendor/model"` reference so custom `OpenRouter` slugs inherit the base vendor's
/// context window and pricing when not listed separately under `OpenRouter`.
#[must_use]
pub fn resolve_catalog_spec(provider: &ModelProvider) -> Option<&'static ModelSpec> {
    match provider {
        ModelProvider::Gemini(model, _) => catalog_get_spec("gcp.gemini", model.api_name().trim()),
        ModelProvider::Anthropic(model, _) => {
            catalog_get_spec("anthropic", model.api_name().trim())
        }
        ModelProvider::OpenAi(model, _) => catalog_get_spec("openai", model.api_name().trim()),
        ModelProvider::DeepSeek(model) => catalog_get_spec("deepseek", model.api_name().trim()),
        ModelProvider::Moonshot(model, _) => catalog_get_spec("moonshot", model.api_name().trim()),
        ModelProvider::Xai(model, _) => catalog_get_spec("xai", model.api_name().trim()),
        ModelProvider::Zai(model, _) => catalog_get_spec("zai", model.api_name().trim()),
        ModelProvider::Mistral(model, _) => catalog_get_spec("mistral", model.api_name().trim()),
        ModelProvider::MiniMax(model, _) => catalog_get_spec("minimax", model.api_name().trim()),
        ModelProvider::Groq(model, _) => catalog_get_spec("groq", model.api_name().trim()),
        ModelProvider::OpenCodeZen(model, _) => {
            let id = model.api_name().trim();
            catalog_get_spec("opencode", id).or_else(|| resolve_any_catalog_spec(id))
        }
        ModelProvider::OpenCodeGo(model, _) => {
            let id = model.api_name().trim();
            catalog_get_spec("opencode-go", id).or_else(|| resolve_any_catalog_spec(id))
        }
        ModelProvider::OpenRouter(slug) => resolve_any_catalog_spec(slug),
        ModelProvider::OpenAiCompatible { model_id, .. } => resolve_any_catalog_spec(model_id),
        ModelProvider::Ollama(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return None;
            }
            catalog_get_spec("ollama", trimmed)
        }
    }
}

/// Returns the model's maximum context window in tokens from [`rig::catalog::Catalog::builtin`], if known.
#[must_use]
pub fn model_context_window(provider: &Option<ModelProvider>) -> Option<u64> {
    model_context_window_with_endpoints(provider, &[])
}

/// Returns the model's maximum context window in tokens, checking custom endpoint overrides first.
///
/// Falls back to [`rig::catalog::Catalog::builtin`] when no custom override is configured.
#[must_use]
pub fn model_context_window_with_endpoints(
    provider: &Option<ModelProvider>,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Option<u64> {
    let p = provider.as_ref()?;
    if let ModelProvider::OpenAiCompatible {
        provider_name,
        model_id,
        ..
    } = p
        && let Some((_, Some(custom_model))) = find_custom_model(endpoints, provider_name, model_id)
        && let Some(ctx) = custom_model.context_window.filter(|&w| w > 0)
    {
        return Some(ctx);
    }

    resolve_catalog_spec(p)
        .and_then(|spec| spec.context_window)
        .map(u64::from)
}

/// Returns the model's maximum input token limit (`ModelSpec::input_limit`) from [`rig::catalog::Catalog::builtin`].
#[must_use]
pub fn model_input_limit(provider: &Option<ModelProvider>) -> Option<u64> {
    model_input_limit_with_endpoints(provider, &[])
}

/// Returns the model's maximum input token limit (`ModelSpec::input_limit`), checking custom overrides first.
///
/// Used by context compaction so models with dedicated output reserves (e.g. GPT-6.1's 922k input
/// limit inside its 1.05M context window) compact before exceeding `limit.input`.
#[must_use]
pub fn model_input_limit_with_endpoints(
    provider: &Option<ModelProvider>,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Option<u64> {
    let p = provider.as_ref()?;
    if let ModelProvider::OpenAiCompatible {
        provider_name,
        model_id,
        ..
    } = p
        && let Some((_, Some(custom_model))) = find_custom_model(endpoints, provider_name, model_id)
        && let Some(ctx) = custom_model.context_window.filter(|&w| w > 0)
    {
        return Some(ctx);
    }

    resolve_catalog_spec(p)
        .and_then(ModelSpec::input_limit)
        .map(u64::from)
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "workspace unreachable_pub = deny forbids pub items in private modules unless re-exported"
)]
pub(crate) fn model_output_limit_for_provider(
    p: &ModelProvider,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Option<u64> {
    let catalog_limit = resolve_catalog_spec(p)
        .and_then(ModelSpec::output_limit)
        .map(u64::from);

    if let ModelProvider::OpenAiCompatible {
        provider_name,
        model_id,
        ..
    } = p
        && let Some((_, Some(custom_model))) = find_custom_model(endpoints, provider_name, model_id)
        && let Some(ctx) = custom_model.context_window.filter(|&w| w > 0)
    {
        return catalog_limit.map(|out| out.min(ctx));
    }

    catalog_limit
}

/// Returns the model's maximum output token limit (`ModelSpec::output_limit`) from [`rig::catalog::Catalog::builtin`].
#[must_use]
pub fn model_output_limit(provider: &Option<ModelProvider>) -> Option<u64> {
    model_output_limit_with_endpoints(provider, &[])
}

/// Returns the model's maximum output token limit (`ModelSpec::output_limit`), clamped to any
/// smaller custom endpoint `context_window` override.
#[must_use]
pub fn model_output_limit_with_endpoints(
    provider: &Option<ModelProvider>,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Option<u64> {
    provider
        .as_ref()
        .and_then(|p| model_output_limit_for_provider(p, endpoints))
}

/// Populates `usage.cost` when the model's pricing is known in [`rig::catalog::Catalog::builtin`].
///
/// Local `Ollama` and flat-rate subscription models are never priced per token.
#[must_use]
pub fn enrich_usage_with_catalog_cost(usage: Usage, provider: &Option<ModelProvider>) -> Usage {
    enrich_usage_with_endpoints_cost(usage, provider, &[])
}

/// Populates `usage.cost` across custom overrides, wire reports, and catalog fallback.
///
/// Resolution order:
/// 1. Custom `OpenAiCompatibleEndpoint` model pricing overrides (or `$0.00` when `endpoint.zero_cost` is true),
/// 2. Valid provider-reported `usage.cost` on the wire,
/// 3. Fallback pricing from [`rig::catalog::Catalog::builtin`].
#[must_use]
pub fn enrich_usage_with_endpoints_cost(
    mut usage: Usage,
    provider: &Option<ModelProvider>,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Usage {
    let Some(p) = provider.as_ref() else {
        return usage;
    };

    // Local Ollama and flat-rate subscription OpenCode Go models never incur per-token USD charges,
    // even if an upstream gateway wire includes a synthetic cost field.
    if matches!(
        p,
        ModelProvider::Ollama(_) | ModelProvider::OpenCodeGo(_, _)
    ) {
        usage.cost = None;
        return usage;
    }

    if let ModelProvider::OpenAiCompatible {
        provider_name,
        model_id,
        ..
    } = p
        && let Some((ep, custom_model)) = find_custom_model(endpoints, provider_name, model_id)
    {
        if let Some(cm) = custom_model {
            let input_rate = cm.input_cost_per_1m.filter(|c| c.is_finite() && *c >= 0.0);
            let output_rate = cm.output_cost_per_1m.filter(|c| c.is_finite() && *c >= 0.0);
            if input_rate.is_some() || output_rate.is_some() {
                let pricing = Pricing::new(input_rate.unwrap_or(0.0), output_rate.unwrap_or(0.0));
                usage.cost = pricing
                    .cost(&usage)
                    .filter(|c| c.total.is_finite() && c.total >= 0.0);
                return usage;
            }
        }
        if ep.zero_cost {
            usage.cost = None;
            return usage;
        }
    }

    // Discard malformed NaN/negative wire costs so catalog fallback can compute a valid total.
    if usage
        .cost
        .is_some_and(|c| !c.total.is_finite() || c.total < 0.0)
    {
        usage.cost = None;
    }

    if usage.cost.is_some() {
        return usage;
    }

    if let Some(spec) = resolve_catalog_spec(p)
        && let Some(pricing) = &spec.pricing
        && let Some(cost) = pricing
            .cost(&usage)
            .filter(|c| c.total.is_finite() && c.total >= 0.0)
    {
        usage.cost = Some(cost);
    }
    usage
}

const fn usage_total_tokens(u: &Usage) -> u64 {
    if let Some(total) = u.total_tokens {
        total
    } else {
        let input = if let Some(i) = u.input_tokens { i } else { 0 };
        let output = if let Some(o) = u.output_tokens { o } else { 0 };
        input.saturating_add(output)
    }
}

/// Extracts the context window token count from the latest non-zero completion request of `res`.
///
/// Falls back to `res.usage` only for single-call responses so multi-step cumulative totals
/// never masquerade as single-window occupancy.
#[must_use]
pub fn extract_response_context_tokens(res: &rig::agent::PromptResponse) -> u64 {
    for call in res.completion_calls.iter().rev() {
        let total = usage_total_tokens(&call.usage);
        if total > 0 {
            return total;
        }
    }

    if res.completion_calls.len() <= 1 {
        usage_total_tokens(&res.usage)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_model_selection::OpenAiApiRoute;
    use rig::completion::Cost;

    #[test]
    fn ignores_nan_or_negative_custom_rates_and_sanitizes_wire_cost() {
        let mut ep = OpenAiCompatibleEndpoint::preset_local_server();
        ep.name = "TestProxy".to_string();
        ep.zero_cost = false;
        ep.models.push(CustomModelConfig {
            id: "anthropic/claude-sonnet-5".to_string(),
            display_name: String::new(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: None,
            thinking_levels: Vec::new(),
            input_cost_per_1m: Some(f64::NAN),
            output_cost_per_1m: Some(-5.0),
        });

        let prov = Some(ModelProvider::OpenAiCompatible {
            provider_name: "TestProxy".to_string(),
            model_id: "anthropic/claude-sonnet-5".to_string(),
            thinking: None,
        });

        let mut usage = Usage::new().input_tokens(100_000).output_tokens(10_000);
        usage.cost = Some(Cost::from_total(f64::NAN));

        // Invalid custom rates and NaN wire cost are both discarded, falling back cleanly to catalog ($0.30).
        let enriched = enrich_usage_with_endpoints_cost(usage, &prov, &[ep]);
        let total = enriched.cost.expect("catalog fallback cost").total;
        assert!((total - 0.30).abs() < 1e-6);
    }

    #[test]
    fn extract_context_tokens_uses_latest_nonzero_call_without_summing_turns() {
        let call_step_1 = serde_json::json!({
            "call_index": 0,
            "usage": { "input_tokens": 50_000, "output_tokens": 500, "total_tokens": 50_500 },
            "raw": null
        });
        let call_step_2_empty = serde_json::json!({
            "call_index": 1,
            "usage": { "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 },
            "raw": null
        });

        let multi_step: rig::agent::PromptResponse = serde_json::from_value(serde_json::json!({
            "content": [{ "type": "text", "text": "Done" }],
            "usage": { "input_tokens": 110_000, "output_tokens": 1_000, "total_tokens": 111_000 },
            "completion_calls": [call_step_1, call_step_2_empty],
            "messages": []
        }))
        .expect("valid PromptResponse fixture");

        // Picks step 1's 50,500 window rather than step 2's 0 or the cumulative 111,000 total.
        assert_eq!(extract_response_context_tokens(&multi_step), 50_500);

        // When all multi-step calls report 0, does not fall back to the cumulative 111,000 sum.
        let all_zero_multi: rig::agent::PromptResponse = serde_json::from_value(serde_json::json!({
            "content": [{ "type": "text", "text": "Done" }],
            "usage": { "input_tokens": 110_000, "output_tokens": 1_000, "total_tokens": 111_000 },
            "completion_calls": [call_step_2_empty.clone(), call_step_2_empty],
            "messages": []
        }))
        .expect("valid PromptResponse fixture");
        assert_eq!(extract_response_context_tokens(&all_zero_multi), 0);
    }
}
