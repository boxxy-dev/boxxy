use lazy_static::lazy_static;
use opentelemetry::metrics::{Meter, MeterProvider as _};
use opentelemetry::KeyValue;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig, Protocol};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use opentelemetry_sdk::Resource;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

lazy_static! {
    static ref SESSION_ID: String = Uuid::new_v4().to_string();
    static ref METRICS_STATE: Arc<Mutex<Option<MetricsState>>> = Arc::new(Mutex::new(None));
}

struct MetricsState {
    provider: SdkMeterProvider,
    meter: Meter,
    install_id: String,
}

pub async fn init() {
    let settings = boxxy_preferences::Settings::load();
    if !settings.enable_telemetry {
        log::info!("Telemetry is disabled by user.");
        return;
    }

    let install_id = settings.install_id.clone().unwrap_or_else(|| "unknown".to_string());

    let endpoint = "https://qfwhesnmixgmczkdnvsu.supabase.co/functions/v1/telemetry-ingest";
    let api_key = "sb_publishable_Z-SbIil88PSj3ri24CM7aw_8vzsCFie";

    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpJson)
        .with_endpoint(endpoint)
        .with_headers(std::collections::HashMap::from([
            ("apikey".to_string(), api_key.to_string()),
            ("Authorization".to_string(), format!("Bearer {}", api_key)),
        ]))
        .with_temporality(Temporality::Delta)
        .build()
        .expect("Failed to create OTLP exporter");

    let reader = PeriodicReader::builder(exporter)
        .with_interval(std::time::Duration::from_secs(600)) // 10 minutes
        .build();
    
    let provider = SdkMeterProvider::builder()
        .with_resource(Resource::builder_empty().with_attributes(vec![KeyValue::new("service.name", "boxxy-terminal")]).build())
        .with_reader(reader)
        .build();
    
    let meter = provider.meter("boxxy-terminal");

    let mut state = METRICS_STATE.lock().await;
    *state = Some(MetricsState {
        provider,
        meter,
        install_id: install_id.clone(),
    });

    log::info!("Telemetry initialized with install_id: {}", install_id);
}

pub async fn shutdown() {
    let mut state_lock = METRICS_STATE.lock().await;
    if let Some(state) = state_lock.take() {
        log::info!("Shutting down telemetry and flushing metrics...");
        // This will flush all readers
        let _ = state.provider.shutdown();
    }
}

pub async fn track_event(name: &str, value: f64, attributes: Vec<KeyValue>) {
    let state_lock = METRICS_STATE.lock().await;
    if let Some(state) = &*state_lock {
        let mut all_attrs = vec![
            KeyValue::new("install_id", state.install_id.clone()),
            KeyValue::new("session_id", SESSION_ID.clone()),
            KeyValue::new("v", 1i64),
        ];
        all_attrs.extend(attributes);

        log::debug!("Telemetry Event: {} = {} ({:?})", name, value, all_attrs);

        // OTel Metrics are usually Counters or Histograms.
        // For simple event tracking, we'll use a Counter.
        let counter = state.meter.f64_counter(name.to_string()).build();
        counter.add(value, &all_attrs);
    }
}

pub async fn track_launch(os: &str, arch: &str, pkg_type: &str, version: &str, shell: &str) {
    track_event(
        "app.launch",
        1.0,
        vec![
            KeyValue::new("os", os.to_string()),
            KeyValue::new("arch", arch.to_string()),
            KeyValue::new("pkg_type", pkg_type.to_string()),
            KeyValue::new("version", version.to_string()),
            KeyValue::new("shell", shell.to_string()),
        ],
    ).await;
}

pub async fn track_ai_tokens(model: &str, role: &str, count: u64, feature: &str) {
    track_event(
        "ai.tokens",
        count as f64,
        vec![
            KeyValue::new("model_name", model.to_string()),
            KeyValue::new("role", role.to_string()),
            KeyValue::new("feature", feature.to_string()),
        ],
    ).await;
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
    ).await;
}

pub async fn track_ai_latency(model: &str, provider: &str, ms: u64, feature: &str) {
    track_event(
        "ai.latency",
        ms as f64,
        vec![
            KeyValue::new("model_name", model.to_string()),
            KeyValue::new("provider", provider.to_string()),
            KeyValue::new("feature", feature.to_string()),
        ],
    ).await;
}

pub async fn track_tool_use(tool_name: &str) {
    track_event(
        "tool.use",
        1.0,
        vec![KeyValue::new("tool_name", tool_name.to_string())],
    ).await;
}

pub async fn track_session_resume(session_type: &str) {
    track_event(
        "claw.session_resume",
        1.0,
        vec![KeyValue::new("session_type", session_type.to_string())],
    ).await;
}
