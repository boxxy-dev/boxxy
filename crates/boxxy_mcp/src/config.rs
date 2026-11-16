use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Minimum timeout floor enforced across MCP handshakes, tool listings, and tool calls.
pub const MIN_TIMEOUT_MS: u64 = 1_000;
/// Minimum initial retry backoff floor in milliseconds.
pub const MIN_BACKOFF_MS: u64 = 100;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        /// Retained for backwards compatibility with saved `settings.json` configs;
        /// all HTTP connections use MCP Streamable HTTP (`2026-07-28` auto-discovery
        /// with `2025-11-25` fallback).
        #[serde(default)]
        streamable: bool,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct BackoffConfig {
    pub initial_ms: u64,
    pub max_ms: u64,
    pub multiplier: f32,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_ms: 1_000,
            max_ms: 30_000,
            multiplier: 2.0,
        }
    }
}

impl BackoffConfig {
    /// Clamped initial backoff delay (`>= 100ms`).
    #[must_use]
    pub fn effective_initial_ms(&self) -> u64 {
        self.initial_ms.max(MIN_BACKOFF_MS)
    }

    /// Clamped maximum backoff delay (`>= effective_initial_ms()`).
    #[must_use]
    pub fn effective_max_ms(&self) -> u64 {
        self.max_ms.max(self.effective_initial_ms())
    }

    /// Validated finite multiplier (`> 1.0`, defaulting to `2.0` if non-finite or `<= 1.0`).
    #[must_use]
    pub fn effective_multiplier(&self) -> f64 {
        if self.multiplier.is_finite() && self.multiplier > 1.0 {
            f64::from(self.multiplier)
        } else {
            2.0
        }
    }

    /// Computes the next exponential backoff delay in milliseconds, clamped to [`Self::effective_max_ms`].
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "clamped positive milliseconds fit comfortably within u64"
    )]
    pub fn next_delay_ms(&self, current_ms: u64) -> u64 {
        let max_ms = self.effective_max_ms();
        let scaled = (current_ms as f64) * self.effective_multiplier();
        if !scaled.is_finite() || scaled >= max_ms as f64 {
            max_ms
        } else {
            (scaled as u64).clamp(MIN_BACKOFF_MS, max_ms)
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub backoff: BackoffConfig,
}

impl McpServerConfig {
    /// Validated per-operation timeout (`>= 1_000ms`).
    #[must_use]
    pub fn effective_timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.max(MIN_TIMEOUT_MS))
    }

    /// Validated connection attempt count (`>= 1`).
    #[must_use]
    pub fn effective_attempts(&self) -> u32 {
        self.max_retries.max(1)
    }
}

const fn default_true() -> bool {
    true
}
const fn default_timeout() -> u64 {
    60_000
}
const fn default_retries() -> u32 {
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_config_handles_degenerate_values_safely() {
        let cfg = BackoffConfig {
            initial_ms: 0,
            max_ms: 10,
            multiplier: f32::NAN,
        };
        assert_eq!(cfg.effective_initial_ms(), MIN_BACKOFF_MS);
        assert_eq!(cfg.effective_max_ms(), MIN_BACKOFF_MS);
        assert_eq!(cfg.next_delay_ms(100), MIN_BACKOFF_MS);

        let normal = BackoffConfig {
            initial_ms: 500,
            max_ms: 2_500,
            multiplier: 2.0,
        };
        assert_eq!(normal.next_delay_ms(500), 1_000);
        assert_eq!(normal.next_delay_ms(1_000), 2_000);
        assert_eq!(normal.next_delay_ms(2_000), 2_500);
    }

    #[test]
    fn serde_defaults_preserve_backward_compatibility() {
        let raw = r#"{
            "name": "local-srv",
            "transport": { "type": "http", "url": "http://127.0.0.1:8080/mcp" }
        }"#;
        let parsed: McpServerConfig = serde_json::from_str(raw).expect("valid config");
        assert!(parsed.enabled);
        assert_eq!(parsed.timeout_ms, 60_000);
        assert_eq!(parsed.max_retries, 3);
        assert_eq!(
            parsed.transport,
            McpTransport::Http {
                url: "http://127.0.0.1:8080/mcp".to_string(),
                headers: HashMap::new(),
                streamable: false,
            }
        );
    }
}
