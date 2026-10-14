use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter, MeterProvider as _};
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use serde_json::json;
use std::collections::HashMap;
use std::sync::LazyLock;
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

static SESSION_ID: LazyLock<String> = LazyLock::new(|| Uuid::new_v4().to_string());
static METRICS_STATE: LazyLock<Mutex<Option<MetricsState>>> = LazyLock::new(|| Mutex::new(None));

static DB: OnceCell<boxxy_db::Db> = OnceCell::const_new();

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

/// Initialize the telemetry database connection.
/// This allows the telemetry crate to write to the local journal.
pub async fn init_db() {
    let settings = boxxy_preferences::Settings::load();
    if !settings.enable_telemetry {
        return;
    }

    if DB.get().is_none() {
        if let Ok(db) = boxxy_db::Db::new().await {
            let _ = DB.set(db);
        }
    }
}

pub async fn init() {
    let settings = boxxy_preferences::Settings::load();
    if !settings.enable_telemetry {
        log::debug!("Telemetry is disabled by user.");
        return;
    }

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

    let reader = PeriodicReader::builder(exporter)
        .with_interval(std::time::Duration::from_secs(600)) // 10 minutes
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

    log::debug!("Telemetry initialized with install_id: {}", install_id);
}

pub async fn shutdown() {
    let mut state_lock = METRICS_STATE.lock().await;
    if let Some(state) = state_lock.take() {
        log::debug!("Shutting down telemetry and flushing metrics...");
        // This will flush all readers
        let _ = state.provider.shutdown();
    }
}

pub async fn seed_dream() {
    if let Some(db) = DB.get() {
        let store = boxxy_db::store::Store::new(db.pool());
        let _ = store.seed_all_raw_interactions().await;
    }
}

pub async fn flush_journal() {
    log::debug!("Telemetry: Starting flush_journal...");
    let db = match DB.get() {
        Some(db) => db,
        None => {
            log::warn!("Telemetry: flush_journal aborted - Database not initialized (DB is None).");
            return;
        }
    };

    let mut state_lock = METRICS_STATE.lock().await;
    let state = match &mut *state_lock {
        Some(state) => state,
        None => {
            log::warn!(
                "Telemetry: flush_journal aborted - OTel state not initialized (METRICS_STATE is None)."
            );
            return;
        }
    };

    loop {
        // 1. Fetch up to 100 pending events
        let rows: Vec<(i32, String, f64, String)> = match sqlx::query_as(
            "SELECT id, metric_name, value, attributes_json FROM telemetry_journal ORDER BY created_at ASC LIMIT 100"
        )
        .fetch_all(db.pool())
        .await {
            Ok(rows) => rows,
            Err(_) => break,
        };

        if rows.is_empty() {
            break;
        }

        log::debug!(
            "Telemetry: Processing {} events from journal...",
            rows.len()
        );

        let mut processed_ids = Vec::new();

        for (id, name, value, attr_json) in &rows {
            let attrs_val: serde_json::Value = match serde_json::from_str(attr_json) {
                Ok(v) => v,
                Err(_) => {
                    processed_ids.push(*id);
                    continue;
                }
            };

            // Convert JSON attributes back to OTel KeyValues
            let mut otel_attrs = Vec::new();
            if let Some(obj) = attrs_val.as_object() {
                for (k, v) in obj {
                    let val = match v {
                        serde_json::Value::String(s) => {
                            opentelemetry::Value::String(s.clone().into())
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

            // DEBUG: Print the exact data being handed to OTel
            log::debug!(
                "TELEMETRY_DEBUG_JSON: metric={} value={} attrs={}",
                name,
                value,
                attr_json
            );

            counter.add(*value, &otel_attrs);
            processed_ids.push(*id);
        }

        // 2. Force OTel flush to the network
        log::debug!("Telemetry: Flushed to OTel SDK, forcing network export...");
        if let Err(e) = state.provider.force_flush() {
            log::error!(
                "Telemetry network flush failed: {:?}. Data stays in journal.",
                e
            );
            break; // Network failed, stop looping and keep remaining data for next attempt
        }

        // 3. Only delete from journal if flush succeeded
        if !processed_ids.is_empty() {
            let mut builder = sqlx::QueryBuilder::new("DELETE FROM telemetry_journal WHERE id IN (");
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
    let state_lock = METRICS_STATE.lock().await;

    let install_id = if let Some(state) = &*state_lock {
        state.install_id.clone()
    } else {
        let settings = boxxy_preferences::Settings::load();
        settings
            .install_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    };

    // Prepare attributes for both Journal and OTel
    let mut journal_attrs = json!({
        "install_id": install_id,
        "session_id": *SESSION_ID,
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
    track_event(
        "app.launch",
        1.0,
        vec![
            KeyValue::new("os", os.to_string()),
            KeyValue::new("arch", arch.to_string()),
            KeyValue::new("pkg_type", pkg_type.to_string()),
            KeyValue::new("version", version.to_string()),
            KeyValue::new("shell", shell.to_string()),
            KeyValue::new("desktop", desktop.to_string()),
        ],
    )
    .await;
}

pub async fn track_ai_tokens(model: &str, provider: &str, role: &str, count: u64, feature: &str) {
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
    track_event(
        "tool.use",
        1.0,
        vec![KeyValue::new("tool_name", tool_name.to_string())],
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

    #[tokio::test]
    async fn test_telemetry_journal_operations() {
        // Set up in-memory DB
        let db = Db::new_in_memory().await.unwrap();
        let _ = DB.set(db.clone());

        // 1. Record direct events
        record_to_journal("test.metric.1", 42.0, serde_json::json!({"key": "value"})).await;
        record_to_journal("test.metric.2", 1.5, serde_json::json!({"foo": "bar"})).await;

        // 2. Track high-level tool event
        track_tool_use("grep_search").await;

        // 3. Verify they are in the database directly
        let rows: Vec<(i32, String, f64, String)> = sqlx::query_as(
            "SELECT id, metric_name, value, attributes_json FROM telemetry_journal ORDER BY id ASC",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].1, "test.metric.1");
        assert_eq!(rows[0].2, 42.0);
        assert_eq!(rows[1].1, "test.metric.2");
        assert_eq!(rows[1].2, 1.5);
        assert_eq!(rows[2].1, "tool.use");
        assert_eq!(rows[2].2, 1.0);

        let parsed: serde_json::Value = serde_json::from_str(&rows[2].3).unwrap();
        assert_eq!(parsed["tool_name"], "grep_search");
        assert_eq!(parsed["v"], 1);
        assert!(parsed.get("session_id").is_some());

        // 4. Test parameterized batch deletion
        let ids_to_delete: Vec<i32> = rows.iter().map(|r| r.0).collect();
        let mut builder = sqlx::QueryBuilder::new("DELETE FROM telemetry_journal WHERE id IN (");
        let mut separated = builder.separated(", ");
        for id in &ids_to_delete {
            separated.push_bind(id);
        }
        separated.push_unseparated(")");
        builder.build().execute(db.pool()).await.unwrap();

        let remaining: Vec<(i32,)> =
            sqlx::query_as("SELECT id FROM telemetry_journal")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert!(remaining.is_empty());
    }
}
