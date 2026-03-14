use boxxy_db::Db;
use std::sync::Arc;
use tokio::sync::Mutex;
use log::{info, debug};

pub async fn run_hygiene(db: Arc<Mutex<Option<Db>>>) -> anyhow::Result<()> {
    let db_guard = db.lock().await;
    let Some(db_val) = db_guard.as_ref() else {
        return Ok(());
    };
    
    let pool = db_val.pool();

    info!("Running Memory Hygiene...");

    // 1. Delete episodic interactions older than 30 days
    let deleted_interactions = sqlx::query(
        "DELETE FROM interactions WHERE last_accessed_at < datetime('now', '-30 days')"
    )
    .execute(pool)
    .await?
    .rows_affected();

    if deleted_interactions > 0 {
        debug!("Hygiene: Pruned {} old episodic interactions.", deleted_interactions);
    }

    // 2. Delete implicit (extracted) memories that haven't been accessed in 30 days
    // We EXPLICITLY KEEP 'manual_sync', 'preference', and 'pinned' memories.
    let deleted_memories = sqlx::query(
        "DELETE FROM memories WHERE category = 'extracted' AND last_accessed_at < datetime('now', '-30 days')"
    )
    .execute(pool)
    .await?
    .rows_affected();

    if deleted_memories > 0 {
        debug!("Hygiene: Pruned {} stale extracted facts.", deleted_memories);
    }

    info!("Memory Hygiene complete.");
    Ok(())
}
