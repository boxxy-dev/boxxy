use boxxy_db::Db;
use boxxy_db::store::Store;
use boxxy_model_selection::ModelProvider;
use boxxy_ai_core::{AiCredentials, create_agent};
use std::sync::Arc;
use tokio::sync::Mutex;
use log::{info, debug};
use directories::ProjectDirs;
use std::fs::OpenOptions;
use std::io::Write;

pub struct DreamOrchestrator {
    db: Arc<Mutex<Option<Db>>>,
    creds: AiCredentials,
    memory_model: Option<ModelProvider>,
}

impl DreamOrchestrator {
    pub fn new(db: Arc<Mutex<Option<Db>>>, creds: AiCredentials, memory_model: Option<ModelProvider>) -> Self {
        Self { db, creds, memory_model }
    }

    pub async fn run_cycle(&self) -> anyhow::Result<()> {
        info!("🧠 Starting Dream Cycle...");

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

        info!("Dreaming about {} interactions...", interactions.len());

        // Phase 2: Scoring & Promotion (LLM)
        let mut fact_candidates = Vec::new();
        let mut patterns = Vec::new();
        
        let interactions_text = interactions.iter()
            .map(|i| format!("[ID: {}][Session: {}][Path: {}] {}", i.id, i.session_id, i.project_path.as_deref().unwrap_or("global"), i.content))
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
            Phase 2: Conflict Resolution. If you see conflicting info (e.g., user changed preference), flag it. \
            \
            Return a JSON object with: \
            - 'facts': array of { 'key': snake_case, 'project_path': string or 'global', 'content': string } \
            - 'patterns': array of strings describing observed behaviors \
            - 'conflicts': array of { 'issue': string, 'details': string } \
            \
            CRITICAL: Be extremely selective. Only extract information that is truly durable and useful for future context. Avoid transient state."
        );

        let prompt = format!("Consolidate these interactions into permanent memories and patterns:\n\n{}", interactions_text);

        let (response, _) = agent.prompt(&prompt).await.map_err(|e| anyhow::anyhow!("LLM Error: {:?}", e))?;

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) {
            if let Some(facts) = json.get("facts").and_then(|f| f.as_array()) {
                for fact in facts {
                    if let (Some(key), Some(content)) = (
                        fact.get("key").and_then(|k| k.as_str()),
                        fact.get("content").and_then(|c| c.as_str())
                    ) {
                        let path = fact.get("project_path").and_then(|p| p.as_str()).unwrap_or("global");
                        fact_candidates.push((key.to_string(), path.to_string(), content.to_string()));
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
            
            // Phase 3: REM (Promotion & Diary)
            let interaction_ids: Vec<i64> = interactions.iter().map(|i| i.id).collect();
            let mut session_ids: Vec<String> = interactions.iter().map(|i| i.session_id.clone()).collect();
            session_ids.sort();
            session_ids.dedup();

            {
                let db_guard = self.db.lock().await;
                if let Some(db) = db_guard.as_ref() {
                    let store = Store::new(db.pool());
                    
                    // Promote facts
                    for (key, path, content) in fact_candidates {
                        let path_opt = if path == "global" { None } else { Some(path.as_str()) };
                        let _ = store.add_memory(&key, path_opt, &content, Some("dreamed"), true, false).await;
                        info!("Dreaming promoted Fact: {} -> {}", key, content);
                    }

                    // Mark interactions as dreamed
                    let _ = store.mark_interactions_as_dreamed(&interaction_ids).await;
                    
                    // Update session dream timestamps
                    for sid in session_ids {
                        let _ = store.update_session_dream_timestamp(&sid).await;
                    }
                }
            }

            // Sync MEMORY.md
            let _ = crate::memories::db::sync_memories_to_markdown(self.db.clone()).await;
            
            // Update DREAMS.md
            self.append_to_dream_diary(&patterns).await?;
        }

        info!("Dream Cycle complete.");
        Ok(())
    }

    async fn append_to_dream_diary(&self, patterns: &[String]) -> anyhow::Result<()> {
        if patterns.is_empty() {
            return Ok(());
        }

        if let Some(dirs) = ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir();
            let dreams_md_path = config_dir.join("boxxyclaw").join("DREAMS.md");

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&dreams_md_path)?;

            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            writeln!(file, "## 🌙 Dream Cycle - {}", timestamp)?;
            for pat in patterns {
                writeln!(file, "- Pattern: {}", pat)?;
            }
            writeln!(file)?;
        }
        Ok(())
    }
}
