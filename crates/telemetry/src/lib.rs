use arc_swap::ArcSwap;
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter, MeterProvider as _};
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

static SESSION_ID: LazyLock<ArcSwap<String>> =
    LazyLock::new(|| ArcSwap::from_pointee(Uuid::new_v4().to_string()));
static METRICS_STATE: LazyLock<Mutex<Option<MetricsState>>> = LazyLock::new(|| Mutex::new(None));

static DB: OnceCell<boxxy_db::Db> = OnceCell::const_new();

#[cfg(test)]
static TEST_TELEMETRY_OVERRIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Exhaustive allowlist of default built-in Boxxy tools (sorted alphabetically).
/// Any tool not in this list (e.g., user-configured MCP tools) is grouped under
/// [`EXTERNAL_MCP_TOOL_NAME`] so private MCP server names never leak into telemetry.
pub const BUILTIN_BOXXY_TOOLS: &[&str] = &[
    "abort_agent_task",
    "acquire_lock",
    "activate_skill",
    "await_tasks",
    "cancel_task",
    "close_agent",
    "delegate_task",
    "delegate_task_async",
    "file_delete",
    "file_read",
    "file_write",
    "get_clipboard",
    "get_system_info",
    "glob_search",
    "grep_search",
    "http_fetch",
    "kill_process",
    "list_active_agents",
    "list_directory",
    "list_my_tasks",
    "list_processes",
    "memory_delete",
    "memory_store",
    "native_web_search",
    "orchestrate_agent",
    "publish_custom_event",
    "read_pane_buffer",
    "read_scrollback_page",
    "release_lock",
    "replace_in_file",
    "restore_background_shell",
    "run_background_command",
    "schedule_task",
    "send_command_to_pane",
    "send_keystrokes_to_pane",
    "set_agent_state",
    "set_clipboard",
    "set_global_intent",
    "spawn_agent",
    "subscribe_to_pane",
    "summon_headless_worker",
    "sys_shell_exec",
    "terminal_exec",
    "web_search",
];

pub const EXTERNAL_MCP_TOOL_NAME: &str = "external_mcp_tool";

struct MetricsState {
    provider: SdkMeterProvider,
    meter: Meter,
    counters: HashMap<String, Counter<f64>>,
    install_id: String,
}

impl MetricsState {
    fn get_counter(&mut self, name: &str) -> &Counter<f64> {
        self.counters
            .entry(name.to_string())
            .or_insert_with(|| self.meter.f64_counter(name.to_string()).build())
    }
}

/// Fast lock-free check of whether telemetry is currently enabled in user preferences.
#[inline]
fn is_telemetry_enabled() -> bool {
    #[cfg(test)]
    {
        match TEST_TELEMETRY_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
            1 => return true,
            2 => return false,
            _ => {}
        }
    }
    boxxy_preferences::Settings::load().enable_telemetry
}

/// Returns the active telemetry `session_id`.
#[must_use]
pub fn session_id() -> String {
    (**SESSION_ID.load()).clone()
}

/// Sets the active telemetry `session_id` (used to synchronize the UI process with `boxxy-agent`).
pub fn set_session_id(id: &str) {
    let trimmed = id.trim();
    if !trimmed.is_empty() {
        SESSION_ID.store(Arc::new(trimmed.to_string()));
    }
}

/// Rotates to a fresh `session_id` (called by `boxxy-agent` when a new UI session attaches)
/// and returns the new UUID string.
#[must_use]
pub fn rotate_session_id() -> String {
    let fresh = Uuid::new_v4().to_string();
    SESSION_ID.store(Arc::new(fresh.clone()));
    fresh
}

/// Normalizes a tool name to either a known built-in Boxxy tool or `"external_mcp_tool"`.
#[must_use]
pub fn normalize_tool_name(tool_name: &str) -> &'static str {
    let trimmed = tool_name.trim();
    if let Ok(idx) = BUILTIN_BOXXY_TOOLS.binary_search(&trimmed) {
        BUILTIN_BOXXY_TOOLS[idx]
    } else {
        EXTERNAL_MCP_TOOL_NAME
    }
}

/// Normalizes an OS name string from `/etc/os-release` or `sysinfo`, stripping quotes.
#[must_use]
pub fn normalize_os(raw: &str) -> String {
    let cleaned = raw.trim().trim_matches(['"', '\'']).trim();
    if cleaned.is_empty() || cleaned.eq_ignore_ascii_case("gnome") {
        "Unknown".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Normalizes a shell path (e.g. `/usr/bin/zsh`, `/run/current-system/sw/bin/fish`) to its binary basename (`zsh`, `fish`).
#[must_use]
pub fn normalize_shell(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed).trim();
    if base.is_empty() {
        "unknown".to_string()
    } else {
        base.to_string()
    }
}

/// Normalizes `XDG_CURRENT_DESKTOP` (e.g. `ubuntu:GNOME`, `zorin:GNOME`, `X-Cinnamon`, `sway:wlroots`).
#[must_use]
pub fn normalize_desktop(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        return "unknown".to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("gnome") {
        "GNOME".to_string()
    } else if lower.contains("kde") || lower.contains("plasma") {
        "KDE".to_string()
    } else if lower.contains("cosmic") {
        "COSMIC".to_string()
    } else if lower.contains("hyprland") {
        "Hyprland".to_string()
    } else if lower.contains("cinnamon") {
        "Cinnamon".to_string()
    } else if lower.contains("sway") {
        "Sway".to_string()
    } else if lower.contains("xfce") {
        "XFCE".to_string()
    } else if lower.contains("mate") {
        "MATE".to_string()
    } else if lower.contains("lxqt") {
        "LXQt".to_string()
    } else if lower.contains("budgie") {
        "Budgie".to_string()
    } else if lower.contains("niri") {
        "Niri".to_string()
    } else {
        trimmed
            .split(':')
            .find(|s| !s.trim().is_empty())
            .unwrap_or(trimmed)
            .trim()
            .to_string()
    }
}

/// Initialize the telemetry database connection.
/// This allows the telemetry crate to write to the local journal.
pub async fn init_db() {
    if !is_telemetry_enabled() {
        return;
    }

    if DB.get().is_none()
        && let Ok(db) = boxxy_db::Db::new().await
    {
        let _ = DB.set(db);
    }
}

pub async fn init() {
    if !is_telemetry_enabled() {
        log::debug!("Telemetry is disabled by user.");
        return;
    }

    {
        let state_guard = METRICS_STATE.lock().await;
        if state_guard.is_some() {
            return;
        }
    }

    let settings = boxxy_preferences::Settings::load();
    let install_id = settings
        .install_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let endpoint = std::env::var("BOXXY_TELEMETRY_ENDPOINT").unwrap_or_else(|_| {
        "https://qfwhesnmixgmczkdnvsu.supabase.co/functions/v1/telemetry-ingest".to_string()
    });
    let api_key = "sb_publishable_Z-SbIil88PSj3ri24CM7aw_8vzsCFie";

    let exporter = match opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpJson)
        .with_endpoint(endpoint)
        .with_headers(std::collections::HashMap::from([(
            "apikey".to_string(),
            api_key.to_string(),
        )]))
        .with_temporality(Temporality::Delta)
        .build()
    {
        Ok(exporter) => exporter,
        Err(e) => {
            log::error!("Failed to create OTLP metric exporter: {e}");
            return;
        }
    };

    // Export is driven explicitly by `flush_journal()` every 30 minutes and on app quit;
    // keep the internal reader interval at 30 minutes to match.
    let reader = PeriodicReader::builder(exporter)
        .with_interval(std::time::Duration::from_secs(30 * 60))
        .build();

    let provider = SdkMeterProvider::builder()
        .with_resource(
            Resource::builder_empty()
                .with_attributes(vec![
                    KeyValue::new("service.name", "boxxy-terminal"),
                    KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                ])
                .build(),
        )
        .with_reader(reader)
        .build();

    let meter = provider.meter("boxxy-terminal");

    let mut state = METRICS_STATE.lock().await;
    *state = Some(MetricsState {
        provider,
        meter,
        counters: HashMap::new(),
        install_id: install_id.clone(),
    });

    log::debug!("Telemetry initialized with install_id: {install_id}");
}

/// Immediately disables in-memory telemetry state and purges all unsent rows from
/// `telemetry_journal` when the user opts out (`enable_telemetry = false`).
pub async fn disable_and_purge() {
    let removed_state = {
        let mut state_lock = METRICS_STATE.lock().await;
        state_lock.take()
    };

    if let Some(state) = removed_state {
        log::info!("Telemetry disabled by user; dropping OTel provider without flushing.");
        // Drop on a blocking thread in case PeriodicReader teardown joins its background thread.
        let _ = tokio::task::spawn_blocking(move || {
            drop(state);
        })
        .await;
    }

    let db_opt = if let Some(db) = DB.get().cloned() {
        Some(db)
    } else {
        boxxy_db::Db::new().await.ok()
    };

    if let Some(db) = db_opt {
        let _ = sqlx::query("DELETE FROM telemetry_journal")
            .execute(db.pool())
            .await;
        log::debug!("Purged local telemetry_journal table.");
    }
}

pub async fn shutdown() {
    let removed_state = {
        let mut state_lock = METRICS_STATE.lock().await;
        state_lock.take()
    };
    if let Some(state) = removed_state {
        if is_telemetry_enabled() {
            log::debug!("Shutting down telemetry and flushing metrics...");
            let _ = tokio::task::spawn_blocking(move || state.provider.shutdown()).await;
        } else {
            let _ = tokio::task::spawn_blocking(move || drop(state)).await;
        }
    }
}

pub async fn seed_dream() {
    if let Some(db) = DB.get() {
        let store = boxxy_db::store::Store::new(db.pool());
        let _ = store.seed_all_raw_interactions().await;
    }
}

pub async fn flush_journal() {
    if !is_telemetry_enabled() {
        disable_and_purge().await;
        return;
    }

    log::debug!("Telemetry: Starting flush_journal...");
    if DB.get().is_none() {
        init_db().await;
    }
    let Some(db) = DB.get() else {
        log::warn!("Telemetry: flush_journal aborted - Database not initialized (DB is None).");
        return;
    };

    if METRICS_STATE.lock().await.is_none() {
        init().await;
    }

    // Prune any stale events older than 7 days so offline machines never grow unboundedly.
    let _ = sqlx::query(
        "DELETE FROM telemetry_journal WHERE created_at < datetime('now', '-7 days')",
    )
    .execute(db.pool())
    .await;

    loop {
        if !is_telemetry_enabled() {
            disable_and_purge().await;
            return;
        }

        // 1. Fetch up to 100 pending events
        let rows: Vec<(i32, String, f64, String)> = match sqlx::query_as(
            "SELECT id, metric_name, value, attributes_json FROM telemetry_journal ORDER BY created_at ASC, id ASC LIMIT 100",
        )
        .fetch_all(db.pool())
        .await
        {
            Ok(rows) => rows,
            Err(_) => break,
        };

        if rows.is_empty() {
            break;
        }

        log::debug!("Telemetry: Processing {} events from journal...", rows.len());

        let mut processed_ids = Vec::with_capacity(rows.len());

        // 2. Record points into OTel counters and clone the provider so we can
        // release `METRICS_STATE` before the blocking network export.
        let provider_for_flush = {
            let mut state_lock = METRICS_STATE.lock().await;
            let Some(state) = &mut *state_lock else {
                log::warn!(
                    "Telemetry: flush_journal aborted - OTel state not initialized (METRICS_STATE is None)."
                );
                return;
            };

            for (id, name, value, attr_json) in &rows {
                if !value.is_finite() || *value < 0.0 {
                    processed_ids.push(*id);
                    continue;
                }

                let Ok(attrs_val) = serde_json::from_str::<serde_json::Value>(attr_json) else {
                    processed_ids.push(*id);
                    continue;
                };

                let mut otel_attrs = Vec::new();
                if let Some(obj) = attrs_val.as_object() {
                    for (k, v) in obj {
                        let val = match v {
                            serde_json::Value::String(s) => {
                                if k == "tool_name" {
                                    opentelemetry::Value::String(normalize_tool_name(s).into())
                                } else {
                                    opentelemetry::Value::String(s.clone().into())
                                }
                            }
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    opentelemetry::Value::I64(i)
                                } else {
                                    opentelemetry::Value::F64(n.as_f64().unwrap_or(0.0))
                                }
                            }
                            serde_json::Value::Bool(b) => opentelemetry::Value::Bool(*b),
                            _ => opentelemetry::Value::String(v.to_string().into()),
                        };
                        otel_attrs.push(KeyValue::new(k.clone(), val));
                    }
                }

                let counter = state.get_counter(name);
                log::debug!(
                    "TELEMETRY_DEBUG_JSON: metric={name} value={value} attrs={attr_json}"
                );
                counter.add(*value, &otel_attrs);
                processed_ids.push(*id);
            }

            state.provider.clone()
        };

        // 3. Force OTel flush to the network on a blocking thread so we never stall Tokio workers.
        log::debug!("Telemetry: Flushed to OTel SDK, forcing network export...");
        let flush_res = tokio::task::spawn_blocking(move || provider_for_flush.force_flush()).await;
        match flush_res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                log::error!("Telemetry network flush failed: {e:?}. Data stays in journal.");
                break;
            }
            Err(join_err) => {
                log::error!("Telemetry flush task panicked: {join_err:?}. Data stays in journal.");
                break;
            }
        }

        // 4. Only delete from journal if flush succeeded
        if !processed_ids.is_empty() {
            let mut builder =
                sqlx::QueryBuilder::new("DELETE FROM telemetry_journal WHERE id IN (");
            let mut separated = builder.separated(", ");
            for id in &processed_ids {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            let _ = builder.build().execute(db.pool()).await;
            log::debug!(
                "Telemetry: Successfully exported and cleared {} events.",
                processed_ids.len()
            );
        }
    }
}

async fn record_to_journal(name: &str, value: f64, attributes: serde_json::Value) {
    if !is_telemetry_enabled() || !value.is_finite() || value < 0.0 {
        return;
    }

    if DB.get().is_none() {
        init_db().await;
    }

    if let Some(db) = DB.get() {
        let attr_str = attributes.to_string();
        let _ = sqlx::query(
            "INSERT INTO telemetry_journal (metric_name, value, attributes_json) VALUES (?, ?, ?)",
        )
        .bind(name)
        .bind(value)
        .bind(attr_str)
        .execute(db.pool())
        .await;
    }
}

pub async fn track_event(name: &str, value: f64, attributes: Vec<KeyValue>) {
    if !is_telemetry_enabled() {
        return;
    }

    let install_id = {
        let state_lock = METRICS_STATE.lock().await;
        if let Some(state) = &*state_lock {
            state.install_id.clone()
        } else {
            drop(state_lock);
            let settings = boxxy_preferences::Settings::load();
            settings
                .install_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        }
    };

    // Prepare attributes for both Journal and OTel
    let mut journal_attrs = json!({
        "install_id": install_id,
        "session_id": session_id(),
        "v": 1
    });
    for attr in &attributes {
        journal_attrs[attr.key.as_str()] = json!(attr.value.to_string());
    }

    // We strictly write to SQLite here. The background boxxy-agent process
    // will drain this table and push it to OTel safely in the background.
    record_to_journal(name, value, journal_attrs).await;
}

pub async fn track_launch(
    os: &str,
    arch: &str,
    pkg_type: &str,
    version: &str,
    shell: &str,
    desktop: &str,
) {
    let clean_os = normalize_os(os);
    let clean_shell = normalize_shell(shell);
    let clean_desktop = normalize_desktop(desktop);

    track_event(
        "app.launch",
        1.0,
        vec![
            KeyValue::new("os", clean_os),
            KeyValue::new("arch", arch.trim().to_string()),
            KeyValue::new("pkg_type", pkg_type.trim().to_string()),
            KeyValue::new("version", version.trim().to_string()),
            KeyValue::new("shell", clean_shell),
            KeyValue::new("desktop", clean_desktop),
        ],
    )
    .await;
}

pub async fn track_ai_tokens(model: &str, provider: &str, role: &str, count: u64, feature: &str) {
    if count == 0 {
        return;
    }
    track_event(
        "ai.tokens",
        count as f64,
        vec![
            KeyValue::new("model_provider", provider.to_string()),
            KeyValue::new("model_name", model.to_string()),
            KeyValue::new("role", role.to_string()),
            KeyValue::new("feature", feature.to_string()),
        ],
    )
    .await;
}

pub async fn track_ai_invocation(provider: &str, model: &str, feature: &str) {
    track_event(
        "ai.invocations",
        1.0,
        vec![
            KeyValue::new("model_provider", provider.to_string()),
            KeyValue::new("model_name", model.to_string()),
            KeyValue::new("feature", feature.to_string()),
        ],
    )
    .await;
}

pub async fn track_ai_latency(model: &str, provider: &str, ms: u64, feature: &str) {
    track_event(
        "ai.latency",
        ms as f64,
        vec![
            KeyValue::new("model_provider", provider.to_string()),
            KeyValue::new("model_name", model.to_string()),
            KeyValue::new("feature", feature.to_string()),
        ],
    )
    .await;
}

pub async fn track_tool_use(tool_name: &str) {
    let normalized = normalize_tool_name(tool_name);
    track_event(
        "tool.use",
        1.0,
        vec![KeyValue::new("tool_name", normalized.to_string())],
    )
    .await;
}

pub async fn track_session_resume(session_type: &str) {
    track_event(
        "claw.session_resume",
        1.0,
        vec![KeyValue::new("session_type", session_type.to_string())],
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_db::Db;
    use serial_test::serial;

    #[test]
    fn test_builtin_boxxy_tools_is_sorted_and_normalizes_mcp_tools() {
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
        // External MCP server tools must be bucketed as external_mcp_tool
        assert_eq!(
            normalize_tool_name("Flathub__get_recently_updated_apps"),
            EXTERNAL_MCP_TOOL_NAME
        );
        assert_eq!(
            normalize_tool_name("Flathub__search_apps"),
            EXTERNAL_MCP_TOOL_NAME
        );
        assert_eq!(
            normalize_tool_name("GNOME_MCP__take_screenshot"),
            EXTERNAL_MCP_TOOL_NAME
        );
    }

    #[test]
    fn test_environment_normalizers() {
        assert_eq!(normalize_os("'Gentoo'"), "Gentoo");
        assert_eq!(normalize_os("\"Fedora Linux\""), "Fedora Linux");
        assert_eq!(normalize_os("GNOME"), "Unknown");

        assert_eq!(normalize_shell("/usr/bin/zsh"), "zsh");
        assert_eq!(
            normalize_shell("/run/current-system/sw/bin/fish"),
            "fish"
        );
        assert_eq!(normalize_shell("/bin/bash"), "bash");

        assert_eq!(normalize_desktop("ubuntu:GNOME"), "GNOME");
        assert_eq!(normalize_desktop("zorin:GNOME"), "GNOME");
        assert_eq!(normalize_desktop("X-Cinnamon"), "Cinnamon");
        assert_eq!(normalize_desktop("sway:wlroots"), "Sway");
    }

    #[test]
    #[serial]
    fn test_session_id_rotation_and_sync() {
        let initial = session_id();
        let rotated = rotate_session_id();
        assert_ne!(initial, rotated);
        assert_eq!(session_id(), rotated);

        set_session_id("custom-session-uuid");
        assert_eq!(session_id(), "custom-session-uuid");
    }

    #[tokio::test]
    #[serial]
    async fn test_telemetry_journal_operations_and_opt_out_purge() {
        let db = Db::new_in_memory().await.unwrap();
        let _ = DB.set(db.clone());
        // Clear any leftover rows from shared OnceCell in test process
        sqlx::query("DELETE FROM telemetry_journal")
            .execute(db.pool())
            .await
            .unwrap();

        TEST_TELEMETRY_OVERRIDE.store(1, std::sync::atomic::Ordering::SeqCst);

        // 1. Record direct events and tool events (built-in + external MCP)
        record_to_journal("test.metric.1", 42.0, serde_json::json!({"key": "value"})).await;
        track_tool_use("grep_search").await;
        track_tool_use("Flathub__search_apps").await;

        // 2. Verify they are in the database and MCP tool was normalized
        let rows: Vec<(i32, String, f64, String)> = sqlx::query_as(
            "SELECT id, metric_name, value, attributes_json FROM telemetry_journal ORDER BY id ASC",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].1, "test.metric.1");
        assert_eq!(rows[0].2, 42.0);
        assert_eq!(rows[1].1, "tool.use");
        let parsed_builtin: serde_json::Value = serde_json::from_str(&rows[1].3).unwrap();
        assert_eq!(parsed_builtin["tool_name"], "grep_search");

        let parsed_mcp: serde_json::Value = serde_json::from_str(&rows[2].3).unwrap();
        assert_eq!(parsed_mcp["tool_name"], EXTERNAL_MCP_TOOL_NAME);

        // 3. Turn off telemetry -> verify new events are ignored 100%
        TEST_TELEMETRY_OVERRIDE.store(2, std::sync::atomic::Ordering::SeqCst);
        track_tool_use("file_read").await;
        record_to_journal("test.metric.ignored", 1.0, serde_json::json!({})).await;

        let count_before_purge: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM telemetry_journal")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count_before_purge.0, 3);

        // 4. Calling disable_and_purge() or flush_journal() while disabled purges the journal
        flush_journal().await;

        let remaining: Vec<(i32,)> = sqlx::query_as("SELECT id FROM telemetry_journal")
            .fetch_all(db.pool())
            .await
            .unwrap();
        assert!(remaining.is_empty());

        TEST_TELEMETRY_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}
