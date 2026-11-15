use crate::models::Interaction;
use crate::store::{SQLITE_BIND_CHUNK_SIZE, Store};
use anyhow::Result;
use sqlx::QueryBuilder;

impl Store<'_> {
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
            "INSERT INTO interactions (session_id, project_path, content, metadata, embedding) VALUES (?, ?, ?, ?, ?)",
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
        if limit <= 0 {
            return Ok(Vec::new());
        }

        let records = sqlx::query_as::<_, Interaction>(
            r"
            SELECT
                id, session_id, project_path, content, metadata,
                embedding, processing_state, created_at, last_accessed_at
            FROM interactions
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
        sqlx::query(
            "UPDATE interactions SET processing_state = 'seeded' WHERE session_id = ? AND processing_state = 'raw'",
        )
        .bind(session_id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn seed_all_raw_interactions(&self) -> Result<()> {
        sqlx::query(
            "UPDATE interactions SET processing_state = 'seeded' WHERE processing_state = 'raw'",
        )
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_undreamed_interactions(&self) -> Result<Vec<Interaction>> {
        let records = sqlx::query_as::<_, Interaction>(
            r"
            SELECT
                id, session_id, project_path, content, metadata,
                embedding, processing_state, created_at, last_accessed_at
            FROM interactions
            WHERE processing_state IN ('raw', 'seeded')
            ORDER BY created_at ASC
            LIMIT 100
            ",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(records)
    }

    pub async fn mark_interactions_as_dreamed(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        for chunk in ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
            let mut builder = QueryBuilder::new(
                "UPDATE interactions SET processing_state = 'dreamed' WHERE id IN (",
            );
            let mut separated = builder.separated(", ");
            for id in chunk {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            builder.build().execute(self.pool).await?;
        }
        Ok(())
    }

    pub async fn search_interactions(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Interaction>> {
        if query.trim().is_empty() || limit <= 0 {
            return Ok(Vec::new());
        }

        let records = sqlx::query_as::<_, Interaction>(
            r"
            SELECT
                m.id, m.session_id, m.project_path, m.content, m.metadata,
                m.embedding, m.processing_state, m.created_at, m.last_accessed_at
            FROM interactions_fts f
            JOIN interactions m ON f.rowid = m.id
            WHERE interactions_fts MATCH ?
            ORDER BY
                CASE WHEN ? IS NOT NULL AND m.project_path = ? THEN 0 ELSE 1 END,
                bm25(interactions_fts, 1.0, 0.5),
                m.last_accessed_at DESC
            LIMIT ?
            ",
        )
        .bind(query)
        .bind(project_path)
        .bind(project_path)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        for chunk in records.chunks(SQLITE_BIND_CHUNK_SIZE) {
            let mut builder = QueryBuilder::new(
                "UPDATE interactions SET last_accessed_at = CURRENT_TIMESTAMP WHERE id IN (",
            );
            let mut separated = builder.separated(", ");
            for interaction in chunk {
                separated.push_bind(interaction.id);
            }
            separated.push_unseparated(")");
            let _ = builder.build().execute(self.pool).await;
        }

        Ok(records)
    }

    pub async fn get_all_embeddings_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<Interaction>> {
        let records = sqlx::query_as::<_, Interaction>(
            r"
            SELECT
                id, session_id, project_path, content, metadata,
                embedding, processing_state, created_at, last_accessed_at
            FROM interactions
            WHERE session_id = ? AND embedding IS NOT NULL
            ",
        )
        .bind(session_id)
        .fetch_all(self.pool)
        .await?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use crate::Db;
    use crate::models::Interaction;
    use crate::store::Store;

    #[tokio::test]
    async fn test_interaction_operations_search_and_fts_integrity() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let session_id = "sess_interactions";
        store
            .create_session(session_id, "Int Session")
            .await
            .unwrap();

        let embedding = vec![0.1_f32, 0.2, 0.3];

        let id1 = store
            .add_interaction(
                session_id,
                Some("proj_alpha"),
                "Explaining how Rust traits work",
                Some("meta1"),
                Some(&embedding),
            )
            .await
            .unwrap();

        let id2 = store
            .add_interaction(
                session_id,
                Some("proj_beta"),
                "Configuring a web server",
                None,
                None,
            )
            .await
            .unwrap();

        let recent_a = store
            .get_recent_interactions_by_path("proj_alpha", 10)
            .await
            .unwrap();
        assert_eq!(recent_a.len(), 1);
        assert_eq!(recent_a[0].id, id1);

        store.touch_interaction(id2).await.unwrap();

        assert_eq!(
            store.search_interactions("", Some("proj_alpha"), 10).await.unwrap().len(),
            0
        );
        let results = store
            .search_interactions("Rust", Some("proj_alpha"), 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Explaining how Rust traits work");

        let embeddings = store
            .get_all_embeddings_for_session(session_id)
            .await
            .unwrap();
        assert_eq!(embeddings.len(), 1);
        assert_eq!(embeddings[0].id, id1);

        let parsed = Interaction::parse_embedding(embeddings[0].embedding.as_deref()).unwrap();
        assert_eq!(parsed, embedding);

        // Verify external-content FTS5 stays 100% consistent across insert, touch, and cascade delete
        sqlx::raw_sql("INSERT INTO interactions_fts(interactions_fts) VALUES ('integrity-check')")
            .execute(db.pool())
            .await
            .unwrap();

        store.delete_session(session_id).await.unwrap();
        sqlx::raw_sql("INSERT INTO interactions_fts(interactions_fts) VALUES ('integrity-check')")
            .execute(db.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_dreaming_lifecycle() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let session_id = "sess_dream";
        store
            .create_session(session_id, "Dream Session")
            .await
            .unwrap();

        let id1 = store
            .add_interaction(session_id, None, "I love rust", None, None)
            .await
            .unwrap();

        let undreamed = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(undreamed.len(), 1);
        assert_eq!(undreamed[0].processing_state.as_deref(), Some("raw"));

        store.mark_interactions_as_seeded(session_id).await.unwrap();
        let undreamed = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(undreamed[0].processing_state.as_deref(), Some("seeded"));

        store.mark_interactions_as_dreamed(&[id1]).await.unwrap();
        assert_eq!(store.get_undreamed_interactions().await.unwrap().len(), 0);

        store
            .update_session_dream_timestamp(session_id)
            .await
            .unwrap();

        store
            .add_interaction(session_id, None, "I love cargo", None, None)
            .await
            .unwrap();

        store.seed_all_raw_interactions().await.unwrap();
        let undreamed = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(undreamed.len(), 1);
        assert_eq!(undreamed[0].processing_state.as_deref(), Some("seeded"));
    }
}
