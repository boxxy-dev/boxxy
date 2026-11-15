use crate::models::SkillRecord;
use crate::store::{SQLITE_BIND_CHUNK_SIZE, Store};
use anyhow::Result;
use sqlx::QueryBuilder;
use std::collections::HashSet;

impl Store<'_> {
    /// Atomically synchronizes enabled skills into SQLite, deleting removed skills in batches
    /// and updating only rows whose `description`, `triggers`, `content`, or `pinned` changed.
    pub async fn sync_skills(&self, skills: &[SkillRecord]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        let existing_names: Vec<String> = sqlx::query_scalar("SELECT name FROM skills")
            .fetch_all(&mut *tx)
            .await?;

        let current_names: HashSet<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        let to_delete: Vec<String> = existing_names
            .into_iter()
            .filter(|name| !current_names.contains(name.as_str()))
            .collect();

        for chunk in to_delete.chunks(SQLITE_BIND_CHUNK_SIZE) {
            let mut builder = QueryBuilder::new("DELETE FROM skills WHERE name IN (");
            let mut separated = builder.separated(", ");
            for name in chunk {
                separated.push_bind(name);
            }
            separated.push_unseparated(")");
            builder.build().execute(&mut *tx).await?;
        }

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
                WHERE skills.description IS NOT excluded.description
                   OR skills.triggers IS NOT excluded.triggers
                   OR skills.content IS NOT excluded.content
                   OR skills.pinned IS NOT excluded.pinned
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
        if query.trim().is_empty() || limit <= 0 {
            return Ok(Vec::new());
        }

        let records = sqlx::query_as::<_, SkillRecord>(
            r"
            SELECT m.name, m.description, m.triggers, m.content, m.pinned, m.updated_at
            FROM skills_fts f
            JOIN skills m ON m.rowid = f.rowid
            WHERE skills_fts MATCH ?
            ORDER BY m.pinned DESC, bm25(skills_fts, 10.0, 5.0, 8.0, 1.0)
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
    use crate::Db;
    use crate::models::SkillRecord;
    use crate::store::Store;

    #[tokio::test]
    async fn test_skill_operations_trigger_fts_and_noop_sync() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let skill1 = SkillRecord {
            name: "RustExpert".to_string(),
            description: "Diagnostics for borrow checker errors".to_string(),
            triggers: "cargo, clippy".to_string(),
            content: "You are a Rust expert.".to_string(),
            pinned: true,
            updated_at: None,
        };

        let skill2 = SkillRecord {
            name: "PythonExpert".to_string(),
            description: "Virtualenv and packaging helper".to_string(),
            triggers: "pytest, poetry".to_string(),
            content: "You are a Python expert.".to_string(),
            pinned: false,
            updated_at: None,
        };

        store
            .sync_skills(&[skill1.clone(), skill2])
            .await
            .unwrap();

        assert_eq!(store.search_skills("", 10).await.unwrap().len(), 0);

        // Search by name/content
        let rust_hits = store.search_skills("Rust", 10).await.unwrap();
        assert_eq!(rust_hits.len(), 1);
        assert_eq!(rust_hits[0].name, "RustExpert");

        // Search by `triggers` keyword that does NOT appear in `name` or `content`
        let trigger_hits = store.search_skills("poetry", 10).await.unwrap();
        assert_eq!(trigger_hits.len(), 1);
        assert_eq!(trigger_hits[0].name, "PythonExpert");

        // Search by `description` keyword that does NOT appear in `name` or `content`
        let desc_hits = store.search_skills("Virtualenv", 10).await.unwrap();
        assert_eq!(desc_hits.len(), 1);
        assert_eq!(desc_hits[0].name, "PythonExpert");

        // Syncing identical skill data must be a write no-op
        sqlx::query("UPDATE skills SET updated_at = '2025-01-01 00:00:00' WHERE name = 'RustExpert'")
            .execute(db.pool())
            .await
            .unwrap();
        store
            .sync_skills(std::slice::from_ref(&skill1))
            .await
            .unwrap();
        let unchanged = store.search_skills("Rust", 10).await.unwrap();
        assert_eq!(
            unchanged[0].updated_at.as_deref(),
            Some("2025-01-01 00:00:00")
        );

        // Updating content updates row + FTS5 cleanly, and PythonExpert was deleted
        let skill1_updated = SkillRecord {
            content: "Updated Rust expert content.".to_string(),
            ..skill1
        };
        store
            .sync_skills(std::slice::from_ref(&skill1_updated))
            .await
            .unwrap();

        let all_skills = store.search_skills("expert", 10).await.unwrap();
        assert_eq!(all_skills.len(), 1);
        assert_eq!(all_skills[0].name, "RustExpert");
        assert_eq!(all_skills[0].content, "Updated Rust expert content.");

        sqlx::raw_sql("INSERT INTO skills_fts(skills_fts) VALUES ('integrity-check')")
            .execute(db.pool())
            .await
            .unwrap();
    }
}
