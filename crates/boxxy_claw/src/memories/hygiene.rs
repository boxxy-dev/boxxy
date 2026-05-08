use boxxy_db::Db;
use log::debug;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn run_hygiene(db: Arc<Mutex<Option<Db>>>) -> anyhow::Result<()> {
    let db_guard = db.lock().await;
    let Some(db_val) = db_guard.as_ref() else {
        return Ok(());
    };

    let pool = db_val.pool();

    debug!("Running Memory Hygiene...");

    // 1. Delete episodic interactions older than 30 days
    let deleted_interactions = sqlx::query(
        "DELETE FROM interactions WHERE last_accessed_at < datetime('now', '-30 days')",
    )
    .execute(pool)
    .await?
    .rows_affected();

    if deleted_interactions > 0 {
        debug!(
            "Hygiene: Pruned {} old episodic interactions.",
            deleted_interactions
        );
    }

    // 2. Delete implicit/candidate memories that haven't been accessed in 30 days
    // We EXPLICITLY KEEP 'manual_sync', 'preference', and 'pinned' memories.
    let deleted_memories = sqlx::query(
        "DELETE FROM memories WHERE category IN ('candidate', 'extracted') AND last_accessed_at < datetime('now', '-30 days')"
    )
    .execute(pool)
    .await?
    .rows_affected();

    if deleted_memories > 0 {
        debug!(
            "Hygiene: Pruned {} stale candidate facts.",
            deleted_memories
        );
    }

    // 3. New Rule: Prune noise from quarantine
    // If a candidate fact has only been observed once and hasn't been reinforced/updated in 14 days, discard it.
    let pruned_noise = sqlx::query(
        "DELETE FROM memories WHERE verified = false AND category IN ('candidate', 'extracted') AND observation_count < 2 AND updated_at < datetime('now', '-14 days')"
    )
    .execute(pool)
    .await?
    .rows_affected();

    if pruned_noise > 0 {
        debug!(
            "Hygiene: Pruned {} noise entries from memory quarantine.",
            pruned_noise
        );
    }

    debug!("Memory Hygiene complete.");
    Ok(())
}
