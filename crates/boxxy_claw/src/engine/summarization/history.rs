use crate::utils::load_prompt_fallback;
use boxxy_db::store::Store;
use boxxy_db::Db;
use boxxy_model_selection::ModelProvider;
use log::{debug, info};
use rig::message::Message;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct DemotionExtractor {
    pub db: Arc<Mutex<Option<Db>>>,
    pub claw_model: Option<ModelProvider>,
    pub creds: boxxy_ai_core::AiCredentials,
    pub project_path: String,
}

impl DemotionExtractor {
    pub async fn on_demote(&self, session_id: &str, messages: Vec<Message>) {
        if messages.is_empty() {
            return;
        }

        let db_clone = self.db.clone();
        let session_id_owned = session_id.to_string();
        let claw_model_clone = self.claw_model.clone();
        let creds_clone = self.creds.clone();
        let project_path_clone = self.project_path.clone();

        tokio::spawn(async move {
            let _ = flush_history(
                db_clone,
                &session_id_owned,
                messages,
                &claw_model_clone,
                &creds_clone,
                &project_path_clone,
            )
            .await;
        });
    }
}

/// Strips dynamic context blocks (`## YOUR IDENTITY`, `## CURRENT TURN CONTEXT`, etc.)
/// from a user message text so background memory flush calls don't waste tokens on snapshots.
pub(crate) fn clean_user_text_for_flush(raw: &str) -> &str {
    if raw.trim_start().starts_with("[ROUNDTABLE TURN") {
        return "";
    }
    for marker in [
        "\n\n--- UPDATE CHANGELOG CONTEXT ---",
        "\n\n## YOUR IDENTITY",
        "\n\n## CURRENT TURN CONTEXT",
        "\n\n--- GLOBAL RADAR",
        "\n\nTerminal Snapshot:\n```",
    ] {
        if let Some(idx) = raw.find(marker) {
            return &raw[..idx];
        }
    }
    raw
}

pub async fn flush_history(
    db: Arc<Mutex<Option<Db>>>,
    session_id: &str,
    evicted_messages: Vec<Message>,
    claw_model: &Option<ModelProvider>,
    creds: &boxxy_ai_core::AiCredentials,
    project_path: &str,
) -> anyhow::Result<()> {
    if evicted_messages.is_empty() {
        return Ok(());
    }

    info!("Processing evicted messages. Triggering Memory Extraction...");

    let mut text_to_summarize = String::new();
    for msg in evicted_messages {
        match msg {
            Message::System { content } => {
                text_to_summarize.push_str(&format!("SYSTEM: {}\n", content));
            }
            Message::User { content } => {
                for c in content.iter() {
                    if let rig::message::UserContent::Text(text) = c {
                        let cleaned = clean_user_text_for_flush(&text.text).trim();
                        if !cleaned.is_empty()
                            && !cleaned.starts_with("[WHILE_YOU_SLEPT]")
                            && !cleaned.starts_with(
                                crate::engine::turn::history::SESSION_CHECKPOINT_PREFIX,
                            )
                        {
                            text_to_summarize.push_str(&format!("USER: {}\n", cleaned));
                        }
                    }
                }
            }
            Message::Assistant(turn) => {
                for c in turn.content.iter() {
                    if let rig::message::AssistantContent::Text(text) = c {
                        let trimmed = crate::engine::turn::history::strip_speaker_tag(&text.text).trim();
                        if !trimmed.is_empty()
                            && trimmed != "[SILENT_ACK]"
                            && trimmed != "[Previous task was interrupted by the user]"
                            && trimmed != crate::engine::turn::history::SESSION_CHECKPOINT_ACK
                        {
                            text_to_summarize.push_str(&format!("ASSISTANT: {}\n", trimmed));
                        }
                    }
                }
            }
        }
    }

    if text_to_summarize.trim().is_empty() {
        return Ok(());
    }

    let flush_prompt_template = load_prompt_fallback(
        "/dev/boxxy/BoxxyTerminal/prompts/memory_flush.md",
        "memory_flush.md",
    );

    let flush_prompt = flush_prompt_template.replace("{{text_to_summarize}}", &text_to_summarize);

    let agent = boxxy_ai_core::create_agent(
        claw_model,
        creds,
        "You are a concise memory extraction system. Output only valid JSON.",
    );

    if let Ok(res) = agent.prompt(&flush_prompt).await {
        if persist_flushed_json(&db, session_id, project_path, &res.0)
            .await
            .unwrap_or(false)
        {
            let _ = crate::memories::db::sync_memories_to_markdown(db.clone()).await;
        }
    }

    info!("Memory Flush complete.");
    Ok(())
}

pub(crate) async fn persist_flushed_json(
    db: &Arc<Mutex<Option<Db>>>,
    session_id: &str,
    project_path: &str,
    raw_response: &str,
) -> anyhow::Result<bool> {
    // Strip optional markdown code fences or surrounding prose if present
    let trimmed = raw_response
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let raw_json = if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && end >= start
    {
        &trimmed[start..=end]
    } else {
        trimmed
    };

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw_json) {
        let db_guard = db.lock().await;
        if let Some(db_val) = db_guard.as_ref() {
            let store = Store::new(db_val.pool());

            // 1. Save Facts
            if let Some(facts) = json.get("facts").and_then(|f| f.as_array()) {
                for fact in facts {
                    if let (Some(key), Some(content)) = (
                        fact.get("key").and_then(|k| k.as_str()),
                        fact.get("content").and_then(|c| c.as_str()),
                    ) {
                        // Implicitly extracted facts are candidates and require reinforcement
                        let _ = store
                            .upsert_dream_candidate(key, Some(project_path), content, 0.5)
                            .await;
                        debug!(
                            "Flushed Fact for project {}: {} -> {}",
                            project_path, key, content
                        );
                    }
                }
            }

            // 2. Save Summary as an interaction (with 0-token local Jaccard deduplication)
            if let Some(summary) = json.get("summary").and_then(|s| s.as_str()) {
                let summary = summary.trim();
                if !summary.is_empty() && summary != "NO_TECHNICAL_CHANGE" {
                    let mut is_dup = false;
                    if let Ok(recent) = store.get_recent_interactions_by_path(project_path, 5).await
                    {
                        for r in &recent {
                            let sim =
                                crate::engine::context::jaccard_similarity(summary, &r.content);
                            if sim >= 0.70 {
                                debug!(
                                    "Local Jaccard duplicate found (ID: {}, sim: {:.2}). Touching timestamp.",
                                    r.id, sim
                                );
                                let _ = store.touch_interaction(r.id).await;
                                is_dup = true;
                                break;
                            }
                        }
                    }

                    if !is_dup {
                        let effective_sid = if session_id.trim().is_empty() {
                            "global"
                        } else {
                            session_id
                        };
                        // Ensure the parent session row exists so the FOREIGN KEY(session_id)
                        // constraint on `interactions` never fails if flush runs before Turn 1 upsert.
                        let _ = store
                            .ensure_session_exists(effective_sid, "", "", project_path)
                            .await;
                        if let Err(e) = store
                            .add_interaction(
                                effective_sid,
                                Some(project_path),
                                summary,
                                Some("flush_summary"),
                                None,
                            )
                            .await
                        {
                            log::warn!(
                                "Failed to store flushed interaction for session {}: {}",
                                effective_sid,
                                e
                            );
                        } else {
                            debug!("Flushed Summary for project {}: {}", project_path, summary);
                        }
                    }
                }
            }

            drop(db_guard);
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_user_text_for_flush_strips_dynamic_blocks() {
        let prompt = "what files are in this directory?\n\n## YOUR IDENTITY\nAgent Name: **levi**\n\nTerminal Snapshot:\n```\nls\n```";
        assert_eq!(
            clean_user_text_for_flush(prompt),
            "what files are in this directory?"
        );

        let plain = "just a normal user prompt";
        assert_eq!(clean_user_text_for_flush(plain), plain);
    }

    #[tokio::test]
    async fn persist_flushed_json_creates_session_and_stores_interaction() {
        let db = Db::new_in_memory().await.unwrap();
        let db_arc = Arc::new(Mutex::new(Some(db.clone())));

        let payload = r#"```json
        {
            "facts": [
                { "key": "pkg_mgr", "content": "Uses bun instead of npm" }
            ],
            "summary": "Configured bun workspace and migrated scripts."
        }
        ```"#;

        persist_flushed_json(&db_arc, "uncreated-session-id", "/tmp/proj", payload)
            .await
            .unwrap();

        let store = Store::new(db.pool());
        let undreamed = store.get_undreamed_interactions().await.unwrap();
        assert_eq!(undreamed.len(), 1);
        assert_eq!(undreamed[0].session_id, "uncreated-session-id");
        assert_eq!(
            undreamed[0].content,
            "Configured bun workspace and migrated scripts."
        );

        let candidate = store
            .get_memory_by_key("pkg_mgr", Some("/tmp/proj"))
            .await
            .unwrap()
            .expect("candidate fact should exist");
        assert_eq!(candidate.content, "Uses bun instead of npm");
        assert_eq!(candidate.verified, Some(false));
    }
}
