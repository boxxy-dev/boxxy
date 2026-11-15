use crate::models::MsgBarHistory;
use crate::store::Store;
use anyhow::Result;

impl Store<'_> {
    pub async fn insert_msgbar_history(&self, text: &str, attachments_json: &str) -> Result<i64> {
        let result = sqlx::query(
            "INSERT INTO msgbar_history (text, attachments_json) VALUES (?, ?)",
        )
        .bind(text)
        .bind(attachments_json)
        .execute(self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_recent_msgbar_history(&self, limit: i64) -> Result<Vec<MsgBarHistory>> {
        if limit <= 0 {
            return Ok(Vec::new());
        }

        let records = sqlx::query_as::<_, MsgBarHistory>(
            r"
            SELECT id, text, COALESCE(attachments_json, '[]') AS attachments_json, created_at
            FROM (
                SELECT id, text, attachments_json, created_at
                FROM msgbar_history
                ORDER BY id DESC
                LIMIT ?
            )
            ORDER BY id ASC
            ",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }

    /// Atomically prunes `msgbar_history` down to the newest `target` rows once total rows
    /// reach `threshold`. Guards against invalid bounds so negative `LIMIT` values can never
    /// be passed to SQLite.
    pub async fn prune_msgbar_history(&self, threshold: i64, target: i64) -> Result<()> {
        if threshold <= 0 || target < 0 || threshold <= target {
            return Ok(());
        }

        sqlx::query(
            r"
            DELETE FROM msgbar_history
            WHERE (SELECT COUNT(*) FROM msgbar_history) >= ?
              AND id NOT IN (
                  SELECT id FROM msgbar_history
                  ORDER BY id DESC
                  LIMIT ?
              )
            ",
        )
        .bind(threshold)
        .bind(target)
        .execute(self.pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Db;
    use crate::store::Store;

    #[tokio::test]
    async fn test_msgbar_history_operations_and_safe_pruning() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let id1 = store
            .insert_msgbar_history("hello world", "[]")
            .await
            .unwrap();
        let id2 = store
            .insert_msgbar_history("testing 123", "[{\"type\": \"image\"}]")
            .await
            .unwrap();

        let recent = store.get_recent_msgbar_history(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, id1);
        assert_eq!(recent[0].text, "hello world");
        assert_eq!(recent[0].attachments_json, "[]");
        assert_eq!(recent[1].id, id2);
        assert_eq!(recent[1].text, "testing 123");
        assert_eq!(recent[1].attachments_json, "[{\"type\": \"image\"}]");

        // Invalid / inverted threshold bounds must be a safe no-op (never wipe rows via negative LIMIT)
        store.prune_msgbar_history(0, 100).await.unwrap();
        store.prune_msgbar_history(2, 5).await.unwrap();
        assert_eq!(store.get_recent_msgbar_history(10).await.unwrap().len(), 2);

        // Insert a 3rd entry and prune from 3 -> newest 1
        store
            .insert_msgbar_history("message 3", "[]")
            .await
            .unwrap();
        store.prune_msgbar_history(3, 1).await.unwrap();

        let recent_after_prune = store.get_recent_msgbar_history(10).await.unwrap();
        assert_eq!(recent_after_prune.len(), 1);
        assert_eq!(recent_after_prune[0].text, "message 3");
        assert_eq!(store.get_recent_msgbar_history(0).await.unwrap().len(), 0);
    }
}
