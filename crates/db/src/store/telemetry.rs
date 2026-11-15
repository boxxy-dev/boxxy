use crate::models::TelemetryJournalEntry;
use crate::store::{SQLITE_BIND_CHUNK_SIZE, Store};
use anyhow::Result;
use sqlx::QueryBuilder;

impl Store<'_> {
    pub async fn insert_telemetry_event(
        &self,
        metric_name: &str,
        value: f64,
        attributes_json: &str,
    ) -> Result<i64> {
        let res = sqlx::query(
            "INSERT INTO telemetry_journal (metric_name, value, attributes_json) VALUES (?, ?, ?)",
        )
        .bind(metric_name)
        .bind(value)
        .bind(attributes_json)
        .execute(self.pool)
        .await?;

        Ok(res.last_insert_rowid())
    }

    pub async fn fetch_telemetry_batch(&self, limit: i64) -> Result<Vec<TelemetryJournalEntry>> {
        if limit <= 0 {
            return Ok(Vec::new());
        }

        let entries = sqlx::query_as::<_, TelemetryJournalEntry>(
            r"
            SELECT id, metric_name, value, attributes_json, created_at
            FROM telemetry_journal
            ORDER BY created_at ASC, id ASC
            LIMIT ?
            ",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(entries)
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

    pub async fn prune_stale_telemetry_events(&self, max_age_days: u32) -> Result<u64> {
        let modifier = format!("-{max_age_days} days");
        let res = sqlx::query("DELETE FROM telemetry_journal WHERE created_at < datetime('now', ?)")
            .bind(modifier)
            .execute(self.pool)
            .await?;

        Ok(res.rows_affected())
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
    use crate::Db;

    #[tokio::test]
    async fn test_telemetry_journal_lifecycle_and_stale_pruning() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        let id1 = store
            .insert_telemetry_event("app.launch", 1.0, "{\"os\":\"linux\"}")
            .await
            .unwrap();
        let id2 = store
            .insert_telemetry_event("tool.use", 1.0, "{\"tool_name\":\"file_read\"}")
            .await
            .unwrap();

        assert_eq!(store.count_telemetry_events().await.unwrap(), 2);

        let batch = store.fetch_telemetry_batch(10).await.unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].id, id1);
        assert_eq!(batch[1].id, id2);

        // Delete first event
        let removed = store.delete_telemetry_events(&[id1]).await.unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.count_telemetry_events().await.unwrap(), 1);

        // Backdate remaining event by 10 days and prune anything older than 7 days
        sqlx::query("UPDATE telemetry_journal SET created_at = datetime('now', '-10 days') WHERE id = ?")
            .bind(id2)
            .execute(db.pool())
            .await
            .unwrap();

        let stale = store.prune_stale_telemetry_events(7).await.unwrap();
        assert_eq!(stale, 1);
        assert_eq!(store.count_telemetry_events().await.unwrap(), 0);

        // Full purge
        store
            .insert_telemetry_event("ai.tokens", 128.0, "{}")
            .await
            .unwrap();
        assert_eq!(store.purge_telemetry_journal().await.unwrap(), 1);
        assert_eq!(store.count_telemetry_events().await.unwrap(), 0);
    }
}
