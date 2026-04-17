use boxxy_db::Db;
use crate::utils::load_prompt_fallback;
use boxxy_db::store::Store;
use boxxy_model_selection::ModelProvider;
use log::{debug, info};
use rig::message::Message;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn flush_history(
    db: Arc<Mutex<Option<Db>>>,
    history: &mut Vec<Message>,
    claw_model: &Option<ModelProvider>,
    creds: &boxxy_ai_core::AiCredentials,
    project_path: &str,
) -> anyhow::Result<()> {
    if history.len() < 15 {
        return Ok(());
    }

    info!("Context window reaching capacity. Triggering Memory Flush...");

    // We take the first 10 messages (5 turns) to summarize and remove
    let evicted_messages = history.drain(0..10).collect::<Vec<_>>();

    let mut text_to_summarize = String::new();
    for msg in evicted_messages {
        match msg {
            Message::System { content } => {
                text_to_summarize.push_str(&format!("SYSTEM: {}\n", content));
            }
            Message::User { content } => {
                for c in content.iter() {
                    if let rig::message::UserContent::Text(text) = c {
                        text_to_summarize.push_str(&format!("USER: {}\n", text));
                    }
                }
            }
            Message::Assistant { content, .. } => {
                for c in content.iter() {
                    if let rig::message::AssistantContent::Text(text) = c {
                        text_to_summarize.push_str(&format!("ASSISTANT: {}\n", text));
                    }
                }
            }
        }
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
        // Attempt to parse the JSON response
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&res.0) {
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
                            // Implicitly extracted facts are NOT verified by default and NOT pinned
                            let _ = store
                                .add_memory(
                                    key,
                                    Some(project_path),
                                    content,
                                    Some("extracted"),
                                    false,
                                    false,
                                )
                                .await;
                            debug!(
                                "Flushed Fact for project {}: {} -> {}",
                                project_path, key, content
                            );
                        }
                    }
                }

                // 2. Save Summary as an interaction
                if let Some(summary) = json.get("summary").and_then(|s| s.as_str()) {
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

                drop(db_guard);
                let _ = crate::memories::db::sync_memories_to_markdown(db.clone()).await;
            }
        }
    }

    info!("Memory Flush complete. History truncated.");
    Ok(())
}
