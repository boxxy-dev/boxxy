use std::collections::BTreeMap;

/// Schema version attached to every exported telemetry datapoint (`v: 1`).
pub(crate) const TELEMETRY_SCHEMA_VERSION: i64 = 1;

pub(crate) const METRIC_APP_LAUNCH: &str = "app.launch";
pub(crate) const METRIC_AI_TOKENS: &str = "ai.tokens";
pub(crate) const METRIC_AI_INVOCATIONS: &str = "ai.invocations";
pub(crate) const METRIC_AI_LATENCY: &str = "ai.latency";
pub(crate) const METRIC_TOOL_USE: &str = "tool.use";
pub(crate) const METRIC_SESSION_RESUME: &str = "claw.session_resume";

/// Pre-validated, canonically ordered telemetry metric ready for `telemetry_journal`.
///
/// Using a `BTreeMap` guarantees deterministic JSON key ordering so identical attribute sets
/// always match SQLite's `UNIQUE(..., attributes_json)` coalescing index.
#[derive(Debug, Clone)]
pub(crate) struct MetricRecord {
    pub(crate) metric_name: &'static str,
    pub(crate) value: f64,
    pub(crate) attributes_json: String,
}

impl MetricRecord {
    #[must_use]
    pub(crate) fn new<'a>(
        metric_name: &'static str,
        value: f64,
        attrs: impl IntoIterator<Item = (&'static str, &'a str)>,
    ) -> Option<Self> {
        if !value.is_finite() || value <= 0.0 {
            return None;
        }
        let map: BTreeMap<&'static str, &'a str> = attrs.into_iter().collect();
        let attributes_json = serde_json::to_string(&map).ok()?;
        Some(Self {
            metric_name,
            value,
            attributes_json,
        })
    }
}
