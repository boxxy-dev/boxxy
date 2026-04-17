use crate::utils::load_prompt_fallback;
use log::debug;

pub async fn summarize_and_store(
    db: &Option<boxxy_db::Db>,
    session_id: &str,
    user_query: &str,
    assistant_response: &str,
    project_path: &str,
    creds: boxxy_ai_core::AiCredentials,
) {
    let settings = boxxy_preferences::Settings::load();

    let summarizer_template = load_prompt_fallback(
        "/dev/boxxy/BoxxyTerminal/prompts/memory_summarizer.md",
        "memory_summarizer.md",
    );

    let summarizer_prompt = summarizer_template
        .replace("{{user_query}}", user_query)
        .replace("{{assistant_response}}", assistant_response);

    // We use a simple agent call for summarization
    let agent = boxxy_ai_core::create_agent(
        &settings.claw_model,
        &creds,
        "You are a robotic memory compactor.",
    );

    if let Ok(res) = agent.prompt(&summarizer_prompt).await {
        if let Some(db) = db.as_ref() {
            let summary = res.0.trim().to_string();

            if summary == "NO_TECHNICAL_CHANGE" {
                debug!("Skipping memory storage: No technical change detected.");
                return;
            }

            let store = boxxy_db::store::Store::new(db.pool());

            // 1. Fetch recent interactions for deduplication check
            if let Ok(recent) = store.get_recent_interactions_by_path(project_path, 3).await {
                if !recent.is_empty() {
                    let mut dedup_context = String::from(
                        "You are a duplication detector. Compare the NEW summary with the EXISTING summaries. \
                        If the NEW summary is semantically identical (>= 90% match) to any EXISTING summary, \
                        output ONLY the ID of that existing summary. If it is unique, output 'UNIQUE'.\n\n\
                        EXISTING SUMMARIES:\n",
                    );

                    for r in &recent {
                        dedup_context.push_str(&format!("{}: {}\n", r.id, r.content));
                    }

                    dedup_context.push_str(&format!(
                        "\nNEW SUMMARY: {}\n\nOUTPUT (ID or UNIQUE):",
                        summary
                    ));

                    let dedup_agent = boxxy_ai_core::create_agent(
                        &settings
                            .memory_model
                            .clone()
                            .or(settings.claw_model.clone()),
                        &creds,
                        "You are a precise duplication detector. Output only ID or UNIQUE.",
                    );

                    if let Ok(dedup_res) = dedup_agent.prompt(&dedup_context).await {
                        let answer = dedup_res.0.trim();
                        if let Ok(id) = answer.parse::<i64>() {
                            debug!("Semantic duplicate found (ID: {}). Updating timestamp.", id);
                            let _ = store.touch_interaction(id).await;
                            return;
                        }
                    }
                }
            }

            // 2. No duplicate found, store as new
            let _ = store
                .add_interaction(session_id, Some(project_path), &summary, None, None)
                .await;
            debug!(
                "Stored new interaction summary for session {}: {}",
                session_id, summary
            );
        }
    }
}
