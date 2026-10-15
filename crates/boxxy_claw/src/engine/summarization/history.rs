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

impl rig::memory::DemotionHook for DemotionExtractor {
    fn on_demote<'a>(
        &'a self,
        _session_id: &'a rig::id::ConversationId,
        messages: Vec<Message>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), rig::memory::MemoryError>> + Send + 'a>> {
        let db_clone = self.db.clone();
        let claw_model_clone = self.claw_model.clone();
        let creds_clone = self.creds.clone();
        let project_path_clone = self.project_path.clone();

        Box::pin(async move {
            if messages.is_empty() {
                return Ok(());
            }

            tokio::spawn(async move {
                let _ = flush_history(
                    db_clone,
                    messages,
                    &claw_model_clone,
                    &creds_clone,
                    &project_path_clone,
                )
                .await;
            });

            Ok(())
        })
    }
}

/// Strips dynamic context blocks (`## YOUR IDENTITY`, `## CURRENT TURN CONTEXT`, etc.)
/// from a user message text so background memory flush calls don't waste tokens on snapshots.
fn clean_user_text_for_flush(raw: &str) -> &str {
    for marker in [
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
                        if !cleaned.is_empty() && !cleaned.starts_with("[WHILE_YOU_SLEPT]") {
                            text_to_summarize.push_str(&format!("USER: {}\n", cleaned));
                        }
                    }
                }
            }
            Message::Assistant(turn) => {
                for c in turn.content.iter() {
                    if let rig::message::AssistantContent::Text(text) = c {
                        let trimmed = text.text.trim();
                        if !trimmed.is_empty()
                            && trimmed != "[SILENT_ACK]"
                            && trimmed != "[Previous task was interrupted by the user]"
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
        // Attempt to parse the JSON response (strip optional markdown code fences if present)
        let raw_json = res
            .0
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
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
                        if let Ok(recent) = store.get_recent_interactions_by_path(project_path, 5).await {
                            for r in &recent {
                                let sim = crate::engine::context::jaccard_similarity(summary, &r.content);
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
                            let _ = store
                                .add_interaction(
                                    "global",
                                    Some(project_path),
                                    summary,
                                    Some("flush_summary"),
                                    None,
                                )
                                .await;
                            debug!("Flushed Summary for project {}: {}", project_path, summary);
                        }
                    }
                }

                drop(db_guard);
                let _ = crate::memories::db::sync_memories_to_markdown(db.clone()).await;
            }
        }
    }

    info!("Memory Flush complete.");
    Ok(())
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
}
