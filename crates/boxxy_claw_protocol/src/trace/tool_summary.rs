//! Single-pass JSON tool-result summarization into `TurnTraceItem` steps.

use super::{TraceCategory, TurnTraceItem};
use crate::str_utils::truncate_chars_with_ellipsis;

pub(super) const MAX_TRACE_PREVIEW_CHARS: usize = 800;
const MAX_CMD_LABEL_CHARS: usize = 72;

pub(super) fn is_file_edit_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "replace_in_file" | "replace" | "file_write" | "write_file"
    )
}

fn clip_preview(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars_with_ellipsis(
        trimmed,
        MAX_TRACE_PREVIEW_CHARS,
    ))
}

/// Clips stdout/stderr into an 800-char preview without cloning multi-megabyte streams.
fn clip_command_streams(stdout: &str, stderr: &str, fallback: &str) -> Option<String> {
    let out = stdout.trim();
    let err = stderr.trim();
    match (out.is_empty(), err.is_empty()) {
        (true, true) => clip_preview(fallback),
        (false, true) => clip_preview(out),
        (true, false) => clip_preview(err),
        (false, false) => {
            if out.len() >= MAX_TRACE_PREVIEW_CHARS
                && out.chars().count() >= MAX_TRACE_PREVIEW_CHARS
            {
                return clip_preview(out);
            }
            let err_prefix = err
                .char_indices()
                .nth(MAX_TRACE_PREVIEW_CHARS)
                .map_or(err, |(idx, _)| &err[..idx]);
            let mut combined = String::with_capacity(out.len() + 1 + err_prefix.len());
            combined.push_str(out);
            combined.push('\n');
            combined.push_str(err_prefix);
            clip_preview(&combined)
        }
    }
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
        if val
            .get("error")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
        {
            return true;
        }
        if val.get("is_error").and_then(serde_json::Value::as_bool) == Some(true) {
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

fn summarize_generate_image_result(
    unquoted: &str,
    parsed: Option<&serde_json::Value>,
) -> (String, Option<String>) {
    if let Some(v) = parsed.filter(|v| v.is_object()) {
        if let Some(err) = v
            .get("error")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return ("Image generation failed".to_string(), clip_preview(err));
        }
        let detail = v
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                v.get("path")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|p| p.rsplit('/').next())
                    .filter(|s| !s.is_empty())
            });
        let label = detail.map_or_else(
            || "Generated image".to_string(),
            |d| {
                let first = d.lines().next().unwrap_or(d).trim();
                format!(
                    "Generated image: {}",
                    truncate_chars_with_ellipsis(first, MAX_CMD_LABEL_CHARS)
                )
            },
        );
        let preview = v
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || clip_preview(unquoted),
                |p| {
                    let model = v
                        .get("model")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if model.is_empty() {
                        Some(p.to_string())
                    } else {
                        Some(format!("{p} ({model})"))
                    }
                },
            );
        return (label, preview);
    }
    ("Generated image".to_string(), clip_preview(unquoted))
}

fn summarize_prepare_character_assets_result(
    unquoted: &str,
    parsed: Option<&serde_json::Value>,
) -> (String, Option<String>) {
    if let Some(v) = parsed.filter(|v| v.is_object()) {
        if let Some(err) = v
            .get("error")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return (
                "Character asset preparation failed".to_string(),
                clip_preview(err),
            );
        }
        let slug = v
            .get("slug")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("character");
        let dir = v
            .get("character_dir")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty());
        return (
            format!("Prepared character assets: {slug}"),
            dir.map_or_else(|| clip_preview(unquoted), |d| Some(d.to_string())),
        );
    }
    (
        "Prepared character assets".to_string(),
        clip_preview(unquoted),
    )
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
                return (label, clip_command_streams(stdout, stderr, unquoted));
            }
            ("$ command".to_string(), clip_preview(unquoted))
        }
        "file_read" | "read_file" => {
            let first_line = unquoted.lines().next().unwrap_or("");
            let label = first_line
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .filter(|s| s.starts_with("Showing "))
                .map_or_else(|| "Read file".to_string(), str::to_string);
            (label, clip_preview(unquoted))
        }
        "list_directory" => ("Listed directory".to_string(), clip_preview(unquoted)),
        "read_scrollback_page" => (
            "Read terminal scrollback".to_string(),
            clip_preview(unquoted),
        ),
        "read_pane_buffer" => ("Read companion pane".to_string(), clip_preview(unquoted)),
        "glob_search" => ("Searched file paths".to_string(), clip_preview(unquoted)),
        "grep_search" => ("Searched file contents".to_string(), clip_preview(unquoted)),
        "web_search" | "native_web_search" => {
            let first = unquoted.lines().next().unwrap_or("Web search").trim();
            let label = first.strip_prefix("Queries: ").map_or_else(
                || "Web search".to_string(),
                |q| {
                    format!(
                        "Web search: {}",
                        truncate_chars_with_ellipsis(q.trim(), MAX_CMD_LABEL_CHARS)
                    )
                },
            );
            (label, clip_preview(unquoted))
        }
        "http_fetch" => ("Fetched URL".to_string(), clip_preview(unquoted)),
        "file_delete" => ("Deleted file".to_string(), clip_preview(unquoted)),
        "list_processes" => ("Inspected running processes".to_string(), None),
        "get_system_info" => (
            "Checked system hardware & OS".to_string(),
            clip_preview(unquoted),
        ),
        "generate_image" => summarize_generate_image_result(unquoted, parsed),
        "prepare_character_assets" => {
            summarize_prepare_character_assets_result(unquoted, parsed)
        }
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

    if is_file_edit_tool(tool_name) {
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
            .and_then(|v| {
                v.as_str()
                    .or_else(|| v.get("content").and_then(serde_json::Value::as_str))
            })
            .unwrap_or(trimmed);
        let name = text
            .lines()
            .find_map(|l| {
                if let Some(rest) = l.strip_prefix("Skill '")
                    && let Some((quoted, _)) = rest.split_once("' activated")
                {
                    return Some(quoted);
                }
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
        "file_read"
        | "read_file"
        | "list_directory"
        | "read_scrollback_page"
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn tool_result_skips_successful_file_writes_and_preserves_bracketed_read_paths() {
        assert!(tool_result_to_trace_item("replace_in_file", r#"{"status":"ok"}"#).is_none());
        assert!(tool_result_to_trace_item("file_write", "\"Success\"").is_none());
        assert!(
            tool_result_to_trace_item(
                "sys_shell_exec",
                r#"{"command":"ls","stdout":"a","stderr":"","error":""}"#
            )
            .is_some_and(|item| matches!(item, TurnTraceItem::Tool { failed: false, .. }))
        );

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

        let read_item = tool_result_to_trace_item(
            "file_read",
            "\"[Showing lines 1-20 of 20 in src/routes/[slug]]\\nfn handler() {}\"",
        )
        .unwrap();
        match read_item {
            TurnTraceItem::Tool { label, .. } => {
                assert_eq!(label, "Showing lines 1-20 of 20 in src/routes/[slug]");
            }
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    #[test]
    fn generate_image_trace_summarizes_prompt_path_and_errors() {
        let ok_json = r#"{"success":true,"path":"/tmp/synthetic/cube.png","prompt":"A geometric blue cube","model":"gemini-3.1-flash-image","markdown":"![A geometric blue cube](/tmp/synthetic/cube.png)"}"#;
        let item = tool_result_to_trace_item("generate_image", ok_json).unwrap();
        assert_eq!(
            item,
            TurnTraceItem::Tool {
                tool_name: "generate_image".to_string(),
                category: TraceCategory::Other,
                label: "Generated image: A geometric blue cube".to_string(),
                preview: Some("/tmp/synthetic/cube.png (gemini-3.1-flash-image)".to_string()),
                failed: false,
            }
        );

        let err_json = r#"{"success":false,"error":"Reference images are not supported by this provider"}"#;
        let err_item = tool_result_to_trace_item("generate_image", err_json).unwrap();
        assert_eq!(
            err_item,
            TurnTraceItem::Tool {
                tool_name: "generate_image".to_string(),
                category: TraceCategory::Other,
                label: "Image generation failed".to_string(),
                preview: Some("Reference images are not supported by this provider".to_string()),
                failed: true,
            }
        );

        let char_ok = r#"{"success":true,"slug":"sora-dev","character_dir":"/tmp/synthetic/sora-dev"}"#;
        let char_item = tool_result_to_trace_item("prepare_character_assets", char_ok).unwrap();
        assert_eq!(
            char_item,
            TurnTraceItem::Tool {
                tool_name: "prepare_character_assets".to_string(),
                category: TraceCategory::Other,
                label: "Prepared character assets: sora-dev".to_string(),
                preview: Some("/tmp/synthetic/sora-dev".to_string()),
                failed: false,
            }
        );

        let skill_json = r#"{"success":true,"content":"Skill 'Boxxy Character Creator' activated. FULL INSTRUCTIONS:\n\n# Boxxy Character Creator Instructions"}"#;
        assert_eq!(
            tool_result_to_trace_item("activate_skill", skill_json),
            Some(TurnTraceItem::Skill {
                name: "Boxxy Character Creator".to_string()
            })
        );
    }
}
