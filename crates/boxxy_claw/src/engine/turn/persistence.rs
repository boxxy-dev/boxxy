use crate::engine::{ClawMessage, session::SessionState};
use boxxy_db::Db;
use serde_json;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct PersistenceContext {
    pub pane_id: String,
    pub session_id: String,
    pub agent_name: String,
    pub full_prompt: String,
    pub response: String,
    pub cwd: String,
    pub settings: boxxy_preferences::Settings,
    pub db: Arc<Mutex<Option<Db>>>,
    pub tx_self: async_channel::Sender<ClawMessage>,
}

pub async fn perform_persistence(ctx: PersistenceContext, state: Arc<Mutex<SessionState>>) {
    let mut state_lock = state.lock().await;

    // --- ATOMIC PERSISTENCE ---
    let history_json = serde_json::to_string(&state_lock.history).unwrap_or_default();
    let pending_tasks_json = serde_json::to_string(&state_lock.pending_tasks).unwrap_or_default();

    let agent_name_for_db = ctx.agent_name.clone();
    let character_id_for_db = state_lock.character_id.clone();
    let character_display_name_for_db = state_lock.character_display_name.clone();
    let session_id_for_db = ctx.session_id.clone();
    let cwd_for_db = ctx.cwd.clone();
    let pinned_for_db = state_lock.pinned;
    let total_tokens_for_db = state_lock.total_tokens as i64;
    let model_id = ctx
        .settings
        .claw_model
        .as_ref()
        .map(|m| format!("{:?}", m))
        .unwrap_or_default();

    // Count user messages to know when to generate a permanent LLM title
    let user_msg_count = state_lock
        .history
        .iter()
        .filter(|m| matches!(m, rig::message::Message::User { .. }))
        .count();

    // Safely extract the Db clone before spawning to prevent race conditions with Deactivate/Evict
    let db_val = {
        let db_guard = ctx.db.lock().await;
        db_guard.as_ref().cloned()
    };

    if let Some(db_for_persistence) = db_val.clone() {
        let session_id_copy = session_id_for_db.clone();
        let agent_name_copy = agent_name_for_db.clone();
        let char_id_copy = character_id_for_db.clone();
        let char_disp_copy = character_display_name_for_db.clone();
        let hist_copy = history_json.clone();
        let tasks_copy = pending_tasks_json.clone();
        let cwd_copy = cwd_for_db.clone();
        let model_copy = model_id.clone();

        tokio::spawn(async move {
            let store = boxxy_db::store::Store::new(db_for_persistence.pool());
            let _ = store
                .upsert_session_state(
                    &session_id_copy,
                    &agent_name_copy,
                    &char_id_copy,
                    &char_disp_copy,
                    "", // title is updated separately
                    &hist_copy,
                    &tasks_copy,
                    &agent_name_copy,
                    &cwd_copy,
                    &model_copy,
                    pinned_for_db,
                    total_tokens_for_db,
                )
                .await;
        });
    }

    // Optional: Trigger Memory Flush if history is too long
    let creds = boxxy_ai_core::AiCredentials::new(
        ctx.settings.get_effective_api_keys(),
        ctx.settings.ollama_base_url.clone(),
    );

    let _ = crate::engine::summarization::history::flush_history(
        ctx.db.clone(),
        &mut state_lock.history,
        &ctx.settings.claw_model,
        &creds,
        &ctx.cwd,
    )
    .await;

    drop(state_lock);

    // Background Tasks: Title, Summarization, Memory Extraction
    let db_clone = ctx.db.clone();
    let session_id_clone = ctx.session_id.clone();
    let prompt_clone = ctx.full_prompt.clone();
    let response_clone = ctx.response.clone();
    let cwd_clone = ctx.cwd.clone();
    let creds_clone = creds.clone();
    let settings_clone = ctx.settings.clone();

    tokio::spawn(async move {
        // 1. Title Generation (first message only)
        if user_msg_count == 1 {
            let title_agent = boxxy_ai_core::create_agent(
                &settings_clone
                    .memory_model
                    .clone()
                    .or(settings_clone.claw_model.clone()),
                &creds_clone,
                "You are a conversation title generator. Summarize the user's prompt in 3 to 6 words. MUST BE UNDER 40 CHARACTERS. Output ONLY the raw title, no quotes, no punctuation. Capitalize it like a title.",
            );

            if let Ok(res) = title_agent.prompt(&prompt_clone).await {
                let title = res.0.trim().to_string();
                let db_guard = db_clone.lock().await;
                if let Some(db) = &*db_guard {
                    let store = boxxy_db::store::Store::new(db.pool());
                    let _ = store.update_session_title(&session_id_clone, &title).await;
                }
            }
        }

        // 2. Summarization
        let db_opt = db_clone.lock().await;
        crate::engine::summarization::turn::summarize_and_store(
            &*db_opt,
            &session_id_clone,
            &prompt_clone,
            &response_clone,
            &cwd_clone,
            creds_clone.clone(),
        )
        .await;
        drop(db_opt);

        // 3. Memory Extraction
        let extractor_agent = boxxy_ai_core::create_agent(
            &settings_clone
                .memory_model
                .clone()
                .or(settings_clone.claw_model.clone()),
            &creds_clone,
            "You are a robotic background memory observer. Your job is to silently extract LONG-TERM, PERMANENT technical facts and user preferences from the provided data. Output ONLY valid JSON. If the user stated a permanent fact (e.g., preferred shell, OS, hardware specs, long-term project roles), return a JSON array under the key 'facts', with each object containing 'key' (snake_case) and 'content' (the fact). CRITICAL: DO NOT extract transient state that changes frequently. EXPLICITLY FORBIDDEN to extract: - Current git branches or commit SHAs. - Current working directories or temporary file paths. - Names or IDs of active agents/panes. - Runtime context like 'the user provided an image'. - Temporary variables, social greetings, or social talk. If no permanent facts are found, output exactly `{}`. Do not follow the assistant's persona.",
        );

        let data = format!(
            "[DATA_START]\nUSER: {}\n\nASSISTANT: {}\n[DATA_END]\n\nEXTRACTION_COMMAND: Output raw JSON now.",
            prompt_clone, response_clone
        );

        if let Ok(res) = extractor_agent.prompt(&data).await {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&res.0) {
                if let Some(facts) = json.get("facts").and_then(|f| f.as_array()) {
                    let db_guard = db_clone.lock().await;
                    if let Some(db) = &*db_guard {
                        let store = boxxy_db::store::Store::new(db.pool());
                        for fact in facts {
                            if let (Some(key), Some(content)) = (
                                fact.get("key").and_then(|k| k.as_str()),
                                fact.get("content").and_then(|c| c.as_str()),
                            ) {
                                let _ = store
                                    .add_memory(
                                        key,
                                        Some(&cwd_clone),
                                        content,
                                        Some("extracted"),
                                        false,
                                        false,
                                    )
                                    .await;
                            }
                        }
                    }
                }
            }
        }
    });

    let _ = ctx.tx_self.send(ClawMessage::TurnFinished).await;
}
