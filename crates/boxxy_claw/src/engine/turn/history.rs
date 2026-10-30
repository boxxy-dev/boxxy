use rig::message::Message;

const SPEAKER_PREFIX: &str = "[SPEAKER_ID:";

/// Prefixes an assistant text response with an internal speaker attribution header
/// so multi-character roundtable sessions know which character authored each turn.
pub fn tag_assistant_text(character_id: &str, display_name: &str, text: &str) -> String {
    if character_id.is_empty() && display_name.is_empty() {
        return text.to_string();
    }
    let clean = strip_speaker_tag(text);
    format!("[SPEAKER_ID:{character_id}|NAME:{display_name}]\n{clean}")
}

/// Parses `[SPEAKER_ID:<id>|NAME:<name>]\n<body>`, returning `(id, name, body)`.
pub fn parse_speaker_tag(text: &str) -> Option<(&str, &str, &str)> {
    let rest = text.strip_prefix(SPEAKER_PREFIX)?;
    let newline_idx = rest.find('\n')?;
    let header = &rest[..newline_idx].strip_suffix(']')?;
    let (id, name) = header.split_once("|NAME:")?;
    let body = &rest[newline_idx + 1..];
    Some((id, name, body))
}

/// Strips the internal `[SPEAKER_ID:...|NAME:...]\n` prefix if present.
pub fn strip_speaker_tag(text: &str) -> &str {
    if let Some((_, _, body)) = parse_speaker_tag(text) {
        body
    } else {
        text
    }
}

/// Prepares the history for a new chat completion using default (single-speaker) perspective.
pub fn prepare_query_history(history: Vec<Message>, history_len: usize) -> Vec<Message> {
    prepare_query_history_for_speaker(history, history_len, "", "")
}

/// Prepares the history from the perspective of `current_speaker_id`.
///
/// - Assistant messages authored by `current_speaker_id` stay `Message::Assistant` (with the
///   internal `[SPEAKER_ID:...]` header stripped).
/// - Pure-text Assistant messages authored by *other* characters in a roundtable are converted
///   into `Message::User` blocks (`[Character <Name> said]: ...`) so the active speaker stays
///   strictly in its own persona without blending voices.
pub fn prepare_query_history_for_speaker(
    history: Vec<Message>,
    history_len: usize,
    current_speaker_id: &str,
    default_host_name: &str,
) -> Vec<Message> {
    // Context Hygiene 2.0: Aggressively strip ALL transient context from previous turns
    // (Skills, Radar, Memories, and Snapshots) so history grows near-zero tokens per turn.
    // Pop the current user message first — it will be passed as query_for_chat instead.
    let mut final_history = history;
    final_history.pop();

    // Remove synthetic `[ROUNDTABLE TURN ...]` handoff prompts from *previous* turns in history.
    // The current turn's `[ROUNDTABLE TURN ...]` prompt is already in `query_for_chat`, and the
    // characters' actual responses from previous rounds are preserved as Assistant / `[Character X said]` turns.
    final_history.retain(|m| {
        if let Message::User { content } = m
            && content.len() == 1
            && let Some(rig::message::UserContent::Text(t)) = content.first()
            && t.text.trim_start().starts_with("[ROUNDTABLE TURN")
        {
            return false;
        }
        true
    });

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

    // Perspective-flip Assistant messages from other characters into `[<Name> said]: ...` User turns
    // when they are pure text responses (not tool-call turns that require paired ToolResults).
    let mut perspective_history = Vec::with_capacity(final_history.len());
    for msg in final_history {
        match msg {
            Message::Assistant(mut turn) => {
                let has_tool_call = turn
                    .content
                    .iter()
                    .any(|c| matches!(c, rig::message::AssistantContent::ToolCall(_)));

                let mut other_speaker: Option<(String, String)> = None;
                for item in turn.content.iter_mut() {
                    if let rig::message::AssistantContent::Text(t) = item {
                        if let Some((spk_id, spk_name, body)) = parse_speaker_tag(&t.text) {
                            let is_other = !current_speaker_id.is_empty()
                                && !spk_id.is_empty()
                                && spk_id != current_speaker_id;
                            let spk_name_owned = spk_name.to_string();
                            let body_owned = body.to_string();
                            t.text = body_owned.clone();
                            if is_other && !has_tool_call {
                                other_speaker = Some((spk_name_owned, body_owned));
                            }
                        }
                    }
                }

                if let Some((spk_name, body)) = other_speaker {
                    let speaker_label = if spk_name.is_empty() {
                        if default_host_name.is_empty() {
                            "Peer Character"
                        } else {
                            default_host_name
                        }
                    } else {
                        &spk_name
                    };
                    perspective_history.push(Message::user(format!(
                        "[Character {speaker_label} said]:\n{body}"
                    )));
                } else {
                    perspective_history.push(Message::Assistant(turn));
                }
            }
            other => perspective_history.push(other),
        }
    }

    // If previous turns were interrupted or perspective-flipped, history may contain consecutive
    // User messages. If the second User message is a `[Character ... said]:` block and the first
    // is a pure text User block, merge them cleanly; otherwise inject a synthetic acknowledgment
    // to maintain strict user/assistant alternation.
    let mut repaired_history: Vec<Message> = Vec::with_capacity(perspective_history.len() + 1);
    for msg in perspective_history {
        if let Message::User { ref content } = msg
            && let Some(Message::User {
                content: prev_content,
            }) = repaired_history.last_mut()
        {
            let curr_peer_text = if content.len() == 1 {
                match content.first() {
                    Some(rig::message::UserContent::Text(t))
                        if t.text.starts_with("[Character ")
                            || t.text.starts_with("[ROUNDTABLE TURN") =>
                    {
                        Some(t.text.clone())
                    }
                    _ => None,
                }
            } else {
                None
            };

            let prev_is_text_only = prev_content
                .iter()
                .all(|c| matches!(c, rig::message::UserContent::Text(_)));

            if let Some(peer_text) = curr_peer_text
                && prev_is_text_only
                && let Some(rig::message::UserContent::Text(prev_t)) = prev_content.iter_mut().next()
            {
                prev_t.text = format!("{}\n\n{}", prev_t.text, peer_text);
                continue;
            }

            repaired_history.push(Message::assistant(
                "[Previous task was interrupted by the user]",
            ));
        }
        repaired_history.push(msg);
    }

    if let Some(Message::User { content }) = repaired_history.last() {
        let is_roundtable_context = content.iter().any(|c| {
            matches!(
                c,
                rig::message::UserContent::Text(t)
                    if t.text.contains("[Character ") || t.text.contains("[ROUNDTABLE TURN")
            )
        });
        let synthetic = if is_roundtable_context {
            "[Listening to the roundtable conversation]"
        } else {
            "[Previous task was interrupted by the user]"
        };
        repaired_history.push(Message::assistant(synthetic));
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

    #[test]
    fn perspective_flip_separates_speakers() {
        let tagged_levi = tag_assistant_text("id-levi", "Levi Kujo", "I prefer static types.");
        let tagged_niko = tag_assistant_text("id-niko", "Niko Una", "Borrow checker is awesome!");

        let history = vec![
            user("Debate types"),
            asst(&tagged_levi),
            user("[ROUNDTABLE TURN]"),
            asst(&tagged_niko),
            user("current turn prompt"),
        ];

        // From Levi's perspective (`id-levi`), Levi's turn is Assistant and Niko's turn is User
        let levi_view =
            prepare_query_history_for_speaker(history.clone(), history.len(), "id-levi", "Niko Una");
        assert!(matches!(levi_view[0], Message::User { .. }));
        assert!(matches!(levi_view[1], Message::Assistant(_)));
        assert!(matches!(levi_view[2], Message::User { .. }));
        assert!(matches!(levi_view.last(), Some(Message::Assistant(_))));

        // From Niko's perspective (`id-niko`), Levi's turn is merged into User and Niko's turn is Assistant
        let niko_view =
            prepare_query_history_for_speaker(history.clone(), history.len(), "id-niko", "Niko Una");
        assert_eq!(niko_view.len(), 2);
        assert!(matches!(niko_view[0], Message::User { .. }));
        assert!(matches!(niko_view[1], Message::Assistant(_)));
    }
}
