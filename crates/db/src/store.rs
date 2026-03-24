use crate::models::{Interaction, Memory, Session, SkillRecord};
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

    // --- Sessions ---
    pub async fn create_session(&self, id: &str, name: &str) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, name) VALUES (?, ?)")
            .bind(id)
            .bind(name)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let session = sqlx::query_as::<_, Session>(
            "SELECT id, name, created_at, updated_at FROM sessions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(session)
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

    pub async fn search_interactions(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Interaction>> {
        // Prioritize current project, then global/others
        let records = sqlx::query_as::<_, Interaction>(
            r"
            SELECT m.id, m.session_id, m.project_path, m.content, m.metadata, m.embedding, m.created_at, m.last_accessed_at
            FROM interactions_fts f
            JOIN interactions m ON f.rowid = m.id
            WHERE interactions_fts MATCH ?
            ORDER BY 
                CASE WHEN m.project_path = ? THEN 0 ELSE 1 END,
                rank,
                m.last_accessed_at DESC
            LIMIT ?
            "
        )
        .bind(query)
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
            "SELECT id, session_id, project_path, content, metadata, embedding, created_at, last_accessed_at FROM interactions WHERE session_id = ? AND embedding IS NOT NULL"
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
            JOIN skills m ON f.rowid = m.name
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
