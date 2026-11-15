//! Structured per-turn agent activity trace (`TurnTraceItem`) and history coalescing.
//!
//! Provides `OpenCode`-style activity summarization (`+ 4 commands, 19 edits, 8 thoughts`),
//! reasoning header extraction, and legacy DB row folding on session resumption.

use crate::{FileDiff, PersistentClawRow};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

const MAX_TRACE_PREVIEW_CHARS: usize = 800;
const MAX_CMD_LABEL_CHARS: usize = 72;

/// High-level bucket for a tool execution inside a turn's collapsible trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceCategory {
    Command,
    Edit,
    Read,
    Search,
    Other,
}

/// A single chronological step recorded while an agent works on a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnTraceItem {
    /// Model reasoning / extended thinking block (`AssistantContent::Reasoning`).
    Thought {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    /// Completed tool execution (shell command, file read, grep/glob, web search, MCP, etc.).
    Tool {
        tool_name: String,
        category: TraceCategory,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preview: Option<String>,
        #[serde(default)]
        failed: bool,
    },
    /// Completed or auto-approved file write / edit carrying a native `FileDiff`.
    Edit {
        diff: FileDiff,
        #[serde(default)]
        auto_approved: bool,
    },
    /// Activated or triggered skill / instruction context.
    Skill { name: String },
}

/// Aggregated counts across a turn's trace, matching `OpenCode`'s `activitySummary` buckets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivitySummary {
    pub commands: usize,
    pub edits: usize,
    pub thoughts: usize,
    pub reads: usize,
    pub tools: usize,
    pub skills: Vec<String>,
}

impl ActivitySummary {
    /// Summarizes a slice of trace items into `OpenCode`-aligned activity buckets.
    #[must_use]
    pub fn summarize(items: &[TurnTraceItem]) -> Self {
        let mut out = Self::default();
        for item in items {
            match item {
                TurnTraceItem::Thought { content, .. } => {
                    if !content.trim().is_empty() {
                        out.thoughts += 1;
                    }
                }
                TurnTraceItem::Edit { .. } => {
                    out.edits += 1;
                }
                TurnTraceItem::Tool { category, .. } => match category {
                    TraceCategory::Command => out.commands += 1,
                    TraceCategory::Edit => out.edits += 1,
                    TraceCategory::Read => out.reads += 1,
                    TraceCategory::Search | TraceCategory::Other => out.tools += 1,
                },
                TurnTraceItem::Skill { name } => {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() && !out.skills.iter().any(|s| s == trimmed) {
                        out.skills.push(trimmed.to_string());
                    }
                }
            }
        }
        out
    }

    #[must_use]
    pub const fn has_steps(&self) -> bool {
        self.commands > 0
            || self.edits > 0
            || self.thoughts > 0
            || self.reads > 0
            || self.tools > 0
    }

    /// Formats the compact 1-line summary in `OpenCode`'s canonical bucket order:
    /// `"4 commands, 19 edits, 8 thoughts, 2 reads, 1 tool"`.
    #[must_use]
    pub fn format_label(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        let push_bucket = |parts: &mut Vec<String>, count: usize, singular: &str| {
            if count > 0 {
                if count == 1 {
                    parts.push(format!("1 {singular}"));
                } else {
                    parts.push(format!("{count} {singular}s"));
                }
            }
        };

        push_bucket(&mut parts, self.commands, "command");
        push_bucket(&mut parts, self.edits, "edit");
        push_bucket(&mut parts, self.thoughts, "thought");
        push_bucket(&mut parts, self.reads, "read");
        push_bucket(&mut parts, self.tools, "tool");

        parts.join(", ")
    }
}

fn strip_redacted(raw: &str) -> Cow<'_, str> {
    if raw.contains("[REDACTED]") {
        Cow::Owned(raw.replace("[REDACTED]", ""))
    } else {
        Cow::Borrowed(raw)
    }
}

/// Cleans a reasoning block and extracts a leading `**Bold Summary**` title when present,
/// mirroring `OpenCode`'s `reasoningSummary()`.
#[must_use]
pub fn extract_reasoning_summary(raw: &str) -> Option<(Option<String>, String)> {
    let cleaned = strip_redacted(raw);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest_after_open) = trimmed.strip_prefix("**")
        && let Some(close_idx) = rest_after_open.find("**")
    {
        let candidate = &rest_after_open[..close_idx];
        if !candidate.is_empty() && !candidate.contains('\n') && !candidate.contains('*') {
            // Allow optional trailing horizontal whitespace (` ` / `\t`) after `**Title**` before `\n`.
            let after_close = rest_after_open[close_idx + 2..].trim_start_matches([' ', '\t']);
            if after_close.is_empty() {
                let title = candidate.trim().to_string();
                if !title.is_empty() {
                    return Some((Some(title.clone()), title));
                }
            } else if let Some(body) = after_close
                .strip_prefix("\r\n\r\n")
                .or_else(|| after_close.strip_prefix("\n\n"))
                .or_else(|| after_close.strip_prefix("\r\n"))
                .or_else(|| after_close.strip_prefix('\n'))
            {
                let title = candidate.trim().to_string();
                if !title.is_empty() {
                    let clean_body = body.trim();
                    let final_body = if clean_body.is_empty() {
                        title.clone()
                    } else {
                        clean_body.to_string()
                    };
                    return Some((Some(title), final_body));
                }
            }
        }
    }

    Some((None, trimmed.to_string()))
}

/// Extracts a completed leading `**Title**` from an in-flight streaming reasoning buffer,
/// scanning only the opening line without cloning the entire buffer on every delta.
#[must_use]
pub fn live_reasoning_title(partial: &str) -> Option<String> {
    let trimmed_start = partial.trim_start();
    let head = trimmed_start.lines().next().unwrap_or(trimmed_start);
    let cleaned = strip_redacted(head);
    let rest = cleaned.trim_start().strip_prefix("**")?;
    let close_idx = rest.find("**")?;
    let candidate = rest[..close_idx].trim();
    if candidate.is_empty() || candidate.contains('*') {
        return None;
    }
    Some(candidate.to_string())
}

/// Human-friendly status string for an actively streaming or running tool call.
#[must_use]
pub fn busy_tool_label(tool_name: &str, streaming_args: bool) -> String {
    let verb = if streaming_args {
        "Preparing"
    } else {
        "Running"
    };
    match tool_name {
        "sys_shell_exec" | "terminal_exec" => format!("{verb} command…"),
        "run_background_command" => "Launching background command…".to_string(),
        "replace_in_file" | "replace" => {
            if streaming_args {
                "Preparing edit…".to_string()
            } else {
                "Editing file…".to_string()
            }
        }
        "file_write" | "write_file" => {
            if streaming_args {
                "Preparing file…".to_string()
            } else {
                "Writing file…".to_string()
            }
        }
        "file_delete" => "Deleting file…".to_string(),
        "file_read" | "read_file" => "Reading file…".to_string(),
        "list_directory" => "Listing directory…".to_string(),
        "read_scrollback_page" => "Reading scrollback…".to_string(),
        "read_pane_buffer" => "Reading pane…".to_string(),
        "glob_search" => "Finding files…".to_string(),
        "grep_search" => "Searching code…".to_string(),
        "web_search" | "native_web_search" => "Searching web…".to_string(),
        "http_fetch" => "Fetching URL…".to_string(),
        "activate_skill" => "Loading skill…".to_string(),
        "list_processes" => "Inspecting processes…".to_string(),
        "get_system_info" => "Checking system info…".to_string(),
        other => format!("{verb} {}…", other.replace('_', " ")),
    }
}

/// Truncates `text` to at most `max_chars` Unicode scalar values, appending `…` if clipped.
fn truncate_chars_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    if text.is_ascii() {
        return format!("{}…", &text[..max_chars]);
    }
    text.char_indices()
        .nth(max_chars)
        .map_or_else(|| text.to_string(), |(byte_idx, _)| format!("{}…", &text[..byte_idx]))
}

fn clip_preview(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars_with_ellipsis(trimmed, MAX_TRACE_PREVIEW_CHARS))
}

/// Parses `trimmed` as JSON only when it looks like a JSON string or object (`"` or `{`),
/// avoiding expensive DOM allocations for plain text or large `list_processes` `[...]` arrays.
fn parse_json_object_or_string(trimmed: &str) -> Option<serde_json::Value> {
    let first = trimmed.as_bytes().first().copied()?;
    if first == b'{' || first == b'"' {
        serde_json::from_str::<serde_json::Value>(trimmed).ok()
    } else {
        None
    }
}

fn detect_result_failure(trimmed: &str, parsed: Option<&serde_json::Value>) -> bool {
    if trimmed.contains("[USER_EXPLICIT_REJECT]") || trimmed.contains("[USER_INTERRUPTION]") {
        return true;
    }
    if let Some(val) = parsed {
        if val.get("error").and_then(serde_json::Value::as_str).is_some() {
            return true;
        }
        if val.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
            return true;
        }
        if let Some(code) = val.get("exit_code").and_then(serde_json::Value::as_i64)
            && code != 0
        {
            return true;
        }
        if let Some(s) = val.as_str()
            && s.trim_start().starts_with("Error:")
        {
            return true;
        }
    }
    trimmed.starts_with("Error:") || trimmed.starts_with("\"Error:")
}

fn extract_readable_result_summary(
    tool_name: &str,
    trimmed: &str,
    parsed: Option<&serde_json::Value>,
) -> (String, Option<String>) {
    let unquoted = parsed
        .and_then(serde_json::Value::as_str)
        .unwrap_or(trimmed);

    match tool_name {
        "sys_shell_exec" | "terminal_exec" | "run_background_command" => {
            if let Some(v) = parsed.filter(|v| v.is_object()) {
                let cmd = v
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| v.get("message").and_then(serde_json::Value::as_str));
                let stdout = v
                    .get("stdout")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let stderr = v
                    .get("stderr")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let combined = if !stderr.trim().is_empty() && !stdout.trim().is_empty() {
                    format!("{}\n{}", stdout.trim(), stderr.trim())
                } else if !stderr.trim().is_empty() {
                    stderr.trim().to_string()
                } else if !stdout.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    unquoted.to_string()
                };
                let label = cmd.map_or_else(
                    || format!("$ {}", tool_name.replace('_', " ")),
                    |c| {
                        let first = c.lines().next().unwrap_or(c).trim();
                        format!(
                            "$ {}",
                            truncate_chars_with_ellipsis(first, MAX_CMD_LABEL_CHARS)
                        )
                    },
                );
                return (label, clip_preview(&combined));
            }
            ("$ command".to_string(), clip_preview(unquoted))
        }
        "file_read" | "read_file" => {
            let first_line = unquoted.lines().next().unwrap_or("");
            let label = if first_line.starts_with("[Showing ") && first_line.ends_with(']') {
                first_line
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string()
            } else {
                "Read file".to_string()
            };
            (label, clip_preview(unquoted))
        }
        "list_directory" => ("Listed directory".to_string(), clip_preview(unquoted)),
        "read_scrollback_page" => ("Read terminal scrollback".to_string(), clip_preview(unquoted)),
        "read_pane_buffer" => ("Read companion pane".to_string(), clip_preview(unquoted)),
        "glob_search" => ("Searched file paths".to_string(), clip_preview(unquoted)),
        "grep_search" => ("Searched file contents".to_string(), clip_preview(unquoted)),
        "web_search" | "native_web_search" => {
            let first = unquoted.lines().next().unwrap_or("Web search").trim();
            let label = first
                .strip_prefix("Queries: ")
                .map_or_else(|| "Web search".to_string(), |q| format!("Web search: {q}"));
            (label, clip_preview(unquoted))
        }
        "http_fetch" => ("Fetched URL".to_string(), clip_preview(unquoted)),
        "file_delete" => ("Deleted file".to_string(), clip_preview(unquoted)),
        "list_processes" => ("Inspected running processes".to_string(), None),
        "get_system_info" => (
            "Checked system hardware & OS".to_string(),
            clip_preview(unquoted),
        ),
        other => (other.replace('_', " "), clip_preview(unquoted)),
    }
}

/// Converts a `ToolResult` payload into a `TurnTraceItem`, parsing JSON at most once.
///
/// Successful `replace_in_file` and `file_write` calls return `None` because their rich
/// `FileDiff` is already captured as [`TurnTraceItem::Edit`] when the write is approved;
/// failed or rejected file edits still emit a failed `Edit` tool item so errors remain visible.
#[must_use]
pub fn tool_result_to_trace_item(tool_name: &str, result_json: &str) -> Option<TurnTraceItem> {
    let trimmed = result_json.trim();
    let parsed = parse_json_object_or_string(trimmed);
    let failed = detect_result_failure(trimmed, parsed.as_ref());

    if matches!(
        tool_name,
        "replace_in_file" | "replace" | "file_write" | "write_file"
    ) {
        if !failed {
            return None;
        }
        let (_, preview) = extract_readable_result_summary(tool_name, trimmed, parsed.as_ref());
        return Some(TurnTraceItem::Tool {
            tool_name: tool_name.to_string(),
            category: TraceCategory::Edit,
            label: format!("{} failed", tool_name.replace('_', " ")),
            preview,
            failed: true,
        });
    }

    if tool_name == "activate_skill" && !failed {
        let text = parsed
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .unwrap_or(trimmed);
        let name = text
            .lines()
            .find_map(|l| {
                l.strip_prefix("# Skill: ")
                    .or_else(|| l.strip_prefix("Skill: "))
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("activated")
            .to_string();
        return Some(TurnTraceItem::Skill { name });
    }

    let category = match tool_name {
        "sys_shell_exec" | "terminal_exec" | "run_background_command" => TraceCategory::Command,
        "file_delete" => TraceCategory::Edit,
        "file_read" | "read_file" | "list_directory" | "read_scrollback_page"
        | "read_pane_buffer" => TraceCategory::Read,
        "glob_search" | "grep_search" | "web_search" | "native_web_search" | "http_fetch" => {
            TraceCategory::Search
        }
        _ => TraceCategory::Other,
    };

    let (label, preview) = extract_readable_result_summary(tool_name, trimmed, parsed.as_ref());
    Some(TurnTraceItem::Tool {
        tool_name: tool_name.to_string(),
        category,
        label,
        preview,
        failed,
    })
}

fn is_different_speaker(
    pending_name: Option<&str>,
    pending_cid: Option<&str>,
    row_name: Option<&str>,
    row_cid: Option<&str>,
) -> bool {
    match (pending_cid, row_cid) {
        (Some(a), Some(b)) => a != b,
        (None, None) => pending_name != row_name,
        _ => match (pending_name, row_name) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        },
    }
}

struct CoalesceState {
    out: Vec<PersistentClawRow>,
    pending_trace: Vec<TurnTraceItem>,
    pending_meta: Option<(String, Option<String>, Option<String>)>,
}

impl CoalesceState {
    fn with_capacity(cap: usize) -> Self {
        Self {
            out: Vec::with_capacity(cap),
            pending_trace: Vec::new(),
            pending_meta: None,
        }
    }

    fn flush_orphan(&mut self) {
        if self.pending_trace.is_empty() {
            self.pending_meta = None;
            return;
        }
        let (pane_id, agent_name, character_id) = self
            .pending_meta
            .take()
            .unwrap_or_else(|| (String::new(), None, None));
        self.out.push(PersistentClawRow::Diagnosis {
            pane_id,
            agent_name,
            character_id,
            content: String::new(),
            trace: std::mem::take(&mut self.pending_trace),
            usage: None,
        });
    }

    fn flush_if_speaker_changed(
        &mut self,
        agent_name: Option<&str>,
        character_id: Option<&str>,
    ) {
        if let Some((_, ref p_name, ref p_cid)) = self.pending_meta
            && is_different_speaker(p_name.as_deref(), p_cid.as_deref(), agent_name, character_id)
        {
            self.flush_orphan();
        }
    }

    fn push_trace_item(
        &mut self,
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        item: TurnTraceItem,
    ) {
        self.flush_if_speaker_changed(agent_name.as_deref(), character_id.as_deref());
        if self.pending_meta.is_none() {
            self.pending_meta = Some((pane_id, agent_name, character_id));
        }
        self.pending_trace.push(item);
    }

    fn take_merged_trace(
        &mut self,
        agent_name: Option<&str>,
        character_id: Option<&str>,
        persisted_turn_trace: Vec<TurnTraceItem>,
    ) -> Vec<TurnTraceItem> {
        self.flush_if_speaker_changed(agent_name, character_id);
        self.pending_meta = None;
        merge_turn_traces(&mut self.pending_trace, persisted_turn_trace)
    }

    fn ingest(&mut self, row: PersistentClawRow) {
        match row {
            PersistentClawRow::ToolCall {
                pane_id,
                agent_name,
                character_id,
                tool_name,
                result,
                ..
            } => {
                if let Some(item) = tool_result_to_trace_item(&tool_name, &result) {
                    self.push_trace_item(pane_id, agent_name, character_id, item);
                }
            }
            PersistentClawRow::ProcessList {
                pane_id,
                agent_name,
                character_id,
                ..
            } => {
                let item = TurnTraceItem::Tool {
                    tool_name: "list_processes".to_string(),
                    category: TraceCategory::Other,
                    label: "Inspected running processes".to_string(),
                    preview: None,
                    failed: false,
                };
                self.push_trace_item(pane_id, agent_name, character_id, item);
            }
            PersistentClawRow::FileDiff {
                pane_id,
                agent_name,
                character_id,
                diff,
                auto_approved,
                ..
            } => {
                let item = TurnTraceItem::Edit {
                    diff,
                    auto_approved,
                };
                self.push_trace_item(pane_id, agent_name, character_id, item);
            }
            PersistentClawRow::Diagnosis {
                pane_id,
                agent_name,
                character_id,
                content,
                trace,
                usage,
            } => {
                let merged_trace =
                    self.take_merged_trace(agent_name.as_deref(), character_id.as_deref(), trace);
                self.out.push(PersistentClawRow::Diagnosis {
                    pane_id,
                    agent_name,
                    character_id,
                    content,
                    trace: merged_trace,
                    usage,
                });
            }
            PersistentClawRow::Suggested {
                pane_id,
                agent_name,
                character_id,
                diagnosis,
                command,
                trace,
                usage,
            } => {
                let merged_trace =
                    self.take_merged_trace(agent_name.as_deref(), character_id.as_deref(), trace);
                self.out.push(PersistentClawRow::Suggested {
                    pane_id,
                    agent_name,
                    character_id,
                    diagnosis,
                    command,
                    trace: merged_trace,
                    usage,
                });
            }
            other @ (PersistentClawRow::User { .. }
            | PersistentClawRow::Command { .. }
            | PersistentClawRow::SystemMessage { .. }) => {
                self.flush_orphan();
                self.out.push(other);
            }
        }
    }
}

/// Coalesces persisted `PersistentClawRow` history on session resumption.
///
/// Sequential `ToolCall`, `ProcessList`, and `FileDiff` events belonging to a turn
/// fold cleanly into the turn's `Diagnosis` or `Suggested` row `trace`.
#[must_use]
pub fn coalesce_restored_history(rows: Vec<PersistentClawRow>) -> Vec<PersistentClawRow> {
    let mut state = CoalesceState::with_capacity(rows.len());
    for row in rows {
        state.ingest(row);
    }
    state.flush_orphan();
    state.out
}

fn merge_turn_traces(
    pending_db_items: &mut Vec<TurnTraceItem>,
    persisted_turn_trace: Vec<TurnTraceItem>,
) -> Vec<TurnTraceItem> {
    if persisted_turn_trace.is_empty() {
        return std::mem::take(pending_db_items);
    }
    // If `persisted_turn_trace` already captured the full ordered turn trace (thoughts + tools + edits),
    // prefer it so exact interleaving of thoughts and tools is preserved without duplicates.
    let has_non_thought = persisted_turn_trace
        .iter()
        .any(|i| !matches!(i, TurnTraceItem::Thought { .. } | TurnTraceItem::Skill { .. }));
    if has_non_thought {
        pending_db_items.clear();
        return persisted_turn_trace;
    }

    // Otherwise prepend thoughts/skills before DB tool/diff rows.
    let mut combined = persisted_turn_trace;
    combined.append(pending_db_items);
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_summary_matches_opencode_bucket_order_and_pluralization() {
        let diff = FileDiff::compute("src/module.rs", Some("a\n"), "b\n");
        let items = vec![
            TurnTraceItem::Thought {
                summary: Some("Planning changes".to_string()),
                content: "Checking types".to_string(),
                duration_ms: Some(1200),
            },
            TurnTraceItem::Tool {
                tool_name: "file_read".to_string(),
                category: TraceCategory::Read,
                label: "Read src/module.rs".to_string(),
                preview: None,
                failed: false,
            },
            TurnTraceItem::Tool {
                tool_name: "file_read".to_string(),
                category: TraceCategory::Read,
                label: "Read src/parser.rs".to_string(),
                preview: None,
                failed: false,
            },
            TurnTraceItem::Edit {
                diff,
                auto_approved: true,
            },
            TurnTraceItem::Tool {
                tool_name: "sys_shell_exec".to_string(),
                category: TraceCategory::Command,
                label: "$ cargo test".to_string(),
                preview: None,
                failed: false,
            },
            TurnTraceItem::Tool {
                tool_name: "grep_search".to_string(),
                category: TraceCategory::Search,
                label: "Searched file contents".to_string(),
                preview: None,
                failed: false,
            },
        ];

        let summary = ActivitySummary::summarize(&items);
        assert_eq!(summary.commands, 1);
        assert_eq!(summary.edits, 1);
        assert_eq!(summary.thoughts, 1);
        assert_eq!(summary.reads, 2);
        assert_eq!(summary.tools, 1);
        assert_eq!(
            summary.format_label(),
            "1 command, 1 edit, 1 thought, 2 reads, 1 tool"
        );
    }

    #[test]
    fn extract_reasoning_summary_parses_leading_bold_title_trailing_spaces_and_strips_redacted() {
        assert_eq!(extract_reasoning_summary("   [REDACTED]  "), None);

        let (title, body) = extract_reasoning_summary(
            "**Inspecting CLI parser**   \n\nNeed to verify flag handling in src/parser.rs.",
        )
        .expect("non-empty thought");
        assert_eq!(title.as_deref(), Some("Inspecting CLI parser"));
        assert_eq!(body, "Need to verify flag handling in src/parser.rs.");

        let (plain_title, plain_body) =
            extract_reasoning_summary("Simple reasoning without bold header.").expect("thought");
        assert!(plain_title.is_none());
        assert_eq!(plain_body, "Simple reasoning without bold header.");

        assert_eq!(
            live_reasoning_title("**Checking types** and now streaming...").as_deref(),
            Some("Checking types")
        );
        assert_eq!(live_reasoning_title("**Incomplete header"), None);
    }

    #[test]
    fn preview_truncation_counts_unicode_characters_not_bytes() {
        let box_chars = "─".repeat(500);
        let preview = clip_preview(&box_chars).unwrap();
        assert_eq!(preview.chars().count(), 500);

        let over = "─".repeat(900);
        let clipped = clip_preview(&over).unwrap();
        assert_eq!(clipped.chars().count(), MAX_TRACE_PREVIEW_CHARS + 1);
    }

    #[test]
    fn tool_result_skips_successful_file_writes_to_avoid_double_counting_diffs() {
        assert!(tool_result_to_trace_item("replace_in_file", r#"{"status":"ok"}"#).is_none());
        assert!(tool_result_to_trace_item("file_write", "\"Success\"").is_none());

        let failed = tool_result_to_trace_item(
            "replace_in_file",
            r#"{"error":"old_string not found in src/module.rs"}"#,
        )
        .expect("failed edit should be recorded");
        assert!(matches!(
            failed,
            TurnTraceItem::Tool {
                category: TraceCategory::Edit,
                failed: true,
                ..
            }
        ));
    }

    #[test]
    fn coalesce_restored_history_folds_legacy_tools_and_diffs_into_diagnosis() {
        let diff = FileDiff::compute("src/module.rs", Some("fn a() {}\n"), "fn b() {}\n");
        let rows = vec![
            PersistentClawRow::User {
                pane_id: "pane-1".to_string(),
                content: "Refactor module".to_string(),
                images: Vec::new(),
            },
            PersistentClawRow::ToolCall {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                tool_name: "file_read".to_string(),
                result: "\"[Showing lines 1-10 of 10 in src/module.rs]\"".to_string(),
                usage: None,
            },
            PersistentClawRow::FileDiff {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                diff,
                auto_approved: false,
                usage: None,
            },
            PersistentClawRow::Diagnosis {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                content: "Updated `src/module.rs`.".to_string(),
                trace: Vec::new(),
                usage: None,
            },
        ];

        let coalesced = coalesce_restored_history(rows);
        assert_eq!(coalesced.len(), 2);
        match &coalesced[1] {
            PersistentClawRow::Diagnosis { content, trace, .. } => {
                assert_eq!(content, "Updated `src/module.rs`.");
                let summary = ActivitySummary::summarize(trace);
                assert_eq!(summary.reads, 1);
                assert_eq!(summary.edits, 1);
            }
            other => panic!("expected coalesced Diagnosis row, got {other:?}"),
        }
    }

    #[test]
    fn coalesce_restored_history_isolates_traces_across_interrupted_speaker_handoff() {
        let rows = vec![
            PersistentClawRow::ToolCall {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Agent Alpha".to_string()),
                character_id: Some("char-alpha".to_string()),
                tool_name: "file_read".to_string(),
                result: "\"[Showing lines 1-5 of 5 in src/parser.rs]\"".to_string(),
                usage: None,
            },
            PersistentClawRow::ToolCall {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Agent Beta".to_string()),
                character_id: Some("char-beta".to_string()),
                tool_name: "sys_shell_exec".to_string(),
                result: r#"{"command":"cargo check","stdout":"ok","stderr":"","exit_code":0}"#
                    .to_string(),
                usage: None,
            },
            PersistentClawRow::Diagnosis {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Agent Beta".to_string()),
                character_id: Some("char-beta".to_string()),
                content: "Check passed.".to_string(),
                trace: Vec::new(),
                usage: None,
            },
        ];

        let coalesced = coalesce_restored_history(rows);
        assert_eq!(coalesced.len(), 2);
        match &coalesced[0] {
            PersistentClawRow::Diagnosis {
                character_id,
                content,
                trace,
                ..
            } => {
                assert_eq!(character_id.as_deref(), Some("char-alpha"));
                assert_eq!(content, "");
                let summary = ActivitySummary::summarize(trace);
                assert_eq!(summary.reads, 1);
                assert_eq!(summary.commands, 0);
            }
            other => panic!("expected flushed orphan Diagnosis for Alpha, got {other:?}"),
        }
        match &coalesced[1] {
            PersistentClawRow::Diagnosis {
                character_id,
                content,
                trace,
                ..
            } => {
                assert_eq!(character_id.as_deref(), Some("char-beta"));
                assert_eq!(content, "Check passed.");
                let summary = ActivitySummary::summarize(trace);
                assert_eq!(summary.commands, 1);
                assert_eq!(summary.reads, 0);
            }
            other => panic!("expected Diagnosis with Beta's command trace, got {other:?}"),
        }
    }
}
