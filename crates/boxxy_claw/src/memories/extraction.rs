use std::sync::Arc;
use tokio::sync::Mutex;
use boxxy_db::Db;
use boxxy_db::store::Store;
use log::info;
use boxxy_model_selection::ModelProvider;

pub async fn extract_implicit_memory(
    db: Arc<Mutex<Option<Db>>>,
    user_prompt: String,
    assistant_response: String,
    memory_model: ModelProvider,
    gemini_key: String,
    ollama_url: String,
    project_path: String,
) {
    let agent = boxxy_ai_core::create_agent(
        &memory_model,
        &gemini_key,
        &ollama_url,
        "You are a background memory observer. Your job is to silently extract permanent facts, preferences, and project-specific paths from the conversation. \
        Output ONLY valid JSON. \
        If the user stated a permanent fact, return a JSON array under the key 'facts', with each object containing 'key' (snake_case) and 'content' (the fact). \
        If the user's message is just a command or transient question, output exactly `{}`. Do not hallucinate."
    );

    let prompt = format!(
        "USER: {}\n\nASSISTANT: {}",
        user_prompt, assistant_response
    );

    if let Ok(response) = agent.prompt(&prompt).await {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) {
            if let Some(facts) = json.get("facts").and_then(|f| f.as_array()) {
                if !facts.is_empty() {
                    let db_guard = db.lock().await;
                    if let Some(db_val) = db_guard.as_ref() {
                        let store = Store::new(db_val.pool());
                        for fact in facts {
                            if let (Some(key), Some(content)) = (
                                fact.get("key").and_then(|k| k.as_str()),
                                fact.get("content").and_then(|c| c.as_str()),
                            ) {
                                // Implicitly extracted facts are NOT verified and NOT pinned
                                let _ = store
                                    .add_memory(
                                        key,
                                        Some(&project_path),
                                        content,
                                        Some("extracted"),
                                        false,
                                        false,
                                    )
                                    .await;
                                info!("Background Observer extracted Fact for project {}: {} -> {}", project_path, key, content);
                            }
                        }
                        drop(db_guard);
                        let _ = crate::memories::db::sync_memories_to_markdown(db.clone()).await;
                    }
                }
            }
        }
    }
}
