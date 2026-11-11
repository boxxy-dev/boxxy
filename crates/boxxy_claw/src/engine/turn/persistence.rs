use crate::engine::{ClawMessage, session::SessionState};
use boxxy_db::Db;
use serde_json;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct PersistenceContext {
    pub session_id: String,
    pub agent_name: String,
    pub user_query: String,
    pub cwd: String,
    pub settings: boxxy_preferences::Settings,
    pub db: Arc<Mutex<Option<Db>>>,
    pub tx_self: async_channel::Sender<ClawMessage>,
}

pub async fn perform_persistence(ctx: PersistenceContext, state: Arc<Mutex<SessionState>>) {
    let mut state_lock = state.lock().await;

    // Optional: Trigger Memory Flush if history is too long
    let creds = boxxy_ai_core::AiCredentials::new(
        ctx.settings.get_effective_api_keys(),
        ctx.settings.ollama_base_url.clone(),
    )
    .with_custom_endpoints(ctx.settings.openai_compatible_endpoints.clone());

    // Token-budget & exchange-boundary compaction (OpenCode V2 + rig 0.46 pattern):
    // Measure against the model's `input_limit` (`limit.input` in rig 0.46, falling back to `context_window`)
    // so models with dedicated output reserves (like GPT-6.1's 922k input limit inside its 1.05M window)
    // compact at 75% (or < 16k headroom) before exceeding the API's maximum input limit.
    // Never cut inside a tool-call loop (`find_safe_exchange_cut`), and always retain at least
    // the newest 6 full user exchanges plus a rolling `[SESSION_CHECKPOINT]` pair at index 0..2.
    let input_limit = boxxy_ai_core::model_input_limit_with_endpoints(
        &ctx.settings.claw_model,
        &ctx.settings.openai_compatible_endpoints,
    );
    let token_budget_exceeded = input_limit.is_some_and(|limit| {
        limit > 0
            && state_lock.context_tokens > 0
            && (state_lock.context_tokens >= (limit * 75 / 100)
                || state_lock.context_tokens.saturating_add(16_000) >= limit)
    });
    let should_compact = token_budget_exceeded || state_lock.history.len() >= 80;

    let evicted_messages = if should_compact
        && let Some(cut) = super::history::find_safe_exchange_cut(&state_lock.history, 6)
    {
        let mut kept = state_lock.history.split_off(cut);
        let demoted = std::mem::take(&mut state_lock.history);
        let demoted_len = demoted.len();
        let already_flushed = state_lock.flushed_watermark.min(demoted_len);
        let mut new_watermark = state_lock.flushed_watermark.saturating_sub(demoted_len);

        if let Some((chk_user, chk_asst)) = super::history::build_checkpoint_pair(&demoted) {
            let mut with_checkpoint = Vec::with_capacity(kept.len() + 2);
            with_checkpoint.push(chk_user);
            with_checkpoint.push(chk_asst);
            with_checkpoint.append(&mut kept);
            kept = with_checkpoint;
            new_watermark += 2;
        }

        state_lock.history = kept;
        state_lock.flushed_watermark = new_watermark;
        demoted.into_iter().skip(already_flushed).collect()
    } else {
        vec![]
    };

    // --- ATOMIC PERSISTENCE ---
    let history_json = serde_json::to_string(&state_lock.history).unwrap_or_default();
    let pending_tasks_json = serde_json::to_string(&state_lock.pending_tasks).unwrap_or_default();

    let agent_name_for_db = ctx.agent_name.clone();
    let character_id_for_db = state_lock.character_id.clone();
    let character_display_name_for_db = state_lock.character_display_name.clone();
    let session_id_for_db = ctx.session_id.clone();
    let cwd_for_db = ctx.cwd.clone();
    let pinned_for_db = state_lock.pinned;
    let total_tokens_for_db = i64::try_from(state_lock.total_tokens).unwrap_or(i64::MAX);
    let context_tokens_for_db = i64::try_from(state_lock.context_tokens).unwrap_or(i64::MAX);
    let total_cost_for_db = state_lock.total_cost_usd;
    let model_id = ctx
        .settings
        .claw_model
        .as_ref()
        .map_or_else(String::new, |m| format!("{m:?}"));

    // Extract the session's first user query from `history` (stripping dynamic context blocks)
    // so that if Turn 1 was interrupted/rejected before completion, the title is still generated
    // from the user's original intent rather than a later follow-up like "thank you".
    let initial_user_query = state_lock
        .history
        .iter()
        .find_map(|m| {
            if let rig::message::Message::User { content } = m {
                content.iter().find_map(|c| {
                    if let rig::message::UserContent::Text(t) = c {
                        let cleaned =
                            crate::engine::summarization::history::clean_user_text_for_flush(
                                &t.text,
                            );
                        let trimmed = cleaned.trim();
                        if !trimmed.is_empty()
                            && !trimmed.starts_with("[WHILE_YOU_SLEPT]")
                            && !trimmed.starts_with(super::history::SESSION_CHECKPOINT_PREFIX)
                        {
                            return Some(trimmed.to_string());
                        }
                    }
                    None
                })
            } else {
                None
            }
        })
        .unwrap_or_else(|| ctx.user_query.clone());

    let should_attempt_title =
        !state_lock.title_requested && !initial_user_query.trim().is_empty();
    if should_attempt_title {
        state_lock.title_requested = true;
    }

    drop(state_lock);

    if !evicted_messages.is_empty() {
        let extractor = crate::engine::summarization::history::DemotionExtractor {
            db: ctx.db.clone(),
            claw_model: ctx
                .settings
                .memory_model
                .clone()
                .or_else(|| ctx.settings.claw_model.clone()),
            creds: creds.clone(),
            project_path: ctx.cwd.clone(),
        };
        extractor.on_demote(ctx.session_id.as_str(), evicted_messages);
    }

    // Safely extract the Db clone before spawning to prevent race conditions with Deactivate/Evict
    let db_val = ctx.db.lock().await.clone();

    if let Some(db_for_persistence) = db_val {
        let state_for_title = state;
        let settings_clone = ctx.settings;

        // Run session upsert FIRST and then conditionally generate the session title
        // inside the same task so `UPDATE sessions SET title = ?` never races ahead of `INSERT`.
        tokio::spawn(async move {
            let store = boxxy_db::store::Store::new(db_for_persistence.pool());
            let _ = store
                .upsert_session_state(
                    &session_id_for_db,
                    &agent_name_for_db,
                    &character_id_for_db,
                    &character_display_name_for_db,
                    "", // title is updated separately once generated
                    &history_json,
                    &pending_tasks_json,
                    &agent_name_for_db,
                    &cwd_for_db,
                    &model_id,
                    pinned_for_db,
                    total_tokens_for_db,
                    context_tokens_for_db,
                    total_cost_for_db,
                )
                .await;

            if !should_attempt_title {
                return;
            }

            let needs_title = match store.get_session(&session_id_for_db).await {
                Ok(Some(s)) => s.title.as_deref().unwrap_or("").trim().is_empty(),
                _ => false,
            };
            if !needs_title {
                return;
            }

            let title_model = settings_clone
                .memory_model
                .clone()
                .or_else(|| settings_clone.claw_model.clone());
            let title_agent = boxxy_ai_core::create_agent(
                &title_model,
                &creds,
                "You are a conversation title generator. Summarize the user's prompt in 3 to 6 words. MUST BE UNDER 40 CHARACTERS. Output ONLY the raw title, no quotes, no punctuation. Capitalize it like a title.",
            );

            match title_agent.prompt(&initial_user_query).await {
                Ok((raw_title, _, _)) => {
                    let title = raw_title
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .trim()
                        .to_string();
                    if title.is_empty() {
                        state_for_title.lock().await.title_requested = false;
                    } else {
                        let _ = store.update_session_title(&session_id_for_db, &title).await;
                        log::info!("Generated session title for {session_id_for_db}: {title}");
                    }
                }
                Err(_) => {
                    // Allow retry on a subsequent turn if transient network/API failure occurred
                    state_for_title.lock().await.title_requested = false;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_needs_title_logic() {
        let db = Db::new_in_memory().await.unwrap();
        let store = boxxy_db::store::Store::new(db.pool());

        let session_id = "test_needs_title_session";

        // 1. Initial upsert with empty title
        store
            .upsert_session_state(
                session_id,
                "dummy-agent",
                "dummy-char-uuid",
                "Dummy Agent",
                "", // empty title initially
                "[]",
                "[]",
                "dummy-agent",
                "/tmp",
                "",
                false,
                0,
                0,
                0.0,
            )
            .await
            .unwrap();

        let s = store.get_session(session_id).await.unwrap().unwrap();
        let needs_title = s.title.as_deref().unwrap_or("").trim().is_empty();
        assert!(needs_title, "Session with empty title should need title");

        // 2. Update title
        store
            .update_session_title(session_id, "Configuring Fish Shell")
            .await
            .unwrap();

        let s_after = store.get_session(session_id).await.unwrap().unwrap();
        let needs_title_after = s_after.title.as_deref().unwrap_or("").trim().is_empty();
        assert!(!needs_title_after, "Session with title should not need title");
        assert_eq!(s_after.title.as_deref(), Some("Configuring Fish Shell"));

        // 3. Ensure subsequent state upsert preserves existing title
        store
            .upsert_session_state(
                session_id,
                "dummy-agent",
                "dummy-char-uuid",
                "Dummy Agent",
                "", // subsequent turn state doesn't wipe existing title
                "[]",
                "[]",
                "dummy-agent",
                "/tmp",
                "",
                false,
                100,
                80,
                0.01,
            )
            .await
            .unwrap();

        let s_final = store.get_session(session_id).await.unwrap().unwrap();
        assert_eq!(s_final.title.as_deref(), Some("Configuring Fish Shell"));
    }
}
