use boxxy_ai_core::{AiCredentials, create_agent};
use boxxy_db::Db;
use boxxy_db::store::Store;
use boxxy_model_selection::ModelProvider;
use log::debug;
use std::fmt::Write as _;
use std::sync::Arc;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

#[derive(Debug, Default, serde::Deserialize)]
struct DreamCyclePayload {
    #[serde(default)]
    facts: Vec<DreamFact>,
    #[serde(default)]
    patterns: Vec<String>,
    #[serde(default)]
    conflicts: Vec<DreamConflict>,
}

const fn default_dream_confidence() -> f64 {
    0.5
}

#[derive(Debug, serde::Deserialize)]
struct DreamFact {
    #[serde(default)]
    key: String,
    #[serde(default)]
    project_path: Option<String>,
    #[serde(default)]
    content: String,
    #[serde(default = "default_dream_confidence")]
    confidence_score: f64,
}

#[derive(Debug, Default, serde::Deserialize)]
struct DreamConflict {
    #[serde(default)]
    key: String,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "deserialized from the Dream Auditor JSON schema for completeness"
    )]
    issue: Option<String>,
    #[serde(default)]
    resolved_content: Option<String>,
}

pub struct DreamOrchestrator {
    db: Arc<Mutex<Option<Db>>>,
    creds: AiCredentials,
    memory_model: Option<ModelProvider>,
}

impl DreamOrchestrator {
    #[must_use]
    pub const fn new(
        db: Arc<Mutex<Option<Db>>>,
        creds: AiCredentials,
        memory_model: Option<ModelProvider>,
    ) -> Self {
        Self {
            db,
            creds,
            memory_model,
        }
    }

    /// Executes a complete three-phase background Dream Cycle.
    ///
    /// # Errors
    /// Returns an error if querying SQLite, calling the memory LLM, or parsing its JSON output fails.
    pub async fn run_cycle(&self) -> anyhow::Result<()> {
        debug!("🧠 Starting Dream Cycle...");

        // Phase 1: Ingestion
        let db_opt = self.db.lock().await.clone();
        let Some(db) = db_opt else {
            return Ok(());
        };
        let interactions = Store::new(db.pool()).get_undreamed_interactions().await?;

        if interactions.is_empty() {
            debug!("No new interactions to dream about.");
            return Ok(());
        }

        debug!("Dreaming about {} interactions...", interactions.len());

        // Phase 2: Scoring & Promotion (LLM)
        let interactions_text = interactions
            .iter()
            .map(|i| {
                format!(
                    "[ID: {}][Session: {}][Path: {}] {}",
                    i.id,
                    i.session_id,
                    i.project_path.as_deref().unwrap_or("global"),
                    i.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n---\n");

        let agent = create_agent(
            &self.memory_model,
            &self.creds,
            "You are the Boxxy Dream Auditor. Your job is to process short-term interaction logs into durable, high-signal facts and patterns. \
            Output ONLY valid JSON. \
            \
            Phase 1: Scoring & Extraction. Identify permanent facts (OS, hardware, preferred tools, user role) and behavioral patterns (prefers 'yarn' over 'npm', always uses 'ripgrep', works on Rust 2024 projects). \
            \
            Phase 2: Conflict Resolution. If you see info that contradicts existing verified memories, flag it. \
            \
            Return a JSON object with: \
            - 'facts': array of { 'key': snake_case string, 'project_path': string or 'global', 'content': string, 'confidence_score': float 0.0-1.0 (how certain you are this is a permanent, durable fact) } \
            - 'patterns': array of strings describing observed behaviors \
            - 'conflicts': array of { 'key': snake_case key of the contradicted fact, 'issue': string, 'resolved_content': string (the new corrected fact content) } \
            \
            CRITICAL: Be extremely selective. Only extract information that is truly durable and useful for future context. Avoid transient state.",
        );

        let prompt = format!(
            "Consolidate these interactions into permanent memories and patterns:\n\n{interactions_text}"
        );

        let (response, _, _) = agent
            .prompt(&prompt)
            .await
            .map_err(|e| anyhow::anyhow!("LLM Error: {e:?}"))?;

        let (promoted_memories, patterns) =
            self.apply_dream_response(&interactions, &response).await?;

        // Sync MEMORY.md
        let _ = crate::memories::db::sync_memories_to_markdown(self.db.clone()).await;

        // Update DREAMS.md
        let _ = self
            .append_to_dream_diary(&patterns, &promoted_memories)
            .await;

        debug!("Dream Cycle complete.");
        Ok(())
    }

    /// Parses the LLM JSON payload (stripping optional markdown code fences) and
    /// executes Phase 3 (candidate upsert, conflict resolution, threshold promotion,
    /// marking interactions as dreamed, and updating `last_dream_at`).
    ///
    /// # Errors
    /// Returns an error if `response` is not valid JSON or if promoting memories in SQLite fails.
    pub async fn apply_dream_response(
        &self,
        interactions: &[boxxy_db::models::Interaction],
        response: &str,
    ) -> anyhow::Result<(Vec<(String, String, i64, f64)>, Vec<String>)> {
        let payload: DreamCyclePayload = crate::utils::parse_llm_json(response).map_err(|e| {
            log::warn!("Dream Cycle: failed to parse LLM JSON response: {e}");
            anyhow::anyhow!("Dream Cycle JSON parse error: {e}")
        })?;

        let interaction_ids: Vec<i64> = interactions.iter().map(|i| i.id).collect();
        let mut session_ids: Vec<String> =
            interactions.iter().map(|i| i.session_id.clone()).collect();
        session_ids.sort();
        session_ids.dedup();

        let mut promoted_memories = Vec::new();
        let db_opt = self.db.lock().await.clone();
        if let Some(db) = db_opt {
            let store = Store::new(db.pool());
            Self::upsert_dream_facts(&store, &payload.facts).await;
            Self::resolve_dream_conflicts(&store, &payload.conflicts).await;
            promoted_memories = store.promote_threshold_memories(2, 0.8).await?;
            let _ = store.mark_interactions_as_dreamed(&interaction_ids).await;
            for sid in session_ids {
                let _ = store.update_session_dream_timestamp(&sid).await;
            }
        }

        Ok((promoted_memories, payload.patterns))
    }

    async fn upsert_dream_facts(store: &Store<'_>, facts: &[DreamFact]) {
        for fact in facts {
            let key = fact.key.trim();
            let content = fact.content.trim();
            if key.is_empty() || content.is_empty() {
                continue;
            }
            let path = fact
                .project_path
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .unwrap_or("global");
            let path_opt = (path != "global").then_some(path);
            let confidence = if fact.confidence_score.is_finite() {
                fact.confidence_score.clamp(0.0, 1.0)
            } else {
                0.5
            };

            let _ = crate::engine::context::upsert_candidate_deduped(
                store, key, path_opt, content, confidence,
            )
            .await;
            debug!("Dreaming stored Candidate Fact: {key} -> {content}");
        }
    }

    async fn resolve_dream_conflicts(store: &Store<'_>, conflicts: &[DreamConflict]) {
        for conflict in conflicts {
            let key = conflict.key.trim();
            if key.is_empty() {
                continue;
            }
            let paths = store.get_memory_paths(key).await.unwrap_or_default();
            let mut demoted_any = false;
            for path in &paths {
                let path_opt = (path != "global").then_some(path.as_str());
                if store.demote_memory_by_key(key, path_opt).await.unwrap_or(false) {
                    demoted_any = true;
                    debug!("Dream Cycle demoted conflicted key '{key}' in path '{path}'");
                }
            }
            if !demoted_any {
                debug!(
                    "Dream Cycle conflict reported for key '{key}', but no demotable memory was found."
                );
            }
            if let Some(resolved) = conflict
                .resolved_content
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let target_path = if paths.iter().any(|p| p == "global") {
                    None
                } else {
                    paths.first().map(String::as_str)
                };
                let _ = store
                    .upsert_dream_candidate(key, target_path, resolved, 0.9)
                    .await;
                debug!("Dream Cycle re-candidated resolved key: {key} -> {resolved}");
            }
        }
    }

    async fn append_to_dream_diary(
        &self,
        patterns: &[String],
        promoted: &[(String, String, i64, f64)],
    ) -> anyhow::Result<()> {
        if patterns.is_empty() && promoted.is_empty() {
            return Ok(());
        }

        let config_dir = boxxy_sys_utils::get_config_dir();
        let dreams_md_path = config_dir.join("boxxyclaw").join("DREAMS.md");
        if let Some(parent) = dreams_md_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let mut entry = String::new();
        let _ = writeln!(entry, "## 🌙 Dream Cycle - {timestamp}");
        for (key, content, count, score) in promoted {
            let _ = writeln!(
                entry,
                "- 🛡️ Verified: {key} -> \"{content}\" ({count} observations, {score:.2} confidence)"
            );
        }
        for pat in patterns {
            let _ = writeln!(entry, "- Pattern: {pat}");
        }
        entry.push('\n');

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&dreams_md_path)
            .await?;
        file.write_all(entry.as_bytes()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apply_dream_response_promotes_candidates_and_resolves_conflicts() {
        let db = Db::new_in_memory().await.unwrap();
        let store = Store::new(db.pool());

        // Seed a session and an interaction
        store.create_session("sess-1", "Test").await.unwrap();
        store
            .add_interaction("sess-1", Some("global"), "User prefers ripgrep", None, None)
            .await
            .unwrap();

        // Seed an existing candidate fact (observation_count = 1, confidence = 0.5)
        store
            .upsert_dream_candidate("search_tool", None, "Uses ripgrep (rg)", 0.5)
            .await
            .unwrap();

        // Seed an existing dreamed fact that will be conflicted and demoted
        store
            .upsert_dream_candidate("old_fact", None, "Uses npm", 0.9)
            .await
            .unwrap();
        store
            .upsert_dream_candidate("old_fact", None, "Uses npm", 0.9)
            .await
            .unwrap();
        let initial_promoted = store.promote_threshold_memories(2, 0.8).await.unwrap();
        assert_eq!(initial_promoted.len(), 1);

        let interactions = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(interactions.len(), 1);

        let orchestrator = DreamOrchestrator::new(
            Arc::new(Mutex::new(Some(db.clone()))),
            AiCredentials::default(),
            None,
        );

        let llm_response = r#"Processed [Session: sess-1]:
```json
{
  "facts": [
    {
      "key": "search_tool",
      "project_path": "global",
      "content": "Uses ripgrep (rg)",
      "confidence_score": 0.92
    }
  ],
  "patterns": [
    "Always runs cargo test after edits"
  ],
  "conflicts": [
    {
      "key": "old_fact",
      "issue": "Switched to bun",
      "resolved_content": "Uses bun"
    }
  ]
}
```
Note: {audit_complete}"#;

        let (promoted, patterns) = orchestrator
            .apply_dream_response(&interactions, llm_response)
            .await
            .unwrap();
        assert_eq!(promoted.len(), 1);
        assert_eq!(patterns.len(), 1);

        // Verify `search_tool` was promoted to verified=true, category='dreamed'
        let search_mem = store
            .get_memory_by_key("search_tool", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(search_mem.verified, Some(true));
        assert_eq!(search_mem.category.as_deref(), Some("dreamed"));
        assert_eq!(search_mem.observation_count, 2);

        // Verify `old_fact` was demoted and re-candidated with "Uses bun"
        let conflict_mem = store
            .get_memory_by_key("old_fact", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conflict_mem.verified, Some(false));
        assert_eq!(conflict_mem.content, "Uses bun");
        assert_eq!(conflict_mem.observation_count, 1);

        // Verify interactions were marked as dreamed
        let remaining = store.get_undreamed_interactions().await.unwrap();
        assert!(remaining.is_empty());
    }
}
