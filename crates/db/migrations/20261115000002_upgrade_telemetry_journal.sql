-- Upgrade `telemetry_journal` to normalized typed columns with in-SQLite counter coalescing,
-- millisecond event timestamps (preserving exact OTLP `timeUnixNano` across offline periods),
-- and per-row UUIDv4 `event_id` for end-to-end idempotent ingestion in Supabase.

CREATE TABLE telemetry_journal_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    install_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    day_bucket INTEGER NOT NULL,
    recorded_at_ms INTEGER NOT NULL,
    metric_name TEXT NOT NULL,
    value REAL NOT NULL CHECK (value > 0.0),
    attributes_json TEXT NOT NULL DEFAULT '{}'
);

INSERT INTO telemetry_journal_new (
    event_id,
    schema_version,
    install_id,
    session_id,
    day_bucket,
    recorded_at_ms,
    metric_name,
    value,
    attributes_json
)
SELECT
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))), 2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))), 2) || '-' ||
    lower(hex(randomblob(6))) AS event_id,
    COALESCE(CAST(json_extract(attributes_json, '$.v') AS INTEGER), 1) AS schema_version,
    TRIM(CAST(json_extract(attributes_json, '$.install_id') AS TEXT)) AS install_id,
    TRIM(CAST(json_extract(attributes_json, '$.session_id') AS TEXT)) AS session_id,
    COALESCE(CAST(unixepoch(created_at) / 86400 AS INTEGER), CAST(unixepoch('now') / 86400 AS INTEGER)) AS day_bucket,
    MAX(COALESCE(CAST(unixepoch(created_at) * 1000 AS INTEGER), CAST(unixepoch('now') * 1000 AS INTEGER))) AS recorded_at_ms,
    TRIM(metric_name) AS metric_name,
    SUM(value) AS value,
    COALESCE(json_remove(attributes_json, '$.install_id', '$.session_id', '$.v', '$.event_id'), '{}') AS attributes_json
FROM telemetry_journal
WHERE value > 0.0
  AND json_valid(attributes_json) = 1
  AND LENGTH(TRIM(COALESCE(CAST(json_extract(attributes_json, '$.install_id') AS TEXT), ''))) = 36
  AND LENGTH(TRIM(COALESCE(CAST(json_extract(attributes_json, '$.session_id') AS TEXT), ''))) = 36
GROUP BY 2, 3, 4, 5, 7, 9;

DROP TABLE telemetry_journal;
ALTER TABLE telemetry_journal_new RENAME TO telemetry_journal;

CREATE UNIQUE INDEX IF NOT EXISTS idx_telemetry_journal_coalesce
    ON telemetry_journal(schema_version, install_id, session_id, day_bucket, metric_name, attributes_json);
CREATE INDEX IF NOT EXISTS idx_telemetry_journal_recorded_at
    ON telemetry_journal(recorded_at_ms ASC, id ASC);
