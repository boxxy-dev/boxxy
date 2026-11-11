use boxxy_db::Db;
use std::sync::Arc;
use tokio::sync::Mutex;

const MAX_WAKE_HISTORY_BYTES: usize = 40_000;

pub async fn summarize_wake_delta(
    db_cell: Arc<Mutex<Option<Db>>>,
    session_id: String,
    sleep_timestamp: Option<i64>,
    tx_self: async_channel::Sender<crate::engine::ClawMessage>,
) {
    let _ts = sleep_timestamp.unwrap_or(0);

    // Clone the DB handle briefly and drop the mutex BEFORE any network LLM call
    // so concurrent event persistence and turn queries are never blocked!
    let db_opt = db_cell.lock().await.clone();
    let Some(db) = db_opt else {
        let _ = tx_self
            .send(crate::engine::ClawMessage::WakeSummaryComplete {
                result: Err("DB not connected".to_string()),
            })
            .await;
        return;
    };

    let mut summary_result = Err("No events found or DB error.".to_string());

    if let Ok(events) = boxxy_db::store::Store::new(db.pool())
        .get_claw_events(&session_id)
        .await
    {
        let mut recent_commands = Vec::new();
        for evt in events {
            if let Ok(crate::engine::PersistentClawRow::Command {
                command, exit_code, ..
            }) = serde_json::from_str::<crate::engine::PersistentClawRow>(&evt)
            {
                recent_commands.push(format!("$ {command} (exit: {exit_code})"));
            }
        }

        if recent_commands.is_empty() {
            summary_result =
                Ok("No notable terminal commands executed while sleeping.".to_string());
        } else if recent_commands.len() <= 3 {
            // Too small to spend an LLM call on; format directly.
            summary_result = Ok(format!(
                "User ran a few commands while you slept:\n{}",
                recent_commands.join("\n")
            ));
        } else {
            let settings = boxxy_preferences::Settings::load();
            let model_config = settings
                .memory_model
                .clone()
                .or_else(|| settings.claw_model.clone());

            let creds = boxxy_ai_core::AiCredentials::new(
                settings.get_effective_api_keys(),
                settings.ollama_base_url.clone(),
            )
            .with_custom_endpoints(settings.openai_compatible_endpoints.clone());

            let raw_history = recent_commands.join("\n");
            let truncated_history = truncate_tail_utf8(&raw_history, MAX_WAKE_HISTORY_BYTES);

            let agent = boxxy_ai_core::create_agent(
                &model_config,
                &creds,
                "You are a specialized developer assistant context-bridge. The user has been working manually in the terminal while you were 'asleep'. Your job is to read their raw terminal history and write a VERY CONCISE, single-paragraph summary of what they accomplished, what files they touched, or what errors they hit. Do not respond with pleasantries. Just write the technical summary.",
            );

            match tokio::time::timeout(
                std::time::Duration::from_secs(30),
                agent.prompt(&format!("Raw Terminal History:\n{truncated_history}")),
            )
            .await
            {
                Ok(Ok((summary, _, _))) => {
                    summary_result = Ok(summary.trim().to_string());
                }
                Ok(Err(e)) => {
                    summary_result = Err(format!("LLM Error: {e}"));
                }
                Err(_) => {
                    summary_result = Err("Summarization timed out.".to_string());
                }
            }
        }
    }

    let _ = tx_self
        .send(crate::engine::ClawMessage::WakeSummaryComplete {
            result: summary_result,
        })
        .await;
}

fn truncate_tail_utf8(raw: &str, max_bytes: usize) -> String {
    if raw.len() <= max_bytes {
        return raw.to_string();
    }
    let start = raw.ceil_char_boundary(raw.len().saturating_sub(max_bytes));
    format!("...[truncated]...\n{}", &raw[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_tail_utf8_respects_multibyte_char_boundaries() {
        let multi = "🦀🔥⚡".repeat(20);
        // Pick an arbitrary byte budget that lands inside a 4-byte emoji
        let out = truncate_tail_utf8(&multi, 25);
        assert!(out.starts_with("...[truncated]...\n"));
        assert!(out.contains('🦀'));
    }
}
