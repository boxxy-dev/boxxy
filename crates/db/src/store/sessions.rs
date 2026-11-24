use crate::models::{Session, SessionUpsert};
use crate::store::Store;
use anyhow::Result;

impl Store<'_> {
    pub async fn create_session(&self, id: &str, name: &str) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, name) VALUES (?, ?)")
            .bind(id)
            .bind(name)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn upsert_session(&self, session: &SessionUpsert<'_>) -> Result<()> {
        sqlx::query(
            r"
            INSERT INTO sessions (
                id, name, character_id, character_display_name, title,
                history_json, pending_tasks_json, agent_name, last_cwd, model_id,
                pinned, total_tokens, context_tokens, total_cost_usd, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                character_id = excluded.character_id,
                character_display_name = excluded.character_display_name,
                title = CASE WHEN excluded.title != '' THEN excluded.title ELSE sessions.title END,
                history_json = excluded.history_json,
                pending_tasks_json = excluded.pending_tasks_json,
                agent_name = excluded.agent_name,
                last_cwd = excluded.last_cwd,
                model_id = excluded.model_id,
                pinned = excluded.pinned,
                total_tokens = excluded.total_tokens,
                context_tokens = excluded.context_tokens,
                total_cost_usd = excluded.total_cost_usd,
                updated_at = CURRENT_TIMESTAMP
            ",
        )
        .bind(session.id)
        .bind(session.name)
        .bind(session.character_id)
        .bind(session.character_display_name)
        .bind(session.title)
        .bind(session.history_json)
        .bind(session.pending_tasks_json)
        .bind(session.agent_name)
        .bind(session.last_cwd)
        .bind(session.model_id)
        .bind(session.pinned)
        .bind(session.total_tokens)
        .bind(session.context_tokens)
        .bind(session.total_cost_usd)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_session_state(
        &self,
        id: &str,
        name: &str,
        character_id: &str,
        character_display_name: &str,
        title: &str,
        history_json: &str,
        pending_tasks_json: &str,
        agent_name: &str,
        last_cwd: &str,
        model_id: &str,
        pinned: bool,
        total_tokens: i64,
        context_tokens: i64,
        total_cost_usd: f64,
    ) -> Result<()> {
        self.upsert_session(&SessionUpsert {
            id,
            name,
            character_id,
            character_display_name,
            title,
            history_json,
            pending_tasks_json,
            agent_name,
            last_cwd,
            model_id,
            pinned,
            total_tokens,
            context_tokens,
            total_cost_usd,
        })
        .await
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let session = sqlx::query_as::<_, Session>(
            r"
            SELECT
                id, name, history_json, pending_tasks_json, agent_name,
                character_id, character_display_name, last_cwd, title, model_id,
                pinned, total_tokens, context_tokens, total_cost_usd,
                last_dream_at, created_at, updated_at,
                0 AS message_count
            FROM sessions
            WHERE id = ?
            ",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(session)
    }

    pub async fn get_recent_active_sessions(&self, limit: i64) -> Result<Vec<Session>> {
        if limit <= 0 {
            return Ok(Vec::new());
        }

        // Walk `idx_sessions_pinned_updated` directly in `updated_at DESC` order using `EXISTS`
        // so SQLite stops scanning unpinned sessions as soon as `limit` active rows are found,
        // and compute `message_count` from the covering `idx_claw_events_session_id` index
        // without reading large `event_json` heap pages or allocating `history_json`.
        let records = sqlx::query_as::<_, Session>(
            r"
            SELECT * FROM (
                SELECT
                    s.id,
                    s.name,
                    NULL AS history_json,
                    NULL AS pending_tasks_json,
                    s.agent_name,
                    s.character_id,
                    s.character_display_name,
                    s.last_cwd,
                    s.title,
                    s.model_id,
                    s.pinned,
                    s.total_tokens,
                    s.context_tokens,
                    s.total_cost_usd,
                    s.last_dream_at,
                    s.created_at,
                    s.updated_at,
                    (SELECT COUNT(*) FROM claw_events e WHERE e.session_id = s.id) AS message_count
                FROM sessions s
                WHERE s.pinned = true
                  AND EXISTS (SELECT 1 FROM claw_events e WHERE e.session_id = s.id)
                ORDER BY s.updated_at DESC
            )
            UNION ALL
            SELECT * FROM (
                SELECT
                    s.id,
                    s.name,
                    NULL AS history_json,
                    NULL AS pending_tasks_json,
                    s.agent_name,
                    s.character_id,
                    s.character_display_name,
                    s.last_cwd,
                    s.title,
                    s.model_id,
                    s.pinned,
                    s.total_tokens,
                    s.context_tokens,
                    s.total_cost_usd,
                    s.last_dream_at,
                    s.created_at,
                    s.updated_at,
                    (SELECT COUNT(*) FROM claw_events e WHERE e.session_id = s.id) AS message_count
                FROM sessions s
                WHERE s.pinned = false
                  AND EXISTS (SELECT 1 FROM claw_events e WHERE e.session_id = s.id)
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
            "SELECT event_json FROM claw_events WHERE session_id = ? ORDER BY id ASC",
        )
        .bind(session_id)
        .fetch_all(self.pool)
        .await?;

        Ok(records.into_iter().map(|(json,)| json).collect())
    }

    pub async fn clear_claw_events(&self, session_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM claw_events WHERE session_id = ?")
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_session_pinned(&self, session_id: &str, pinned: bool) -> Result<()> {
        sqlx::query("UPDATE sessions SET pinned = ? WHERE id = ?")
            .bind(pinned)
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_session_title(&self, session_id: &str, title: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET title = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(title)
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_session_pending_tasks(
        &self,
        session_id: &str,
        pending_tasks_json: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET pending_tasks_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(pending_tasks_json)
        .bind(session_id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Persists a character reassignment so orphan fallbacks are only resolved once.
    /// Preserves `updated_at` so relative session age in `/resume` is not altered.
    pub async fn update_session_character(
        &self,
        session_id: &str,
        character_id: &str,
        character_display_name: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET character_id = ?, character_display_name = ?, name = ?, agent_name = ? WHERE id = ?",
        )
        .bind(character_id)
        .bind(character_display_name)
        .bind(character_display_name)
        .bind(character_display_name)
        .bind(session_id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Atomically migrates all sessions belonging to `old_character_id` to a replacement character,
    /// recording a `Character Switch` system event in `claw_events` for any non-empty session
    /// while preserving all historical message rows and `updated_at` timestamps.
    pub async fn migrate_character_sessions(
        &self,
        old_character_id: &str,
        new_character_id: &str,
        new_display_name: &str,
    ) -> Result<u64> {
        if old_character_id == new_character_id {
            return Ok(0);
        }

        let mut tx = self.pool.begin().await?;

        let affected_sessions: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
            r"
            SELECT s.id, s.character_display_name, s.agent_name, s.name
            FROM sessions s
            WHERE s.character_id = ?
              AND EXISTS (SELECT 1 FROM claw_events e WHERE e.session_id = s.id)
            ",
        )
        .bind(old_character_id)
        .fetch_all(&mut *tx)
        .await?;

        let res = sqlx::query(
            "UPDATE sessions SET character_id = ?, character_display_name = ?, name = ?, agent_name = ? WHERE character_id = ?",
        )
        .bind(new_character_id)
        .bind(new_display_name)
        .bind(new_display_name)
        .bind(new_display_name)
        .bind(old_character_id)
        .execute(&mut *tx)
        .await?;

        let new_pretty = prettify_character_label(new_display_name);
        for (sess_id, char_disp, agent_name, sess_name) in affected_sessions {
            let old_raw = if !char_disp.trim().is_empty() {
                char_disp
            } else if let Some(a) = agent_name.filter(|s| !s.trim().is_empty()) {
                a
            } else if !sess_name.trim().is_empty() {
                sess_name
            } else {
                "Previous Character".to_string()
            };
            let old_pretty = prettify_character_label(&old_raw);
            if !old_pretty.eq_ignore_ascii_case(&new_pretty) {
                let switch_content = format!("Character Switch: {old_pretty} → {new_pretty}");
                let event_json = serde_json::json!({
                    "SystemMessage": {
                        "pane_id": "system",
                        "content": switch_content
                    }
                })
                .to_string();
                sqlx::query("INSERT INTO claw_events (session_id, event_json) VALUES (?, ?)")
                    .bind(&sess_id)
                    .bind(&event_json)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        tx.commit().await?;
        Ok(res.rows_affected())
    }

    /// Returns all distinct `character_id` values currently stored in `sessions`.
    pub async fn get_distinct_session_character_ids(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT character_id FROM sessions")
            .fetch_all(self.pool)
            .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Ensures a session row exists with its character identity before Turn 1 visual events
    /// are inserted into `claw_events` (which has a foreign key to `sessions(id)`).
    pub async fn ensure_session_exists(
        &self,
        id: &str,
        character_id: &str,
        character_display_name: &str,
        cwd: &str,
    ) -> Result<()> {
        sqlx::query(
            r"
            INSERT INTO sessions (id, name, character_id, character_display_name, agent_name, last_cwd)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                character_id = CASE WHEN excluded.character_id != '' THEN excluded.character_id ELSE sessions.character_id END,
                character_display_name = CASE WHEN excluded.character_display_name != '' THEN excluded.character_display_name ELSE sessions.character_display_name END,
                agent_name = CASE WHEN excluded.agent_name != '' THEN excluded.agent_name ELSE sessions.agent_name END
            ",
        )
        .bind(id)
        .bind(character_display_name)
        .bind(character_id)
        .bind(character_display_name)
        .bind(character_display_name)
        .bind(cwd)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_session_dream_timestamp(&self, session_id: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET last_dream_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Updates cumulative `total_tokens` and `total_cost_usd` for an already-persisted session
    /// (such as after an ephemeral `/btw` side question) without creating a phantom session row
    /// or modifying `context_tokens` / `updated_at`.
    pub async fn update_session_usage_totals(
        &self,
        session_id: &str,
        total_tokens: i64,
        total_cost_usd: f64,
    ) -> Result<()> {
        sqlx::query("UPDATE sessions SET total_tokens = ?, total_cost_usd = ? WHERE id = ?")
            .bind(total_tokens)
            .bind(total_cost_usd)
            .bind(session_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn prettify_character_label(raw: &str) -> String {
    raw.split(['-', ' '])
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use crate::Db;
    use crate::store::Store;

    #[tokio::test]
    async fn test_session_operations_and_pending_tasks() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let session_id = "sess_1";
        let session_name = "Session 1";

        store
            .create_session(session_id, session_name)
            .await
            .unwrap();

        let session = store
            .get_session(session_id)
            .await
            .unwrap()
            .expect("Session should exist");
        assert_eq!(session.id, session_id);
        assert_eq!(session.name, session_name);
        assert_eq!(session.title, None);
        assert!(!session.pinned);

        store
            .upsert_session_state(
                session_id,
                session_name,
                "00000001-0000-4000-8000-000000000001",
                "niko una",
                "Updated Title",
                "[\"history\"]",
                "[\"tasks\"]",
                "Agent X",
                "workspace/app",
                "model_1",
                true,
                42,
                21,
                0.05,
            )
            .await
            .unwrap();

        let updated = store
            .get_session(session_id)
            .await
            .unwrap()
            .expect("Session should exist");
        assert_eq!(updated.title.as_deref(), Some("Updated Title"));
        assert!(updated.pinned);
        assert_eq!(updated.total_tokens, 42);
        assert_eq!(updated.context_tokens, 21);
        assert!((updated.total_cost_usd - 0.05).abs() < 1e-9);

        store
            .update_session_pending_tasks(session_id, "[{\"id\":\"t1\"}]")
            .await
            .unwrap();
        let after_tasks = store.get_session(session_id).await.unwrap().unwrap();
        assert_eq!(
            after_tasks.pending_tasks_json.as_deref(),
            Some("[{\"id\":\"t1\"}]")
        );

        store
            .add_claw_event(session_id, "{\"event\": 1}")
            .await
            .unwrap();
        store
            .add_claw_event(session_id, "{\"event\": 2}")
            .await
            .unwrap();

        let events = store.get_claw_events(session_id).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], "{\"event\": 1}");

        let recent = store.get_recent_active_sessions(10).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, session_id);
        assert_eq!(recent[0].message_count, 2);

        store.clear_claw_events(session_id).await.unwrap();
        assert_eq!(
            store.get_claw_events(session_id).await.unwrap(),
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn test_update_session_character() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        let session_id = "sess_char_update";
        store
            .upsert_session_state(
                session_id,
                "Test Session",
                "old-uuid-111",
                "Old Character",
                "",
                "[]",
                "[]",
                "old-character",
                "workspace/app",
                "",
                false,
                0,
                0,
                0.0,
            )
            .await
            .unwrap();

        let before = store.get_session(session_id).await.unwrap().unwrap();
        assert_eq!(before.character_id, "old-uuid-111");
        assert_eq!(before.character_display_name, "Old Character");

        store
            .update_session_character(session_id, "new-uuid-222", "New Character")
            .await
            .unwrap();

        let after = store.get_session(session_id).await.unwrap().unwrap();
        assert_eq!(after.character_id, "new-uuid-222");
        assert_eq!(after.character_display_name, "New Character");
        assert_eq!(after.name, "New Character");
        assert_eq!(after.agent_name.as_deref(), Some("New Character"));
    }

    #[tokio::test]
    async fn test_migrate_character_sessions_and_ensure_session_exists() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        store
            .ensure_session_exists("sess_a", "char-uuid-b", "Character B", "workspace")
            .await
            .unwrap();
        store
            .ensure_session_exists("sess_b", "char-uuid-b", "Character B", "workspace")
            .await
            .unwrap();
        store
            .ensure_session_exists("sess_c", "char-uuid-c", "Character C", "workspace")
            .await
            .unwrap();

        store
            .add_claw_event("sess_a", "{\"type\":\"user\"}")
            .await
            .unwrap();

        let distinct = store.get_distinct_session_character_ids().await.unwrap();
        assert!(distinct.contains(&"char-uuid-b".to_string()));
        assert!(distinct.contains(&"char-uuid-c".to_string()));

        // Self-migration is a safe no-op
        assert_eq!(
            store
                .migrate_character_sessions("char-uuid-b", "char-uuid-b", "Character B")
                .await
                .unwrap(),
            0
        );

        let migrated = store
            .migrate_character_sessions("char-uuid-b", "char-uuid-c", "Character C")
            .await
            .unwrap();
        assert_eq!(migrated, 2);

        let s_a = store.get_session("sess_a").await.unwrap().unwrap();
        assert_eq!(s_a.character_id, "char-uuid-c");
        assert_eq!(s_a.character_display_name, "Character C");
        assert_eq!(s_a.agent_name.as_deref(), Some("Character C"));
    }

    #[tokio::test]
    async fn test_session_pin_toggle_and_delete() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        for (id, char_id, name, title, tokens) in [
            ("sess_pin_1", "char-1", "niko", "First Session", 10),
            ("sess_pin_2", "char-2", "levi", "Second Session", 20),
        ] {
            store
                .upsert_session_state(
                    id, name, char_id, name, title, "[]", "[]", name, "workspace", "model",
                    false, tokens, tokens, 0.0,
                )
                .await
                .unwrap();
            store
                .add_claw_event(id, "{\"type\":\"user\"}")
                .await
                .unwrap();
        }

        store.set_session_pinned("sess_pin_1", true).await.unwrap();
        let recent = store.get_recent_active_sessions(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, "sess_pin_1");
        assert!(recent[0].pinned);
        assert!(recent[0].history_json.is_none());
        assert!(recent[0].pending_tasks_json.is_none());

        let full_s1 = store.get_session("sess_pin_1").await.unwrap().unwrap();
        assert_eq!(full_s1.history_json.as_deref(), Some("[]"));
        assert_eq!(full_s1.pending_tasks_json.as_deref(), Some("[]"));

        store.set_session_pinned("sess_pin_1", false).await.unwrap();
        assert!(!store.get_session("sess_pin_1").await.unwrap().unwrap().pinned);

        store.delete_session("sess_pin_1").await.unwrap();
        assert!(store.get_session("sess_pin_1").await.unwrap().is_none());
        assert_eq!(
            store.get_claw_events("sess_pin_1").await.unwrap(),
            Vec::<String>::new()
        );

        let remaining = store.get_recent_active_sessions(10).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "sess_pin_2");

        // `update_session_usage_totals` updates cumulative counters without touching context_tokens
        // and never inserts a phantom row for a non-existent session ID.
        store
            .update_session_usage_totals("sess_nonexistent", 500, 0.02)
            .await
            .unwrap();
        assert!(store.get_session("sess_nonexistent").await.unwrap().is_none());

        store
            .update_session_usage_totals("sess_pin_2", 350, 0.04)
            .await
            .unwrap();
        let s2 = store.get_session("sess_pin_2").await.unwrap().unwrap();
        assert_eq!(s2.total_tokens, 350);
        assert_eq!(s2.context_tokens, 20);
        assert!((s2.total_cost_usd - 0.04).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_migrate_character_sessions_preserves_events_and_appends_switch_banner() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        store
            .upsert_session_state(
                "sess_migrated",
                "custom-hero",
                "char-custom",
                "Custom Hero",
                "Refactor Task",
                "[]",
                "[]",
                "Custom Hero",
                "workspace",
                "model",
                false,
                50,
                50,
                0.01,
            )
            .await
            .unwrap();
        store
            .add_claw_event(
                "sess_migrated",
                "{\"Diagnosis\":{\"pane_id\":\"p1\",\"agent_name\":\"Custom Hero\",\"character_id\":\"char-custom\",\"content\":\"Hello\",\"usage\":null}}",
            )
            .await
            .unwrap();

        let affected = store
            .migrate_character_sessions("char-custom", "char-niko", "Niko Una")
            .await
            .unwrap();
        assert_eq!(affected, 1);

        let updated = store.get_session("sess_migrated").await.unwrap().unwrap();
        assert_eq!(updated.character_id, "char-niko");
        assert_eq!(updated.character_display_name, "Niko Una");

        let events = store.get_claw_events("sess_migrated").await.unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("\"agent_name\":\"Custom Hero\""));
        assert!(events[0].contains("\"character_id\":\"char-custom\""));
        assert!(events[1].contains("Character Switch: Custom Hero → Niko Una"));
    }
}
