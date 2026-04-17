use std::sync::Arc;
use tokio::sync::Mutex;
use boxxy_db::Db;

pub async fn summarize_wake_delta(
    db_cell: Arc<Mutex<Option<Db>>>,
    session_id: String,
    sleep_timestamp: Option<i64>,
    tx_self: async_channel::Sender<crate::engine::ClawMessage>,
) {
    let db_guard = db_cell.lock().await;
    if let Some(db) = db_guard.as_ref() {
        let ts = sleep_timestamp.unwrap_or(0);
        let mut summary_result = Err("No events found or DB error.".to_string());
        
        if let Ok(events) = boxxy_db::store::Store::new(db.pool()).get_claw_events(&session_id).await {
            let mut recent_commands = Vec::new();
            for evt in events {
                // TODO: Add timestamp filtering when db allows it
                if let Ok(parsed) = serde_json::from_str::<crate::engine::PersistentClawRow>(&evt) {
                    if let crate::engine::PersistentClawRow::Command { command, exit_code, .. } = parsed {
                        recent_commands.push(format!("$ {} (exit: {})", command, exit_code));
                    }
                }
            }

            if recent_commands.is_empty() {
                summary_result = Ok("No notable terminal commands executed while sleeping.".to_string());
            } else if recent_commands.len() <= 3 {
                // Too small to waste an LLM call on, just summarize manually
                summary_result = Ok(format!("User ran a few commands while you slept:\n{}", recent_commands.join("\n")));
            } else {
                // It's a large delta, we need an LLM to digest it
                let settings = boxxy_preferences::Settings::load();
                let model_config = if settings.memory_model.is_some() {
                    settings.memory_model.clone()
                } else {
                    settings.claw_model.clone()
                };

                let creds = boxxy_ai_core::AiCredentials::new(
                    settings.get_effective_api_keys(),
                    settings.ollama_base_url.clone(),
                );

                let raw_history = recent_commands.join("\n");
                // Truncate to roughly 10k tokens (40k chars) to prevent blowing up the context window
                let truncated_history = if raw_history.len() > 40000 {
                    format!("...[truncated]...\n{}", &raw_history[raw_history.len() - 40000..])
                } else {
                    raw_history
                };

                let agent = boxxy_ai_core::create_agent(
                    &model_config,
                    &creds,
                    "You are a specialized developer assistant context-bridge. The user has been working manually in the terminal while you were 'asleep'. Your job is to read their raw terminal history and write a VERY CONCISE, single-paragraph summary of what they accomplished, what files they touched, or what errors they hit. Do not respond with pleasantries. Just write the technical summary.",
                );

                match tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    agent.prompt(&format!("Raw Terminal History:\n{}", truncated_history))
                ).await {
                    Ok(Ok((summary, _))) => {
                        summary_result = Ok(summary.trim().to_string());
                    }
                    Ok(Err(e)) => {
                        summary_result = Err(format!("LLM Error: {}", e));
                    }
                    Err(_) => {
                        summary_result = Err("Summarization timed out.".to_string());
                    }
                }
            }
        }
        
        let _ = tx_self.send(crate::engine::ClawMessage::WakeSummaryComplete { result: summary_result }).await;
    } else {
        let _ = tx_self.send(crate::engine::ClawMessage::WakeSummaryComplete { result: Err("DB not connected".to_string()) }).await;
    }
}
