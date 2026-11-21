//! Thinking-level to `rig::completion::Reasoning` resolution with catalog & custom-endpoint clamping.

use crate::catalog::{find_custom_model, model_output_limit_for_provider, resolve_catalog_spec};
use boxxy_model_selection::{ModelProvider, OpenAiCompatibleEndpoint, ThinkingLevel};
use rig::completion::{Effort, Reasoning};

const MIN_THINKING_BUDGET_TOKENS: u32 = 1_024;
const DEFAULT_BUDGET_HEADROOM_TOKENS: u64 = 8_192;
const MIN_GATEWAY_MAX_TOKENS_FLOOR: u64 = 4_097;

/// Maps a Boxxy [`ThinkingLevel`] directly to a portable [`Reasoning`] configuration.
#[must_use]
pub const fn map_thinking_level(level: ThinkingLevel) -> Reasoning {
    match level {
        ThinkingLevel::None => Reasoning::Off,
        ThinkingLevel::Minimal => Reasoning::Effort(Effort::Minimal),
        ThinkingLevel::Low => Reasoning::Effort(Effort::Low),
        ThinkingLevel::Medium => Reasoning::Effort(Effort::Medium),
        ThinkingLevel::High => Reasoning::Effort(Effort::High),
        ThinkingLevel::XHigh => Reasoning::Effort(Effort::XHigh),
    }
}

const fn effort_rank(effort: Effort) -> i32 {
    match effort {
        Effort::Minimal => 0,
        Effort::Low => 1,
        Effort::Medium => 2,
        Effort::XHigh | Effort::Max => 4,
        // `Effort` is `#[non_exhaustive]`; `High` and any future tier share rank 3.
        _ => 3,
    }
}

const fn thinking_rank(level: ThinkingLevel) -> i32 {
    match level {
        ThinkingLevel::None => -1,
        ThinkingLevel::Minimal => 0,
        ThinkingLevel::Low => 1,
        ThinkingLevel::Medium => 2,
        ThinkingLevel::High => 3,
        ThinkingLevel::XHigh => 4,
    }
}

/// Maps a [`ThinkingLevel`] to a token budget for budget-based reasoning models
/// (before clamping to `ModelSpec.reasoning.budget()`).
const fn budget_tokens_for_level(level: ThinkingLevel) -> u32 {
    match level {
        ThinkingLevel::None => 0,
        ThinkingLevel::Minimal => 1_024,
        ThinkingLevel::Low => 2_000,
        ThinkingLevel::Medium => 8_000,
        ThinkingLevel::High => 32_000,
        ThinkingLevel::XHigh => 64_000,
    }
}

/// Clamps `target` to the nearest supported [`Effort`] in `supported` (preferring the lower
/// rank on ties so stale cross-model settings never reject with HTTP 400 or spike latency).
fn clamp_effort(target: Effort, supported: &[Effort]) -> Effort {
    if supported.is_empty() || supported.contains(&target) {
        return target;
    }

    let target_rank = effort_rank(target);
    let mut best = supported[0];
    let mut best_dist = (effort_rank(best) - target_rank).abs();

    for &cand in &supported[1..] {
        let dist = (effort_rank(cand) - target_rank).abs();
        if dist < best_dist || (dist == best_dist && effort_rank(cand) < effort_rank(best)) {
            best = cand;
            best_dist = dist;
        }
    }

    best
}

/// Clamps a non-`None` [`ThinkingLevel`] to the nearest non-`None` level in `supported`.
fn clamp_custom_thinking_level(
    target: ThinkingLevel,
    supported: &[ThinkingLevel],
) -> Option<ThinkingLevel> {
    if supported.contains(&target) {
        return Some(target);
    }

    let target_rank = thinking_rank(target);
    let mut best: Option<ThinkingLevel> = None;
    let mut best_dist = i32::MAX;

    for &cand in supported {
        if cand == ThinkingLevel::None {
            continue;
        }
        let dist = (thinking_rank(cand) - target_rank).abs();
        if dist < best_dist
            || (dist == best_dist && best.is_some_and(|b| thinking_rank(cand) < thinking_rank(b)))
        {
            best = Some(cand);
            best_dist = dist;
        }
    }

    best
}

fn clamp_budget_range(tokens: u32, range: &std::ops::RangeInclusive<u32>) -> u32 {
    let lo = *range.start();
    let hi = *range.end();
    if lo <= hi {
        tokens.clamp(lo, hi)
    } else {
        tokens.max(lo)
    }
}

/// Clamps a thinking token budget both to `range` and strictly below `output_limit`
/// (leaving up to 4,096 reply tokens while preserving Anthropic's `>= 1024` floor).
fn clamp_budget_with_output_limit(
    tokens: u32,
    range: &std::ops::RangeInclusive<u32>,
    output_limit: Option<u64>,
) -> Option<u32> {
    let range_clamped = clamp_budget_range(tokens, range).max(MIN_THINKING_BUDGET_TOKENS);
    let Some(out_limit) = output_limit else {
        return Some(range_clamped);
    };

    let min_allowed = (*range.start()).max(MIN_THINKING_BUDGET_TOKENS);
    if out_limit <= u64::from(min_allowed) {
        return None;
    }

    let reserve = (out_limit / 4).clamp(1, 4_096);
    let max_budget_u64 = out_limit
        .saturating_sub(reserve)
        .max(u64::from(min_allowed))
        .min(out_limit - 1);
    let max_budget = u32::try_from(max_budget_u64).unwrap_or(u32::MAX);

    Some(range_clamped.min(max_budget).max(min_allowed))
}

/// Computes an explicit `max_tokens` cap when `reasoning` is [`Reasoning::Budget`].
///
/// Returns `None` for all non-budget reasoning modes (`None`, `Off`, `Effort`) so providers
/// that strictly cap output length (Gemini, Ollama, `OpenRouter`, Cohere) keep their native
/// defaults. For budget-thinking models (Anthropic Messages and gateway dialects that default
/// `max_tokens` to `4096`), ensures `max_tokens > tokens` and `max_tokens <= output_limit`
/// so Rig never strips `thinking` under `OnUnsupported::Ignore`.
#[must_use]
pub fn recommended_max_tokens_for_reasoning(
    provider: &ModelProvider,
    reasoning: Option<Reasoning>,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Option<u64> {
    let Some(Reasoning::Budget { tokens }) = reasoning else {
        return None;
    };
    let budget = u64::from(tokens);
    let desired = budget
        .saturating_add(DEFAULT_BUDGET_HEADROOM_TOKENS)
        .max(MIN_GATEWAY_MAX_TOKENS_FLOOR);

    if let Some(out_limit) = model_output_limit_for_provider(provider, endpoints) {
        if out_limit <= budget {
            return None;
        }
        Some(desired.min(out_limit))
    } else {
        Some(desired)
    }
}

/// Resolves the effective [`Reasoning`] option for `provider` and `thinking` against
/// [`rig::catalog::Catalog::builtin`].
#[must_use]
pub fn resolve_model_reasoning(
    provider: &ModelProvider,
    thinking: Option<ThinkingLevel>,
) -> Option<Reasoning> {
    resolve_model_reasoning_with_endpoints(provider, thinking, &[])
}

/// Resolves the effective [`Reasoning`] option for `provider` and `thinking`, honoring
/// user-configured [`OpenAiCompatibleEndpoint`] `thinking_levels` overrides first and
/// falling back to [`rig::catalog::Catalog::builtin`].
#[must_use]
pub fn resolve_model_reasoning_with_endpoints(
    provider: &ModelProvider,
    thinking: Option<ThinkingLevel>,
    endpoints: &[OpenAiCompatibleEndpoint],
) -> Option<Reasoning> {
    let level = thinking?;
    let spec = resolve_catalog_spec(provider);

    // If this is a custom OpenAI-compatible model with explicit user-defined thinking levels,
    // honor the user's configured levels while still respecting budget-only model specs.
    if let ModelProvider::OpenAiCompatible {
        provider_name,
        model_id,
        ..
    } = provider
        && let Some((_, Some(cm))) = find_custom_model(endpoints, provider_name, model_id)
        && !cm.thinking_levels.is_empty()
    {
        if level == ThinkingLevel::None {
            if cm.thinking_levels.contains(&ThinkingLevel::None)
                || spec.is_none_or(|s| s.reasoning.can_disable().unwrap_or(true))
            {
                return Some(Reasoning::Off);
            }
            return None;
        }

        let clamped_level = clamp_custom_thinking_level(level, &cm.thinking_levels)?;
        if let Some(s) = spec
            && s.reasoning.levels().is_none_or(<[_]>::is_empty)
            && let Some(range) = s.reasoning.budget()
        {
            let output_limit = model_output_limit_for_provider(provider, endpoints);
            return clamp_budget_with_output_limit(
                budget_tokens_for_level(clamped_level),
                range,
                output_limit,
            )
            .map(|tokens| Reasoning::Budget { tokens });
        }
        return Some(map_thinking_level(clamped_level));
    }

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
        if let Some(spec_levels) = s.reasoning.levels()
            && !spec_levels.is_empty()
        {
            if let Reasoning::Effort(target_effort) = map_thinking_level(level) {
                return Some(Reasoning::Effort(clamp_effort(target_effort, spec_levels)));
            }
        } else if let Some(range) = s.reasoning.budget() {
            let output_limit = model_output_limit_for_provider(provider, endpoints);
            return clamp_budget_with_output_limit(
                budget_tokens_for_level(level),
                range,
                output_limit,
            )
            .map(|tokens| Reasoning::Budget { tokens });
        }
    }

    Some(map_thinking_level(level))
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_model_selection::AnthropicModel;

    #[test]
    fn clamps_unsupported_effort_to_nearest_supported() {
        let supported = [Effort::Low, Effort::Medium, Effort::High];
        assert_eq!(clamp_effort(Effort::Minimal, &supported), Effort::Low);
        assert_eq!(clamp_effort(Effort::XHigh, &supported), Effort::High);
        assert_eq!(clamp_effort(Effort::Medium, &supported), Effort::Medium);

        // Models listing `Effort::Max` (without `XHigh`) map `ThinkingLevel::XHigh` to `Effort::Max`
        let max_only = [Effort::Low, Effort::High, Effort::Max];
        assert_eq!(clamp_effort(Effort::XHigh, &max_only), Effort::Max);

        // Tie between Low (1) and High (3) for Medium (2) prefers lower rank Low
        let ends_only = [Effort::Low, Effort::High];
        assert_eq!(clamp_effort(Effort::Medium, &ends_only), Effort::Low);
    }

    #[test]
    fn budget_tokens_scale_monotonically_across_levels() {
        assert!(
            budget_tokens_for_level(ThinkingLevel::Minimal)
                < budget_tokens_for_level(ThinkingLevel::Low)
        );
        assert!(
            budget_tokens_for_level(ThinkingLevel::Low)
                < budget_tokens_for_level(ThinkingLevel::Medium)
        );
        assert!(
            budget_tokens_for_level(ThinkingLevel::Medium)
                < budget_tokens_for_level(ThinkingLevel::High)
        );
        assert!(
            budget_tokens_for_level(ThinkingLevel::High)
                < budget_tokens_for_level(ThinkingLevel::XHigh)
        );
    }

    #[test]
    fn budget_clamps_below_output_limit_and_recommends_safe_max_tokens() {
        // Claude Haiku 4.5 is budget-only with a 64,000-token output limit.
        let haiku = ModelProvider::Anthropic(
            AnthropicModel::new("claude-haiku-4-5"),
            Some(ThinkingLevel::XHigh),
        );
        let reasoning = resolve_model_reasoning(&haiku, Some(ThinkingLevel::XHigh));
        assert_eq!(reasoning, Some(Reasoning::Budget { tokens: 59_904 }));
        assert_eq!(
            recommended_max_tokens_for_reasoning(&haiku, reasoning, &[]),
            Some(64_000)
        );

        // High (32k) stays at 32,000 and recommends 40,192 (<= 64,000, > 32,000)
        let high = resolve_model_reasoning(&haiku, Some(ThinkingLevel::High));
        assert_eq!(high, Some(Reasoning::Budget { tokens: 32_000 }));
        assert_eq!(
            recommended_max_tokens_for_reasoning(&haiku, high, &[]),
            Some(40_192)
        );

        // Output limits <= 1024 suppress unsendable budgets instead of failing silently
        assert_eq!(
            clamp_budget_with_output_limit(2_000, &(1_024..=64_000), Some(1_024)),
            None
        );

        // Non-budget Effort models never receive a synthetic `max_tokens` cap
        let sonnet = ModelProvider::Anthropic(
            AnthropicModel::new("claude-sonnet-5-5"),
            Some(ThinkingLevel::High),
        );
        assert_eq!(
            recommended_max_tokens_for_reasoning(
                &sonnet,
                Some(Reasoning::Effort(Effort::High)),
                &[]
            ),
            None
        );
    }
}
