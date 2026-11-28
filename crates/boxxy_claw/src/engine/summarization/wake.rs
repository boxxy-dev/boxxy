const MAX_WAKE_HISTORY_BYTES: usize = 40_000;

pub async fn summarize_wake_delta<S: std::hash::BuildHasher + Send + Sync>(
    sleep_commands: Vec<(String, i32)>,
    sleep_timestamp: Option<i64>,
    api_keys: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, String, S>>>,
    ollama_url: std::sync::Arc<tokio::sync::RwLock<String>>,
    tx_self: async_channel::Sender<crate::engine::ClawMessage>,
) {
    let _ts = sleep_timestamp.unwrap_or(0);

    let recent_commands: Vec<String> = sleep_commands
        .into_iter()
        .map(|(cmd, exit_code)| format!("$ {cmd} (exit: {exit_code})"))
        .collect();

    let summary_result = if recent_commands.is_empty() {
        // Nothing executed while asleep — return empty string so 0 tokens are added to history.
        Ok(String::new())
    } else if recent_commands.len() <= 3 {
        // Small delta: format directly without spending an LLM call.
        Ok(format!(
            "User ran commands while you slept:\n{}",
            recent_commands.join("\n")
        ))
    } else {
        let settings = boxxy_preferences::Settings::load();
        let model_config = settings.effective_memory_model();

        let creds = {
            let ipc_keys = api_keys.read().await;
            let ipc_ollama = ollama_url.read().await;
            crate::engine::agent_config::merge_session_credentials(
                &settings,
                &ipc_keys,
                &ipc_ollama,
            )
        };

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
            Ok(Ok((summary, _, _))) => Ok(summary.trim().to_string()),
            Ok(Err(e)) => Err(format!("LLM Error: {e}")),
            Err(_) => Err("Summarization timed out.".to_string()),
        }
    };

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
