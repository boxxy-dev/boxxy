//! Structured per-turn agent activity trace (`TurnTraceItem`) and history coalescing.
//!
//! Provides `OpenCode`-style activity summarization (`+ 4 commands, 19 edits, 8 thoughts`),
//! reasoning header extraction, and legacy DB row folding on session resumption.

mod coalesce;
mod tool_summary;

pub use coalesce::coalesce_restored_history;
pub use tool_summary::tool_result_to_trace_item;

use crate::FileDiff;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Write as _;

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
        self.commands > 0 || self.edits > 0 || self.thoughts > 0 || self.reads > 0 || self.tools > 0
    }

    /// Formats the compact 1-line summary in `OpenCode`'s canonical bucket order:
    /// `"4 commands, 19 edits, 8 thoughts, 2 reads, 1 tool"`.
    #[must_use]
    pub fn format_label(&self) -> String {
        if !self.has_steps() {
            return String::new();
        }

        let mut out = String::with_capacity(48);
        let push_bucket = |out: &mut String, count: usize, singular: &str| {
            if count > 0 {
                if !out.is_empty() {
                    out.push_str(", ");
                }
                if count == 1 {
                    let _ = write!(out, "1 {singular}");
                } else {
                    let _ = write!(out, "{count} {singular}s");
                }
            }
        };

        push_bucket(&mut out, self.commands, "command");
        push_bucket(&mut out, self.edits, "edit");
        push_bucket(&mut out, self.thoughts, "thought");
        push_bucket(&mut out, self.reads, "read");
        push_bucket(&mut out, self.tools, "tool");

        out
    }
}

fn strip_redacted(raw: &str) -> Cow<'_, str> {
    if raw.contains("[REDACTED]") {
        Cow::Owned(raw.replace("[REDACTED]", ""))
    } else {
        Cow::Borrowed(raw)
    }
}

fn is_valid_bold_title_candidate(raw_candidate: &str) -> bool {
    !raw_candidate.is_empty()
        && !raw_candidate.contains('\n')
        && !raw_candidate.starts_with('*')
        && !raw_candidate.ends_with('*')
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
        if is_valid_bold_title_candidate(candidate) {
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
                .or_else(|| {
                    after_close
                        .strip_prefix("\r\n")
                        .or_else(|| after_close.strip_prefix('\n'))
                })
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
    let raw_candidate = &rest[..close_idx];
    if !is_valid_bold_title_candidate(raw_candidate) {
        return None;
    }
    let candidate = raw_candidate.trim();
    (!candidate.is_empty()).then(|| candidate.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_summary_matches_opencode_bucket_order_and_pluralization() {
        assert_eq!(ActivitySummary::default().format_label(), "");

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
    fn extract_reasoning_summary_supports_globs_trailing_spaces_and_strips_redacted() {
        assert_eq!(extract_reasoning_summary("   [REDACTED]  "), None);

        let (title, body) = extract_reasoning_summary(
            "**Inspecting crates/*/Cargo.toml**   \n\nNeed to verify flag handling in src/parser.rs.",
        )
        .expect("non-empty thought");
        assert_eq!(title.as_deref(), Some("Inspecting crates/*/Cargo.toml"));
        assert_eq!(body, "Need to verify flag handling in src/parser.rs.");

        let (plain_title, plain_body) =
            extract_reasoning_summary("Simple reasoning without bold header.").expect("thought");
        assert!(plain_title.is_none());
        assert_eq!(plain_body, "Simple reasoning without bold header.");

        assert_eq!(
            live_reasoning_title("**Checking *.rs types** and now streaming...").as_deref(),
            Some("Checking *.rs types")
        );
        assert_eq!(live_reasoning_title("***Triple star rejected***"), None);
        assert_eq!(live_reasoning_title("**Incomplete header"), None);
    }
}
