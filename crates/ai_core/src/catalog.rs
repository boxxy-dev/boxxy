//! Catalog specification lookup, context/input limits, and 3-tier token cost accounting.

use boxxy_model_selection::{
    CustomModelConfig, ModelProvider, OpenAiCompatibleEndpoint, catalog_get_spec,
    resolve_any_catalog_spec,
};
use rig::catalog::{ModelSpec, Pricing, Rates};
use rig::completion::{Cost, Usage};

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
        ModelProvider::ChatGpt(model, _) => {
            let id = model.api_name().trim();
            catalog_get_spec("chatgpt", id).or_else(|| resolve_any_catalog_spec(id))
        }
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

fn valid_rate(rate: Option<f64>) -> Option<f64> {
    rate.filter(|r| r.is_finite() && *r >= 0.0)
}

/// Finds the highest-threshold context tier in `spec.details.cost` exceeded by `input_tokens`,
/// matching `OpenCode`'s `SessionUsage.calculateCost` (`context > tier.size`, descending by `size`).
fn matching_context_tier(spec: &ModelSpec, input_tokens: u64) -> Option<Rates> {
    if let Some(best) = spec
        .details
        .cost
        .tiers
        .iter()
        .filter(|t| t.tier.kind == "context" && input_tokens > t.tier.size)
        .max_by_key(|t| t.tier.size)
    {
        return Some(best.rates);
    }

    if spec.details.cost.tiers.is_empty() && input_tokens > 200_000 {
        return spec.details.cost.context_over_200k;
    }

    None
}

/// Calculates USD cost for a single completion `Usage` from `spec`, applying long-context
/// `tiers` (`> 100k` / `> 200k` / `> 272k`), `context_over_200k`, and separate `reasoning`
/// rates when present in `spec.details.cost`.
#[expect(
    clippy::cast_precision_loss,
    reason = "LLM token counters stay well within f64's exact 53-bit integer mantissa"
)]
fn calculate_catalog_cost(spec: &ModelSpec, usage: &Usage) -> Option<Cost> {
    let base = spec.pricing.as_ref()?;
    let input_tokens = usage.input_tokens?;
    let output_tokens = usage.output_tokens?;

    let tier = matching_context_tier(spec, input_tokens);
    let input_rate = tier
        .and_then(|t| valid_rate(t.input))
        .or_else(|| valid_rate(Some(base.input)))?;
    let output_rate = tier
        .and_then(|t| valid_rate(t.output))
        .or_else(|| valid_rate(Some(base.output)))?;
    let cache_read_rate = tier
        .and_then(|t| valid_rate(t.cache_read))
        .or_else(|| valid_rate(base.cache_read));
    let cache_write_rate = tier
        .and_then(|t| valid_rate(t.cache_write))
        .or_else(|| valid_rate(base.cache_write));
    let reasoning_rate = tier
        .and_then(|t| valid_rate(t.reasoning))
        .or_else(|| valid_rate(spec.details.cost.reasoning));

    let mut effective_pricing = Pricing::new(input_rate, output_rate);
    if let Some(cr) = cache_read_rate {
        effective_pricing = effective_pricing.with_cache_read(cr);
    }
    if let Some(cw) = cache_write_rate {
        effective_pricing = effective_pricing.with_cache_write(cw);
    }

    let reasoning_tokens = usage.reasoning_tokens.unwrap_or(0).min(output_tokens);
    let Some(r_rate) = reasoning_rate.filter(|_| reasoning_tokens > 0) else {
        return effective_pricing
            .cost(usage)
            .filter(|c| c.total.is_finite() && c.total >= 0.0);
    };

    // Split output into visible vs reasoning tokens when the catalog lists a distinct reasoning rate.
    let visible_output = output_tokens.saturating_sub(reasoning_tokens);
    let mut visible_only = *usage;
    visible_only.output_tokens = Some(visible_output);
    let mut base_cost = effective_pricing.cost(&visible_only)?;
    let reasoning_usd = (reasoning_tokens as f64) * r_rate / 1_000_000.0;
    let combined_out = base_cost.output.unwrap_or(0.0) + reasoning_usd;
    let combined_total = base_cost.total + reasoning_usd;
    if !combined_total.is_finite() || combined_total < 0.0 {
        return None;
    }
    base_cost.total = combined_total;
    base_cost = base_cost.output(combined_out);
    Some(base_cost)
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
/// 2. Valid provider-reported `usage.cost` on the wire (including an authoritative `$0.00`),
/// 3. Tier-aware fallback pricing from [`rig::catalog::Catalog::builtin`] (`tiers` / `context_over_200k` / `reasoning`).
#[must_use]
pub fn enrich_usage_with_endpoints_cost(
    mut usage: Usage,
    provider: &Option<ModelProvider>,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Usage {
    let Some(p) = provider.as_ref() else {
        return usage;
    };

    // Local Ollama and flat-rate subscription models (ChatGPT / OpenCode Go) never incur per-token
    // USD charges, even if an upstream gateway wire includes a synthetic cost field.
    if matches!(
        p,
        ModelProvider::Ollama(_) | ModelProvider::OpenCodeGo(_, _) | ModelProvider::ChatGpt(_, _)
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
            let input_rate = valid_rate(cm.input_cost_per_1m);
            let output_rate = valid_rate(cm.output_cost_per_1m);
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

    let spec_opt = resolve_catalog_spec(p);

    // When a provider wire omits `cost`, Rig's `ReplyFold::response` pre-populates `usage.cost`
    // using `spec.pricing.cost(&usage)` (base tier only, ignoring `tiers` and `details.cost.reasoning`).
    // Detect that exact base-tier prefill (plus Anthropic's local cache-TTL prefill) whenever
    // long-context tiers or distinct reasoning rates apply so tiered catalog pricing takes effect,
    // while keeping genuine wire-reported gateway costs (`Cost::from_total`, including `$0.00`) intact.
    if let Some(spec) = spec_opt {
        let needs_extended_pricing = usage
            .input_tokens
            .is_some_and(|in_tok| matching_context_tier(spec, in_tok).is_some())
            || (spec.details.cost.reasoning.is_some()
                && usage.reasoning_tokens.unwrap_or(0) > 0);
        if needs_extended_pricing {
            let is_rig_base_prefill = usage.cost.is_some()
                && spec.pricing.as_ref().and_then(|base| base.cost(&usage)) == usage.cost;
            if is_rig_base_prefill || matches!(p, ModelProvider::Anthropic(_, _)) {
                usage.cost = None;
            }
        }
    }

    if usage.cost.is_some() {
        return usage;
    }

    if let Some(spec) = spec_opt
        && let Some(cost) = calculate_catalog_cost(spec, &usage)
    {
        usage.cost = Some(cost);
    }
    usage
}

/// Enriches a [`rig::agent::PromptResponse`]'s aggregated [`Usage`] by pricing each
/// [`rig::agent::CompletionCall`] individually before summing.
///
/// Pricing per completion call (matching `OpenCode`'s per-step `SessionUsage.record`) prevents
/// multi-step tool runs whose summed `input_tokens` cross a long-context tier threshold (e.g.
/// two `160k` calls summing to `320k > 272k`) from being overcharged at the long-context tier,
/// while accurately charging individual steps that genuinely exceed the tier threshold.
#[must_use]
pub fn enrich_prompt_response_usage(
    res: &rig::agent::PromptResponse,
    provider: &Option<ModelProvider>,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Usage {
    if res.completion_calls.len() <= 1 {
        return enrich_usage_with_endpoints_cost(res.usage, provider, endpoints);
    }

    let Some(p) = provider.as_ref() else {
        return res.usage;
    };
    if matches!(
        p,
        ModelProvider::Ollama(_) | ModelProvider::OpenCodeGo(_, _) | ModelProvider::ChatGpt(_, _)
    ) {
        let mut cleared = res.usage;
        cleared.cost = None;
        return cleared;
    }

    let mut summed_cost: Option<Cost> = None;
    let mut reported_calls = 0usize;
    let mut priced_calls = 0usize;

    for call in &res.completion_calls {
        if !call.usage.is_reported() {
            continue;
        }
        reported_calls += 1;
        let enriched = enrich_usage_with_endpoints_cost(call.usage, provider, endpoints);
        if let Some(c) = enriched
            .cost
            .filter(|c| c.total.is_finite() && c.total >= 0.0)
        {
            priced_calls += 1;
            summed_cost = Some(match summed_cost {
                Some(acc) => acc + c,
                None => c,
            });
        }
    }

    if reported_calls > 0 && priced_calls == reported_calls {
        let mut out = res.usage;
        out.cost = summed_cost;
        out
    } else {
        enrich_usage_with_endpoints_cost(res.usage, provider, endpoints)
    }
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
    use boxxy_model_selection::{AnthropicModel, OpenAiApiRoute, OpenAiModel};

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

    #[test]
    fn applies_long_context_tiers_and_prices_multi_step_calls_individually() {
        // GPT-6.1 Terra: base $1/M input, $5/M output, $0.05/M cache_read;
        // >272k context tier: $2/M input, $7.5/M output, $0.10/M cache_read.
        let terra = Some(ModelProvider::OpenAi(
            OpenAiModel::new("gpt-6.1-terra"),
            None,
        ));

        // 1. Single call at 300k input (100k cached + 200k uncached) + 10k output -> >272k tier:
        //    200k * $2/M ($0.40) + 100k * $0.10/M ($0.01) + 10k * $7.5/M ($0.075) = $0.485.
        let over_tier = Usage::new()
            .input_tokens(300_000)
            .cached_input_tokens(100_000)
            .output_tokens(10_000);
        let priced_over = enrich_usage_with_catalog_cost(over_tier, &terra);
        let total_over = priced_over.cost.expect("tiered cost").total;
        assert!(
            (total_over - 0.485).abs() < 1e-6,
            "expected $0.485 long-context tier cost, got {total_over}"
        );

        // 2. Multi-step run where two 160k calls sum to 320k (>272k), yet neither call exceeded 272k!
        //    Each call costs 160k * $1/M + 1k * $5/M = $0.165; combined = $0.330 (NOT 2x tier $0.655).
        let call_a = serde_json::json!({
            "call_index": 0,
            "usage": { "input_tokens": 160_000, "output_tokens": 1_000, "total_tokens": 161_000 },
            "raw": null
        });
        let call_b = serde_json::json!({
            "call_index": 1,
            "usage": { "input_tokens": 160_000, "output_tokens": 1_000, "total_tokens": 161_000 },
            "raw": null
        });
        let sub_tier_multi: rig::agent::PromptResponse = serde_json::from_value(serde_json::json!({
            "content": [{ "type": "text", "text": "Done" }],
            "usage": { "input_tokens": 320_000, "output_tokens": 2_000, "total_tokens": 322_000 },
            "completion_calls": [call_a.clone(), call_b],
            "messages": []
        }))
        .expect("valid multi-step PromptResponse");

        let enriched_sub = enrich_prompt_response_usage(&sub_tier_multi, &terra, &[]);
        let total_sub = enriched_sub.cost.expect("per-call base cost").total;
        assert!(
            (total_sub - 0.330).abs() < 1e-6,
            "expected $0.330 per-call base rate, got {total_sub}"
        );

        // 3. Mixed multi-step run: Step 0 at 160k ($0.165 base) + Step 1 at 300k ($0.6075 >272k tier) = $0.7725.
        let call_over = serde_json::json!({
            "call_index": 1,
            "usage": { "input_tokens": 300_000, "output_tokens": 1_000, "total_tokens": 301_000 },
            "raw": null
        });
        let mixed_multi: rig::agent::PromptResponse = serde_json::from_value(serde_json::json!({
            "content": [{ "type": "text", "text": "Done" }],
            "usage": { "input_tokens": 460_000, "output_tokens": 2_000, "total_tokens": 462_000 },
            "completion_calls": [call_a, call_over],
            "messages": []
        }))
        .expect("valid mixed PromptResponse");

        let enriched_mixed = enrich_prompt_response_usage(&mixed_multi, &terra, &[]);
        let total_mixed = enriched_mixed.cost.expect("mixed tier cost").total;
        assert!(
            (total_mixed - 0.7725).abs() < 1e-6,
            "expected $0.7725 mixed per-call cost, got {total_mixed}"
        );

        // 4. Anthropic decoder pre-filled base-tier cost is superseded when >100k tier applies on Haiku 5.5
        //    (base $0.10/$0.50; >100k tier $0.50/$2.50 -> 150k input + 2k output = $0.075 + $0.005 = $0.080).
        let haiku_55 = Some(ModelProvider::Anthropic(
            AnthropicModel::new("claude-haiku-5-5"),
            None,
        ));
        let mut prefilled = Usage::new().input_tokens(150_000).output_tokens(2_000);
        prefilled.cost = Some(Cost::from_total(0.016)); // stale base-tier prefill
        let re_priced = enrich_usage_with_catalog_cost(prefilled, &haiku_55);
        let haiku_total = re_priced.cost.expect("anthropic tiered cost").total;
        assert!(
            (haiku_total - 0.080).abs() < 1e-6,
            "expected $0.080 >100k Haiku 5.5 cost, got {haiku_total}"
        );

        // 5. Authoritative $0.00 wire cost on OpenRouter `:free` route is preserved, not overwritten.
        let or_prov = Some(ModelProvider::OpenRouter(
            "anthropic/claude-sonnet-5".to_string(),
        ));
        let mut free_wire = Usage::new().input_tokens(50_000).output_tokens(5_000);
        free_wire.cost = Some(Cost::from_total(0.0));
        let free_enriched = enrich_usage_with_catalog_cost(free_wire, &or_prov);
        assert_eq!(free_enriched.cost.map(|c| c.total), Some(0.0));
    }
}
