//! Thinking-level to `rig::completion::Reasoning` resolution with catalog & custom-endpoint clamping.

use crate::catalog::{find_custom_model, resolve_catalog_spec};
use boxxy_model_selection::{ModelProvider, OpenAiCompatibleEndpoint, ThinkingLevel};
use rig::completion::{Effort, Reasoning};

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
        Effort::XHigh => 4,
        // `Effort` is `#[non_exhaustive]`; `High` and any future high-tier variant share rank 3.
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
            let tokens = clamp_budget_range(budget_tokens_for_level(clamped_level), range);
            return Some(Reasoning::Budget { tokens });
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
            let tokens = clamp_budget_range(budget_tokens_for_level(level), range);
            return Some(Reasoning::Budget { tokens });
        }
    }

    Some(map_thinking_level(level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_unsupported_effort_to_nearest_supported() {
        let supported = [Effort::Low, Effort::Medium, Effort::High];
        assert_eq!(clamp_effort(Effort::Minimal, &supported), Effort::Low);
        assert_eq!(clamp_effort(Effort::XHigh, &supported), Effort::High);
        assert_eq!(clamp_effort(Effort::Medium, &supported), Effort::Medium);

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
}
