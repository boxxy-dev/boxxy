use crate::models::{Interaction, Memory, MsgBarHistory, Session, SkillRecord};
use anyhow::Result;
use sqlx::SqlitePool;

pub struct Store<'a> {
    pool: &'a SqlitePool,
}

impl<'a> Store<'a> {
    #[must_use]
    pub const fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    // --- Message Bar History ---

    pub async fn insert_msgbar_history(&self, text: &str, attachments_json: &str) -> Result<i64> {
        let result = sqlx::query(
            r"
            INSERT INTO msgbar_history (text, attachments_json)
            VALUES (?, ?)
            ",
        )
        .bind(text)
        .bind(attachments_json)
        .execute(self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_recent_msgbar_history(&self, limit: i64) -> Result<Vec<MsgBarHistory>> {
        let records = sqlx::query_as::<_, MsgBarHistory>(
            r"
            SELECT * FROM (
                SELECT id, text, attachments_json, created_at
                FROM msgbar_history
                ORDER BY id DESC
                LIMIT ?
            ) ORDER BY id ASC
            ",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }

    pub async fn prune_msgbar_history(&self, threshold: i64, target: i64) -> Result<()> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM msgbar_history")
            .fetch_one(self.pool)
            .await?;

        if count >= threshold {
            let to_delete = count - target;
            sqlx::query(
                r"
                DELETE FROM msgbar_history 
                WHERE id IN (
                    SELECT id FROM msgbar_history 
                    ORDER BY id ASC 
                    LIMIT ?
                )
                ",
            )
            .bind(to_delete)
            .execute(self.pool)
            .await?;
        }
        Ok(())
    }

    // --- Sessions ---
    pub async fn create_session(&self, id: &str, name: &str) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, name) VALUES (?, ?)")
            .bind(id)
            .bind(name)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn upsert_session_state(
        &self,
        id: &str,
        name: &str,
        title: &str,
        history_json: &str,
        pending_tasks_json: &str,
        agent_name: &str,
        last_cwd: &str,
        model_id: &str,
        pinned: bool,
        total_tokens: i64,
    ) -> Result<()> {
        sqlx::query(
            r"
            INSERT INTO sessions (id, name, title, history_json, pending_tasks_json, agent_name, last_cwd, model_id, pinned, total_tokens, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                title = CASE WHEN excluded.title != '' THEN excluded.title ELSE sessions.title END,
                history_json = excluded.history_json,
                pending_tasks_json = excluded.pending_tasks_json,
                agent_name = excluded.agent_name,
                last_cwd = excluded.last_cwd,
                model_id = excluded.model_id,
                pinned = excluded.pinned,
                total_tokens = excluded.total_tokens,
                updated_at = CURRENT_TIMESTAMP
            ",
        )
        .bind(id)
        .bind(name)
        .bind(title)
        .bind(history_json)
        .bind(pending_tasks_json)
        .bind(agent_name)
        .bind(last_cwd)
        .bind(model_id)
        .bind(pinned)
        .bind(total_tokens)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let session = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool)
            .await?;
        Ok(session)
    }

    pub async fn get_recent_active_sessions(&self, limit: i64) -> Result<Vec<Session>> {
        let records = sqlx::query_as::<_, Session>(
            r"
            SELECT * FROM (
                SELECT s.*, COUNT(e.id) as message_count 
                FROM sessions s
                JOIN claw_events e ON s.id = e.session_id
                WHERE s.pinned = true
                GROUP BY s.id
                ORDER BY s.updated_at DESC
            )
            UNION ALL
            SELECT * FROM (
                SELECT s.*, COUNT(e.id) as message_count 
                FROM sessions s
                JOIN claw_events e ON s.id = e.session_id
                WHERE s.pinned = false
                GROUP BY s.id
                ORDER BY s.updated_at DESC
                LIMIT ?
            )
            ORDER BY pinned DESC, updated_at DESC
            ",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(records)
    }

    // --- Claw Events (UI History) ---
    pub async fn add_claw_event(&self, session_id: &str, event_json: &str) -> Result<()> {
        sqlx::query("INSERT INTO claw_events (session_id, event_json) VALUES (?, ?)")
            .bind(session_id)
            .bind(event_json)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_claw_events(&self, session_id: &str) -> Result<Vec<String>> {
        let records: Vec<(String,)> = sqlx::query_as(
            r"
            SELECT e.event_json 
            FROM claw_events e
            JOIN sessions s ON e.session_id = s.id
            WHERE e.session_id = ? 
              AND (s.cleared_at IS NULL OR e.created_at > s.cleared_at)
            ORDER BY e.id ASC
            ",
        )
        .bind(session_id)
        .fetch_all(self.pool)
        .await?;

        Ok(records.into_iter().map(|(json,)| json).collect())
    }

    pub async fn mark_session_cleared(&self, session_id: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET cleared_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn clear_claw_events(&self, session_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM claw_events WHERE session_id = ?")
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    // --- Interactions (Episodic Memory) ---
    pub async fn add_interaction(
        &self,
        session_id: &str,
        project_path: Option<&str>,
        content: &str,
        metadata: Option<&str>,
        embedding: Option<&[f32]>,
    ) -> Result<i64> {
        let embed_blob = embedding.map(Interaction::serialize_embedding);

        let result = sqlx::query(
            "INSERT INTO interactions (session_id, project_path, content, metadata, embedding) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(session_id)
        .bind(project_path)
        .bind(content)
        .bind(metadata)
        .bind(embed_blob)
        .execute(self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_recent_interactions_by_path(
        &self,
        project_path: &str,
        limit: i64,
    ) -> Result<Vec<Interaction>> {
        let records = sqlx::query_as::<_, Interaction>(
            r"
            SELECT * FROM interactions 
            WHERE project_path = ? 
            ORDER BY last_accessed_at DESC 
            LIMIT ?
            ",
        )
        .bind(project_path)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }

    pub async fn touch_interaction(&self, id: i64) -> Result<()> {
        sqlx::query("UPDATE interactions SET last_accessed_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_interactions_as_seeded(&self, session_id: &str) -> Result<()> {
        sqlx::query("UPDATE interactions SET processing_state = 'seeded' WHERE session_id = ? AND processing_state = 'raw'")
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn seed_all_raw_interactions(&self) -> Result<()> {
        sqlx::query("UPDATE interactions SET processing_state = 'seeded' WHERE processing_state = 'raw'")
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_undreamed_interactions(&self) -> Result<Vec<Interaction>> {
        let records = sqlx::query_as::<_, Interaction>(
            "SELECT * FROM interactions WHERE processing_state = 'seeded' OR processing_state = 'raw' ORDER BY created_at ASC"
        )
        .fetch_all(self.pool)
        .await?;
        Ok(records)
    }

    pub async fn mark_interactions_as_dreamed(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        // SQLite has a limit on parameters, but for dream batches it should be fine.
        let query = format!(
            "UPDATE interactions SET processing_state = 'dreamed' WHERE id IN ({})",
            ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")
        );

        sqlx::query(&query).execute(self.pool).await?;
        Ok(())
    }

    pub async fn update_session_dream_timestamp(&self, session_id: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET last_dream_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn search_interactions(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Interaction>> {
        // Prioritize current project, then global/others
        let records = sqlx::query_as::<_, Interaction>(
            r"
            SELECT m.id, m.session_id, m.project_path, m.content, m.metadata, m.embedding, m.processing_state, m.created_at, m.last_accessed_at
            FROM interactions_fts f
            JOIN interactions m ON f.rowid = m.id
            WHERE interactions_fts MATCH ?
            ORDER BY
                CASE WHEN m.project_path = ? THEN 0 ELSE 1 END,
                rank,
                m.last_accessed_at DESC
            LIMIT ?
            "
        )        .bind(query)
        .bind(project_path)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        // Update last_accessed_at for interactions
        for interaction in &records {
            let _ = sqlx::query(
                "UPDATE interactions SET last_accessed_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(interaction.id)
            .execute(self.pool)
            .await;
        }

        Ok(records)
    }

    pub async fn get_all_embeddings_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<Interaction>> {
        let records = sqlx::query_as::<_, Interaction>(
            "SELECT id, session_id, project_path, content, metadata, embedding, processing_state, created_at, last_accessed_at FROM interactions WHERE session_id = ? AND embedding IS NOT NULL"
        )
        .bind(session_id)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }

    // --- Global Memories (Long-term Facts) ---

    /// Upsert a memory by key (`ZeroClaw` model)
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
        sqlx::query(
            r"
            INSERT INTO memories (key, project_path, content, category, verified, pinned, updated_at, last_accessed_at) 
            VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(key, project_path) DO UPDATE SET 
                content = excluded.content,
                category = COALESCE(excluded.category, memories.category),
                verified = excluded.verified,
                pinned = excluded.pinned,
                updated_at = CURRENT_TIMESTAMP,
                last_accessed_at = CURRENT_TIMESTAMP
            "
        )
        .bind(key)
        .bind(path)
        .bind(content)
        .bind(category)
        .bind(verified)
        .bind(pinned)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_memory_by_key(
        &self,
        key: &str,
        project_path: Option<&str>,
    ) -> Result<Option<Memory>> {
        let path = project_path.unwrap_or("global");
        let memory = sqlx::query_as::<_, Memory>(
            "SELECT id, key, project_path, content, category, verified, pinned, created_at, updated_at, last_accessed_at, access_count FROM memories WHERE key = ? AND project_path = ?"
        )
        .bind(key)
        .bind(path)
        .fetch_optional(self.pool)
        .await?;
        Ok(memory)
    }

    pub async fn search_memories(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Memory>> {
        let records = sqlx::query_as::<_, Memory>(
            r"
            SELECT m.id, m.key, m.project_path, m.content, m.category, m.verified, m.pinned, m.created_at, m.updated_at, m.last_accessed_at, m.access_count
            FROM memories_fts f
            JOIN memories m ON f.rowid = m.id
            WHERE memories_fts MATCH ? AND m.verified = true AND m.pinned = false
            ORDER BY 
                CASE WHEN m.project_path = ? THEN 0 WHEN m.project_path = 'global' THEN 1 ELSE 2 END,
                rank,
                m.access_count DESC,
                m.updated_at DESC
            LIMIT ?
            "
        )
        .bind(query)
        .bind(project_path)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        // Increment access count and update last_accessed_at for found memories
        for mem in &records {
            let _ = sqlx::query("UPDATE memories SET access_count = access_count + 1, last_accessed_at = CURRENT_TIMESTAMP WHERE id = ?")
                .bind(mem.id)
                .execute(self.pool)
                .await;
        }

        Ok(records)
    }

    pub async fn get_pinned_memories(&self, project_path: Option<&str>) -> Result<Vec<Memory>> {
        let path = project_path.unwrap_or("global");
        let records = sqlx::query_as::<_, Memory>(
            "SELECT id, key, project_path, content, category, verified, pinned, created_at, updated_at, last_accessed_at, access_count FROM memories WHERE pinned = true AND (project_path = ? OR project_path = 'global') ORDER BY project_path DESC, key ASC"
        )
        .bind(path)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }

    pub async fn get_all_memories(&self) -> Result<Vec<Memory>> {
        let records = sqlx::query_as::<_, Memory>(
            "SELECT id, key, project_path, content, category, verified, pinned, created_at, updated_at, last_accessed_at, access_count FROM memories ORDER BY project_path DESC, key ASC"
        )
        .fetch_all(self.pool)
        .await?;
        Ok(records)
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

    // --- Skills ---

    pub async fn sync_skills(&self, skills: &[SkillRecord]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // 1. Get all existing skill names to identify which ones should be deleted
        let existing_names: Vec<String> = sqlx::query_scalar("SELECT name FROM skills")
            .fetch_all(&mut *tx)
            .await?;

        let current_names: std::collections::HashSet<String> =
            skills.iter().map(|s| s.name.clone()).collect();

        // 2. Delete skills that are no longer on disk
        for name in existing_names {
            if !current_names.contains(&name) {
                sqlx::query("DELETE FROM skills WHERE name = ?")
                    .bind(name)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        // 3. Upsert all current skills
        for skill in skills {
            sqlx::query(
                r"
                INSERT INTO skills (name, description, triggers, content, pinned, updated_at)
                VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
                ON CONFLICT(name) DO UPDATE SET
                    description = excluded.description,
                    triggers = excluded.triggers,
                    content = excluded.content,
                    pinned = excluded.pinned,
                    updated_at = CURRENT_TIMESTAMP
                ",
            )
            .bind(&skill.name)
            .bind(&skill.description)
            .bind(&skill.triggers)
            .bind(&skill.content)
            .bind(skill.pinned)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn search_skills(&self, query: &str, limit: i64) -> Result<Vec<SkillRecord>> {
        let records = sqlx::query_as::<_, SkillRecord>(
            r"
            SELECT m.name, m.description, m.triggers, m.content, m.pinned, m.updated_at
            FROM skills_fts f
            JOIN skills m ON m.name = f.name
            WHERE skills_fts MATCH ?
            ORDER BY m.pinned DESC, rank
            LIMIT ?
            ",
        )
        .bind(query)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;

    #[tokio::test]
    async fn test_memory_insertion_and_retrieval() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let key = "test_fact";
        let project_path = "global";
        let content = "The sky is blue.";
        let category = Some("general");

        store
            .add_memory(key, Some(project_path), content, category, true, false)
            .await
            .unwrap();

        let memory = store.get_memory_by_key(key, Some(project_path)).await.unwrap().expect("Memory should exist");
        assert_eq!(memory.key, key);
        assert_eq!(memory.project_path, project_path);
        assert_eq!(memory.content, content);
        assert_eq!(memory.category.as_deref(), category);
        assert_eq!(memory.verified, Some(true));
        assert_eq!(memory.pinned, Some(false));
    }

    #[tokio::test]
    async fn test_memory_upsert() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let key = "upsert_fact";
        let project_path = "/tmp/test";
        
        // Initial insert
        store
            .add_memory(key, Some(project_path), "Initial content", None, false, false)
            .await
            .unwrap();

        // Upsert with new content and verified status
        store
            .add_memory(key, Some(project_path), "Updated content", Some("new_cat"), true, true)
            .await
            .unwrap();

        let memory = store.get_memory_by_key(key, Some(project_path)).await.unwrap().expect("Memory should exist");
        
        // Ensure properties are updated
        assert_eq!(memory.content, "Updated content");
        assert_eq!(memory.category.as_deref(), Some("new_cat"));
        assert_eq!(memory.verified, Some(true));
        assert_eq!(memory.pinned, Some(true));

        // Ensure we didn't just insert a duplicate row
        let all_memories = store.get_all_memories().await.unwrap();
        assert_eq!(all_memories.len(), 1);
    }

    #[tokio::test]
    async fn test_memory_fts_search() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        // Insert a verified memory (searchable)
        store
            .add_memory("verified_fact", None, "Rust is a fast and memory-safe language.", None, true, false)
            .await
            .unwrap();

        // Insert an unverified memory (not searchable by default)
        store
            .add_memory("unverified_fact", None, "Rust might be from space.", None, false, false)
            .await
            .unwrap();
            
        // Insert a pinned memory (not searchable by default via search_memories)
        store
            .add_memory("pinned_fact", None, "Rust is oxidized iron.", None, true, true)
            .await
            .unwrap();

        // Search for "Rust"
        let results = store.search_memories("Rust", None, 10).await.unwrap();
        
        assert_eq!(results.len(), 1, "Only the verified, unpinned memory should be returned in search");
        assert_eq!(results[0].key, "verified_fact");
        assert_eq!(results[0].content, "Rust is a fast and memory-safe language.");
    }

    #[tokio::test]
    async fn test_memory_deletion() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let key = "delete_me";
        store
            .add_memory(key, None, "I will be deleted", None, true, false)
            .await
            .unwrap();

        assert!(store.get_memory_by_key(key, None).await.unwrap().is_some());

        store.delete_memory(key, None).await.unwrap();

        assert!(store.get_memory_by_key(key, None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_msgbar_history_operations() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        // Insert some history
        let id1 = store.insert_msgbar_history("hello world", "[]").await.unwrap();
        let id2 = store.insert_msgbar_history("testing 123", "[{\"type\": \"image\"}]").await.unwrap();

        // Retrieve history
        let recent = store.get_recent_msgbar_history(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, id1);
        assert_eq!(recent[0].text, "hello world");
        assert_eq!(recent[0].attachments_json, "[]");
        
        assert_eq!(recent[1].id, id2);
        assert_eq!(recent[1].text, "testing 123");
        assert_eq!(recent[1].attachments_json, "[{\"type\": \"image\"}]");

        // Prune history
        // Insert a 3rd message
        store.insert_msgbar_history("message 3", "[]").await.unwrap();
        
        // threshold 3, target 1 (keep 1) -> deletes 2
        store.prune_msgbar_history(3, 1).await.unwrap();

        let recent_after_prune = store.get_recent_msgbar_history(10).await.unwrap();
        assert_eq!(recent_after_prune.len(), 1);
        assert_eq!(recent_after_prune[0].text, "message 3");
    }

    #[tokio::test]
    async fn test_session_operations() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let session_id = "sess_1";
        let session_name = "Session 1";

        // Create session
        store.create_session(session_id, session_name).await.unwrap();

        // Get session
        let session = store.get_session(session_id).await.unwrap().expect("Session should exist");
        assert_eq!(session.id, session_id);
        assert_eq!(session.name, session_name);
        assert_eq!(session.title, None);
        assert_eq!(session.pinned, false);

        // Upsert state
        store.upsert_session_state(
            session_id,
            session_name,
            "Updated Title",
            "[\"history\"]",
            "[\"tasks\"]",
            "Agent X",
            "/tmp/cwd",
            "model_1",
            true, // pinned
            42,
        ).await.unwrap();

        // Verify update
        let updated = store.get_session(session_id).await.unwrap().expect("Session should exist");
        assert_eq!(updated.title.as_deref(), Some("Updated Title"));
        assert_eq!(updated.pinned, true);
        assert_eq!(updated.total_tokens, 42);

        // UI History / Claw Events
        store.add_claw_event(session_id, "{\"event\": 1}").await.unwrap();
        store.add_claw_event(session_id, "{\"event\": 2}").await.unwrap();

        let events = store.get_claw_events(session_id).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], "{\"event\": 1}");
        
        // Debug raw DB state
        let raw_sessions: Vec<(String, bool)> = sqlx::query_as("SELECT id, pinned FROM sessions").fetch_all(store.pool).await.unwrap();
        println!("RAW SESSIONS: {:?}", raw_sessions);
        let raw_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM claw_events").fetch_one(store.pool).await.unwrap();
        println!("RAW EVENTS COUNT: {}", raw_events);

        // Recent active sessions (since pinned = true, it should appear)
        let recent = store.get_recent_active_sessions(10).await.unwrap();
        println!("RECENT SESSIONS: {:?}", recent);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, session_id);
        assert_eq!(recent[0].message_count, 2);

        // Mark cleared
        store.mark_session_cleared(session_id).await.unwrap();
        
        // Sleep to ensure the timestamp advances before inserting the next event
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        
        // add event after clear
        store.add_claw_event(session_id, "{\"event\": 3}").await.unwrap();
        let events_after = store.get_claw_events(session_id).await.unwrap();
        assert_eq!(events_after.len(), 1);
        assert_eq!(events_after[0], "{\"event\": 3}");

        // Clear all events
        store.clear_claw_events(session_id).await.unwrap();
        let empty_events = store.get_claw_events(session_id).await.unwrap();
        assert!(empty_events.is_empty());
    }

    #[tokio::test]
    async fn test_interaction_operations_and_search() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let session_id = "sess_interactions";
        store.create_session(session_id, "Int Session").await.unwrap();

        let embedding = vec![0.1f32, 0.2, 0.3];

        let id1 = store.add_interaction(
            session_id,
            Some("/proj/A"),
            "Explaining how Rust traits work",
            Some("meta1"),
            Some(&embedding),
        ).await.unwrap();

        let id2 = store.add_interaction(
            session_id,
            Some("/proj/B"),
            "Configuring a web server",
            None,
            None,
        ).await.unwrap();

        // get recent
        let recent_a = store.get_recent_interactions_by_path("/proj/A", 10).await.unwrap();
        assert_eq!(recent_a.len(), 1);
        assert_eq!(recent_a[0].id, id1);

        // touch
        store.touch_interaction(id2).await.unwrap();

        // fts search
        let results = store.search_interactions("Rust", Some("/proj/A"), 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Explaining how Rust traits work");

        // embeddings
        let embeddings = store.get_all_embeddings_for_session(session_id).await.unwrap();
        assert_eq!(embeddings.len(), 1);
        assert_eq!(embeddings[0].id, id1);
        
        let parsed = Interaction::parse_embedding(embeddings[0].embedding.clone()).unwrap();
        assert_eq!(parsed, embedding);
    }

    #[tokio::test]
    async fn test_skill_operations_and_search() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let skill1 = SkillRecord {
            name: "RustExpert".to_string(),
            description: "Helps with Rust".to_string(),
            triggers: "rust, cargo".to_string(),
            content: "You are a Rust expert.".to_string(),
            pinned: true,
            updated_at: None,
        };

        let skill2 = SkillRecord {
            name: "PythonExpert".to_string(),
            description: "Helps with Python".to_string(),
            triggers: "python, pip".to_string(),
            content: "You are a Python expert.".to_string(),
            pinned: false,
            updated_at: None,
        };

        // Sync (Insert)
        store.sync_skills(&[skill1.clone(), skill2.clone()]).await.unwrap();

        // Also do a direct query to see if it's in the FTS table
        let raw_fts: Vec<(i64, String, String)> = sqlx::query_as("SELECT rowid, name, content FROM skills_fts").fetch_all(store.pool).await.unwrap();
        println!("RAW FTS BEFORE SEARCH: {:?}", raw_fts);
        
        let match_test: Vec<(i64,)> = sqlx::query_as("SELECT rowid FROM skills_fts WHERE skills_fts MATCH 'Rust'").fetch_all(store.pool).await.unwrap();
        println!("MATCH TEST FOR 'Rust': {:?}", match_test);

        let raw_skills: Vec<(String, String)> = sqlx::query_as("SELECT name, content FROM skills").fetch_all(store.pool).await.unwrap();
        println!("RAW SKILLS BEFORE SEARCH: {:?}", raw_skills);

        // Search
        let results = store.search_skills("Rust", 10).await.unwrap();
        println!("SEARCH RESULTS: {:?}", results);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "RustExpert");

        // Sync (Update and Delete)
        let skill1_updated = SkillRecord {
            content: "Updated Rust expert content.".to_string(),
            ..skill1
        };

        store.sync_skills(&[skill1_updated.clone()]).await.unwrap();

        let all_skills = store.search_skills("expert", 10).await.unwrap();
        println!("ALL SKILLS AFTER UPDATE: {:?}", all_skills);
        
        // Also do a direct query to see if it's in the FTS table
        let raw_fts: Vec<(i64, String, String)> = sqlx::query_as("SELECT rowid, name, content FROM skills_fts").fetch_all(store.pool).await.unwrap();
        println!("RAW FTS: {:?}", raw_fts);

        assert_eq!(all_skills.len(), 1);
        assert_eq!(all_skills[0].name, "RustExpert");
        assert_eq!(all_skills[0].content, "Updated Rust expert content.");
    }

    #[tokio::test]
    async fn test_dreaming_lifecycle() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());
        
        let session_id = "sess_dream";
        store.create_session(session_id, "Dream Session").await.unwrap();
        
        // 1. Insert interaction (defaults to 'raw')
        let id1 = store.add_interaction(session_id, None, "I love rust", None, None).await.unwrap();
        
        // Check undreamed includes 'raw'
        let undreamed = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(undreamed.len(), 1);
        assert_eq!(undreamed[0].processing_state.as_deref(), Some("raw"));
        
        // 2. Mark as seeded
        store.mark_interactions_as_seeded(session_id).await.unwrap();
        
        // Check undreamed includes 'seeded'
        let undreamed = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(undreamed.len(), 1);
        assert_eq!(undreamed[0].processing_state.as_deref(), Some("seeded"));
        
        // 3. Mark as dreamed
        store.mark_interactions_as_dreamed(&[id1]).await.unwrap();
        
        // Check undreamed is now empty
        let undreamed = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(undreamed.len(), 0);
        
        // 4. Update session timestamp
        store.update_session_dream_timestamp(session_id).await.unwrap();
        
        // Add another raw interaction
        let _id2 = store.add_interaction(session_id, None, "I love cargo", None, None).await.unwrap();
        
        // 5. Global seed all
        store.seed_all_raw_interactions().await.unwrap();
        let undreamed = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(undreamed.len(), 1);
        assert_eq!(undreamed[0].processing_state.as_deref(), Some("seeded"));
    }
}
