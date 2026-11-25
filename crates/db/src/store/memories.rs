use crate::models::{HygieneStats, Memory};
use crate::store::{
    SQLITE_BIND_CHUNK_SIZE, Store, normalize_scope, push_scope_filter, push_scope_rank,
};
use anyhow::Result;
use sqlx::QueryBuilder;

impl Store<'_> {
    /// Upserts an authoritative memory by key (`manual_sync`, `preference`, or `system`).
    pub async fn add_memory(
        &self,
        key: &str,
        project_path: Option<&str>,
        content: &str,
        category: Option<&str>,
        verified: bool,
        pinned: bool,
    ) -> Result<()> {
        let path = project_path.unwrap_or("global");
        let cat = category.unwrap_or("preference");

        sqlx::query(
            r"
            INSERT INTO memories (
                key, project_path, content, category, verified, pinned,
                observation_count, confidence_score, updated_at, last_accessed_at
            )
            VALUES (?, ?, ?, ?, ?, ?, 1, 1.0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(key, project_path) DO UPDATE SET
                content = excluded.content,
                category = excluded.category,
                verified = excluded.verified,
                pinned = excluded.pinned,
                observation_count = memories.observation_count,
                confidence_score = 1.0,
                updated_at = CURRENT_TIMESTAMP,
                last_accessed_at = CURRENT_TIMESTAMP
            ",
        )
        .bind(key)
        .bind(path)
        .bind(content)
        .bind(cat)
        .bind(verified)
        .bind(pinned)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Upserts a memory candidate from a dream or batch extraction.
    ///
    /// Increments `observation_count` and updates `confidence_score` (taking the `MAX`).
    /// Never overwrites `manual_sync`, `preference`, `system`, `pinned`, or already-`verified`
    /// memory content (verified memories must be explicitly demoted via conflict resolution first).
    pub async fn upsert_dream_candidate(
        &self,
        key: &str,
        project_path: Option<&str>,
        content: &str,
        confidence: f64,
    ) -> Result<()> {
        let path = project_path.unwrap_or("global");
        let clamped_confidence = if confidence.is_finite() {
            confidence.clamp(0.0, 1.0)
        } else {
            0.0
        };

        sqlx::query(
            r"
            INSERT INTO memories (
                key, project_path, content, category, verified, pinned,
                observation_count, confidence_score, updated_at, last_accessed_at, access_count
            )
            VALUES (?, ?, ?, 'candidate', false, false, 1, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 0)
            ON CONFLICT(key, project_path) DO UPDATE SET
                content = CASE
                    WHEN COALESCE(memories.verified, false) = false THEN excluded.content
                    ELSE memories.content
                END,
                observation_count = memories.observation_count + 1,
                confidence_score = MAX(memories.confidence_score, excluded.confidence_score),
                updated_at = CURRENT_TIMESTAMP,
                last_accessed_at = CURRENT_TIMESTAMP
            WHERE COALESCE(memories.pinned, false) = false
              AND COALESCE(memories.category, '') NOT IN ('manual_sync', 'preference', 'system')
            ",
        )
        .bind(key)
        .bind(path)
        .bind(content)
        .bind(clamped_confidence)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Promotes unverified candidates to `verified = true` (`category = 'dreamed'`) once they
    /// meet both `min_observations` and `min_confidence`.
    pub async fn promote_threshold_memories(
        &self,
        min_observations: i64,
        min_confidence: f64,
    ) -> Result<Vec<(String, String, i64, f64)>> {
        let promoted = sqlx::query_as::<_, (String, String, i64, f64)>(
            r"
            UPDATE memories
            SET verified = true, category = 'dreamed', updated_at = CURRENT_TIMESTAMP
            WHERE verified = false
              AND category IN ('candidate', 'extracted')
              AND observation_count >= ?
              AND confidence_score >= ?
            RETURNING key, content, observation_count, confidence_score
            ",
        )
        .bind(min_observations)
        .bind(min_confidence)
        .fetch_all(self.pool)
        .await?;

        Ok(promoted)
    }

    /// Demotes a conflicted memory back to unverified candidate status (`observation_count = 0`,
    /// `confidence_score = 0.0`). Protected (`manual_sync`, `preference`, `system`) and `pinned`
    /// memories are never demoted.
    pub async fn demote_memory_by_key(
        &self,
        key: &str,
        project_path: Option<&str>,
    ) -> Result<bool> {
        let path = project_path.unwrap_or("global");
        let result = sqlx::query(
            r"
            UPDATE memories
            SET verified = false,
                category = 'candidate',
                observation_count = 0,
                confidence_score = 0.0,
                updated_at = CURRENT_TIMESTAMP
            WHERE key = ?
              AND project_path = ?
              AND COALESCE(pinned, false) = false
              AND category IN ('candidate', 'extracted', 'dreamed')
            ",
        )
        .bind(key)
        .bind(path)
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn get_memory_paths(&self, key: &str) -> Result<Vec<String>> {
        let records: Vec<(String,)> =
            sqlx::query_as("SELECT project_path FROM memories WHERE key = ?")
                .bind(key)
                .fetch_all(self.pool)
                .await?;
        Ok(records.into_iter().map(|(p,)| p).collect())
    }

    /// Retrieves unverified candidates matching an FTS5 `query` within `project_path` (including
    /// ancestor/subdirectory scopes in the same project tree) or `'global'`.
    pub async fn search_provisional_memories(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Memory>> {
        if query.trim().is_empty() || limit <= 0 {
            return Ok(Vec::new());
        }

        let scope = normalize_scope(project_path);
        let mut builder = QueryBuilder::new(
            "SELECT m.id, m.key, m.project_path, m.content, m.category, m.verified, m.pinned, \
                    m.observation_count, m.confidence_score, m.created_at, m.updated_at, \
                    m.last_accessed_at, m.access_count \
             FROM memories_fts f \
             JOIN memories m ON f.rowid = m.id \
             WHERE memories_fts MATCH ",
        );
        builder.push_bind(query.to_owned());
        builder.push(" AND m.verified = false AND m.category IN ('candidate', 'extracted') AND ");
        push_scope_filter(&mut builder, "m.project_path", scope);
        builder.push(" ORDER BY ");
        push_scope_rank(&mut builder, "m.project_path", scope);
        builder.push(", bm25(memories_fts, 5.0, 1.0, 0.5), m.updated_at DESC LIMIT ");
        builder.push_bind(limit);

        let records: Vec<Memory> = builder
            .build_query_as::<Memory>()
            .fetch_all(self.pool)
            .await?;

        Ok(records)
    }

    /// Unconditionally retrieves core bootstrap facts (`keys`) alongside verified user
    /// preferences and manual sync entries scoped to `project_path` (including ancestor/child
    /// directories in the same project tree) or `'global'`.
    pub async fn get_bootstrap_memories(
        &self,
        keys: &[&str],
        project_path: Option<&str>,
    ) -> Result<Vec<Memory>> {
        let scope = normalize_scope(project_path);

        let mut builder = QueryBuilder::new(
            "SELECT id, key, project_path, content, category, verified, pinned, \
                    observation_count, confidence_score, created_at, updated_at, \
                    last_accessed_at, access_count \
             FROM memories \
             WHERE verified = true AND pinned = false AND ",
        );
        push_scope_filter(&mut builder, "project_path", scope);
        builder.push(" AND (category IN ('preference', 'manual_sync')");

        if !keys.is_empty() {
            builder.push(" OR key IN (");
            let mut separated = builder.separated(", ");
            for key in keys {
                separated.push_bind((*key).to_owned());
            }
            separated.push_unseparated(")");
        }
        builder.push(") ORDER BY ");
        push_scope_rank(&mut builder, "project_path", scope);
        builder.push(", updated_at DESC, key ASC LIMIT 25");

        let records: Vec<Memory> = builder
            .build_query_as::<Memory>()
            .fetch_all(self.pool)
            .await?;
        Ok(records)
    }

    pub async fn get_memory_by_key(
        &self,
        key: &str,
        project_path: Option<&str>,
    ) -> Result<Option<Memory>> {
        let path = project_path.unwrap_or("global");
        let memory = sqlx::query_as::<_, Memory>(
            r"
            SELECT
                id, key, project_path, content, category, verified, pinned,
                observation_count, confidence_score, created_at, updated_at,
                last_accessed_at, access_count
            FROM memories
            WHERE key = ? AND project_path = ?
            ",
        )
        .bind(key)
        .bind(path)
        .fetch_optional(self.pool)
        .await?;
        Ok(memory)
    }

    /// Retrieves memories belonging to `project_path` (or its ancestor/child directories) or `'global'`,
    /// avoiding loading unrelated project scopes during candidate key deduplication.
    pub async fn get_memories_for_scope(&self, project_path: Option<&str>) -> Result<Vec<Memory>> {
        let scope = normalize_scope(project_path);
        let mut builder = QueryBuilder::new(
            "SELECT id, key, project_path, content, category, verified, pinned, \
                    observation_count, confidence_score, created_at, updated_at, \
                    last_accessed_at, access_count \
             FROM memories \
             WHERE ",
        );
        push_scope_filter(&mut builder, "project_path", scope);
        builder.push(" ORDER BY ");
        push_scope_rank(&mut builder, "project_path", scope);
        builder.push(", key ASC");

        let records = builder
            .build_query_as::<Memory>()
            .fetch_all(self.pool)
            .await?;
        Ok(records)
    }

    pub async fn search_memories(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Memory>> {
        if query.trim().is_empty() || limit <= 0 {
            return Ok(Vec::new());
        }

        let scope = normalize_scope(project_path);
        let mut builder = QueryBuilder::new(
            "SELECT m.id, m.key, m.project_path, m.content, m.category, m.verified, m.pinned, \
                    m.observation_count, m.confidence_score, m.created_at, m.updated_at, \
                    m.last_accessed_at, m.access_count \
             FROM memories_fts f \
             JOIN memories m ON f.rowid = m.id \
             WHERE memories_fts MATCH ",
        );
        builder.push_bind(query.to_owned());
        builder.push(" AND m.verified = true AND m.pinned = false AND ");
        push_scope_filter(&mut builder, "m.project_path", scope);
        builder.push(" ORDER BY ");
        push_scope_rank(&mut builder, "m.project_path", scope);
        builder.push(", bm25(memories_fts, 5.0, 1.0, 0.5), m.access_count DESC, m.updated_at DESC LIMIT ");
        builder.push_bind(limit);

        let records: Vec<Memory> = builder
            .build_query_as::<Memory>()
            .fetch_all(self.pool)
            .await?;

        // `memories_au` is column-scoped to `(key, content, project_path)`, so updating
        // `access_count` and `last_accessed_at` here incurs zero FTS5 re-indexing overhead.
        for chunk in records.chunks(SQLITE_BIND_CHUNK_SIZE) {
            let mut update_builder = QueryBuilder::new(
                "UPDATE memories SET access_count = access_count + 1, last_accessed_at = CURRENT_TIMESTAMP WHERE id IN (",
            );
            let mut separated = update_builder.separated(", ");
            for mem in chunk {
                separated.push_bind(mem.id);
            }
            separated.push_unseparated(")");
            let _ = update_builder.build().execute(self.pool).await;
        }

        Ok(records)
    }

    pub async fn get_pinned_memories(&self, project_path: Option<&str>) -> Result<Vec<Memory>> {
        let scope = normalize_scope(project_path);
        let mut builder = QueryBuilder::new(
            "SELECT id, key, project_path, content, category, verified, pinned, \
                    observation_count, confidence_score, created_at, updated_at, \
                    last_accessed_at, access_count \
             FROM memories \
             WHERE pinned = true AND ",
        );
        push_scope_filter(&mut builder, "project_path", scope);
        builder.push(" ORDER BY ");
        push_scope_rank(&mut builder, "project_path", scope);
        builder.push(", key ASC");

        let records = builder
            .build_query_as::<Memory>()
            .fetch_all(self.pool)
            .await?;

        Ok(records)
    }

    pub async fn get_all_memories(&self) -> Result<Vec<Memory>> {
        let records = sqlx::query_as::<_, Memory>(
            r"
            SELECT
                id, key, project_path, content, category, verified, pinned,
                observation_count, confidence_score, created_at, updated_at,
                last_accessed_at, access_count
            FROM memories
            ORDER BY project_path DESC, key ASC
            ",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(records)
    }

    pub async fn count_memories(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memories")
            .fetch_one(self.pool)
            .await?;
        Ok(count)
    }

    pub async fn delete_memory(&self, key: &str, project_path: Option<&str>) -> Result<()> {
        let path = project_path.unwrap_or("global");
        sqlx::query("DELETE FROM memories WHERE key = ? AND project_path = ?")
            .bind(key)
            .bind(path)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Prunes expired episodic interactions (> 30 days since last access), stale unverified
    /// candidate facts (> 30 days since last access), and single-observation quarantine noise
    /// (> 14 days without reinforcement), while strictly preserving verified, pinned,
    /// `manual_sync`, `preference`, and `system` memories.
    pub async fn run_memory_hygiene(&self) -> Result<HygieneStats> {
        let mut tx = self.pool.begin().await?;

        let deleted_interactions = sqlx::query(
            "DELETE FROM interactions WHERE last_accessed_at < datetime('now', '-30 days')",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();

        let deleted_stale_candidates = sqlx::query(
            r"
            DELETE FROM memories
            WHERE COALESCE(verified, false) = false
              AND COALESCE(pinned, false) = false
              AND category IN ('candidate', 'extracted')
              AND last_accessed_at < datetime('now', '-30 days')
            ",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();

        let deleted_quarantine_noise = sqlx::query(
            r"
            DELETE FROM memories
            WHERE COALESCE(verified, false) = false
              AND COALESCE(pinned, false) = false
              AND category IN ('candidate', 'extracted')
              AND observation_count < 2
              AND updated_at < datetime('now', '-14 days')
            ",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();

        tx.commit().await?;

        Ok(HygieneStats {
            deleted_interactions,
            deleted_stale_candidates,
            deleted_quarantine_noise,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::Db;
    use crate::store::Store;

    #[tokio::test]
    async fn test_memory_crud_and_fts_search() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        store
            .add_memory("test_fact", Some("global"), "The sky is blue.", Some("general"), true, false)
            .await
            .unwrap();

        let memory = store
            .get_memory_by_key("test_fact", Some("global"))
            .await
            .unwrap()
            .expect("Memory should exist");
        assert_eq!(store.count_memories().await.unwrap(), 1);
        assert_eq!(memory.content, "The sky is blue.");
        assert_eq!(memory.category.as_deref(), Some("general"));
        assert_eq!(memory.verified, Some(true));
        assert_eq!(memory.pinned, Some(false));

        // Upsert updates existing row without duplicating
        store
            .add_memory("upsert_fact", Some("proj_alpha"), "Initial", None, false, false)
            .await
            .unwrap();
        store
            .add_memory("upsert_fact", Some("proj_alpha"), "Updated", Some("new_cat"), true, true)
            .await
            .unwrap();
        let updated = store
            .get_memory_by_key("upsert_fact", Some("proj_alpha"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.content, "Updated");
        assert_eq!(updated.pinned, Some(true));
        assert_eq!(store.get_all_memories().await.unwrap().len(), 2);

        // Scope query returns only matching project + global
        store
            .add_memory("other_proj_fact", Some("proj_beta"), "Isolated", None, true, false)
            .await
            .unwrap();
        let alpha_scope = store
            .get_memories_for_scope(Some("proj_alpha"))
            .await
            .unwrap();
        assert_eq!(alpha_scope.len(), 2);

        // FTS search excludes unverified and pinned rows, and handles blank queries safely
        assert_eq!(store.search_memories("   ", None, 10).await.unwrap().len(), 0);
        let hits = store.search_memories("blue", None, 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "test_fact");

        // Verify access_count was incremented without corrupting FTS5
        let after_hit = store.get_memory_by_key("test_fact", None).await.unwrap().unwrap();
        assert_eq!(after_hit.access_count, 1);
        sqlx::raw_sql("INSERT INTO memories_fts(memories_fts) VALUES ('integrity-check')")
            .execute(db.pool())
            .await
            .unwrap();

        store.delete_memory("test_fact", None).await.unwrap();
        assert!(store.get_memory_by_key("test_fact", None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_threshold_reinforcement_and_verified_protection() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        store
            .upsert_dream_candidate("os", None, "Fedora", 0.5)
            .await
            .unwrap();
        store
            .upsert_dream_candidate("os", None, "Fedora Linux", 0.9)
            .await
            .unwrap();

        let mem = store.get_memory_by_key("os", None).await.unwrap().unwrap();
        assert_eq!(mem.verified, Some(false));
        assert_eq!(mem.observation_count, 2);
        assert!((mem.confidence_score - 0.9).abs() < 1e-9);
        assert_eq!(mem.content, "Fedora Linux");

        assert_eq!(
            store.promote_threshold_memories(3, 0.8).await.unwrap(),
            Vec::new()
        );
        let promoted = store.promote_threshold_memories(2, 0.8).await.unwrap();
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].0, "os");

        // Once verified (`dreamed`), an unverified low-confidence candidate MUST NOT overwrite content
        store
            .upsert_dream_candidate("os", None, "Unverified Noise", 0.4)
            .await
            .unwrap();
        let protected = store.get_memory_by_key("os", None).await.unwrap().unwrap();
        assert_eq!(protected.content, "Fedora Linux");
        assert_eq!(protected.verified, Some(true));

        // Explicit conflict demotion resets both `observation_count` and `confidence_score`
        assert!(store.demote_memory_by_key("os", None).await.unwrap());
        let demoted = store.get_memory_by_key("os", None).await.unwrap().unwrap();
        assert_eq!(demoted.verified, Some(false));
        assert_eq!(demoted.observation_count, 0);
        assert!((demoted.confidence_score - 0.0).abs() < 1e-9);

        // Protected categories (`manual_sync`) and `pinned` memories cannot be demoted
        store
            .add_memory("shell", None, "fish", Some("manual_sync"), true, false)
            .await
            .unwrap();
        assert!(!store.demote_memory_by_key("shell", None).await.unwrap());

        store
            .add_memory("pinned_rule", None, "keep", Some("dreamed"), true, true)
            .await
            .unwrap();
        assert!(!store.demote_memory_by_key("pinned_rule", None).await.unwrap());
    }

    #[tokio::test]
    async fn test_bootstrap_provisional_and_key_fts_recall() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        store
            .upsert_dream_candidate("editor", None, "User uses vim", 0.5)
            .await
            .unwrap();
        let prov = store
            .search_provisional_memories("vim", None, 10)
            .await
            .unwrap();
        assert_eq!(prov.len(), 1);
        assert_eq!(prov[0].key, "editor");

        store
            .add_memory("os_type", None, "Linux", Some("system"), true, false)
            .await
            .unwrap();
        store
            .add_memory("package_manager", None, "pacman", Some("system"), true, false)
            .await
            .unwrap();
        store
            .add_memory("pkg_alias", None, "paru", Some("preference"), true, false)
            .await
            .unwrap();

        let bootstrap = store
            .get_bootstrap_memories(&["os_type", "package_manager", "missing_key"], None)
            .await
            .unwrap();
        assert_eq!(bootstrap.len(), 3);

        store
            .add_memory("build_target_key", None, "x86_64-unknown-linux-gnu", Some("dreamed"), true, false)
            .await
            .unwrap();
        let key_hits = store.search_memories("\"build_target\"*", None, 10).await.unwrap();
        assert!(key_hits.iter().any(|m| m.key == "build_target_key"));
    }

    #[tokio::test]
    async fn test_memory_hygiene_prunes_stale_noise_and_preserves_protected() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        store.create_session("s_hyg", "Session").await.unwrap();
        let old_int = store
            .add_interaction("s_hyg", Some("proj_alpha"), "ancient summary", None, None)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE interactions SET last_accessed_at = datetime('now', '-35 days') WHERE id = ?",
        )
        .bind(old_int)
        .execute(db.pool())
        .await
        .unwrap();

        store
            .upsert_dream_candidate("quarantine_noise", None, "one-off guess", 0.4)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE memories SET updated_at = datetime('now', '-20 days') WHERE key = 'quarantine_noise'",
        )
        .execute(db.pool())
        .await
        .unwrap();

        store
            .add_memory("pinned_cand", None, "keep me", Some("candidate"), false, true)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE memories SET updated_at = datetime('now', '-45 days'), last_accessed_at = datetime('now', '-45 days') WHERE key = 'pinned_cand'",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let stats = store.run_memory_hygiene().await.unwrap();
        assert_eq!(stats.deleted_interactions, 1);
        assert_eq!(stats.deleted_quarantine_noise, 1);
        assert!(store.get_memory_by_key("quarantine_noise", None).await.unwrap().is_none());
        assert!(store.get_memory_by_key("pinned_cand", None).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_hierarchical_scope_inherits_parent_and_isolates_sibling_projects() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        store
            .add_memory("global_tool", None, "Uses ripgrep everywhere", Some("preference"), true, false)
            .await
            .unwrap();
        store
            .add_memory(
                "alpha_pin",
                Some("/workspace/alpha_proj"),
                "Strict clippy lints in alpha",
                Some("preference"),
                true,
                true,
            )
            .await
            .unwrap();
        store
            .add_memory(
                "alpha_fact",
                Some("/workspace/alpha_proj"),
                "Alpha build uses mold linker",
                Some("dreamed"),
                true,
                false,
            )
            .await
            .unwrap();
        store
            .upsert_dream_candidate(
                "alpha_cand",
                Some("/workspace/alpha_proj"),
                "Alpha port is 9090",
                0.6,
            )
            .await
            .unwrap();

        // Sibling project sharing a string prefix (`/workspace/alpha_proj_two` vs `/workspace/alpha_proj`)
        // and completely separate `/workspace/beta_proj` MUST NEVER leak into `/workspace/alpha_proj`.
        store
            .add_memory(
                "sibling_secret",
                Some("/workspace/alpha_proj_two"),
                "Sibling build uses lld linker",
                Some("dreamed"),
                true,
                false,
            )
            .await
            .unwrap();
        store
            .upsert_dream_candidate(
                "beta_cand",
                Some("/workspace/beta_proj"),
                "Beta port is 7070",
                0.7,
            )
            .await
            .unwrap();

        // 1. Querying from a subdirectory `/workspace/alpha_proj/crates/sub` inherits alpha_proj + global
        let sub_scope = Some("/workspace/alpha_proj/crates/sub");
        let pinned = store.get_pinned_memories(sub_scope).await.unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].key, "alpha_pin");

        let boot = store.get_bootstrap_memories(&["os_type"], sub_scope).await.unwrap();
        assert!(boot.iter().any(|m| m.key == "global_tool"));

        let verified_hits = store.search_memories("linker", sub_scope, 10).await.unwrap();
        assert_eq!(verified_hits.len(), 1);
        assert_eq!(verified_hits[0].key, "alpha_fact");

        let cand_hits = store
            .search_provisional_memories("port", sub_scope, 10)
            .await
            .unwrap();
        assert_eq!(cand_hits.len(), 1);
        assert_eq!(cand_hits[0].key, "alpha_cand");

        // 2. Querying from `/workspace/beta_proj` isolates out alpha_proj and alpha_proj_two
        let beta_verified = store
            .search_memories("linker", Some("/workspace/beta_proj"), 10)
            .await
            .unwrap();
        assert!(beta_verified.is_empty());

        let beta_cands = store
            .search_provisional_memories("port", Some("/workspace/beta_proj"), 10)
            .await
            .unwrap();
        assert_eq!(beta_cands.len(), 1);
        assert_eq!(beta_cands[0].key, "beta_cand");

        // 3. Fallback root CWD `"/"` never acts as a wildcard ancestor for `/workspace/...`
        assert!(
            store
                .search_memories("linker", Some("/"), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
