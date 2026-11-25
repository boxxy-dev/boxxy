use rig::message::Message;

const SPEAKER_PREFIX: &str = "[SPEAKER_ID:";
pub const SESSION_CHECKPOINT_PREFIX: &str = "[SESSION_CHECKPOINT]";
pub const SESSION_CHECKPOINT_ACK: &str = "Understood. Retaining session checkpoint context.";
pub const WHILE_YOU_SLEPT_ACK: &str = "Understood. Noted terminal activity while sleeping.";
pub const UNDO_HISTORY_PREFIX: &str = "[System: The user ran /undo";

/// Maximum characters retained per tool result in turns older than the freshness window
/// (matches `OpenCode` V2's `TRANSCRIPT_CLIP = 2000` in `session/compaction.ts`).
const TRANSCRIPT_CLIP_CHARS: usize = 2_000;
const TRANSCRIPT_CLIP_SUFFIX: &str =
    "\n...[stale tool output clipped — re-run file_read or grep_search if current contents are needed]";
const ATTACHMENT_CLIP_SUFFIX: &str = "\n...[stale attachment output clipped]";
const ROUNDTABLE_HANDOFF_BRIDGE: &str = "[Roundtable handoff]";
const COMPLETED_TOOL_STEP_BRIDGE: &str = "[Completed tool step]";
const USER_INTERRUPTION_PREFIX: &str =
    "[USER_INTERRUPTION]: The user did not approve this proposal and instead sent a new instruction: \"";
const USER_INTERRUPTION_SUFFIX: &str =
    "\". Do NOT retry the rejected proposal; address the user's new instruction immediately.";

/// Extracts the user's steering instruction from an `ApprovalOutcome::Interrupted` tool error string.
#[must_use]
pub(crate) fn extract_interruption_user_text(raw: &str) -> Option<&str> {
    let rest = raw.trim().strip_prefix(USER_INTERRUPTION_PREFIX)?;
    let inner = rest.strip_suffix(USER_INTERRUPTION_SUFFIX).unwrap_or(rest);
    let cleaned = crate::engine::summarization::history::clean_user_text_for_flush(inner).trim();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Extracts a steered user instruction embedded inside a `ToolResult` (either raw text or JSON `error`).
#[must_use]
pub(crate) fn extract_interruption_from_tool_result(
    tr: &rig::message::ToolResult,
) -> Option<String> {
    for part in &tr.content {
        match part {
            rig::message::ToolResultContent::Text(t) => {
                if let Some(msg) = extract_interruption_user_text(&t.text) {
                    return Some(msg.to_string());
                }
            }
            rig::message::ToolResultContent::Json { value } => {
                if let Some(err_str) = value.get("error").and_then(serde_json::Value::as_str)
                    && let Some(msg) = extract_interruption_user_text(err_str)
                {
                    return Some(msg.to_string());
                }
            }
            rig::message::ToolResultContent::Image(_) => {}
        }
    }
    None
}

/// Extracts a `--- UPDATE CHANGELOG CONTEXT ---` block from `section` (stripping any
/// trailing `## YOUR IDENTITY` / `## CURRENT TURN CONTEXT` metadata) so recent follow-up
/// turns inside the 4-turn freshness window retain post-update changelog visibility.
#[must_use]
fn extract_changelog_block(section: &str) -> Option<String> {
    let marker = super::WHATS_NEW_CONTEXT_MARKER;
    let start = section.find(marker)?;
    let from_marker = &section[start..];
    let end = from_marker
        .find("\n\n## YOUR IDENTITY")
        .or_else(|| from_marker.find("\n\n## CURRENT TURN CONTEXT"))
        .or_else(|| from_marker.find("\n\nTerminal Snapshot:\n```"))
        .unwrap_or(from_marker.len());
    let trimmed = from_marker[..end].trim();
    (!trimmed.is_empty()).then(|| format!("\n\n{trimmed}"))
}

/// Extracts a `--- REFERENCED FILES (PRE-LOADED VIA @MENTION) --- ... --- END REFERENCED FILES ---`
/// block from `section` so recent turns inside the 4-turn freshness window retain pre-loaded `@file` slices.
#[must_use]
fn extract_referenced_files_block(section: &str) -> Option<String> {
    let header = super::file_refs::REFERENCED_FILES_HEADER;
    let footer_line = format!("\n{}", super::file_refs::REFERENCED_FILES_FOOTER);
    let start = section.find(header)?;
    let after_header = start + header.len();
    let rel_end = section[after_header..].find(&footer_line)?;
    let end = after_header + rel_end + footer_line.len();
    Some(format!("\n\n{}", section[start..end].trim()))
}

/// Builds the settled user message text stored in `SessionState.history` and SQLite.
///
/// Retains user-authored text (plus inline attachments), `@file` slices, and changed non-empty
/// terminal snapshots for the 4-turn freshness window without storing transient per-turn
/// identity, skills, past memories, radar, or `EMPTY`/`NO_CHANGE` snapshot blocks.
#[must_use]
pub(crate) fn build_settled_user_history_text(
    annotated_prompt: &str,
    referenced_files_block: &str,
    final_snapshot_text: &str,
) -> String {
    let mut out = annotated_prompt.trim_end_matches(['\r', '\n']).to_string();
    let refs = referenced_files_block.trim();
    if !refs.is_empty() {
        out.push_str("\n\n");
        out.push_str(refs);
    }
    let snap = final_snapshot_text.trim();
    if !snap.is_empty()
        && snap != super::snapshot::SNAPSHOT_EMPTY_MARKER
        && snap != super::snapshot::SNAPSHOT_NO_CHANGE_MARKER
    {
        out.push_str("\n\n");
        out.push_str(&super::snapshot::format_terminal_snapshot_block(
            final_snapshot_text,
        ));
    }
    out
}

/// Returns `true` if `msg` is the start of a real user exchange (contains user text,
/// is not a tool-result continuation, and is not a synthetic checkpoint/roundtable header).
#[must_use]
pub fn is_exchange_start(msg: &Message) -> bool {
    let Message::User { content } = msg else {
        return false;
    };
    if content.iter().any(|c| c.as_tool_result().is_some()) {
        return false;
    }
    content
        .iter()
        .filter_map(rig::message::UserContent::as_text)
        .any(|text| {
            let trimmed = text.trim_start();
            !trimmed.is_empty()
                && !trimmed.starts_with(SESSION_CHECKPOINT_PREFIX)
                && !trimmed.starts_with("[WHILE_YOU_SLEPT]")
                && !trimmed.starts_with(UNDO_HISTORY_PREFIX)
                && !trimmed.starts_with("[ROUNDTABLE TURN")
                && !trimmed.starts_with("[COLLABORATIVE HANDOFF")
                && !trimmed.starts_with(ROUNDTABLE_HANDOFF_BRIDGE)
        })
}

/// Finds a turn-boundary-safe index in `history` to compact older exchanges while
/// keeping at least `min_kept_exchanges` full recent exchanges intact.
///
/// Never cuts inside a multi-step tool loop (`Assistant(ToolCall)` ↔ `User(ToolResult)`).
#[must_use]
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
#[must_use]
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
            if line.is_empty() {
                line.push_str("- ");
            } else {
                line.push_str(" | ");
            }
            line.push_str(&format!("**Tools:** {}", tools.join(", ")));
            tools.clear();
        }
        if let Some(r) = reply.take() {
            if line.is_empty() {
                line.push_str("- ");
            } else {
                line.push_str(" | ");
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
                for item in content {
                    match item {
                        rig::message::UserContent::Text(raw_text) => {
                            let trimmed = raw_text.text.trim();
                            if let Some(rest) = trimmed.strip_prefix(SESSION_CHECKPOINT_PREFIX) {
                                prior_checkpoint = rest.trim().to_string();
                                continue;
                            }
                            let cleaned = crate::engine::summarization::history::clean_user_text_for_flush(
                                trimmed,
                            )
                            .trim();
                            if !cleaned.is_empty()
                                && !cleaned.starts_with("[WHILE_YOU_SLEPT]")
                                && !cleaned.starts_with("[ROUNDTABLE TURN")
                                && !cleaned.starts_with("[COLLABORATIVE HANDOFF")
                                && !cleaned.starts_with(ROUNDTABLE_HANDOFF_BRIDGE)
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
                        rig::message::UserContent::ToolResult(tr) => {
                            if let Some(steered) = extract_interruption_from_tool_result(tr) {
                                flush_entry(
                                    &mut entries,
                                    &mut current_user,
                                    &mut current_tools,
                                    &mut current_reply,
                                );
                                let one_line = steered.split_whitespace().collect::<Vec<_>>().join(" ");
                                let end = one_line.floor_char_boundary(180);
                                let snippet = if end < one_line.len() {
                                    format!("{}...", &one_line[..end])
                                } else {
                                    one_line
                                };
                                current_user = Some(snippet);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Message::Assistant(turn) => {
                for item in &turn.content {
                    if let Some(tc) = item.as_tool_call() {
                        let name = &tc.function.name;
                        let arg_hint = tc
                            .function
                            .arguments
                            .get("path")
                            .or_else(|| tc.function.arguments.get("command"))
                            .or_else(|| tc.function.arguments.get("pattern"))
                            .and_then(|v| v.as_str())
                            .map_or_else(
                                || name.to_string(),
                                |s| {
                                    let end = s.floor_char_boundary(48);
                                    if end < s.len() {
                                        format!("{name}({}...)", &s[..end])
                                    } else {
                                        format!("{name}({s})")
                                    }
                                },
                            );
                        if current_tools.len() < 8 && !current_tools.contains(&arg_hint) {
                            current_tools.push(arg_hint);
                        }
                    } else if let Some(raw_text) = item.as_text() {
                        let body = strip_speaker_tag(raw_text).trim();
                        if !body.is_empty()
                            && body != SESSION_CHECKPOINT_ACK
                            && body != WHILE_YOU_SLEPT_ACK
                            && body != "[SILENT_ACK]"
                            && body != "[Previous task was interrupted by the user]"
                            && body != COMPLETED_TOOL_STEP_BRIDGE
                            && body != "[Listening to the roundtable conversation]"
                        {
                            let one_line = body.split_whitespace().collect::<Vec<_>>().join(" ");
                            let end = one_line.floor_char_boundary(220);
                            let snippet = if end < one_line.len() {
                                format!("{}...", &one_line[..end])
                            } else {
                                one_line
                            };
                            current_reply = Some(snippet);
                        }
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
#[must_use]
pub fn tag_assistant_text(character_id: &str, display_name: &str, text: &str) -> String {
    if character_id.is_empty() && display_name.is_empty() {
        return text.to_string();
    }
    let clean = strip_speaker_tag(text);
    format!("[SPEAKER_ID:{character_id}|NAME:{display_name}]\n{clean}")
}

/// Parses `[SPEAKER_ID:<id>|NAME:<name>]\n<body>`, returning `(id, name, body)`.
#[must_use]
pub fn parse_speaker_tag(text: &str) -> Option<(&str, &str, &str)> {
    let rest = text.strip_prefix(SPEAKER_PREFIX)?;
    let newline_idx = rest.find('\n')?;
    let header = &rest[..newline_idx].strip_suffix(']')?;
    let (id, name) = header.split_once("|NAME:")?;
    let body = &rest[newline_idx + 1..];
    Some((id, name, body))
}

/// Strips the internal `[SPEAKER_ID:...|NAME:...]\n` prefix if present.
#[must_use]
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

/// Locates the first `\n\n`-prefixed dynamic context block outside backtick code fences so
/// pasted logs or markdown inside `--- ATTACHMENTS ---` never prematurely truncate an attachment.
#[must_use]
fn find_dynamic_tail_outside_fences(s: &str) -> Option<usize> {
    const MARKERS: &[&str] = &[
        "--- UPDATE CHANGELOG CONTEXT ---",
        "## YOUR IDENTITY",
        "## CURRENT TURN CONTEXT",
        "--- REFERENCED FILES (PRE-LOADED VIA @MENTION) ---",
        "--- GLOBAL RADAR",
        "Terminal Snapshot:\n```",
    ];

    if !MARKERS.iter().any(|m| s.contains(m)) {
        return None;
    }

    let mut offset = 0usize;
    let mut active_fence_ticks: Option<usize> = None;

    for raw_line in s.split_inclusive('\n') {
        if active_fence_ticks.is_none() && offset >= 2 && &s[offset - 2..offset] == "\n\n" {
            let rest = &s[offset..];
            if MARKERS.iter().any(|m| rest.starts_with(m)) {
                return Some(offset - 2);
            }
        }

        let trimmed_end = raw_line.trim_end_matches(['\r', '\n']);
        if let Some(fence_len) = active_fence_ticks {
            let trimmed = trimmed_end.trim();
            let ticks = trimmed.bytes().take_while(|&b| b == b'`').count();
            if ticks >= fence_len && trimmed.len() == ticks {
                active_fence_ticks = None;
            }
        } else {
            let trimmed_start = trimmed_end.trim_start();
            let ticks = trimmed_start.bytes().take_while(|&b| b == b'`').count();
            if ticks >= 3 {
                active_fence_ticks = Some(ticks);
            }
        }

        offset += raw_line.len();
    }

    None
}

/// Returns the opening backtick count if `s` leaves a fenced code block unclosed.
#[must_use]
fn unclosed_fence_ticks(s: &str) -> Option<usize> {
    let mut active_fence_ticks: Option<usize> = None;
    for raw_line in s.lines() {
        let trimmed_end = raw_line.trim_end_matches('\r');
        if let Some(fence_len) = active_fence_ticks {
            let trimmed = trimmed_end.trim();
            let ticks = trimmed.bytes().take_while(|&b| b == b'`').count();
            if ticks >= fence_len && trimmed.len() == ticks {
                active_fence_ticks = None;
            }
        } else {
            let trimmed_start = trimmed_end.trim_start();
            let ticks = trimmed_start.bytes().take_while(|&b| b == b'`').count();
            if ticks >= 3 {
                active_fence_ticks = Some(ticks);
            }
        }
    }
    active_fence_ticks
}

/// Clips oversized `--- ATTACHMENTS ---` payloads (`> 2,000` chars) once a user turn ages
/// outside the 4-turn freshness window, closing any truncated code fence cleanly.
fn clip_stale_attachments_in_place(text: &mut String) {
    if text.ends_with(ATTACHMENT_CLIP_SUFFIX) {
        return;
    }
    let Some(att_idx) = text.find(super::ATTACHMENTS_CONTEXT_MARKER) else {
        return;
    };
    let att_slice = &text[att_idx..];
    if att_slice.len() <= TRANSCRIPT_CLIP_CHARS {
        return;
    }
    let cut = att_idx + att_slice.floor_char_boundary(TRANSCRIPT_CLIP_CHARS);
    text.truncate(cut);
    if let Some(ticks) = unclosed_fence_ticks(&text[att_idx..]) {
        text.push('\n');
        for _ in 0..ticks {
            text.push('`');
        }
    }
    text.push_str(ATTACHMENT_CLIP_SUFFIX);
}

/// Prepares the history for a new chat completion using default (single-speaker) perspective.
#[must_use]
pub fn prepare_query_history(history: Vec<Message>, history_len: usize) -> Vec<Message> {
    prepare_query_history_for_speaker(history, history_len, "", "")
}

/// Prepares the history from the perspective of `current_speaker_id`, popping the
/// trailing current-turn user message first (since `spawn_turn` pushes the active prompt
/// to `SessionState.history` before calling the model).
#[must_use]
pub fn prepare_query_history_for_speaker(
    mut history: Vec<Message>,
    history_len: usize,
    current_speaker_id: &str,
    default_host_name: &str,
) -> Vec<Message> {
    let _ = history_len;
    // Pop the current user message first — it will be passed as `query_for_chat` instead.
    history.pop();
    prepare_settled_history_for_speaker(history, current_speaker_id, default_host_name)
}

/// Prepares an already-settled history slice from the perspective of `current_speaker_id`
/// **without** popping the last message.
///
/// Shared by both `prepare_query_history_for_speaker` (after popping the outgoing prompt)
/// and `/btw` side queries on idle sessions so both paths produce a byte-for-byte identical
/// KV prompt-cache prefix.
#[must_use]
pub fn prepare_settled_history_for_speaker(
    mut final_history: Vec<Message>,
    current_speaker_id: &str,
    default_host_name: &str,
) -> Vec<Message> {
    // Remove synthetic `[ROUNDTABLE TURN ...]` handoff prompts from *previous* turns in history.
    // The current turn's `[ROUNDTABLE TURN ...]` prompt is already in `query_for_chat`, and the
    // characters' actual responses from previous rounds are preserved as Assistant / `[Character X said]` turns.
    final_history.retain(|m| {
        if let Message::User { content } = m
            && content.len() == 1
            && content
                .first()
                .and_then(rig::message::UserContent::as_text)
                .is_some_and(|t| {
                    let trimmed = t.trim_start();
                    trimmed.starts_with("[ROUNDTABLE TURN")
                        || trimmed.starts_with("[COLLABORATIVE HANDOFF")
                })
        {
            return false;
        }
        true
    });

    // Count how many real User messages precede each index from the end so we keep
    // verbatim snapshots and full tool outputs for the last 4 user turns (regardless
    // of how many tool-call / tool-result messages each turn generated).
    let is_counted_user_turn = |content: &[rig::message::UserContent]| -> bool {
        content
            .iter()
            .filter_map(rig::message::UserContent::as_text)
            .any(|t| {
                let trimmed = t.trim_start();
                !trimmed.starts_with(SESSION_CHECKPOINT_PREFIX)
                    && !trimmed.starts_with("[WHILE_YOU_SLEPT]")
                    && !trimmed.starts_with(UNDO_HISTORY_PREFIX)
                    && !trimmed.starts_with(ROUNDTABLE_HANDOFF_BRIDGE)
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
                        let dyn_start = find_dynamic_tail_outside_fences(&text.text);

                        if let Some(idx) = dyn_start {
                            if is_old {
                                // Outside the 4-turn freshness window: strip all dynamic context,
                                // pre-loaded `@file` slices, and snapshots.
                                text.text.truncate(idx);
                            } else {
                                // Inside the 4-turn freshness window: strip redundant identity/memories/skills/radar
                                // (which are freshly injected in the current USER PROMPT), while keeping any
                                // post-update changelog, `@file` slices, and changed non-empty Terminal Snapshot.
                                let tail_slice = &text.text[idx..];
                                let kept_changelog = extract_changelog_block(tail_slice);
                                let kept_refs = extract_referenced_files_block(tail_slice);
                                let kept_snap = tail_slice
                                    .find("\n\nTerminal Snapshot:\n```")
                                    .map(|rel| &tail_slice[rel..])
                                    .filter(|snap_block| {
                                        !snap_block.contains(super::snapshot::SNAPSHOT_NO_CHANGE_MARKER)
                                            && !snap_block.contains(super::snapshot::SNAPSHOT_EMPTY_MARKER)
                                            && snap_block
                                                .trim()
                                                .strip_prefix("Terminal Snapshot:\n")
                                                .is_some_and(|fenced| {
                                                    fenced
                                                        .lines()
                                                        .any(|l| !l.trim().bytes().all(|b| b == b'`'))
                                                })
                                    })
                                    .map(str::to_string);

                                text.text.truncate(idx);
                                if let Some(changelog_block) = kept_changelog {
                                    text.text.push_str(&changelog_block);
                                }
                                if let Some(refs_block) = kept_refs {
                                    text.text.push_str(&refs_block);
                                }
                                if let Some(snap_block) = kept_snap {
                                    text.text.push_str(&snap_block);
                                }
                            }
                        }

                        if is_old {
                            clip_stale_attachments_in_place(&mut text.text);
                        }
                    }
                    rig::message::UserContent::ToolResult(tool_res) if is_old => {
                        // Safeguard 1: Never delete the Message or ToolResult wrapper/call_id.
                        // Only clip oversized inner text or JSON strings (> 2,000 chars) in the outgoing
                        // LLM history copy so UI renderers and provider tool pairing stay intact.
                        for tc in &mut tool_res.content {
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

    // Perspective-flip Assistant messages from other characters into `[<Name> said]: ...` User turns
    // when they are pure text responses (not tool-call turns that require paired ToolResults).
    let mut perspective_history = Vec::with_capacity(final_history.len());
    for msg in final_history {
        match msg {
            Message::Assistant(mut turn) => {
                let has_tool_call = turn.content.iter().any(|c| c.as_tool_call().is_some());

                let mut other_speaker: Option<(String, String)> = None;
                for item in &mut turn.content {
                    if let rig::message::AssistantContent::Text(t) = item
                        && let Some((spk_id, spk_name, body)) = parse_speaker_tag(&t.text) {
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

    // Enforce strict bidirectional `User` / `Assistant` alternation across interrupted turns,
    // perspective-flipped peer speeches, and guest tool-call handoffs.
    let mut repaired_history: Vec<Message> = Vec::with_capacity(perspective_history.len() + 2);
    for msg in perspective_history {
        match (&msg, repaired_history.last_mut()) {
            (
                Message::User { content },
                Some(Message::User {
                    content: prev_content,
                }),
            ) => {
                let curr_peer_text = if content.len() == 1 {
                    content
                        .first()
                        .and_then(rig::message::UserContent::as_text)
                        .filter(|t| {
                            t.starts_with("[Character ") || t.starts_with("[ROUNDTABLE TURN")
                        })
                        .map(str::to_string)
                } else {
                    None
                };

                let prev_is_text_only = prev_content
                    .iter()
                    .all(|c| matches!(c, rig::message::UserContent::Text(_)));

                if let Some(ref peer_text) = curr_peer_text
                    && prev_is_text_only
                    && let Some(rig::message::UserContent::Text(prev_t)) =
                        prev_content.iter_mut().next()
                {
                    prev_t.text = format!("{}\n\n{}", prev_t.text, peer_text);
                    continue;
                }

                let bridge = if curr_peer_text.is_some() {
                    COMPLETED_TOOL_STEP_BRIDGE
                } else {
                    "[Previous task was interrupted by the user]"
                };
                repaired_history.push(Message::assistant(bridge));
            }
            (Message::Assistant(_), Some(Message::Assistant(_))) => {
                repaired_history.push(Message::user(ROUNDTABLE_HANDOFF_BRIDGE));
            }
            _ => {}
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

/// Formats a compact synthetic assistant bridge for an in-progress turn so `/btw` knows
/// what step or approval proposal is currently unsettled while preserving strict
/// `User` / `Assistant` alternation right before the `/btw` user question.
#[must_use]
pub fn format_in_flight_bridge(
    active_status: Option<&str>,
    pending_proposal_summary: Option<&str>,
    recent_trace: &[boxxy_claw_protocol::TurnTraceItem],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let trace_summary = boxxy_claw_protocol::ActivitySummary::summarize(recent_trace).format_label();
    if !trace_summary.is_empty() {
        parts.push(format!("completed so far in this turn: {trace_summary}"));
    }
    if let Some(proposal) = pending_proposal_summary.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("paused awaiting user approval on: {proposal}"));
    } else if let Some(status) = active_status.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("currently active step: {status}"));
    }
    if parts.is_empty() {
        "[Working on the current turn…]".to_string()
    } else {
        format!("[Working on the current turn — {}]", parts.join("; "))
    }
}

/// Builds the history vector for an ephemeral `/btw` side question without mutating session state.
#[must_use]
pub fn build_btw_history(
    in_flight_messages: Option<&[ std::sync::Arc<Message> ]>,
    settled_history: Vec<Message>,
    current_speaker_id: &str,
    default_host_name: &str,
    active_status: Option<&str>,
    pending_proposal_summary: Option<&str>,
    recent_trace: &[boxxy_claw_protocol::TurnTraceItem],
) -> Vec<Message> {
    if let Some(live) = in_flight_messages
        && !live.is_empty()
    {
        let mut out: Vec<Message> = Vec::with_capacity(live.len() + 1);
        out.extend(live.iter().map(|m| (**m).clone()));
        if matches!(out.last(), Some(Message::User { .. })) {
            out.push(Message::assistant(format_in_flight_bridge(
                active_status,
                pending_proposal_summary,
                recent_trace,
            )));
        }
        return out;
    }

    prepare_settled_history_for_speaker(settled_history, current_speaker_id, default_host_name)
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

    #[test]
    fn build_btw_history_handles_idle_and_mid_turn_tool_batches() {
        // 1. Idle session (`in_flight_messages` is None): preserves full settled history without popping
        let settled = vec![
            user("Read src/module.rs"),
            tool_call_msg("call_1", "file_read", "src/module.rs"),
            tool_result_msg("call_1", "file_read", "pub fn run() {}"),
            asst("`src/module.rs` defines `run()`."),
        ];
        let idle_btw = build_btw_history(None, settled, "", "", None, None, &[]);
        assert_eq!(idle_btw.len(), 4);
        assert!(matches!(idle_btw.last(), Some(Message::Assistant(_))));

        // 2. Active mid-turn (`in_flight_messages` ends on a settled ToolResult batch):
        //    keeps exact prefix and appends synthetic Assistant bridge with active status & trace.
        let in_flight = vec![
            std::sync::Arc::new(user("Refactor src/parser.rs")),
            std::sync::Arc::new(tool_call_msg("call_2", "file_read", "src/parser.rs")),
            std::sync::Arc::new(tool_result_msg("call_2", "file_read", "struct Parser;")),
        ];
        let trace = vec![boxxy_claw_protocol::TurnTraceItem::Tool {
            tool_name: "file_read".to_string(),
            category: boxxy_claw_protocol::TraceCategory::Read,
            label: "src/parser.rs".to_string(),
            preview: None,
            failed: false,
        }];
        let active_btw = build_btw_history(
            Some(&in_flight),
            Vec::new(),
            "",
            "",
            Some("Running cargo test…"),
            Some("file_write to `src/parser.rs`"),
            &trace,
        );
        assert_eq!(active_btw.len(), 4);
        assert!(matches!(active_btw[0], Message::User { .. }));
        assert!(matches!(active_btw[1], Message::Assistant(_)));
        assert!(matches!(active_btw[2], Message::User { .. }));
        if let Some(Message::Assistant(bridge)) = active_btw.last()
            && let Some(text) = bridge.content.first().and_then(|c| c.as_text())
        {
            assert!(text.contains("1 read"));
            assert!(text.contains("file_write to `src/parser.rs`"));
        } else {
            panic!("Expected trailing synthetic Assistant bridge message");
        }
    }

    #[test]
    fn referenced_files_stay_in_4_turn_window_and_steered_interruptions_enter_checkpoint() {
        let ref_block = format!(
            "\n\n{}\nThe user attached files:\n\n### `@src/parser.rs` (`/workspace/src/parser.rs`)\n[Showing lines 1-2 of 2]\n```text\n1: fn version() -> u32 {{ 1 }}\n2: // --------------------------------------------------\n```\n{}",
            super::super::file_refs::REFERENCED_FILES_HEADER,
            super::super::file_refs::REFERENCED_FILES_FOOTER,
        );

        let turn1_old = format!(
            "Explain @src/parser.rs\n\n## YOUR IDENTITY\nAgent Name: **niko**{ref_block}\n\nTerminal Snapshot:\n```\n{}\n```",
            super::super::snapshot::SNAPSHOT_EMPTY_MARKER,
        );
        let turn5_recent = format!(
            "Inspect @src/parser.rs again\n\n## YOUR IDENTITY\nAgent Name: **niko**{ref_block}\n\nTerminal Snapshot:\n```\n{}\n```",
            super::super::snapshot::SNAPSHOT_EMPTY_MARKER,
        );

        let history = vec![
            user(&turn1_old),
            asst("Explained turn 1."),
            user("Turn 2"),
            asst("Reply 2"),
            user("Turn 3"),
            asst("Reply 3"),
            user("Turn 4"),
            asst("Reply 4"),
            user(&turn5_recent),
            asst("Explained turn 5."),
            user("Turn 6 current"),
        ];

        let prepared = prepare_query_history(history, 0);

        // Turn 1 (outside 4-turn window) strips `@src/parser.rs` preload AND empty snapshot
        if let Message::User { content } = &prepared[0]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert_eq!(t.text, "Explain @src/parser.rs");
        } else {
            panic!("Expected stripped Turn 1 user message");
        }

        // Turn 5 (inside 4-turn window) retains `@src/parser.rs` (even with 50 dashes inside!)
        // while stripping `## YOUR IDENTITY` and `[TERMINAL_SNAPSHOT: EMPTY]`
        if let Message::User { content } = &prepared[8]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert!(t.text.starts_with("Inspect @src/parser.rs again"));
            assert!(!t.text.contains("## YOUR IDENTITY"));
            assert!(!t.text.contains("SNAPSHOT_EMPTY"));
            assert!(t.text.contains("1: fn version() -> u32 { 1 }"));
            assert!(t.text.contains("2: // --------------------------------------------------"));
        } else {
            panic!("Expected Turn 5 user message with retained @file block");
        }

        // Verify steered `[USER_INTERRUPTION]` inside a JSON `ToolResult` is preserved in `build_checkpoint_pair`
        let steered_err = boxxy_core_toolbox::ApprovalOutcome::Interrupted(format!(
            "Switch to @src/other.rs instead.{ref_block}"
        ))
        .rejection_message();
        let steered_tool_res = Message::User {
            content: vec![rig::message::UserContent::ToolResult(
                rig::message::ToolResult {
                    call: rig::message::ProviderCallId::new("call_steer").unwrap().into(),
                    name: rig::message::ToolName::new("replace_in_file").unwrap(),
                    content: vec![rig::message::ToolResultContent::Json {
                        value: serde_json::json!({
                            "success": false,
                            "error": steered_err,
                        }),
                    }],
                    is_error: false,
                },
            )],
        };
        let evicted = vec![
            user("Update src/module.rs"),
            tool_call_msg("call_steer", "replace_in_file", "src/module.rs"),
            steered_tool_res,
            asst("Updated src/other.rs instead."),
        ];
        let (chk_u, _) = build_checkpoint_pair(&evicted).expect("checkpoint");
        if let Message::User { content } = &chk_u
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert!(t.text.contains("Switch to @src/other.rs instead."));
            assert!(!t.text.contains("REFERENCED FILES"));
        } else {
            panic!("Expected steered user text in checkpoint");
        }

        // Settled history builder strips EMPTY/NO_CHANGE snapshots while keeping real snapshots & @file refs,
        // and `[WHILE_YOU_SLEPT]` + `WHILE_YOU_SLEPT_ACK` preserves alternation without counting toward 4-turn eviction.
        let settled_empty = build_settled_user_history_text(
            "Check status",
            "",
            super::super::snapshot::SNAPSHOT_EMPTY_MARKER,
        );
        assert_eq!(settled_empty, "Check status");
        let settled_changed =
            build_settled_user_history_text("Check status", ref_block.trim(), "$ cargo check\nok");
        assert!(settled_changed.contains("REFERENCED FILES"));
        assert!(settled_changed.contains("$ cargo check\nok"));

        let sleep_history = vec![
            user("[WHILE_YOU_SLEPT]\n$ cargo check (exit: 0)"),
            asst(WHILE_YOU_SLEPT_ACK),
            user("Next prompt"),
        ];
        let prepared_sleep = prepare_query_history(sleep_history, 0);
        assert_eq!(prepared_sleep.len(), 2);
        assert!(matches!(prepared_sleep[0], Message::User { .. }));
        assert!(matches!(prepared_sleep[1], Message::Assistant(_)));
    }

    #[test]
    fn whats_new_changelog_stays_in_4_turn_window_and_undo_does_not_evict_real_turns() {
        let wn_old = "Summarize!\n\n--- UPDATE CHANGELOG CONTEXT ---\nCommits:\n- old commit\n\n## YOUR IDENTITY\nAgent Name: **Levi Kujo**";
        let wn_recent = "Summarize!\n\n--- UPDATE CHANGELOG CONTEXT ---\nCommits:\n- recent vte fix\n\n## YOUR IDENTITY\nAgent Name: **Levi Kujo**";

        let history = vec![
            user(wn_old),
            asst("Old summary."),
            user(wn_recent),
            asst("Recent summary."),
            user("Follow-up 1"),
            asst("Reply 1"),
            user("[System: The user ran /undo to revert file changes from the last turn.]"),
            asst("Reverted."),
            user("Follow-up 2"),
            asst("Reply 2"),
            user("Follow-up 3"),
            asst("Reply 3"),
            user("Current prompt"),
        ];

        let prepared = prepare_query_history(history, 0);

        // `wn_old` (> 4 real user turns ago) drops the heavy changelog completely
        if let Message::User { content } = &prepared[0]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert_eq!(t.text, "Summarize!");
        } else {
            panic!("Expected stripped old What's New turn");
        }

        // `wn_recent` (4th real user turn back because `/undo` is not counted!) retains changelog
        // while stripping `## YOUR IDENTITY`
        if let Message::User { content } = &prepared[2]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert!(t.text.contains("recent vte fix"));
            assert!(!t.text.contains("## YOUR IDENTITY"));
        } else {
            panic!("Expected recent What's New turn to retain changelog");
        }
    }

    #[test]
    fn attachments_preserve_fenced_debug_markers_inside_window_and_clip_outside_window() {
        let huge_paste = "line_payload_data = 1;\n".repeat(160);
        assert!(huge_paste.len() > 3_200);
        let old_attachment = format!(
            "Inspect [Pasted ~160 lines]\n\n--- ATTACHMENTS ---\n\n**[Pasted ~160 lines]**\n```toml\n{huge_paste}```\n\nTerminal Snapshot:\n```\n[TERMINAL_SNAPSHOT: EMPTY]\n```"
        );
        let recent_fenced_debug = "Explain [Pasted ~6 lines]\n\n--- ATTACHMENTS ---\n\n**[Pasted ~6 lines]**\n```text\nsample log\n\n## YOUR IDENTITY\nAgent Name: **Sample**\n```\n\nTerminal Snapshot:\n```\n$ cargo test\nok\n```";

        let history = vec![
            user(&old_attachment),
            asst("Reviewed old paste."),
            user("Turn 2"),
            asst("Reply 2"),
            user("Turn 3"),
            asst("Reply 3"),
            user("Turn 4"),
            asst("Reply 4"),
            user(recent_fenced_debug),
            asst("Reviewed recent paste."),
            user("Turn 6 current"),
        ];

        let prepared = prepare_query_history(history, 0);

        // 1. Old attachment outside the 4-turn window is clipped at ~2,000 chars and closes its code fence!
        if let Message::User { content } = &prepared[0]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert!(t.text.len() < 2_200);
            assert!(t.text.ends_with(ATTACHMENT_CLIP_SUFFIX));
            assert!(
                unclosed_fence_ticks(&t.text).is_none(),
                "clipped attachment must close any open markdown code fence:\n{}",
                t.text
            );
        } else {
            panic!("Expected clipped old attachment user turn");
        }

        // 2. Recent attachment with literal `\n\n## YOUR IDENTITY` inside its ```text fence
        //    is preserved verbatim without premature truncation!
        if let Message::User { content } = &prepared[8]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert!(t.text.contains("Agent Name: **Sample**"));
            assert!(t.text.contains("Terminal Snapshot:\n```\n$ cargo test\nok\n```"));
        } else {
            panic!("Expected intact recent fenced attachment");
        }
    }

    #[test]
    fn collaborative_handoff_with_guest_tool_call_maintains_strict_bidirectional_alternation() {
        let niko_opening = tag_assistant_text("id-niko", "Niko Una", "Stepping aside for @levi-kujo!");
        let levi_reply = tag_assistant_text("id-levi", "Levi Kujo", "Generated the asset cleanly.");

        // Host Niko calls `invite_character` -> Guest Levi runs `[COLLABORATIVE HANDOFF]` and calls `generate_image` -> User sends Turn 2 to Niko!
        let history = vec![
            user("Create an icon"),
            tool_call_msg("call_inv_1", "invite_character", "levi-kujo"),
            tool_result_msg("call_inv_1", "invite_character", "{\"success\":true}"),
            asst(&niko_opening),
            user("[COLLABORATIVE HANDOFF — 1 Turn]\nComplete the requested task."),
            tool_call_msg("call_img_1", "generate_image", "assets/icon.png"),
            tool_result_msg("call_img_1", "generate_image", "{\"success\":true}"),
            asst(&levi_reply),
            user("Now add that icon to README.md"),
        ];

        let niko_view =
            prepare_query_history_for_speaker(history.clone(), history.len(), "id-niko", "Niko Una");

        // Verify strict User/Assistant alternation with zero consecutive Assistant or consecutive User messages!
        for pair in niko_view.windows(2) {
            let is_u0 = matches!(pair[0], Message::User { .. });
            let is_u1 = matches!(pair[1], Message::User { .. });
            assert_ne!(
                is_u0, is_u1,
                "Expected strict User/Assistant alternation across guest tool handoff, got adjacent messages:\n{pair:#?}"
            );
        }
        assert!(
            matches!(niko_view.last(), Some(Message::Assistant(_))),
            "Prepared history must end on Assistant before outgoing User prompt"
        );
        let serialized = format!("{niko_view:#?}");
        assert!(
            !serialized.contains("[Previous task was interrupted by the user]"),
            "Completed peer tool calls must never emit false user-interruption markers:\n{serialized}"
        );
    }

    #[test]
    fn nested_four_backtick_osc133_snapshots_retain_inside_window_and_strip_outside_window() {
        let osc133_raw = "\n[PROMPT]\n```text\nbox$ \n```\n\n[COMMAND]\n```bash\ncargo test\n```\n\n[OUTPUT]\n```text\ntest result: ok. 8 passed\n```\n";
        let mut last_hash = None;
        let processed = super::super::snapshot::process_snapshot(osc133_raw, &mut last_hash);
        let settled = build_settled_user_history_text("Did tests pass?", "", &processed);
        assert!(settled.contains("\n\nTerminal Snapshot:\n````\n[PROMPT]\n```text"));

        let history = vec![
            user(&settled),
            asst("Turn 1 passed."),
            user("Turn 2"),
            asst("Reply 2"),
            user("Turn 3"),
            asst("Reply 3"),
            user("Turn 4"),
            asst("Reply 4"),
            user(&settled),
            asst("Turn 5 passed."),
            user("Turn 6 current"),
        ];

        let prepared = prepare_query_history(history, 0);

        // Turn 1 (outside 4-turn window) strips the entire 4-backtick OSC 133 snapshot cleanly
        if let Message::User { content } = &prepared[0]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert_eq!(t.text, "Did tests pass?");
        } else {
            panic!("Expected stripped Turn 1 OSC 133 snapshot");
        }

        // Turn 5 (inside 4-turn window) retains the full nested OSC 133 `[PROMPT]`/`[COMMAND]`/`[OUTPUT]` blocks
        if let Message::User { content } = &prepared[8]
            && let Some(rig::message::UserContent::Text(t)) = content.first()
        {
            assert!(t.text.contains("[COMMAND]\n```bash\ncargo test\n```"));
            assert!(t.text.contains("test result: ok. 8 passed"));
        } else {
            panic!("Expected retained Turn 5 OSC 133 snapshot");
        }
    }
}
