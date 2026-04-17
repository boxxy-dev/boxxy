use log::debug;
use crate::utils::load_prompt_fallback;

/// Scans the config directory for characters to build the session context.
pub fn load_session_context() -> String {
    let mut context = String::new();

    if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        let config_dir = dirs.config_dir();
        let boxxyclaw_dir = config_dir.join("boxxyclaw");

        // 1. Load Default Character (Boxxy)
        // For now, we just load Boxxy. Later, we'll allow selection.
        let boxxy_md = boxxyclaw_dir
            .join("characters")
            .join("boxxy")
            .join("BOXXY.md");
        if let Ok(content) = std::fs::read_to_string(boxxy_md) {
            context.push_str("\n--- CHARACTER ---\n");
            context.push_str(&content);
            context.push('\n');
        }

        // 2. Load Long-term Memory (CLAW_STATE.md)
        let memory_md = boxxyclaw_dir.join("CLAW_STATE.md");
        if let Ok(content) = std::fs::read_to_string(memory_md) {
            context.push_str("\n--- LONG-TERM MEMORY ---\n");
            context.push_str(
                "Below are facts, preferences, and lessons you've learned in past sessions. \
            Respect these rules and use this information to be more helpful.\n\n",
            );
            context.push_str(&content);
            context.push('\n');
        }
    }

    context
}

pub async fn retrieve_memories(
    db: &Option<boxxy_db::Db>,
    query: &str,
    project_path: &str,
) -> String {
    if let Some(db) = db.as_ref() {
        let store = boxxy_db::store::Store::new(db.pool());
        let settings = boxxy_preferences::Settings::load();

        let expansion_prompt_template = load_prompt_fallback(
            "/dev/boxxy/BoxxyTerminal/prompts/memory_expansion.md",
            "memory_expansion.md",
        );

        let expansion_prompt = expansion_prompt_template.replace("{{query}}", query);

        let model = settings
            .memory_model
            .clone()
            .or(settings.claw_model.clone());

        let creds = boxxy_ai_core::AiCredentials::new(
            settings.get_effective_api_keys(),
            settings.ollama_base_url.clone(),
        );

        let agent = boxxy_ai_core::create_agent(
            &model,
            &creds,
            "You are a search optimizer. Output only comma-separated keywords.",
        );

        // Fallback to basic cleaned query if expansion fails
        let mut fts_query = query.replace(['"', '\'', '?'], "");

        if let Ok(res) = agent.prompt(&expansion_prompt).await {
            let keywords = res.0;
            let cleaned = keywords.trim().replace(", ", " OR ").replace(',', " OR ");
            // FTS5 can fail if the LLM outputs weird syntax, but we'll try it
            fts_query = cleaned;
            debug!("Expanded memory search query: {}", fts_query);
        }

        // Search both episodic interactions and long-term facts
        let mut result = String::new();
        let mut current_budget_chars = 0;
        let max_budget_chars = 2000; // Roughly 500 tokens. A lightweight token budget.

        // 0. Always inject Pinned Memories first
        if let Ok(pinned_memories) = store.get_pinned_memories(Some(project_path)).await
            && !pinned_memories.is_empty()
        {
            result.push_str("\n--- PINNED FACTS (CRITICAL) ---\n");
            for mem in pinned_memories {
                let line = format!("- {}: {}\n", mem.key, mem.content);
                result.push_str(&line);
                current_budget_chars += line.len();
            }
        }

        // 1. Search Long-term Memories (Facts)
        // We pull more records initially (e.g., 20) and then filter by budget
        if let Ok(memories) = store
            .search_memories(&fts_query, Some(project_path), 20)
            .await
            && !memories.is_empty()
            && current_budget_chars < max_budget_chars
        {
            result.push_str("\n--- RELEVANT PREFERENCES & FACTS ---\n");
            for mem in memories {
                let line = format!("- {}: {}\n", mem.key, mem.content);
                if current_budget_chars + line.len() > max_budget_chars {
                    break;
                }
                result.push_str(&line);
                current_budget_chars += line.len();
            }
        }

        // 2. Search Past Interactions (Summaries)
        if let Ok(interactions) = store
            .search_interactions(&fts_query, Some(project_path), 20)
            .await
            && !interactions.is_empty()
            && current_budget_chars < max_budget_chars
        {
            result.push_str(
                "\n--- RELEVANT PAST INTERACTIONS ---\n\
                Below are relevant experiences or facts you've encountered in previous sessions:\n",
            );
            for interaction in interactions {
                let line = format!("- {}\n", interaction.content);
                if current_budget_chars + line.len() > max_budget_chars {
                    break;
                }
                result.push_str(&line);
                current_budget_chars += line.len();
            }
        }

        if !result.is_empty() {
            result.push('\n');
        }
        return result;
    }
    String::new()
}
