use crate::event::{MetricRecord, TELEMETRY_SCHEMA_VERSION};
use crate::identity::{is_telemetry_enabled, session_id, valid_install_id};
use boxxy_db::Db;
use boxxy_db::models::TelemetryJournalInsert;
use parking_lot::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Maximum age of unsent local telemetry rows before automatic pruning (7 days).
pub(crate) const JOURNAL_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
/// Hard cap on coalesced rows in `telemetry_journal` so offline machines never grow unboundedly.
pub(crate) const JOURNAL_MAX_ROWS: i64 = 5_000;

static DB_HANDLE: RwLock<Option<Db>> = RwLock::new(None);

#[inline]
pub(crate) fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Injects an already-opened [`boxxy_db::Db`] pool (used by `boxxy-agent` and unit tests
/// to avoid opening duplicate SQLite pools).
pub(crate) fn set_db(db: Db) {
    *DB_HANDLE.write() = Some(db);
}

#[must_use]
pub(crate) fn get_cached_db() -> Option<Db> {
    DB_HANDLE.read().clone()
}

/// Ensures the default `boxxy-db` handle is initialized when telemetry is enabled.
pub(crate) async fn ensure_db() -> Option<Db> {
    if let Some(existing) = get_cached_db() {
        return Some(existing);
    }
    match Db::new().await {
        Ok(db) => {
            let mut guard = DB_HANDLE.write();
            if guard.is_none() {
                *guard = Some(db);
            }
            guard.clone()
        }
        Err(e) => {
            log::warn!("Telemetry: failed to open local journal database: {e}");
            None
        }
    }
}

/// Records one or more pre-validated metrics into `telemetry_journal` in a single transaction.
pub(crate) async fn record_metrics(records: &[MetricRecord]) {
    if records.is_empty() || !is_telemetry_enabled() {
        return;
    }

    let Some(install_id) = valid_install_id() else {
        log::debug!("Telemetry: skipping local journal write because install_id is missing/invalid.");
        return;
    };
    let sid = session_id();
    let Some(db) = ensure_db().await else {
        return;
    };

    let recorded_at_ms = now_epoch_ms();
    let event_ids: Vec<String> = (0..records.len())
        .map(|_| Uuid::new_v4().to_string())
        .collect();

    let inserts: Vec<TelemetryJournalInsert<'_>> = records
        .iter()
        .zip(&event_ids)
        .map(|(rec, eid)| TelemetryJournalInsert {
            event_id: eid.as_str(),
            schema_version: TELEMETRY_SCHEMA_VERSION,
            install_id: install_id.as_str(),
            session_id: sid.as_str(),
            recorded_at_ms,
            metric_name: rec.metric_name,
            value: rec.value,
            attributes_json: rec.attributes_json.as_str(),
        })
        .collect();

    if let Err(e) = db.store().record_telemetry_events(&inserts).await {
        log::debug!("Telemetry: failed to persist metric batch to journal: {e}");
    } else {
        log::debug!(
            "Telemetry: recorded {} metric(s) into local SQLite journal.",
            inserts.len()
        );
    }
}

/// Purges all unsent rows from `telemetry_journal` (invoked immediately when the user opts out).
pub(crate) async fn purge_all() {
    let db_opt = if let Some(db) = get_cached_db() {
        Some(db)
    } else {
        Db::new().await.ok()
    };

    if let Some(db) = db_opt {
        if let Err(e) = db.store().purge_telemetry_journal().await {
            log::debug!("Telemetry: failed to purge telemetry_journal: {e}");
        } else {
            log::debug!("Purged local telemetry_journal table.");
        }
    }
}
