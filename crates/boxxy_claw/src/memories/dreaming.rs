use boxxy_ai_core::{AiCredentials, create_agent};
use boxxy_db::Db;
use boxxy_db::store::Store;
use boxxy_model_selection::ModelProvider;
use log::debug;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct DreamOrchestrator {
    db: Arc<Mutex<Option<Db>>>,
    creds: AiCredentials,
    memory_model: Option<ModelProvider>,
}

impl DreamOrchestrator {
    pub fn new(
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

    pub async fn run_cycle(&self) -> anyhow::Result<()> {
        debug!("🧠 Starting Dream Cycle...");

        // Phase 1: Ingestion
        let interactions = {
            let db_guard = self.db.lock().await;
            if let Some(db) = db_guard.as_ref() {
                let store = Store::new(db.pool());
                store.get_undreamed_interactions().await?
            } else {
                return Ok(());
            }
        };

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
            "Consolidate these interactions into permanent memories and patterns:\n\n{}",
            interactions_text
        );

        let (response, _, _) = agent
            .prompt(&prompt)
            .await
            .map_err(|e| anyhow::anyhow!("LLM Error: {:?}", e))?;

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
    pub async fn apply_dream_response(
        &self,
        interactions: &[boxxy_db::models::Interaction],
        response: &str,
    ) -> anyhow::Result<(Vec<(String, String, i64, f64)>, Vec<String>)> {
        let cleaned = extract_json_object(response);
        let json: serde_json::Value = serde_json::from_str(cleaned).map_err(|e| {
            log::warn!("Dream Cycle: failed to parse LLM JSON response: {}", e);
            anyhow::anyhow!("Dream Cycle JSON parse error: {e}")
        })?;

        // Phase 3: REM (Promotion & Diary)
        let interaction_ids: Vec<i64> = interactions.iter().map(|i| i.id).collect();
        let mut session_ids: Vec<String> =
            interactions.iter().map(|i| i.session_id.clone()).collect();
        session_ids.sort();
        session_ids.dedup();

        let mut promoted_memories = Vec::new();
        let mut patterns = Vec::new();

        {
            let db_guard = self.db.lock().await;
            if let Some(db) = db_guard.as_ref() {
                let store = Store::new(db.pool());

                // 1. Process Facts (Candidates)
                if let Some(facts) = json.get("facts").and_then(|f| f.as_array()) {
                    for fact in facts {
                        if let (Some(key), Some(content)) = (
                            fact.get("key").and_then(|k| k.as_str()),
                            fact.get("content").and_then(|c| c.as_str()),
                        ) {
                            let path = fact
                                .get("project_path")
                                .and_then(|p| p.as_str())
                                .unwrap_or("global");
                            let path_opt = if path == "global" { None } else { Some(path) };
                            let confidence = fact
                                .get("confidence_score")
                                .and_then(|c| c.as_f64())
                                .unwrap_or(0.5);

                            let _ = store
                                .upsert_dream_candidate(key, path_opt, content, confidence)
                                .await;
                            debug!("Dreaming stored Candidate Fact: {} -> {}", key, content);
                        }
                    }
                }

                // 2. Handle Conflicts
                if let Some(conflicts) = json.get("conflicts").and_then(|c| c.as_array()) {
                    for conflict in conflicts {
                        if let Some(key) = conflict.get("key").and_then(|k| k.as_str()) {
                            // Find all paths where this key exists and demote them
                            let paths = store.get_memory_paths(key).await.unwrap_or_default();
                            let mut demoted_any = false;
                            for path in &paths {
                                let path_opt = if path == "global" {
                                    None
                                } else {
                                    Some(path.as_str())
                                };
                                if store
                                    .demote_memory_by_key(key, path_opt)
                                    .await
                                    .unwrap_or(false)
                                {
                                    demoted_any = true;
                                    debug!(
                                        "Dream Cycle demoted conflicted key '{}' in path '{}'",
                                        key, path
                                    );
                                }
                            }

                            if !demoted_any {
                                debug!(
                                    "Dream Cycle conflict reported for key '{}', but no demotable memory was found.",
                                    key
                                );
                            }

                            if let Some(resolved) =
                                conflict.get("resolved_content").and_then(|c| c.as_str())
                            {
                                // Re-candidate the fix. We default to 'global' if we demoted something global,
                                // otherwise we use the first path we found or just global.
                                let target_path = if paths.contains(&"global".to_string()) {
                                    None
                                } else {
                                    paths.first().map(|p| p.as_str())
                                };
                                let _ = store
                                    .upsert_dream_candidate(key, target_path, resolved, 0.9)
                                    .await;
                                debug!(
                                    "Dream Cycle re-candidated resolved key: {} -> {}",
                                    key, resolved
                                );
                            }
                        }
                    }
                }

                // 3. Promote Threshold Memories (Deep Sleep)
                promoted_memories = store.promote_threshold_memories(2, 0.8).await?;

                // Mark interactions as dreamed
                let _ = store.mark_interactions_as_dreamed(&interaction_ids).await;

                // Update session dream timestamps
                for sid in session_ids {
                    let _ = store.update_session_dream_timestamp(&sid).await;
                }
            }
        }

        if let Some(pats) = json.get("patterns").and_then(|p| p.as_array()) {
            for pat in pats {
                if let Some(p) = pat.as_str() {
                    patterns.push(p.to_string());
                }
            }
        }

        Ok((promoted_memories, patterns))
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
            let _ = std::fs::create_dir_all(parent);
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&dreams_md_path)?;

        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        writeln!(file, "## 🌙 Dream Cycle - {}", timestamp)?;

        for (key, content, count, score) in promoted {
            writeln!(
                file,
                "- 🛡️ Verified: {} -> \"{}\" ({} observations, {:.2} confidence)",
                key, content, count, score
            )?;
        }

        for pat in patterns {
            writeln!(file, "- Pattern: {}", pat)?;
        }
        writeln!(file)?;
        Ok(())
    }
}

fn extract_json_object(raw: &str) -> &str {
    let trimmed = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && end >= start
    {
        return &trimmed[start..=end];
    }
    trimmed
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

        let llm_response = r#"Here is the consolidated output:
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
```"#;

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
