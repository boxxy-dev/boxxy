use crate::models::{TelemetryJournalAck, TelemetryJournalEntry, TelemetryJournalInsert};
use crate::store::{SQLITE_BIND_CHUNK_SIZE, Store};
use anyhow::Result;
use sqlx::{QueryBuilder, SqliteExecutor};

const MS_PER_UTC_DAY: i64 = 86_400_000;
const FLOAT_ACK_EPSILON: f64 = 1e-9;

const UPSERT_TELEMETRY_SQL: &str = r"
INSERT INTO telemetry_journal (
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
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT (
    schema_version,
    install_id,
    session_id,
    day_bucket,
    metric_name,
    attributes_json
)
DO UPDATE SET
    value = telemetry_journal.value + excluded.value,
    recorded_at_ms = MAX(telemetry_journal.recorded_at_ms, excluded.recorded_at_ms)
";

async fn execute_upsert<'e, E: SqliteExecutor<'e>>(
    executor: E,
    entry: TelemetryJournalInsert<'_>,
) -> Result<()> {
    if !entry.value.is_finite() || entry.value <= 0.0 {
        return Ok(());
    }
    let day_bucket = entry.recorded_at_ms.div_euclid(MS_PER_UTC_DAY);
    sqlx::query(UPSERT_TELEMETRY_SQL)
        .bind(entry.event_id)
        .bind(entry.schema_version)
        .bind(entry.install_id)
        .bind(entry.session_id)
        .bind(day_bucket)
        .bind(entry.recorded_at_ms)
        .bind(entry.metric_name)
        .bind(entry.value)
        .bind(entry.attributes_json)
        .execute(executor)
        .await?;
    Ok(())
}

impl Store<'_> {
    /// Appends or coalesces a single telemetry metric increment in `telemetry_journal`.
    pub async fn record_telemetry_event(&self, entry: TelemetryJournalInsert<'_>) -> Result<()> {
        execute_upsert(self.pool, entry).await
    }

    /// Appends or coalesces a slice of telemetry metrics atomically in a single transaction
    /// when `entries.len() > 1` (e.g. a full AI turn's invocation, latency, and token metrics).
    pub async fn record_telemetry_events(
        &self,
        entries: &[TelemetryJournalInsert<'_>],
    ) -> Result<()> {
        match entries {
            [] => Ok(()),
            [single] => execute_upsert(self.pool, *single).await,
            many => {
                let mut tx = self.pool.begin().await?;
                for entry in many {
                    execute_upsert(&mut *tx, *entry).await?;
                }
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub async fn fetch_telemetry_batch(&self, limit: i64) -> Result<Vec<TelemetryJournalEntry>> {
        if limit <= 0 {
            return Ok(Vec::new());
        }

        let entries = sqlx::query_as::<_, TelemetryJournalEntry>(
            r"
            SELECT
                id,
                event_id,
                schema_version,
                install_id,
                session_id,
                day_bucket,
                recorded_at_ms,
                metric_name,
                value,
                attributes_json
            FROM telemetry_journal
            ORDER BY recorded_at_ms ASC, id ASC
            LIMIT ?
            ",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(entries)
    }

    /// Atomically acknowledges exported journal rows:
    /// - Deletes rows whose current `value` has not grown since `fetch_telemetry_batch`.
    /// - Subtracts `exported_value` and assigns `replacement_event_id` if new increments
    ///   coalesced into the row while the HTTP export was in flight.
    pub async fn ack_telemetry_events(&self, acks: &[TelemetryJournalAck<'_>]) -> Result<u64> {
        if acks.is_empty() {
            return Ok(0);
        }

        let mut tx = self.pool.begin().await?;
        let mut affected = 0_u64;

        for ack in acks {
            let threshold = ack.exported_value + FLOAT_ACK_EPSILON;
            let del = sqlx::query("DELETE FROM telemetry_journal WHERE id = ? AND value <= ?")
                .bind(ack.id)
                .bind(threshold)
                .execute(&mut *tx)
                .await?;

            if del.rows_affected() > 0 {
                affected += del.rows_affected();
            } else {
                let upd = sqlx::query(
                    "UPDATE telemetry_journal \
                     SET value = value - ?, event_id = ? \
                     WHERE id = ? AND value > ?",
                )
                .bind(ack.exported_value)
                .bind(ack.replacement_event_id)
                .bind(ack.id)
                .bind(threshold)
                .execute(&mut *tx)
                .await?;
                affected += upd.rows_affected();
            }
        }

        tx.commit().await?;
        Ok(affected)
    }

    pub async fn delete_telemetry_events(&self, ids: &[i64]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }

        let mut deleted = 0_u64;
        for chunk in ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
            let mut builder = QueryBuilder::new("DELETE FROM telemetry_journal WHERE id IN (");
            let mut separated = builder.separated(", ");
            for id in chunk {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            deleted += builder.build().execute(self.pool).await?.rows_affected();
        }

        Ok(deleted)
    }

    /// Prunes journal entries older than `cutoff_ms` and enforces a strict upper bound of `max_rows`
    /// so offline machines can never accumulate unbounded SQLite storage.
    pub async fn prune_telemetry_journal(&self, cutoff_ms: i64, max_rows: i64) -> Result<u64> {
        let mut pruned = sqlx::query("DELETE FROM telemetry_journal WHERE recorded_at_ms < ?")
            .bind(cutoff_ms)
            .execute(self.pool)
            .await?
            .rows_affected();

        if max_rows > 0 {
            let count = self.count_telemetry_events().await?;
            if count > max_rows {
                let excess = count - max_rows;
                pruned += sqlx::query(
                    "DELETE FROM telemetry_journal WHERE id IN ( \
                        SELECT id FROM telemetry_journal \
                        ORDER BY recorded_at_ms ASC, id ASC \
                        LIMIT ? \
                    )",
                )
                .bind(excess)
                .execute(self.pool)
                .await?
                .rows_affected();
            }
        }

        Ok(pruned)
    }

    pub async fn purge_telemetry_journal(&self) -> Result<u64> {
        let res = sqlx::query("DELETE FROM telemetry_journal")
            .execute(self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    pub async fn count_telemetry_events(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_journal")
            .fetch_one(self.pool)
            .await?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;

    const INSTALL_A: &str = "11111111-1111-4111-8111-111111111111";
    const SESSION_A: &str = "22222222-2222-4222-8222-222222222222";

    #[tokio::test]
    async fn test_telemetry_journal_coalescing_multi_day_and_inflight_ack() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        let day1_ms = 1_760_000_000_000_i64;
        let day2_ms = day1_ms + MS_PER_UTC_DAY + 5_000;

        // 1. Two identical tool calls on the same UTC day coalesce into a single row (value = 2.0)
        //    while keeping the original `event_id` and advancing `recorded_at_ms`.
        store
            .record_telemetry_event(TelemetryJournalInsert {
                event_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                schema_version: 1,
                install_id: INSTALL_A,
                session_id: SESSION_A,
                recorded_at_ms: day1_ms,
                metric_name: "tool.use",
                value: 1.0,
                attributes_json: r#"{"tool_name":"file_read"}"#,
            })
            .await
            .unwrap();

        store
            .record_telemetry_event(TelemetryJournalInsert {
                event_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
                schema_version: 1,
                install_id: INSTALL_A,
                session_id: SESSION_A,
                recorded_at_ms: day1_ms + 2_500,
                metric_name: "tool.use",
                value: 1.0,
                attributes_json: r#"{"tool_name":"file_read"}"#,
            })
            .await
            .unwrap();

        // Same metric on the NEXT UTC day stays a separate row to preserve daily DAU & timestamps
        store
            .record_telemetry_event(TelemetryJournalInsert {
                event_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
                schema_version: 1,
                install_id: INSTALL_A,
                session_id: SESSION_A,
                recorded_at_ms: day2_ms,
                metric_name: "tool.use",
                value: 3.0,
                attributes_json: r#"{"tool_name":"file_read"}"#,
            })
            .await
            .unwrap();

        let batch = store.fetch_telemetry_batch(10).await.unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].event_id, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        assert!((batch[0].value - 2.0).abs() < f64::EPSILON);
        assert_eq!(batch[0].recorded_at_ms, day1_ms + 2_500);
        assert_eq!(batch[1].event_id, "cccccccc-cccc-4ccc-8ccc-cccccccccccc");
        assert!((batch[1].value - 3.0).abs() < f64::EPSILON);

        // 2. Simulate a concurrent +5.0 increment arriving on row 0 while export of 2.0 is in flight
        store
            .record_telemetry_event(TelemetryJournalInsert {
                event_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
                schema_version: 1,
                install_id: INSTALL_A,
                session_id: SESSION_A,
                recorded_at_ms: day1_ms + 4_000,
                metric_name: "tool.use",
                value: 5.0,
                attributes_json: r#"{"tool_name":"file_read"}"#,
            })
            .await
            .unwrap();

        // Acknowledge exported batch (row 0 exported 2.0, row 1 exported 3.0)
        store
            .ack_telemetry_events(&[
                TelemetryJournalAck {
                    id: batch[0].id,
                    exported_value: 2.0,
                    replacement_event_id: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
                },
                TelemetryJournalAck {
                    id: batch[1].id,
                    exported_value: 3.0,
                    replacement_event_id: "ffffffff-ffff-4fff-8fff-ffffffffffff",
                },
            ])
            .await
            .unwrap();

        // Row 1 was deleted completely; row 0 retained residual value 5.0 with rotated event_id!
        let remaining = store.fetch_telemetry_batch(10).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining[0].event_id,
            "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"
        );
        assert!((remaining[0].value - 5.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_telemetry_journal_batch_prune_and_row_cap() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        let base_ms = 1_760_000_000_000_i64;
        store
            .record_telemetry_events(&[
                TelemetryJournalInsert {
                    event_id: "10000000-0000-4000-8000-000000000001",
                    schema_version: 1,
                    install_id: INSTALL_A,
                    session_id: SESSION_A,
                    recorded_at_ms: base_ms - (10 * MS_PER_UTC_DAY),
                    metric_name: "app.launch",
                    value: 1.0,
                    attributes_json: r#"{"os":"Linux"}"#,
                },
                TelemetryJournalInsert {
                    event_id: "10000000-0000-4000-8000-000000000002",
                    schema_version: 1,
                    install_id: INSTALL_A,
                    session_id: SESSION_A,
                    recorded_at_ms: base_ms,
                    metric_name: "ai.invocations",
                    value: 1.0,
                    attributes_json: r#"{"feature":"claw","model_provider":"Gemini"}"#,
                },
                TelemetryJournalInsert {
                    event_id: "10000000-0000-4000-8000-000000000003",
                    schema_version: 1,
                    install_id: INSTALL_A,
                    session_id: SESSION_A,
                    recorded_at_ms: base_ms + 1_000,
                    metric_name: "ai.tokens",
                    value: 512.0,
                    attributes_json: r#"{"feature":"claw","model_provider":"Gemini","role":"input"}"#,
                },
            ])
            .await
            .unwrap();

        assert_eq!(store.count_telemetry_events().await.unwrap(), 3);

        // Prune rows older than 7 days AND cap max_rows to 1 -> drops the 10d-old row + oldest remaining
        let cutoff = base_ms - (7 * MS_PER_UTC_DAY);
        let pruned = store.prune_telemetry_journal(cutoff, 1).await.unwrap();
        assert_eq!(pruned, 2);

        let kept = store.fetch_telemetry_batch(10).await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].metric_name, "ai.tokens");

        assert_eq!(store.purge_telemetry_journal().await.unwrap(), 1);
        assert_eq!(store.count_telemetry_events().await.unwrap(), 0);
    }
}
