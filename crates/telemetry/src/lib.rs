#![allow(clippy::redundant_pub_crate)]

mod event;
mod exporter;
mod identity;
mod journal;
mod normalize;

pub use identity::{rotate_session_id, session_id, set_session_id};
pub use normalize::{
    BUILTIN_BOXXY_TOOLS, BUILTIN_PROVIDERS, CUSTOM_ENDPOINT_PROVIDER_NAME, EXTERNAL_MCP_TOOL_NAME,
    REDACTED_MODEL_NAME, normalize_desktop, normalize_model_name, normalize_os, normalize_pkg_type,
    normalize_provider_name, normalize_shell, normalize_theme, normalize_tool_name,
    resolve_theme_telemetry,
};

use event::{
    METRIC_AI_INVOCATIONS, METRIC_AI_LATENCY, METRIC_AI_TOKENS, METRIC_APP_LAUNCH,
    METRIC_SESSION_RESUME, METRIC_TOOL_USE, MetricRecord,
};
use identity::is_telemetry_enabled;
use normalize::sanitize_attr_text;

/// Shares an already-opened [`boxxy_db::Db`] pool with the telemetry journal
/// (avoiding duplicate SQLite connection pools inside `boxxy-agent`).
pub fn init_with_db(db: boxxy_db::Db) {
    journal::set_db(db);
}

/// Lazily initializes the local SQLite telemetry journal connection if telemetry is enabled.
pub async fn init_db() {
    if !is_telemetry_enabled() {
        return;
    }
    let _ = journal::ensure_db().await;
}

/// Initializes the background OTLP HTTP exporter client when telemetry is enabled.
pub fn init() {
    exporter::init_exporter();
}

/// Immediately drops exporter state and purges all unsent rows from `telemetry_journal`
/// when the user opts out (`enable_telemetry = false`).
pub async fn disable_and_purge() {
    exporter::drop_exporter();
    journal::purge_all().await;
}

/// Drains `telemetry_journal` and exports coalesced OTLP/HTTP JSON batches to the collector.
/// Serialized by an internal async mutex so concurrent callers never double-export a batch.
pub async fn flush_journal() {
    exporter::flush_all().await;
}

#[allow(clippy::too_many_arguments)]
pub async fn track_launch(
    os: &str,
    arch: &str,
    pkg_type: &str,
    version: &str,
    shell: &str,
    desktop: &str,
    theme: &str,
) {
    if !is_telemetry_enabled() {
        return;
    }

    let clean_os = normalize_os(os);
    let clean_arch = sanitize_attr_text(arch);
    let clean_pkg = normalize_pkg_type(pkg_type);
    let clean_ver = sanitize_attr_text(version);
    let clean_shell = normalize_shell(shell);
    let clean_desktop = normalize_desktop(desktop);
    let clean_theme = normalize_theme(theme);

    if let Some(rec) = MetricRecord::new(
        METRIC_APP_LAUNCH,
        1.0,
        [
            ("os", clean_os.as_str()),
            ("arch", clean_arch.as_str()),
            ("pkg_type", clean_pkg),
            ("version", clean_ver.as_str()),
            ("shell", clean_shell.as_str()),
            ("desktop", clean_desktop.as_str()),
            ("theme", clean_theme),
        ],
    ) {
        journal::record_metrics(&[rec]).await;
    }
}

/// Records all metrics for a completed AI turn (invocation, latency, input tokens, output tokens)
/// in a single atomic SQLite transaction (4x fewer WAL commits than separate calls).
pub async fn track_ai_turn(
    provider: &str,
    model: &str,
    feature: &str,
    latency_ms: u64,
    input_tokens: u64,
    output_tokens: u64,
) {
    if !is_telemetry_enabled() {
        return;
    }

    let clean_provider = normalize_provider_name(provider);
    let clean_model = normalize_model_name(clean_provider, model);
    let clean_feature = sanitize_attr_text(feature);

    let mut batch = Vec::with_capacity(4);

    if let Some(inv) = MetricRecord::new(
        METRIC_AI_INVOCATIONS,
        1.0,
        [
            ("model_provider", clean_provider),
            ("model_name", clean_model.as_str()),
            ("feature", clean_feature.as_str()),
        ],
    ) {
        batch.push(inv);
    }

    #[allow(clippy::cast_precision_loss)]
    if latency_ms > 0
        && let Some(lat) = MetricRecord::new(
            METRIC_AI_LATENCY,
            latency_ms as f64,
            [
                ("model_provider", clean_provider),
                ("model_name", clean_model.as_str()),
                ("feature", clean_feature.as_str()),
            ],
        )
    {
        batch.push(lat);
    }

    #[allow(clippy::cast_precision_loss)]
    if input_tokens > 0
        && let Some(inp) = MetricRecord::new(
            METRIC_AI_TOKENS,
            input_tokens as f64,
            [
                ("model_provider", clean_provider),
                ("model_name", clean_model.as_str()),
                ("role", "input"),
                ("feature", clean_feature.as_str()),
            ],
        )
    {
        batch.push(inp);
    }

    #[allow(clippy::cast_precision_loss)]
    if output_tokens > 0
        && let Some(out) = MetricRecord::new(
            METRIC_AI_TOKENS,
            output_tokens as f64,
            [
                ("model_provider", clean_provider),
                ("model_name", clean_model.as_str()),
                ("role", "output"),
                ("feature", clean_feature.as_str()),
            ],
        )
    {
        batch.push(out);
    }

    journal::record_metrics(&batch).await;
}

pub async fn track_ai_tokens(model: &str, provider: &str, role: &str, count: u64, feature: &str) {
    if count == 0 || !is_telemetry_enabled() {
        return;
    }
    let clean_provider = normalize_provider_name(provider);
    let clean_model = normalize_model_name(clean_provider, model);
    let clean_role = sanitize_attr_text(role);
    let clean_feature = sanitize_attr_text(feature);

    #[allow(clippy::cast_precision_loss)]
    if let Some(rec) = MetricRecord::new(
        METRIC_AI_TOKENS,
        count as f64,
        [
            ("model_provider", clean_provider),
            ("model_name", clean_model.as_str()),
            ("role", clean_role.as_str()),
            ("feature", clean_feature.as_str()),
        ],
    ) {
        journal::record_metrics(&[rec]).await;
    }
}

pub async fn track_ai_invocation(provider: &str, model: &str, feature: &str) {
    if !is_telemetry_enabled() {
        return;
    }
    let clean_provider = normalize_provider_name(provider);
    let clean_model = normalize_model_name(clean_provider, model);
    let clean_feature = sanitize_attr_text(feature);

    if let Some(rec) = MetricRecord::new(
        METRIC_AI_INVOCATIONS,
        1.0,
        [
            ("model_provider", clean_provider),
            ("model_name", clean_model.as_str()),
            ("feature", clean_feature.as_str()),
        ],
    ) {
        journal::record_metrics(&[rec]).await;
    }
}

pub async fn track_ai_latency(model: &str, provider: &str, ms: u64, feature: &str) {
    if ms == 0 || !is_telemetry_enabled() {
        return;
    }
    let clean_provider = normalize_provider_name(provider);
    let clean_model = normalize_model_name(clean_provider, model);
    let clean_feature = sanitize_attr_text(feature);

    #[allow(clippy::cast_precision_loss)]
    if let Some(rec) = MetricRecord::new(
        METRIC_AI_LATENCY,
        ms as f64,
        [
            ("model_provider", clean_provider),
            ("model_name", clean_model.as_str()),
            ("feature", clean_feature.as_str()),
        ],
    ) {
        journal::record_metrics(&[rec]).await;
    }
}

pub async fn track_tool_use(tool_name: &str) {
    if !is_telemetry_enabled() {
        return;
    }
    let normalized = normalize_tool_name(tool_name);
    if let Some(rec) = MetricRecord::new(METRIC_TOOL_USE, 1.0, [("tool_name", normalized)]) {
        journal::record_metrics(&[rec]).await;
    }
}

pub async fn track_session_resume(session_type: &str) {
    if !is_telemetry_enabled() {
        return;
    }
    let clean_type = sanitize_attr_text(session_type);
    if let Some(rec) = MetricRecord::new(
        METRIC_SESSION_RESUME,
        1.0,
        [("session_type", clean_type.as_str())],
    ) {
        journal::record_metrics(&[rec]).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::TEST_TELEMETRY_OVERRIDE;
    use boxxy_db::Db;
    use boxxy_model_selection::{
        AnthropicModel, ChatGptModel, DeepSeekModel, GeminiModel, GroqModel, MiniMaxModel,
        MistralModel, ModelProvider, MoonshotModel, OpenAiModel, OpenCodeGoModel, OpenCodeZenModel,
        XaiModel, ZaiModel,
    };
    use serial_test::serial;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_builtin_boxxy_tools_is_sorted_and_normalizes_all_tools() {
        assert_eq!(BUILTIN_BOXXY_TOOLS.len(), 46);
        for window in BUILTIN_BOXXY_TOOLS.windows(2) {
            assert!(
                window[0] < window[1],
                "BUILTIN_BOXXY_TOOLS must be strictly sorted: {} >= {}",
                window[0],
                window[1]
            );
        }

        assert_eq!(normalize_tool_name("file_read"), "file_read");
        assert_eq!(normalize_tool_name("sys_shell_exec"), "sys_shell_exec");
        assert_eq!(
            normalize_tool_name("native_web_search"),
            "native_web_search"
        );
        assert_eq!(normalize_tool_name("invite_character"), "invite_character");
        assert_eq!(
            normalize_tool_name("request_assistance"),
            "request_assistance"
        );

        // External MCP server tools must be bucketed as external_mcp_tool
        assert_eq!(
            normalize_tool_name("Flathub__get_recently_updated_apps"),
            EXTERNAL_MCP_TOOL_NAME
        );
        assert_eq!(
            normalize_tool_name("GNOME_MCP__take_screenshot"),
            EXTERNAL_MCP_TOOL_NAME
        );
    }

    #[test]
    fn test_all_model_provider_variants_sync_with_telemetry_allowlist() {
        let variants = [
            ModelProvider::ChatGpt(ChatGptModel::new("gpt-6.2-sol"), None),
            ModelProvider::Gemini(GeminiModel::new("gemini-3.8-flash"), None),
            ModelProvider::Ollama("llama3:8b".to_string()),
            ModelProvider::Anthropic(AnthropicModel::new("claude-sonnet-5-5"), None),
            ModelProvider::OpenAi(OpenAiModel::new("gpt-6-astra"), None),
            ModelProvider::OpenRouter("anthropic/claude-sonnet-5-5".to_string()),
            ModelProvider::DeepSeek(DeepSeekModel::new("deepseek-flash")),
            ModelProvider::Moonshot(MoonshotModel::new("kimi-k3"), None),
            ModelProvider::Xai(XaiModel::new("grok-4.7"), None),
            ModelProvider::Zai(ZaiModel::new("glm-5.3"), None),
            ModelProvider::Mistral(MistralModel::new("mistral-large-latest"), None),
            ModelProvider::MiniMax(MiniMaxModel::new("MiniMax-M3"), None),
            ModelProvider::Groq(GroqModel::new("openai/gpt-oss-120b"), None),
            ModelProvider::OpenCodeZen(OpenCodeZenModel::new("claude-sonnet-5-5"), None),
            ModelProvider::OpenCodeGo(OpenCodeGoModel::new("glm-5.3"), None),
            ModelProvider::OpenAiCompatible {
                provider_name: "Private Internal vLLM".to_string(),
                model_id: "finetuned-internal-70b".to_string(),
                thinking: None,
            },
        ];

        for variant in &variants {
            let expected = variant.telemetry_provider_name();
            let normalized = normalize_provider_name(variant.provider_name());
            assert_eq!(
                normalized, expected,
                "Provider {variant:?} drifted between ModelProvider::telemetry_provider_name ({expected}) and normalize_provider_name ({normalized})"
            );
            if !matches!(variant, ModelProvider::OpenAiCompatible { .. }) {
                assert!(
                    BUILTIN_PROVIDERS.contains(&normalized),
                    "Built-in provider {normalized} missing from BUILTIN_PROVIDERS"
                );
            }
        }

        // Model-name privacy guard: Ollama & Custom Endpoints must redact model names
        assert_eq!(
            normalize_model_name("Ollama", "private-local:7b"),
            REDACTED_MODEL_NAME
        );
        assert_eq!(
            normalize_model_name(CUSTOM_ENDPOINT_PROVIDER_NAME, "company-finetune-v2"),
            REDACTED_MODEL_NAME
        );
        assert_eq!(normalize_model_name("OpenCode Go", "glm-5.3"), "glm-5.3");
        assert_eq!(
            normalize_model_name("Gemini", "gemini-3.8-flash"),
            "gemini-3.8-flash"
        );
    }

    #[test]
    fn test_environment_and_pkg_normalizers() {
        assert_eq!(normalize_os("'Gentoo'"), "Gentoo");
        assert_eq!(normalize_os("\"Fedora Linux\""), "Fedora Linux");
        assert_eq!(normalize_os("CachyOS"), "CachyOS Linux");
        assert_eq!(normalize_os("GNOME"), "Unknown");

        assert_eq!(normalize_shell("/usr/bin/zsh"), "zsh");
        assert_eq!(normalize_shell("/run/current-system/sw/bin/fish"), "fish");
        assert_eq!(normalize_shell("/bin/bash"), "bash");

        assert_eq!(normalize_desktop("ubuntu:GNOME"), "GNOME");
        assert_eq!(normalize_desktop("zorin:GNOME"), "GNOME");
        assert_eq!(normalize_desktop("X-Cinnamon"), "Cinnamon");
        assert_eq!(normalize_desktop("sway:wlroots"), "Sway");

        assert_eq!(normalize_pkg_type("flatpak"), "flatpak");
        assert_eq!(normalize_pkg_type("nix"), "distro");
        assert_eq!(normalize_pkg_type("system-package"), "distro");
        assert_eq!(normalize_pkg_type("distro"), "distro");
        assert_eq!(normalize_pkg_type("native"), "native");

        assert_eq!(
            resolve_theme_telemetry(boxxy_preferences::AppThemeMode::Boxxy, "boxxy-dark"),
            "Boxxy Dark"
        );
        assert_eq!(
            resolve_theme_telemetry(boxxy_preferences::AppThemeMode::Boxxy, "boxxy-light"),
            "Boxxy Light"
        );
        assert_eq!(
            resolve_theme_telemetry(boxxy_preferences::AppThemeMode::Boxxy, "bauhaus"),
            "Boxxy Community"
        );
        assert_eq!(
            resolve_theme_telemetry(boxxy_preferences::AppThemeMode::DefaultGtk, "boxxy-dark"),
            "Adwaita"
        );
    }

    #[test]
    #[serial]
    fn test_session_id_rotation_and_uuid_validation() {
        let initial = session_id();
        assert!(uuid::Uuid::parse_str(&initial).is_ok());

        let rotated = rotate_session_id();
        assert_ne!(initial, rotated);
        assert_eq!(session_id(), rotated);

        // Non-UUID session IDs must be rejected
        set_session_id("not-a-valid-uuid");
        assert_eq!(session_id(), rotated);

        let valid_custom = "12345678-1234-4234-8234-1234567890ab";
        set_session_id(valid_custom);
        assert_eq!(session_id(), valid_custom);
    }

    #[tokio::test]
    #[serial]
    async fn test_coalescing_redaction_opt_out_and_serialized_otlp_export() {
        let db = Db::new_in_memory().await.unwrap();
        init_with_db(db.clone());
        let store = db.store();

        TEST_TELEMETRY_OVERRIDE.store(1, Ordering::SeqCst);
        let _ = rotate_session_id();

        // 1. Repeated tool calls coalesce into a single SQLite row; MCP tools are masked
        track_tool_use("grep_search").await;
        track_tool_use("grep_search").await;
        track_tool_use("Flathub__search_apps").await;

        // 2. AI turn writes invocation + latency + input/output tokens atomically and redacts Ollama model_name
        track_ai_turn("Ollama", "private-local-model:32b", "claw", 450, 120, 80).await;

        let rows = store.fetch_telemetry_batch(100).await.unwrap();
        // Expect: 1 coalesced grep_search (2.0), 1 external_mcp_tool (1.0), 1 invocation, 1 latency, 2 token rows = 6 rows
        assert_eq!(rows.len(), 6);

        let grep_row = rows
            .iter()
            .find(|r| r.attributes_json.contains("grep_search"))
            .unwrap();
        assert!((grep_row.value - 2.0).abs() < f64::EPSILON);

        for ai_row in rows.iter().filter(|r| r.metric_name.starts_with("ai.")) {
            assert!(
                !ai_row.attributes_json.contains("private-local-model"),
                "Private Ollama model leaked into local journal: {}",
                ai_row.attributes_json
            );
            assert!(ai_row.attributes_json.contains(REDACTED_MODEL_NAME));
        }

        // 3. Spin up an in-memory loopback HTTP server that returns 503 once (testing retry)
        //    and then 202 Accepted, counting successful deliveries under concurrent flush_journal().
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let attempt_counter = Arc::new(AtomicUsize::new(0));
        let accepted_counter = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempt_counter.clone();
        let accepted_clone = accepted_counter.clone();

        let server_task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0_u8; 32_768];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req_str = String::from_utf8_lossy(&buf[..n]);
                assert!(req_str.contains("\"resourceMetrics\""));
                assert!(req_str.contains("\"event_id\""));

                let call_idx = attempts_clone.fetch_add(1, Ordering::SeqCst);
                let resp = if call_idx == 0 {
                    "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    accepted_clone.fetch_add(1, Ordering::SeqCst);
                    "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: 21\r\nConnection: close\r\n\r\n{\"status\":\"accepted\"}"
                };
                let _ = socket.write_all(resp.as_bytes()).await;
            }
        });

        let endpoint_url = format!("http://{addr}/v1/metrics");
        unsafe { std::env::set_var("BOXXY_TELEMETRY_ENDPOINT", &endpoint_url) };

        // Concurrent flushes must serialize via FLUSH_LOCK and export the journal strictly ONCE
        tokio::join!(flush_journal(), flush_journal());

        unsafe { std::env::remove_var("BOXXY_TELEMETRY_ENDPOINT") };
        server_task.abort();

        assert_eq!(attempt_counter.load(Ordering::SeqCst), 2);
        assert_eq!(accepted_counter.load(Ordering::SeqCst), 1);
        assert_eq!(store.count_telemetry_events().await.unwrap(), 0);

        // 4. Opt-out compliance: disabled telemetry ignores new events and purges any unsent rows
        track_tool_use("file_read").await;
        assert_eq!(store.count_telemetry_events().await.unwrap(), 1);

        TEST_TELEMETRY_OVERRIDE.store(2, Ordering::SeqCst);
        track_tool_use("file_write").await;
        assert_eq!(store.count_telemetry_events().await.unwrap(), 1);

        flush_journal().await;
        assert_eq!(store.count_telemetry_events().await.unwrap(), 0);

        TEST_TELEMETRY_OVERRIDE.store(0, Ordering::SeqCst);
    }
}
