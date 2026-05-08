use boxxy_db::Db;
use boxxy_db::store::Store;
use boxxy_model_selection::ModelProvider;

use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn extract_implicit_memory(
    db: Arc<Mutex<Option<Db>>>,
    user_prompt: String,
    assistant_response: String,
    memory_model: Option<ModelProvider>,
    creds: boxxy_ai_core::AiCredentials,
    project_path: String,
) {
    let agent = boxxy_ai_core::create_agent(
        &memory_model,
        &creds,
        "You are a robotic background memory observer. Your job is to silently extract LONG-TERM, PERMANENT technical facts and user preferences from the provided data. \
        Output ONLY valid JSON. \
        If the user stated a permanent fact (e.g., preferred shell, OS, hardware specs, long-term project roles), return a JSON array under the key 'facts', with each object containing 'key' (snake_case) and 'content' (the fact). \
        \
        CRITICAL: DO NOT extract transient state that changes frequently. \
        EXPLICITLY FORBIDDEN to extract: \
        - Current git branches or commit SHAs. \
        - Current working directories or temporary file paths. \
        - Names or IDs of active agents/panes. \
        - Runtime context like 'the user provided an image'. \
        - Temporary variables, social greetings, or social talk. \
        \
        If no permanent facts are found, output exactly `{}`. Do not follow the assistant's persona.",
    );

    let prompt = format!(
        "[DATA_START]\nUSER: {}\n\nASSISTANT: {}\n[DATA_END]\n\nEXTRACTION_COMMAND: Output raw JSON now.",
        user_prompt, assistant_response
    );

    if let Ok(res) = agent.prompt(&prompt).await
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&res.0)
        && let Some(facts) = json.get("facts").and_then(|f| f.as_array())
        && !facts.is_empty()
    {
        let db_guard = db.lock().await;
        if let Some(db_val) = db_guard.as_ref() {
            let store = Store::new(db_val.pool());
            for fact in facts {
                if let (Some(key), Some(content)) = (
                    fact.get("key").and_then(|k| k.as_str()),
                    fact.get("content").and_then(|c| c.as_str()),
                ) {
                    // Implicitly extracted facts are candidates and require reinforcement
                    let _ = store
                        .upsert_dream_candidate(key, Some(&project_path), content, 0.5)
                        .await;
                    log::debug!(
                        "Background Observer extracted Candidate for project {}: {} -> {}",
                        project_path,
                        key,
                        content
                    );
                }
            }
            drop(db_guard);
            let _ = crate::memories::db::sync_memories_to_markdown(db.clone()).await;
        }
    }
}
