use boxxy_db::Db;
use log::debug;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Prunes expired episodic interactions and stale unverified candidate facts while
/// preserving verified preferences, manual sync entries, and pinned memories.
///
/// # Errors
/// Returns an error if any SQLite pruning query fails.
pub async fn run_hygiene(db: Arc<Mutex<Option<Db>>>) -> anyhow::Result<()> {
    let db_opt = db.lock().await.clone();
    let Some(db_val) = db_opt else {
        return Ok(());
    };

    debug!("Running Memory Hygiene...");

    let stats = db_val.store().run_memory_hygiene().await?;

    if stats.deleted_interactions > 0 {
        debug!(
            "Hygiene: Pruned {} old episodic interactions.",
            stats.deleted_interactions
        );
    }

    if stats.deleted_stale_candidates > 0 {
        debug!(
            "Hygiene: Pruned {} stale candidate facts.",
            stats.deleted_stale_candidates
        );
    }

    if stats.deleted_quarantine_noise > 0 {
        debug!(
            "Hygiene: Pruned {} noise entries from memory quarantine.",
            stats.deleted_quarantine_noise
        );
    }

    debug!("Memory Hygiene complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_db::store::Store;

    #[tokio::test]
    async fn hygiene_prunes_stale_noise_but_preserves_pinned_and_verified_memories() {
        let db = Db::new_in_memory().await.unwrap();
        let pool = db.pool();
        let store = Store::new(pool);

        // 1. Stale 1-observation noise candidate (20 days old) -> should be pruned
        store
            .upsert_dream_candidate("stale_noise", None, "temporary branch", 0.4)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE memories SET updated_at = datetime('now', '-20 days') WHERE key = 'stale_noise'",
        )
        .execute(pool)
        .await
        .unwrap();

        // 2. Pinned candidate (40 days old) -> MUST be preserved
        store
            .add_memory(
                "pinned_candidate",
                None,
                "important rule",
                Some("candidate"),
                false,
                true,
            )
            .await
            .unwrap();
        sqlx::query(
            "UPDATE memories SET updated_at = datetime('now', '-40 days'), last_accessed_at = datetime('now', '-40 days') WHERE key = 'pinned_candidate'",
        )
        .execute(pool)
        .await
        .unwrap();

        // 3. Verified preference (40 days old) -> MUST be preserved
        store
            .add_memory(
                "user_pref",
                None,
                "prefers dark theme",
                Some("preference"),
                true,
                false,
            )
            .await
            .unwrap();
        sqlx::query(
            "UPDATE memories SET updated_at = datetime('now', '-40 days'), last_accessed_at = datetime('now', '-40 days') WHERE key = 'user_pref'",
        )
        .execute(pool)
        .await
        .unwrap();

        let db_arc = Arc::new(Mutex::new(Some(db.clone())));
        run_hygiene(db_arc).await.unwrap();

        assert!(
            store
                .get_memory_by_key("stale_noise", None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_memory_by_key("pinned_candidate", None)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .get_memory_by_key("user_pref", None)
                .await
                .unwrap()
                .is_some()
        );
    }
}
