use rig::message::Message;

/// Prepares the history for a new chat completion.
/// Returns a history ready to either be passed directly to `rig` (non-multimodal)
/// or have the current turn's multimodal message appended (multimodal path).
pub fn prepare_query_history(history: Vec<Message>, history_len: usize) -> Vec<Message> {
    // Context Hygiene 2.0: Aggressively strip ALL transient context from previous turns
    // (Skills, Radar, Memories, and Snapshots) so history grows near-zero tokens per turn.
    let mut final_history: Vec<Message> = history
        .into_iter()
        .enumerate()
        .map(|(i, mut msg)| {
            // Rule 1B: Freshness Buffer - only keep the verbatim snapshot for the last 4 turns.
            // Every user + assistant exchange is 2 messages, so 4 turns = 8 messages.
            // We strip the dynamic blocks from anything older than the last 8 messages.
            let is_old = history_len.saturating_sub(i) > 8;

            if let Message::User { content } = &mut msg {
                let mut items: Vec<rig::message::UserContent> =
                    content.clone().into_iter().collect();
                for item in &mut items {
                    if let rig::message::UserContent::Text(text) = item {
                        if is_old {
                            // Find the start of the dynamic block and truncate everything after it
                            if let Some(idx) = text.text.find("\n\n## YOUR IDENTITY") {
                                text.text.truncate(idx);
                            } else if let Some(idx) = text.text.find("\n\n## CURRENT TURN CONTEXT")
                            {
                                text.text.truncate(idx);
                            } else if let Some(idx) = text.text.find("\n\n--- GLOBAL RADAR") {
                                text.text.truncate(idx);
                            } else if let Some(idx) = text.text.find("\n\nTerminal Snapshot:\n```")
                            {
                                text.text.truncate(idx);
                            }
                        }
                    }
                }
                if let Ok(new_content) = rig::OneOrMany::many(items) {
                    *content = new_content;
                }
            }
            msg
        })
        .collect();

    // Remove the current user message we just pushed — it will be passed as query_for_chat
    // instead (or replaced by a multimodal version), so rig doesn't receive it twice.
    final_history.pop();

    // If the previous turn was also interrupted, history may now end with a bare User message
    // (no following Assistant). Inject a synthetic acknowledgment to maintain the alternating
    // user/assistant format required by the Anthropic API.
    if matches!(final_history.last(), Some(Message::User { .. })) {
        final_history.push(Message::assistant(
            "[Previous task was interrupted by the user]",
        ));
    }

    final_history
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::message::Message;

    fn user(s: &str) -> Message {
        Message::user(s.to_string())
    }
    fn asst(s: &str) -> Message {
        Message::assistant(s.to_string())
    }

    #[test]
    fn normal_turn_no_repair_needed() {
        // [user_A, asst_A, user_current] → pop → [user_A, asst_A]
        let history = vec![user("A"), asst("reply A"), user("current")];
        let result = prepare_query_history(history.clone(), history.len());
        assert_eq!(result.len(), 2);
        assert!(matches!(result.last(), Some(Message::Assistant { .. })));
    }

    #[test]
    fn single_interrupt_injects_synthetic() {
        // [user_A, user_current] → pop → [user_A] → ends with User → inject synthetic
        let history = vec![user("A"), user("current")];
        let result = prepare_query_history(history.clone(), history.len());
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0], Message::User { .. }));
        assert!(matches!(result[1], Message::Assistant { .. }));
    }

    #[test]
    fn double_interrupt_still_valid() {
        // [user_A, user_B, user_current] → pop → [user_A, user_B] → inject → [user_A, user_B, asst_synthetic]
        let history = vec![user("A"), user("B"), user("current")];
        let result = prepare_query_history(history.clone(), history.len());
        assert_eq!(result.len(), 3);
        assert!(matches!(result.last(), Some(Message::Assistant { .. })));
    }

    #[test]
    fn empty_history_no_panic() {
        // Only the current user message was present — pop leaves empty, no injection
        let history = vec![user("current")];
        let result = prepare_query_history(history.clone(), history.len());
        assert!(result.is_empty());
    }
}
