use rig::message::Message;

const SPEAKER_PREFIX: &str = "[SPEAKER_ID:";
pub const SESSION_CHECKPOINT_PREFIX: &str = "[SESSION_CHECKPOINT]";
pub const SESSION_CHECKPOINT_ACK: &str = "Understood. Retaining session checkpoint context.";

/// Maximum characters retained per tool result in turns older than the freshness window
/// (matches OpenCode V2's `TRANSCRIPT_CLIP = 2000` in `session/compaction.ts`).
const TRANSCRIPT_CLIP_CHARS: usize = 2_000;
const TRANSCRIPT_CLIP_SUFFIX: &str =
    "\n...[stale tool output clipped — re-run file_read or grep_search if current contents are needed]";

/// Returns `true` if `msg` is the start of a real user exchange (contains user text,
/// is not a tool-result continuation, and is not a synthetic checkpoint/roundtable header).
pub fn is_exchange_start(msg: &Message) -> bool {
    let Message::User { content } = msg else {
        return false;
    };
    let has_tool_result = content
        .iter()
        .any(|c| matches!(c, rig::message::UserContent::ToolResult(_)));
    if has_tool_result {
        return false;
    }
    content.iter().any(|c| {
        if let rig::message::UserContent::Text(t) = c {
            let trimmed = t.text.trim_start();
            !trimmed.is_empty()
                && !trimmed.starts_with(SESSION_CHECKPOINT_PREFIX)
                && !trimmed.starts_with("[ROUNDTABLE TURN")
        } else {
            false
        }
    })
}

/// Finds a turn-boundary-safe index in `history` to compact older exchanges while
/// keeping at least `min_kept_exchanges` full recent exchanges intact.
///
/// Never cuts inside a multi-step tool loop (`Assistant(ToolCall)` ↔ `User(ToolResult)`).
pub fn find_safe_exchange_cut(history: &[Message], min_kept_exchanges: usize) -> Option<usize> {
    let exchange_starts: Vec<usize> = history
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| is_exchange_start(msg).then_some(idx))
        .collect();

    if exchange_starts.len() <= min_kept_exchanges {
        return None;
    }

    let cut_exchange_pos = exchange_starts.len() - min_kept_exchanges;
    let cut_idx = exchange_starts[cut_exchange_pos];
    (cut_idx > 0).then_some(cut_idx)
}

/// Builds a rolling in-session `[SESSION_CHECKPOINT]` (User + Assistant pair) summarizing
/// evicted exchanges so multi-hour coding sessions retain task goals, modified files, and
/// conclusions at index `0..2` of `SessionState.history` with 0ms latency and zero DB schema changes.
pub fn build_checkpoint_pair(evicted: &[Message]) -> Option<(Message, Message)> {
    const MAX_CHECKPOINT_CHARS: usize = 3_200;
    let mut prior_checkpoint = String::new();
    let mut entries: Vec<String> = Vec::new();
    let mut current_user: Option<String> = None;
    let mut current_tools: Vec<String> = Vec::new();
    let mut current_reply: Option<String> = None;

    let flush_entry = |entries: &mut Vec<String>,
                       user_q: &mut Option<String>,
                       tools: &mut Vec<String>,
                       reply: &mut Option<String>| {
        if user_q.is_none() && tools.is_empty() && reply.is_none() {
            return;
        }
        let mut line = String::new();
        if let Some(q) = user_q.take() {
            line.push_str(&format!("- **User:** {q}"));
        }
        if !tools.is_empty() {
            if !line.is_empty() {
                line.push_str(" | ");
            } else {
                line.push_str("- ");
            }
            line.push_str(&format!("**Tools:** {}", tools.join(", ")));
            tools.clear();
        }
        if let Some(r) = reply.take() {
            if !line.is_empty() {
                line.push_str(" | ");
            } else {
                line.push_str("- ");
            }
            line.push_str(&format!("**Outcome:** {r}"));
        }
        if !line.is_empty() {
            entries.push(line);
        }
    };

    for msg in evicted {
        match msg {
            Message::User { content } => {
                for item in content.iter() {
                    if let rig::message::UserContent::Text(t) = item {
                        let trimmed = t.text.trim();
                        if let Some(rest) = trimmed.strip_prefix(SESSION_CHECKPOINT_PREFIX) {
                            prior_checkpoint = rest.trim().to_string();
                            continue;
                        }
                        let cleaned =
                            crate::engine::summarization::history::clean_user_text_for_flush(
                                trimmed,
                            )
                            .trim();
                        if !cleaned.is_empty()
                            && !cleaned.starts_with("[WHILE_YOU_SLEPT]")
                            && !cleaned.starts_with("[ROUNDTABLE TURN")
                        {
                            flush_entry(
                                &mut entries,
                                &mut current_user,
                                &mut current_tools,
                                &mut current_reply,
                            );
                            let one_line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
                            let end = one_line.floor_char_boundary(180);
                            let snippet = if end < one_line.len() {
                                format!("{}...", &one_line[..end])
                            } else {
                                one_line
                            };
                            current_user = Some(snippet);
                        }
                    }
                }
            }
            Message::Assistant(turn) => {
                for item in turn.content.iter() {
                    match item {
                        rig::message::AssistantContent::ToolCall(tc) => {
                            let name = &tc.function.name;
                            let arg_hint = tc
                                .function
                                .arguments
                                .get("path")
                                .or_else(|| tc.function.arguments.get("command"))
                                .or_else(|| tc.function.arguments.get("pattern"))
                                .and_then(|v| v.as_str())
                                .map(|s| {
                                    let end = s.floor_char_boundary(48);
                                    if end < s.len() {
                                        format!("{name}({}...)", &s[..end])
                                    } else {
                                        format!("{name}({s})")
                                    }
                                })
                                .unwrap_or_else(|| name.to_string());
                            if current_tools.len() < 8 && !current_tools.contains(&arg_hint) {
                                current_tools.push(arg_hint);
                            }
                        }
                        rig::message::AssistantContent::Text(t) => {
                            let body = strip_speaker_tag(&t.text).trim();
                            if !body.is_empty()
                                && body != SESSION_CHECKPOINT_ACK
                                && body != "[SILENT_ACK]"
                                && body != "[Previous task was interrupted by the user]"
                                && body != "[Listening to the roundtable conversation]"
                            {
                                let one_line =
                                    body.split_whitespace().collect::<Vec<_>>().join(" ");
                                let end = one_line.floor_char_boundary(220);
                                let snippet = if end < one_line.len() {
                                    format!("{}...", &one_line[..end])
                                } else {
                                    one_line
                                };
                                current_reply = Some(snippet);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Message::System { .. } => {}
        }
    }
    flush_entry(
        &mut entries,
        &mut current_user,
        &mut current_tools,
        &mut current_reply,
    );

    if prior_checkpoint.is_empty() && entries.is_empty() {
        return None;
    }

    let mut combined = String::new();
    if !prior_checkpoint.is_empty() {
        combined.push_str(&prior_checkpoint);
    }
    for entry in entries {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&entry);
    }

    // Keep the newest lines if the rolling checkpoint exceeds MAX_CHECKPOINT_CHARS
    if combined.len() > MAX_CHECKPOINT_CHARS {
        let mut kept_lines: Vec<&str> = Vec::new();
        let mut total = 0usize;
        for line in combined.lines().rev() {
            let add = line.len() + 1;
            if total + add > MAX_CHECKPOINT_CHARS && !kept_lines.is_empty() {
                break;
            }
            kept_lines.push(line);
            total += add;
        }
        kept_lines.reverse();
        combined = kept_lines.join("\n");
    }

    let user_msg = Message::user(format!("{SESSION_CHECKPOINT_PREFIX}\n{combined}"));
    let asst_msg = Message::assistant(SESSION_CHECKPOINT_ACK);
    Some((user_msg, asst_msg))
}

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

/// Clips oversized string fields inside a `serde_json::Value` in-place when a stale
/// `ToolResultContent::Json` payload exceeds `TRANSCRIPT_CLIP_CHARS`.
fn clip_json_strings_in_place(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::String(s) => {
            if s.len() > TRANSCRIPT_CLIP_CHARS && !s.ends_with(TRANSCRIPT_CLIP_SUFFIX) {
                let cut = s.floor_char_boundary(TRANSCRIPT_CLIP_CHARS);
                s.truncate(cut);
                s.push_str(TRANSCRIPT_CLIP_SUFFIX);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                clip_json_strings_in_place(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                clip_json_strings_in_place(v);
            }
        }
        _ => {}
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

    // Count how many real User messages precede each index from the end so we keep
    // verbatim snapshots and full tool outputs for the last 4 user turns (regardless
    // of how many tool-call / tool-result messages each turn generated).
    let is_counted_user_turn = |content: &[rig::message::UserContent]| -> bool {
        content.iter().any(|c| {
            if let rig::message::UserContent::Text(t) = c {
                !t.text.trim_start().starts_with(SESSION_CHECKPOINT_PREFIX)
            } else {
                false
            }
        })
    };

    let total_user_msgs = final_history
        .iter()
        .filter(|m| matches!(m, Message::User { content } if is_counted_user_turn(content)))
        .count();

    let mut seen_user_msgs = 0;
    for msg in &mut final_history {
        if let Message::User { content } = msg {
            if is_counted_user_turn(content) {
                seen_user_msgs += 1;
            }
            let is_old = total_user_msgs.saturating_sub(seen_user_msgs) >= 4;

            for item in content.iter_mut() {
                match item {
                    rig::message::UserContent::Text(text) => {
                        let dyn_start = text
                            .text
                            .find("\n\n--- UPDATE CHANGELOG CONTEXT ---")
                            .or_else(|| text.text.find("\n\n## YOUR IDENTITY"))
                            .or_else(|| text.text.find("\n\n## CURRENT TURN CONTEXT"))
                            .or_else(|| text.text.find("\n\n--- GLOBAL RADAR"))
                            .or_else(|| text.text.find("\n\nTerminal Snapshot:\n```"));

                        if let Some(idx) = dyn_start {
                            if is_old {
                                // Outside the 4-turn freshness window: strip all dynamic context and snapshots
                                text.text.truncate(idx);
                            } else if let Some(snap_idx) = text.text[idx..].find("\n\nTerminal Snapshot:\n```") {
                                // Inside the 4-turn freshness window: strip redundant identity/memories/skills/radar
                                // (which are already freshly injected in the current USER PROMPT), while keeping the
                                // turn's Terminal Snapshot if it actually changed.
                                let abs_snap_idx = idx + snap_idx;
                                let snapshot_block = &text.text[abs_snap_idx..];
                                if snapshot_block.contains("[TERMINAL_SNAPSHOT: NO_CHANGE]") {
                                    text.text.truncate(idx);
                                } else {
                                    let snap_owned = snapshot_block.to_string();
                                    text.text.truncate(idx);
                                    text.text.push_str(&snap_owned);
                                }
                            } else {
                                text.text.truncate(idx);
                            }
                        }
                    }
                    rig::message::UserContent::ToolResult(tool_res) if is_old => {
                        // Safeguard 1: Never delete the Message or ToolResult wrapper/call_id.
                        // Only clip oversized inner text or JSON strings (> 2,000 chars) in the outgoing
                        // LLM history copy so UI renderers and provider tool pairing stay intact.
                        for tc in tool_res.content.iter_mut() {
                            match tc {
                                rig::message::ToolResultContent::Text(t) => {
                                    if t.text.len() > TRANSCRIPT_CLIP_CHARS
                                        && !t.text.ends_with(TRANSCRIPT_CLIP_SUFFIX)
                                    {
                                        let cut = t.text.floor_char_boundary(TRANSCRIPT_CLIP_CHARS);
                                        t.text.truncate(cut);
                                        t.text.push_str(TRANSCRIPT_CLIP_SUFFIX);
                                    }
                                }
                                rig::message::ToolResultContent::Json { value } => {
                                    clip_json_strings_in_place(value);
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
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

    fn tool_call_msg(id: &str, name: &str, path: &str) -> Message {
        Message::Assistant(rig::message::AssistantMessage::new(vec![
            rig::message::AssistantContent::tool_call(
                id,
                rig::message::ToolName::new(name).unwrap(),
                serde_json::json!({ "path": path }),
            ),
        ]))
    }

    fn tool_result_msg(id: &str, name: &str, payload: &str) -> Message {
        Message::tool_result(
            rig::message::ProviderCallId::new(id).unwrap().into(),
            rig::message::ToolName::new(name).unwrap(),
            payload,
        )
    }

    #[test]
    fn safe_exchange_cut_never_splits_tool_loops() {
        // Exchange 1 has 2 tool calls (6 messages total); Exchanges 2 & 3 are simple.
        let history = vec![
            user("Refactor parser"),
            tool_call_msg("call_1", "file_read", "src/parser.rs"),
            tool_result_msg("call_1", "file_read", "fn parse() {}"),
            tool_call_msg("call_2", "replace_in_file", "src/parser.rs"),
            tool_result_msg("call_2", "replace_in_file", "{\"success\":true}"),
            asst("Updated parser.rs."),
            user("Run tests"),
            asst("Tests passed."),
            user("Check git status"),
            asst("Working tree clean."),
        ];

        // Keeping 3 exchanges when only 3 exist should not cut
        assert_eq!(find_safe_exchange_cut(&history, 3), None);

        // Keeping 2 exchanges must cut right at index 6 ("Run tests"), skipping all tool calls of Exchange 1
        assert_eq!(find_safe_exchange_cut(&history, 2), Some(6));
        assert!(is_exchange_start(&history[6]));
    }

    #[test]
    fn stale_tool_results_are_clipped_while_recent_ones_remain_verbatim() {
        let big_payload = "x".repeat(5_000);
        let mut history = vec![
            user("Turn 1 old"),
            tool_call_msg("tc_old", "file_read", "src/old.rs"),
            tool_result_msg("tc_old", "file_read", &big_payload),
            asst("Read old.rs"),
        ];
        // Add 4 newer user turns so Turn 1 is outside the 4-turn freshness window
        for i in 2..=4 {
            history.push(user(&format!("Turn {i}")));
            history.push(asst(&format!("Reply {i}")));
        }
        // Turn 5 (recent, inside the last 4 turns) also has a big tool result
        history.push(user("Turn 5 recent"));
        history.push(tool_call_msg("tc_recent", "file_read", "src/recent.rs"));
        history.push(tool_result_msg("tc_recent", "file_read", &big_payload));
        history.push(asst("Read recent.rs"));
        // Current turn (popped by prepare_query_history)
        history.push(user("Turn 6 current"));

        let prepared = prepare_query_history(history, 0);

        // Old tool result at index 2 must still be a ToolResult with call "tc_old", but clipped
        if let Message::User { content } = &prepared[2]
            && let Some(rig::message::UserContent::ToolResult(tr)) = content.first()
            && let Some(rig::message::ToolResultContent::Text(t)) = tr.content.first()
        {
            assert_eq!(tr.call.to_string(), "tc_old");
            assert!(t.text.len() < 2_200);
            assert!(t.text.ends_with(TRANSCRIPT_CLIP_SUFFIX));
        } else {
            panic!("Expected clipped ToolResult at index 2");
        }

        // Recent tool result in Turn 5 must remain 100% verbatim (5,000 chars)
        let recent_tr_msg = &prepared[prepared.len() - 2];
        if let Message::User { content } = recent_tr_msg
            && let Some(rig::message::UserContent::ToolResult(tr)) = content.first()
            && let Some(rig::message::ToolResultContent::Text(t)) = tr.content.first()
        {
            assert_eq!(tr.call.to_string(), "tc_recent");
            assert_eq!(t.text.len(), 5_000);
        } else {
            panic!("Expected verbatim recent ToolResult");
        }
    }

    #[test]
    fn checkpoint_pair_rolls_forward_and_maintains_alternation() {
        let evicted_batch_1 = vec![
            user("Implement auth middleware\n\n## CURRENT TURN CONTEXT\nCWD: `/tmp`"),
            tool_call_msg("tc_1", "replace_in_file", "src/auth.rs"),
            tool_result_msg("tc_1", "replace_in_file", "{\"success\":true}"),
            asst("Implemented JWT validation in src/auth.rs."),
        ];
        let (chk_u1, chk_a1) =
            build_checkpoint_pair(&evicted_batch_1).expect("checkpoint should be created");

        // Second compaction merges the first checkpoint with newly evicted turns
        let evicted_batch_2 = vec![
            chk_u1,
            chk_a1,
            user("Add rate limiting"),
            tool_call_msg("tc_2", "file_write", "src/rate_limit.rs"),
            tool_result_msg("tc_2", "file_write", "{\"success\":true}"),
            asst("Added token-bucket rate limiter."),
        ];
        let (chk_u2, chk_a2) =
            build_checkpoint_pair(&evicted_batch_2).expect("merged checkpoint should be created");

        if let Message::User { content } = &chk_u2
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert!(t.text.starts_with(SESSION_CHECKPOINT_PREFIX));
            assert!(t.text.contains("Implement auth middleware"));
            assert!(t.text.contains("replace_in_file(src/auth.rs)"));
            assert!(t.text.contains("Add rate limiting"));
            assert!(t.text.contains("file_write(src/rate_limit.rs)"));
        } else {
            panic!("Expected user checkpoint message");
        }

        // Verify prepare_query_history keeps the checkpoint pair intact and valid
        let full = vec![
            chk_u2,
            chk_a2,
            user("Now write unit tests"),
            asst("Tests written."),
            user("Current prompt"),
        ];
        let prepared = prepare_query_history(full, 0);
        assert_eq!(prepared.len(), 4);
        assert!(matches!(prepared[0], Message::User { .. }));
        assert!(matches!(prepared[1], Message::Assistant(_)));
        assert!(matches!(prepared[2], Message::User { .. }));
        assert!(matches!(prepared[3], Message::Assistant(_)));
    }

    #[test]
    fn stale_json_tool_results_and_redundant_recent_turn_headers_are_cleaned() {
        let big_file = "a".repeat(4_500);
        let json_tool_res = Message::User {
            content: vec![rig::message::UserContent::ToolResult(
                rig::message::ToolResult {
                    call: rig::message::ProviderCallId::new("call_json_1").unwrap().into(),
                    name: rig::message::ToolName::new("file_read").unwrap(),
                    content: vec![rig::message::ToolResultContent::Json {
                        value: serde_json::json!({
                            "content": big_file,
                        }),
                    }],
                    is_error: false,
                },
            )],
        };

        let mut history = vec![
            user("Turn 1 old"),
            tool_call_msg("call_json_1", "file_read", "src/module.rs"),
            json_tool_res,
            asst("Read module.rs"),
            user("Turn 2"),
            asst("Reply 2"),
            user("Turn 3"),
            asst("Reply 3"),
            user(
                "Turn 4 recent\n\n## YOUR IDENTITY\nAgent Name: **red-pony**\n\n--- RELEVANT PAST INTERACTIONS ---\n- old recall\n\nTerminal Snapshot:\n```\n$ cargo test\nok\n```",
            ),
            asst("Reply 4"),
            user(
                "Turn 5 recent no change\n\n## YOUR IDENTITY\nAgent Name: **red-pony**\n\nTerminal Snapshot:\n```\n[TERMINAL_SNAPSHOT: NO_CHANGE]\n```",
            ),
            asst("Reply 5"),
            user("Turn 6 current"),
        ];

        let prepared = prepare_query_history(std::mem::take(&mut history), 0);

        // 1. Stale JSON ToolResult (Turn 1) has its inner `"content"` string clipped
        if let Message::User { content } = &prepared[2]
            && let Some(rig::message::UserContent::ToolResult(tr)) = content.first()
            && let Some(rig::message::ToolResultContent::Json { value }) = tr.content.first()
        {
            let inner = value["content"].as_str().unwrap();
            assert!(inner.len() < 2_200);
            assert!(inner.ends_with(TRANSCRIPT_CLIP_SUFFIX));
        } else {
            panic!("Expected clipped Json ToolResult at index 2");
        }

        // 2. Recent Turn 4 strips `## YOUR IDENTITY` & `RELEVANT PAST INTERACTIONS`, but keeps changed `Terminal Snapshot`
        if let Message::User { content } = &prepared[8]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert!(t.text.starts_with("Turn 4 recent"));
            assert!(!t.text.contains("## YOUR IDENTITY"));
            assert!(!t.text.contains("RELEVANT PAST INTERACTIONS"));
            assert!(t.text.contains("Terminal Snapshot:\n```\n$ cargo test\nok\n```"));
        } else {
            panic!("Expected cleaned Turn 4 user message");
        }

        // 3. Recent Turn 5 strips `## YOUR IDENTITY` and also drops `[TERMINAL_SNAPSHOT: NO_CHANGE]`
        if let Message::User { content } = &prepared[10]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert_eq!(t.text, "Turn 5 recent no change");
        } else {
            panic!("Expected cleaned Turn 5 user message");
        }
    }
}
