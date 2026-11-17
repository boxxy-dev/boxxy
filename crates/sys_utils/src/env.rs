//! Declarative registry and typed readers for all `BOXXY_*` and provider API-key
//! environment variables.
//!
//! Centralizing every environment variable here guarantees:
//! 1. One consistent zero-allocation truthy/falsy parser across every crate.
//! 2. Zero undocumented `BOXXY_*` or `*_API_KEY` flags (`boxxy-terminal +env` renders directly from [`ENV_SPECS`]).
//! 3. Data-driven Flatpak/native daemon environment propagation (`daemon_spawn_env`).
//! 4. Data-driven child-PTY environment scrubbing (`scrub_child_pty_env`).

use std::path::Path;

/// Logical grouping used by `boxxy-terminal +env` and `--help`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvCategory {
    /// Config directory & side-by-side isolated/ephemeral instance flags.
    Isolation,
    /// Interactive shell integration controls.
    Shell,
    /// Logging, context inspection, and QA simulation variables.
    Diagnostics,
    /// Built-in LLM and web-search provider API key environment variables.
    ProviderKey,
    /// Internal session/PTY markers managed by Boxxy itself (hidden unless `--internal`).
    Internal,
}

impl EnvCategory {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Isolation => "Paths & Instance Isolation",
            Self::Shell => "Shell Integration",
            Self::Diagnostics => "Observability & Diagnostics",
            Self::ProviderKey => "AI Provider API Keys",
            Self::Internal => "Internal PTY Plumbing",
        }
    }
}

/// Static specification for a single environment variable recognized by Boxxy.
#[derive(Clone, Copy, Debug)]
pub struct EnvVarSpec {
    pub name: &'static str,
    pub category: EnvCategory,
    /// Equivalent CLI flag on `boxxy-terminal`, if one exists.
    pub cli_flag: Option<&'static str>,
    pub value_hint: &'static str,
    pub summary: &'static str,
    /// Mask live values when printing in `boxxy-terminal +env`.
    pub secret: bool,
    /// Forward raw value from UI to `boxxy-agent` when spawning the daemon.
    /// (Note: `BOXXY_CONFIG_DIR`, `BOXXY_INSTANCE`, `BOXXY_EPHEMERAL`, and
    /// `BOXXY_DEBUG_CONTEXT` are synthesized from merged CLI + env state in [`daemon_spawn_env`].)
    pub propagate_raw_to_daemon: bool,
    /// Remove from child PTY shells so daemon/dev markers never leak into user shells.
    pub scrub_from_child_pty: bool,
}

/// Canonical list of all `BOXXY_*` and built-in provider environment variables.
pub const ENV_SPECS: &[EnvVarSpec] = &[
    // --- Paths & Instance Isolation ---
    EnvVarSpec {
        name: "BOXXY_CONFIG_DIR",
        category: EnvCategory::Isolation,
        cli_flag: Some("--config-dir <PATH>"),
        value_hint: "<ABS_PATH>",
        summary: "Override the active configuration and SQLite directory (must be absolute).",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_DEV",
        category: EnvCategory::Isolation,
        cli_flag: Some("--dev"),
        value_hint: "0|1",
        summary: "Run as an isolated development instance (`dev.boxxy.BoxxyTerminal.Devel`).",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_INSTANCE",
        category: EnvCategory::Isolation,
        cli_flag: Some("--instance <SLUG>"),
        value_hint: "<SLUG>",
        summary: "Run as a named isolated instance using `~/.config/boxxy-terminal-<slug>/`.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_EPHEMERAL",
        category: EnvCategory::Isolation,
        cli_flag: Some("--ephemeral"),
        value_hint: "0|1",
        summary: "Use a clean factory-default directory in `$XDG_RUNTIME_DIR`.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_SYNC_SETTINGS",
        category: EnvCategory::Isolation,
        cli_flag: Some("--sync-settings"),
        value_hint: "0|1",
        summary: "Copy non-DB settings, themes, characters, and skills from Stable into the dev dir.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    // --- Shell Integration ---
    EnvVarSpec {
        name: "BOXXY_SHELL_INTEGRATION",
        category: EnvCategory::Shell,
        cli_flag: None,
        value_hint: "0|1",
        summary: "Set to `0` (`false`) to opt out of automatic OSC 7 / OSC 133 shell shims.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    // --- Observability & Diagnostics ---
    EnvVarSpec {
        name: "BOXXY_DEBUG_CONTEXT",
        category: EnvCategory::Diagnostics,
        cli_flag: Some("--debug-context"),
        value_hint: "0|1",
        summary: "Log full LLM system prompts, history, and tool payloads on `model-context`.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_SIMULATE_UPDATE",
        category: EnvCategory::Diagnostics,
        cli_flag: None,
        value_hint: "<FROM..TO>",
        summary: "Preview the post-update 'What's New' dialog for a GitHub commit range.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_TELEMETRY_ENDPOINT",
        category: EnvCategory::Diagnostics,
        cli_flag: None,
        value_hint: "<URL>",
        summary: "Override the OTLP metrics collector URL for local telemetry testing.",
        secret: false,
        propagate_raw_to_daemon: true,
        scrub_from_child_pty: true,
    },
    // --- Built-in Provider API Keys ---
    EnvVarSpec {
        name: "GEMINI_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for Google Gemini.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "ANTHROPIC_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for Anthropic Claude.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "OPENAI_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for OpenAI.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "OPENROUTER_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for OpenRouter.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "DEEPSEEK_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for DeepSeek.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "MOONSHOT_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for Moonshot (Kimi).",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "XAI_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for xAI (Grok).",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "ZAI_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for Z.ai (GLM).",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "MISTRAL_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for Mistral.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "MINIMAX_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for MiniMax.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "GROQ_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for Groq.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "OPENCODE_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for OpenCode Zen and OpenCode Go.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    EnvVarSpec {
        name: "TAVILY_API_KEY",
        category: EnvCategory::ProviderKey,
        cli_flag: None,
        value_hint: "<KEY>",
        summary: "API key override for Tavily Web Search.",
        secret: true,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: false,
    },
    // --- Internal PTY Plumbing ---
    EnvVarSpec {
        name: "BOXXY_WINDOW_ID",
        category: EnvCategory::Internal,
        cli_flag: None,
        value_hint: "<ID>",
        summary: "Internal window identifier marker set on local PTY builders.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_SHELL_INTEGRATION_LOADED",
        category: EnvCategory::Internal,
        cli_flag: None,
        value_hint: "1",
        summary: "Internal guard preventing duplicate shell integration hook sourcing.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_SHELL_INTEGRATION_XDG_DIR",
        category: EnvCategory::Internal,
        cli_flag: None,
        value_hint: "<PATH>",
        summary: "Internal temporary `XDG_DATA_DIRS` entry for Fish/Nushell autoload.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_LOGIN_SHELL",
        category: EnvCategory::Internal,
        cli_flag: None,
        value_hint: "1",
        summary: "Internal marker forwarding `--login` state to shell integration shims.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
    EnvVarSpec {
        name: "BOXXY_ORIG_ZDOTDIR",
        category: EnvCategory::Internal,
        cli_flag: None,
        value_hint: "<PATH>",
        summary: "Internal backup of the user's original `ZDOTDIR` restored by the Zsh shim.",
        secret: false,
        propagate_raw_to_daemon: false,
        scrub_from_child_pty: true,
    },
];

/// Canonical `(ENV_VAR_NAME, ProviderName)` mapping for built-in AI and search providers.
pub const PROVIDER_ENV_KEY_MAP: &[(&str, &str)] = &[
    ("GEMINI_API_KEY", "Gemini"),
    ("ANTHROPIC_API_KEY", "Anthropic"),
    ("OPENAI_API_KEY", "OpenAI"),
    ("OPENROUTER_API_KEY", "OpenRouter"),
    ("DEEPSEEK_API_KEY", "DeepSeek"),
    ("MOONSHOT_API_KEY", "Moonshot"),
    ("XAI_API_KEY", "xAI"),
    ("ZAI_API_KEY", "Z.ai"),
    ("MISTRAL_API_KEY", "Mistral"),
    ("MINIMAX_API_KEY", "MiniMax"),
    ("GROQ_API_KEY", "Groq"),
    ("OPENCODE_API_KEY", "OpenCode Zen"),
    ("OPENCODE_API_KEY", "OpenCode Go"),
    ("TAVILY_API_KEY", "Tavily"),
];

/// Parses a standard boolean environment variable string (`1/true/yes/on` vs `0/false/no/off`)
/// without heap allocation.
#[must_use]
pub fn parse_bool_str(raw: &str) -> Option<bool> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("1")
        || trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("yes")
        || trimmed.eq_ignore_ascii_case("on")
    {
        Some(true)
    } else if trimmed.eq_ignore_ascii_case("0")
        || trimmed.eq_ignore_ascii_case("false")
        || trimmed.eq_ignore_ascii_case("no")
        || trimmed.eq_ignore_ascii_case("off")
    {
        Some(false)
    } else {
        None
    }
}

/// Returns `true` if `var_name` is set in the process environment to `1`, `true`, `yes`, or `on`.
#[must_use]
pub fn env_truthy(var_name: &str) -> bool {
    std::env::var(var_name)
        .ok()
        .and_then(|v| parse_bool_str(&v))
        == Some(true)
}

/// Returns `true` if `var_name` is set in the process environment to `0`, `false`, `no`, or `off`.
#[must_use]
pub fn env_falsy(var_name: &str) -> bool {
    std::env::var(var_name)
        .ok()
        .and_then(|v| parse_bool_str(&v))
        == Some(false)
}

/// Returns `true` when verbose LLM context logging is enabled (`--debug-context` or `BOXXY_DEBUG_CONTEXT=1`).
#[must_use]
pub fn debug_context() -> bool {
    crate::dev_mode::cli_debug_context() || env_truthy("BOXXY_DEBUG_CONTEXT")
}

/// Returns `true` if a `BOXXY_SHELL_INTEGRATION` value explicitly disables shell shims.
#[must_use]
pub fn is_shell_integration_disabled_value(raw: &str) -> bool {
    parse_bool_str(raw) == Some(false)
}

/// Returns `true` if shell integration is enabled in the current environment (default `true`).
#[must_use]
pub fn shell_integration_enabled() -> bool {
    !env_falsy("BOXXY_SHELL_INTEGRATION")
}

fn non_empty_trimmed_env(var_name: &str) -> Option<String> {
    let val = std::env::var(var_name).ok()?;
    let trimmed = val.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Returns the raw `BOXXY_SIMULATE_UPDATE` string if set and non-empty.
#[must_use]
pub fn simulate_update_raw() -> Option<String> {
    non_empty_trimmed_env("BOXXY_SIMULATE_UPDATE")
}

/// Returns the custom `BOXXY_TELEMETRY_ENDPOINT` URL if set and non-empty.
#[must_use]
pub fn telemetry_endpoint() -> Option<String> {
    non_empty_trimmed_env("BOXXY_TELEMETRY_ENDPOINT")
}

/// Masks a secret value for safe CLI display (`"sk-abc...xyz"` -> `"sk-a•••••••• (32 chars)"`).
///
/// Only reveals a 4-character prefix when the secret has at least 12 characters
/// and contains no control characters; shorter values are fully masked so half of
/// a short password or token is never exposed.
#[must_use]
pub fn mask_secret(val: &str) -> String {
    const MIN_LEN_FOR_PREFIX: usize = 12;

    let trimmed = val.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut prefix = String::with_capacity(4);
    let mut char_count = 0usize;
    let mut has_control = false;
    for ch in trimmed.chars() {
        if char_count < 4 {
            if ch.is_control() {
                has_control = true;
            } else {
                prefix.push(ch);
            }
        }
        char_count += 1;
    }

    if char_count < MIN_LEN_FOR_PREFIX || has_control {
        "••••••••".to_string()
    } else {
        format!("{prefix}•••••••• ({char_count} chars)")
    }
}

/// Yields the names of all `BOXXY_*` variables that must be scrubbed from child PTYs.
pub fn scrubbed_pty_var_names() -> impl Iterator<Item = &'static str> {
    ENV_SPECS
        .iter()
        .filter(|spec| spec.scrub_from_child_pty)
        .map(|spec| spec.name)
}

/// Scrubs Boxxy daemon/dev/internal markers from a child PTY environment vector while
/// preserving explicit `BOXXY_SHELL_INTEGRATION=0` (`false`/`off`) opt-outs.
pub fn scrub_child_pty_env(env: &mut Vec<(String, String)>) {
    env.retain_mut(|(k, v)| {
        // Fast path: every scrubbed marker in `ENV_SPECS` uses the `BOXXY_` namespace
        // (enforced by `test_env_specs_invariants`).
        if !k.starts_with("BOXXY_") {
            return true;
        }
        if k == "BOXXY_SHELL_INTEGRATION" {
            if is_shell_integration_disabled_value(v) {
                // Normalize falsy spellings ("false", "off") to "0" in-place for shell_integration.rs
                if v != "0" {
                    v.clear();
                    v.push('0');
                }
                return true;
            }
            return false;
        }
        !ENV_SPECS
            .iter()
            .any(|spec| spec.scrub_from_child_pty && spec.name == k.as_str())
    });
}

/// Computes the canonical `(ENV_NAME, VALUE)` pairs that `agent_deployer` should forward
/// when spawning `boxxy-agent` (used identically for Flatpak `--env=` and native `cmd.env()`).
#[must_use]
pub fn daemon_spawn_env(expected_config_dir: &Path) -> Vec<(&'static str, String)> {
    let mut out = Vec::with_capacity(6);
    if debug_context() {
        out.push(("BOXXY_DEBUG_CONTEXT", "1".to_string()));
    }
    let (slug, ephemeral) = crate::dev_mode::resolve_instance_and_ephemeral();
    if let Some(slug) = slug {
        out.push(("BOXXY_INSTANCE", slug));
    }
    if ephemeral {
        out.push(("BOXXY_EPHEMERAL", "1".to_string()));
    }
    for spec in ENV_SPECS.iter().filter(|s| s.propagate_raw_to_daemon) {
        if let Some(trimmed) = non_empty_trimmed_env(spec.name) {
            out.push((spec.name, trimmed));
        }
    }
    out.push((
        "BOXXY_CONFIG_DIR",
        expected_config_dir.to_string_lossy().into_owned(),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EnvStateGuard;
    use crate::{CliDevOverrides, configure_dev_options, get_config_dir};
    use serial_test::serial;
    use std::collections::HashSet;

    #[test]
    fn test_env_specs_and_provider_map_invariants() {
        let mut seen = HashSet::with_capacity(ENV_SPECS.len());
        for spec in ENV_SPECS {
            assert_ne!(spec.name, "");
            assert_ne!(spec.value_hint, "");
            assert_ne!(spec.summary, "");
            assert!(
                seen.insert(spec.name),
                "Duplicate ENV_SPECS entry: {}",
                spec.name
            );
            if spec.scrub_from_child_pty {
                assert!(
                    spec.name.starts_with("BOXXY_"),
                    "scrub_child_pty_env fast-path requires scrubbed vars to start with BOXXY_: {}",
                    spec.name
                );
            }
            if spec.category == EnvCategory::ProviderKey {
                assert!(
                    spec.secret,
                    "ProviderKey {} must be marked secret",
                    spec.name
                );
                assert!(
                    !spec.scrub_from_child_pty,
                    "ProviderKey {} must not be scrubbed from user PTYs",
                    spec.name
                );
            }
            if spec.category == EnvCategory::Internal {
                assert!(
                    spec.scrub_from_child_pty,
                    "Internal marker {} must be scrubbed from child PTYs",
                    spec.name
                );
                assert!(spec.cli_flag.is_none());
            }
        }

        // Every PROVIDER_ENV_KEY_MAP variable must exist in ENV_SPECS as a secret ProviderKey
        for &(env_var, provider_name) in PROVIDER_ENV_KEY_MAP {
            assert_ne!(provider_name, "");
            let spec = ENV_SPECS
                .iter()
                .find(|s| s.name == env_var)
                .unwrap_or_else(|| {
                    panic!("PROVIDER_ENV_KEY_MAP entry {env_var} missing from ENV_SPECS")
                });
            assert_eq!(spec.category, EnvCategory::ProviderKey);
            assert!(spec.secret);
        }
    }

    #[test]
    fn test_parse_bool_str_and_mask_secret() {
        for truthy in ["1", "true", "TRUE", " Yes ", "on", "ON"] {
            assert_eq!(parse_bool_str(truthy), Some(true));
        }
        for falsy in ["0", "false", "FALSE", " no ", "Off"] {
            assert_eq!(parse_bool_str(falsy), Some(false));
        }
        assert_eq!(parse_bool_str(""), None);
        assert_eq!(parse_bool_str("maybe"), None);

        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("   "), "");
        // Short secrets (< 12 chars) must never leak their first 4 characters
        assert_eq!(mask_secret("short77"), "••••••••");
        assert_eq!(mask_secret("12345678901"), "••••••••");
        // Control characters in prefix must never be emitted raw
        assert_eq!(mask_secret("sk-\n1234567890123"), "••••••••");
        // Standard-length API keys show safe 4-char prefix + character count
        assert_eq!(
            mask_secret("sk-test-0123456789abcdef"),
            "sk-t•••••••• (24 chars)"
        );
    }

    #[test]
    #[serial(config_dir)]
    fn test_env_registry_daemon_propagation_and_pty_scrubbing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("custom-cfg");
        let mut guard = EnvStateGuard::new(None);

        configure_dev_options(CliDevOverrides {
            dev: true,
            ephemeral: true,
            debug_context: true,
            config_dir: Some(cfg.clone()),
            ..Default::default()
        });
        assert_eq!(get_config_dir(), cfg);
        assert!(debug_context());

        guard.set_extra_var(
            "BOXXY_TELEMETRY_ENDPOINT",
            "https://collector.example.com/v1/metrics",
        );

        let spawned = daemon_spawn_env(&cfg);
        assert!(spawned.contains(&("BOXXY_DEBUG_CONTEXT", "1".to_string())));
        assert!(spawned.contains(&("BOXXY_INSTANCE", "dev".to_string())));
        assert!(spawned.contains(&("BOXXY_EPHEMERAL", "1".to_string())));
        assert!(spawned.contains(&(
            "BOXXY_TELEMETRY_ENDPOINT",
            "https://collector.example.com/v1/metrics".to_string()
        )));
        assert!(spawned.contains(&("BOXXY_CONFIG_DIR", cfg.to_string_lossy().into_owned())));

        let mut pty_env = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("BOXXY_DEV".to_string(), "1".to_string()),
            ("BOXXY_DEBUG_CONTEXT".to_string(), "1".to_string()),
            ("BOXXY_ORIG_ZDOTDIR".to_string(), "/tmp/zdot".to_string()),
            ("BOXXY_SHELL_INTEGRATION".to_string(), "false".to_string()),
            ("TAVILY_API_KEY".to_string(), "tvly-synthetic".to_string()),
        ];
        scrub_child_pty_env(&mut pty_env);
        assert_eq!(
            pty_env,
            vec![
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("BOXXY_SHELL_INTEGRATION".to_string(), "0".to_string()),
                ("TAVILY_API_KEY".to_string(), "tvly-synthetic".to_string()),
            ]
        );
    }
}
