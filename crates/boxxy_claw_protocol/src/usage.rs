//! Turn-level and session-level LLM token & USD cost accounting and display formatting.

use rig::completion::Usage;
use serde::{Deserialize, Serialize};

/// Serializable DTO wrapper for [`rig::completion::Usage`] enriched with Boxxy's
/// per-turn context-window footprint (`context_tokens` and `context_window`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageWrapper {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Tokens occupying the model's context window on the final completion call of a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Maximum context window of the model that served the turn, if known from the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

impl From<&Usage> for UsageWrapper {
    fn from(u: &Usage) -> Self {
        let input = u.input_tokens.unwrap_or(0);
        let output = u.output_tokens.unwrap_or(0);
        let summed = input.saturating_add(output);
        let total = u.total_tokens.unwrap_or(summed);
        let cost_usd = u
            .cost
            .and_then(|c| (c.total.is_finite() && c.total >= 0.0).then_some(c.total));

        Self {
            input_tokens: input,
            output_tokens: output,
            reasoning_tokens: u.reasoning_tokens.filter(|&r| r > 0),
            cached_input_tokens: u.cached_input_tokens.filter(|&c| c > 0),
            cost_usd,
            context_tokens: (total > 0).then_some(total),
            context_window: None,
        }
    }
}

impl From<Usage> for UsageWrapper {
    fn from(u: Usage) -> Self {
        Self::from(&u)
    }
}

impl UsageWrapper {
    /// Total tokens across this turn (`input_tokens + output_tokens`), saturated on overflow.
    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// Context window footprint of the final call in this turn, falling back to `total_tokens()`.
    #[must_use]
    pub const fn effective_context_tokens(&self) -> u64 {
        match self.context_tokens {
            Some(ctx) => ctx,
            None => self.total_tokens(),
        }
    }
}

/// Live session-level context and cost metrics displayed in the Claw sidebar.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionUsage {
    /// Tokens in the current context window (from the latest completion request).
    pub context_tokens: u64,
    /// Maximum context window of the active model, if known from the catalog.
    pub context_window: Option<u64>,
    /// Cumulative tokens consumed across all turns in the session.
    pub total_tokens: u64,
    /// Cumulative cost in USD across all turns in the session.
    pub cost_usd: f64,
    /// Number of tool calls executed in the session.
    #[serde(default)]
    pub tools_used: u64,
}

impl SessionUsage {
    /// Context window utilization percentage (rounded to nearest integer, matching `OpenCode`),
    /// or `None` if the model's context window is unknown.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "token counts are positive integers well within f64's 53-bit mantissa and rounded to non-negative u64 percentage"
    )]
    pub fn context_percent(&self) -> Option<u64> {
        let limit = self.context_window.filter(|&w| w > 0)?;
        let tokens = self.display_tokens();
        let pct = ((tokens as f64 / limit as f64) * 100.0).round();
        if pct.is_finite() && pct >= 0.0 {
            Some(pct as u64)
        } else {
            Some(0)
        }
    }

    /// Tokens to display in the Context header (`context_tokens` when non-zero, else `total_tokens`).
    #[must_use]
    pub const fn display_tokens(&self) -> u64 {
        if self.context_tokens > 0 {
            self.context_tokens
        } else {
            self.total_tokens
        }
    }

    /// Applies a turn's `UsageWrapper` to this session accumulator.
    pub fn record_turn(&mut self, usage: &UsageWrapper) {
        let turn_total = usage.total_tokens();
        self.total_tokens = self.total_tokens.saturating_add(turn_total);
        let ctx = usage.effective_context_tokens();
        if ctx > 0 {
            self.context_tokens = ctx;
        }
        if let Some(window) = usage.context_window {
            self.context_window = Some(window);
        }
        if let Some(cost) = usage.cost_usd
            && cost.is_finite()
            && cost > 0.0
        {
            self.cost_usd += cost;
        }
    }
}

/// Formats an integer with ASCII comma thousands separators (e.g. `25236` -> `"25,236"`)
/// using a stack buffer so only the final `String` is allocated.
#[must_use]
pub fn format_number_with_commas(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }

    // Max formatted `u64::MAX` is `"18,446,744,073,709,551,615"` (26 ASCII bytes).
    let mut buf = [0u8; 26];
    let mut pos = buf.len();
    let mut digits_in_group = 0u8;

    while n > 0 {
        if digits_in_group == 3 {
            pos -= 1;
            buf[pos] = b',';
            digits_in_group = 0;
        }
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
        digits_in_group += 1;
    }

    // All written bytes are guaranteed ASCII digits and commas (`0x2C..=0x39`).
    std::str::from_utf8(&buf[pos..])
        .unwrap_or("0")
        .to_string()
}

/// Formats a USD amount like `OpenCode`'s `Intl.NumberFormat("en-US", { style: "currency", currency: "USD" })`
/// (e.g. `0.05` -> `"$0.05"`).
#[must_use]
pub fn format_usd(amount: f64) -> String {
    let safe = if amount.is_finite() && amount > 0.0 {
        amount
    } else {
        0.0
    };
    format!("${safe:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_wrapper_from_rig_usage_handles_values_and_defaults() {
        let usage = Usage::default()
            .input_tokens(150)
            .output_tokens(50)
            .reasoning_tokens(20)
            .cached_input_tokens(100);

        let wrapper = UsageWrapper::from(&usage);
        assert_eq!(wrapper.input_tokens, 150);
        assert_eq!(wrapper.output_tokens, 50);
        assert_eq!(wrapper.reasoning_tokens, Some(20));
        assert_eq!(wrapper.cached_input_tokens, Some(100));
        assert_eq!(wrapper.cost_usd, None);
        assert_eq!(wrapper.context_tokens, Some(200));
        assert_eq!(wrapper.context_window, None);
        assert_eq!(wrapper, UsageWrapper::from(usage));

        let empty_wrapper = UsageWrapper::from(Usage::default());
        assert_eq!(empty_wrapper.input_tokens, 0);
        assert_eq!(empty_wrapper.output_tokens, 0);
        assert_eq!(empty_wrapper.reasoning_tokens, None);
        assert_eq!(empty_wrapper.cached_input_tokens, None);
        assert_eq!(empty_wrapper.context_tokens, None);
    }

    #[test]
    fn session_usage_and_formatting_handle_boundaries() {
        assert_eq!(format_number_with_commas(0), "0");
        assert_eq!(format_number_with_commas(999), "999");
        assert_eq!(format_number_with_commas(1_000), "1,000");
        assert_eq!(format_number_with_commas(25_236), "25,236");
        assert_eq!(format_number_with_commas(1_048_576), "1,048,576");
        assert_eq!(
            format_number_with_commas(u64::MAX),
            "18,446,744,073,709,551,615"
        );

        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(-5.0), "$0.00");
        assert_eq!(format_usd(f64::NAN), "$0.00");
        assert_eq!(format_usd(f64::INFINITY), "$0.00");
        assert_eq!(format_usd(0.0512), "$0.05");
        assert_eq!(format_usd(1.2), "$1.20");

        let mut su = SessionUsage::default();
        assert_eq!(su.context_percent(), None);

        su.record_turn(&UsageWrapper {
            input_tokens: 20_000,
            output_tokens: 5_236,
            reasoning_tokens: None,
            cached_input_tokens: None,
            cost_usd: Some(0.05),
            context_tokens: Some(25_236),
            context_window: Some(1_048_576),
        });

        assert_eq!(su.total_tokens, 25_236);
        assert_eq!(su.context_tokens, 25_236);
        assert_eq!(su.context_window, Some(1_048_576));
        assert_eq!(su.context_percent(), Some(2));
        assert!((su.cost_usd - 0.05).abs() < 1e-9);

        // Switching to an unpriced local model preserves accumulated session USD cost
        su.record_turn(&UsageWrapper {
            input_tokens: 1_000,
            output_tokens: 500,
            reasoning_tokens: None,
            cached_input_tokens: None,
            cost_usd: None,
            context_tokens: Some(26_736),
            context_window: Some(128_000),
        });
        assert_eq!(su.total_tokens, 26_736);
        assert_eq!(su.context_window, Some(128_000));
        assert!((su.cost_usd - 0.05).abs() < 1e-9);
    }
}
