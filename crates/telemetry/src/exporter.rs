use crate::identity::{is_telemetry_enabled, parse_canonical_uuid};
use crate::journal::{JOURNAL_MAX_AGE_MS, JOURNAL_MAX_ROWS, ensure_db, now_epoch_ms, purge_all};
use crate::normalize::{
    normalize_desktop, normalize_model_name, normalize_os, normalize_pkg_type,
    normalize_provider_name, normalize_shell, normalize_theme, normalize_tool_name,
    sanitize_attr_text,
};
use boxxy_db::models::{TelemetryJournalAck, TelemetryJournalEntry};
use parking_lot::RwLock;
use rand::RngExt;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

const DEFAULT_INGEST_ENDPOINT: &str =
    "https://qfwhesnmixgmczkdnvsu.supabase.co/functions/v1/telemetry-ingest";
const SUPABASE_PUBLISHABLE_KEY: &str = "sb_publishable_Z-SbIil88PSj3ri24CM7aw_8vzsCFie";

/// Batch size per HTTP export (comfortably below the Edge Function's 250-datapoint cap).
const EXPORT_BATCH_LIMIT: i64 = 150;
const MAX_EXPORT_ATTEMPTS: u32 = 3;
const MAX_RETRY_AFTER_SECS: u64 = 10;

/// Serializes all `flush_journal()` invocations across periodic timers, client disconnects,
/// daemon reloads, and `SIGTERM`/`SIGINT` so concurrent callers can never export a batch twice.
static FLUSH_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

static EXPORTER_CLIENT: RwLock<Option<reqwest::Client>> = RwLock::new(None);

// --- Standard OTLP/HTTP JSON (`ExportMetricsServiceRequest`) Wire Types ---

#[derive(Serialize)]
struct OtlpExportRequest<'a> {
    #[serde(rename = "resourceMetrics")]
    resource_metrics: Vec<OtlpResourceMetrics<'a>>,
}

#[derive(Serialize)]
struct OtlpResourceMetrics<'a> {
    resource: OtlpResource<'a>,
    #[serde(rename = "scopeMetrics")]
    scope_metrics: Vec<OtlpScopeMetrics<'a>>,
}

#[derive(Serialize)]
struct OtlpResource<'a> {
    attributes: Vec<OtlpKeyValue<'a>>,
}

#[derive(Serialize)]
struct OtlpScopeMetrics<'a> {
    scope: OtlpScope<'a>,
    metrics: Vec<OtlpMetric<'a>>,
}

#[derive(Serialize)]
struct OtlpScope<'a> {
    name: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
struct OtlpMetric<'a> {
    name: &'a str,
    sum: OtlpSum,
}

#[derive(Serialize)]
struct OtlpSum {
    /// `1` = `AGGREGATION_TEMPORALITY_DELTA` in the OTLP protobuf/JSON specification.
    #[serde(rename = "aggregationTemporality")]
    aggregation_temporality: i32,
    #[serde(rename = "isMonotonic")]
    is_monotonic: bool,
    #[serde(rename = "dataPoints")]
    data_points: Vec<OtlpNumberDataPoint>,
}

#[derive(Serialize)]
struct OtlpNumberDataPoint {
    #[serde(rename = "asDouble")]
    as_double: f64,
    #[serde(rename = "timeUnixNano")]
    time_unix_nano: String,
    attributes: Vec<OtlpOwnedKeyValue>,
}

#[derive(Serialize)]
struct OtlpKeyValue<'a> {
    key: &'a str,
    value: OtlpBorrowedAnyValue<'a>,
}

#[derive(Serialize)]
struct OtlpBorrowedAnyValue<'a> {
    #[serde(rename = "stringValue")]
    string_value: &'a str,
}

#[derive(Serialize)]
struct OtlpOwnedKeyValue {
    key: String,
    value: OtlpOwnedAnyValue,
}

#[derive(Serialize)]
#[serde(untagged)]
enum OtlpOwnedAnyValue {
    String {
        #[serde(rename = "stringValue")]
        string_value: String,
    },
    Int {
        #[serde(rename = "intValue")]
        int_value: i64,
    },
}

fn ensure_http_client() -> Option<reqwest::Client> {
    let cached = EXPORTER_CLIENT.read().clone();
    if let Some(existing) = cached {
        return Some(existing);
    }
    match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => {
            *EXPORTER_CLIENT.write() = Some(client.clone());
            Some(client)
        }
        Err(e) => {
            log::error!("Telemetry: failed to construct OTLP HTTP client: {e}");
            None
        }
    }
}

/// Pre-initializes the OTLP HTTP exporter client when telemetry is enabled.
pub(crate) fn init_exporter() {
    if !is_telemetry_enabled() {
        log::debug!("Telemetry is disabled by user.");
        return;
    }
    let _ = ensure_http_client();
}

/// Drops any cached exporter HTTP client when telemetry is disabled.
pub(crate) fn drop_exporter() {
    *EXPORTER_CLIENT.write() = None;
}

fn resolve_endpoint() -> String {
    boxxy_sys_utils::env::telemetry_endpoint()
        .unwrap_or_else(|| DEFAULT_INGEST_ENDPOINT.to_string())
}

fn entry_to_datapoint(entry: &TelemetryJournalEntry) -> Option<OtlpNumberDataPoint> {
    if !entry.value.is_finite() || entry.value <= 0.0 {
        return None;
    }
    let install_id = parse_canonical_uuid(&entry.install_id)?;
    let session_id = parse_canonical_uuid(&entry.session_id)?;
    let event_id =
        parse_canonical_uuid(&entry.event_id).unwrap_or_else(|| Uuid::new_v4().to_string());

    let parsed_attrs =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&entry.attributes_json)
            .ok()?;

    let normalized_provider = parsed_attrs
        .get("model_provider")
        .and_then(serde_json::Value::as_str)
        .map(normalize_provider_name);

    let mut otlp_attrs = Vec::with_capacity(parsed_attrs.len() + 4);
    otlp_attrs.push(OtlpOwnedKeyValue {
        key: "v".to_string(),
        value: OtlpOwnedAnyValue::Int {
            int_value: entry.schema_version.max(1),
        },
    });
    otlp_attrs.push(OtlpOwnedKeyValue {
        key: "event_id".to_string(),
        value: OtlpOwnedAnyValue::String {
            string_value: event_id,
        },
    });
    otlp_attrs.push(OtlpOwnedKeyValue {
        key: "install_id".to_string(),
        value: OtlpOwnedAnyValue::String {
            string_value: install_id,
        },
    });
    otlp_attrs.push(OtlpOwnedKeyValue {
        key: "session_id".to_string(),
        value: OtlpOwnedAnyValue::String {
            string_value: session_id,
        },
    });

    for (k, v) in &parsed_attrs {
        if matches!(k.as_str(), "v" | "event_id" | "install_id" | "session_id") {
            continue;
        }
        let Some(raw_str) = v.as_str() else {
            continue;
        };

        let scrubbed = match k.as_str() {
            "tool_name" => normalize_tool_name(raw_str).to_string(),
            "model_provider" => normalize_provider_name(raw_str).to_string(),
            "model_name" => {
                let prov = normalized_provider.unwrap_or("Custom Endpoints");
                normalize_model_name(prov, raw_str)
            }
            "os" => normalize_os(raw_str),
            "shell" => normalize_shell(raw_str),
            "desktop" => normalize_desktop(raw_str),
            "pkg_type" => normalize_pkg_type(raw_str).to_string(),
            "theme" => normalize_theme(raw_str).to_string(),
            _ => sanitize_attr_text(raw_str),
        };

        otlp_attrs.push(OtlpOwnedKeyValue {
            key: sanitize_attr_text(k),
            value: OtlpOwnedAnyValue::String {
                string_value: scrubbed,
            },
        });
    }

    let ts_ns = i128::from(entry.recorded_at_ms.max(0)).saturating_mul(1_000_000);
    log::debug!(
        "TELEMETRY_DEBUG_JSON: metric={} value={} ts_ms={} attrs={}",
        entry.metric_name,
        entry.value,
        entry.recorded_at_ms,
        entry.attributes_json
    );

    Some(OtlpNumberDataPoint {
        as_double: entry.value,
        time_unix_nano: ts_ns.to_string(),
        attributes: otlp_attrs,
    })
}

/// Converts a slice of SQLite journal entries into a spec-compliant OTLP/HTTP JSON payload,
/// returning `(payload, valid_acks, corrupt_ids_to_drop)`.
pub(crate) fn build_otlp_batch(
    rows: &[TelemetryJournalEntry],
) -> (Option<String>, Vec<(i64, f64)>, Vec<i64>) {
    let mut grouped_points: BTreeMap<&str, Vec<OtlpNumberDataPoint>> = BTreeMap::new();
    let mut valid_acks = Vec::with_capacity(rows.len());
    let mut invalid_ids = Vec::new();

    for entry in rows {
        if let Some(dp) = entry_to_datapoint(entry) {
            grouped_points
                .entry(entry.metric_name.as_str())
                .or_default()
                .push(dp);
            valid_acks.push((entry.id, entry.value));
        } else {
            invalid_ids.push(entry.id);
        }
    }

    if grouped_points.is_empty() {
        return (None, valid_acks, invalid_ids);
    }

    let metrics: Vec<OtlpMetric<'_>> = grouped_points
        .into_iter()
        .map(|(name, data_points)| OtlpMetric {
            name,
            sum: OtlpSum {
                aggregation_temporality: 1,
                is_monotonic: true,
                data_points,
            },
        })
        .collect();

    let req = OtlpExportRequest {
        resource_metrics: vec![OtlpResourceMetrics {
            resource: OtlpResource {
                attributes: vec![
                    OtlpKeyValue {
                        key: "service.name",
                        value: OtlpBorrowedAnyValue {
                            string_value: "boxxy-terminal",
                        },
                    },
                    OtlpKeyValue {
                        key: "service.version",
                        value: OtlpBorrowedAnyValue {
                            string_value: env!("CARGO_PKG_VERSION"),
                        },
                    },
                ],
            },
            scope_metrics: vec![OtlpScopeMetrics {
                scope: OtlpScope {
                    name: "boxxy-terminal",
                    version: env!("CARGO_PKG_VERSION"),
                },
                metrics,
            }],
        }],
    };

    let json_body = serde_json::to_string(&req).ok();
    (json_body, valid_acks, invalid_ids)
}

const fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let val = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs = val.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(secs.min(MAX_RETRY_AFTER_SECS)))
}

async fn post_otlp_with_retry(
    client: &reqwest::Client,
    endpoint: &str,
    body: &str,
) -> Result<(), String> {
    for attempt in 0..MAX_EXPORT_ATTEMPTS {
        if !is_telemetry_enabled() {
            return Err("telemetry disabled during export".to_string());
        }

        let res = client
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("apikey", SUPABASE_PUBLISHABLE_KEY)
            .body(body.to_owned())
            .send()
            .await;

        match res {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) if is_retryable_status(resp.status()) && attempt + 1 < MAX_EXPORT_ATTEMPTS => {
                let retry_delay = parse_retry_after(resp.headers()).unwrap_or_else(|| {
                    let base_ms = 500_u64.saturating_mul(1_u64 << attempt);
                    let jitter_ms = rand::rng().random_range(0_u64..250_u64);
                    Duration::from_millis(base_ms + jitter_ms)
                });
                log::debug!(
                    "Telemetry: transient HTTP {} on attempt {}/{}; retrying in {:?}",
                    resp.status(),
                    attempt + 1,
                    MAX_EXPORT_ATTEMPTS,
                    retry_delay
                );
                tokio::time::sleep(retry_delay).await;
            }
            Ok(resp) => {
                return Err(format!("HTTP {}", resp.status()));
            }
            Err(err) if attempt + 1 < MAX_EXPORT_ATTEMPTS => {
                let base_ms = 500_u64.saturating_mul(1_u64 << attempt);
                let jitter_ms = rand::rng().random_range(0_u64..250_u64);
                tokio::time::sleep(Duration::from_millis(base_ms + jitter_ms)).await;
                log::debug!(
                    "Telemetry: network error on attempt {}/{}: {err}",
                    attempt + 1,
                    MAX_EXPORT_ATTEMPTS
                );
            }
            Err(err) => {
                return Err(err.to_string());
            }
        }
    }
    Err("exhausted export retries".to_string())
}

pub(crate) async fn flush_all() {
    let _flush_guard = FLUSH_LOCK.lock().await;

    if !is_telemetry_enabled() {
        drop_exporter();
        purge_all().await;
        return;
    }

    let Some(db) = ensure_db().await else {
        log::warn!("Telemetry: flush_journal aborted - database unavailable.");
        return;
    };
    let Some(client) = ensure_http_client() else {
        return;
    };

    let store = db.store();
    let cutoff_ms = now_epoch_ms().saturating_sub(JOURNAL_MAX_AGE_MS);
    let _ = store
        .prune_telemetry_journal(cutoff_ms, JOURNAL_MAX_ROWS)
        .await;

    let endpoint = resolve_endpoint();

    loop {
        if !is_telemetry_enabled() {
            drop_exporter();
            purge_all().await;
            return;
        }

        let Ok(rows) = store.fetch_telemetry_batch(EXPORT_BATCH_LIMIT).await else {
            break;
        };
        if rows.is_empty() {
            break;
        }

        let (payload_opt, valid_acks, invalid_ids) = build_otlp_batch(&rows);

        if !invalid_ids.is_empty() {
            let _ = store.delete_telemetry_events(&invalid_ids).await;
        }

        let Some(payload) = payload_opt else {
            continue;
        };

        log::debug!(
            "Telemetry: exporting {} coalesced metrics to {endpoint}...",
            valid_acks.len()
        );

        match post_otlp_with_retry(&client, &endpoint, &payload).await {
            Ok(()) => {
                let replacement_ids: Vec<String> = (0..valid_acks.len())
                    .map(|_| Uuid::new_v4().to_string())
                    .collect();
                let ack_specs: Vec<TelemetryJournalAck<'_>> = valid_acks
                    .iter()
                    .zip(&replacement_ids)
                    .map(|(&(id, exported_value), rep_id)| TelemetryJournalAck {
                        id,
                        exported_value,
                        replacement_event_id: rep_id.as_str(),
                    })
                    .collect();
                let _ = store.ack_telemetry_events(&ack_specs).await;
                log::debug!(
                    "Telemetry: exported and acknowledged {} journal entries.",
                    ack_specs.len()
                );
            }
            Err(err) => {
                log::warn!("Telemetry export failed ({err}); retaining entries in local journal.");
                break;
            }
        }
    }
}
