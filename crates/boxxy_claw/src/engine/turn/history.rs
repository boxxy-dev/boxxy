use rig::message::Message;

/// Prepares the history for a new chat completion.
/// Returns a history ready to either be passed directly to `rig` (non-multimodal)
/// or have the current turn's multimodal message appended (multimodal path).
pub fn prepare_query_history(history: Vec<Message>, history_len: usize) -> Vec<Message> {
    // Context Hygiene 2.0: Aggressively strip ALL transient context from previous turns
    // (Skills, Radar, Memories, and Snapshots) so history grows near-zero tokens per turn.
    // Pop the current user message first — it will be passed as query_for_chat instead.
    let mut final_history = history;
    final_history.pop();

    // Count how many User messages precede each index from the end so we keep
    // the verbatim snapshot only for the last 4 user turns (regardless of how
    // many tool-call / tool-result messages each turn generated).
    let total_user_msgs = final_history
        .iter()
        .filter(|m| {
            matches!(m, Message::User { content } if content.iter().any(|c| matches!(c, rig::message::UserContent::Text(_))))
        })
        .count();

    let mut seen_user_msgs = 0;
    for msg in &mut final_history {
        if let Message::User { content } = msg {
            let has_text = content
                .iter()
                .any(|c| matches!(c, rig::message::UserContent::Text(_)));
            if has_text {
                seen_user_msgs += 1;
            }
            let is_old = total_user_msgs.saturating_sub(seen_user_msgs) >= 4;

            for item in content.iter_mut() {
                if let rig::message::UserContent::Text(text) = item {
                    if is_old {
                        // Find the start of the dynamic block and truncate everything after it
                        if let Some(idx) = text.text.find("\n\n--- UPDATE CHANGELOG CONTEXT ---") {
                            text.text.truncate(idx);
                        } else if let Some(idx) = text.text.find("\n\n## YOUR IDENTITY") {
                            text.text.truncate(idx);
                        } else if let Some(idx) = text.text.find("\n\n## CURRENT TURN CONTEXT") {
                            text.text.truncate(idx);
                        } else if let Some(idx) = text.text.find("\n\n--- GLOBAL RADAR") {
                            text.text.truncate(idx);
                        } else if let Some(idx) = text.text.find("\n\nTerminal Snapshot:\n```") {
                            text.text.truncate(idx);
                        }
                    }
                }
            }
        }
    }
    let _ = history_len;

    // If previous turns were interrupted, history may contain consecutive User messages
    // (no following Assistant). Inject synthetic acknowledgments between any adjacent User
    // messages and after a trailing User message to maintain strict user/assistant alternation.
    let mut repaired_history = Vec::with_capacity(final_history.len() + 1);
    for msg in final_history {
        if matches!(msg, Message::User { .. })
            && matches!(repaired_history.last(), Some(Message::User { .. }))
        {
            repaired_history.push(Message::assistant(
                "[Previous task was interrupted by the user]",
            ));
        }
        repaired_history.push(msg);
    }

    if matches!(repaired_history.last(), Some(Message::User { .. })) {
        repaired_history.push(Message::assistant(
            "[Previous task was interrupted by the user]",
        ));
    }

    repaired_history
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
        assert!(matches!(result.last(), Some(Message::Assistant(_))));
    }

    #[test]
    fn single_interrupt_injects_synthetic() {
        // [user_A, user_current] → pop → [user_A] → ends with User → inject synthetic
        let history = vec![user("A"), user("current")];
        let result = prepare_query_history(history.clone(), history.len());
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0], Message::User { .. }));
        assert!(matches!(result[1], Message::Assistant(_)));
    }

    #[test]
    fn double_interrupt_still_valid() {
        // [user_A, user_B, user_current] → pop → [user_A, user_B] → inject between & at end
        // → [user_A, asst_synthetic, user_B, asst_synthetic]
        let history = vec![user("A"), user("B"), user("current")];
        let result = prepare_query_history(history.clone(), history.len());
        assert_eq!(result.len(), 4);
        assert!(matches!(result[0], Message::User { .. }));
        assert!(matches!(result[1], Message::Assistant(_)));
        assert!(matches!(result[2], Message::User { .. }));
        assert!(matches!(result[3], Message::Assistant(_)));
    }

    #[test]
    fn empty_history_no_panic() {
        // Only the current user message was present — pop leaves empty, no injection
        let history = vec![user("current")];
        let result = prepare_query_history(history.clone(), history.len());
        assert!(result.is_empty());
    }
}
