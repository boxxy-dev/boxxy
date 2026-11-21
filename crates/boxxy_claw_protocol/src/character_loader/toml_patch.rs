//! Multi-line-aware TOML lexer and lossless top-level string key patcher.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Intermediate struct for TOML parsing — `id` and `color` are optional so we can detect
/// first-time loads (auto-generating a UUID) and allow v2 packages to declare their default
/// color exclusively under `[theme].accent_color` in `manifest.toml`.
#[derive(Deserialize, Serialize)]
pub(super) struct CharacterToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    pub(super) display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) default_color: Option<String>,
    pub(super) duties: String,
    pub(super) personality: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) greetings: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TomlScanState {
    Normal,
    MultiBasic,
    MultiLiteral,
}

/// Advances the multi-line TOML string lexer across a single line so top-level key matching
/// never mistakes lines inside `"""..."""` or `'''...'''` blocks for top-level keys.
fn advance_toml_scan_state(line: &str, mut state: TomlScanState) -> TomlScanState {
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match state {
            TomlScanState::Normal => match bytes[i] {
                b'#' => break,
                b'"' => {
                    if bytes[i..].starts_with(b"\"\"\"") {
                        state = TomlScanState::MultiBasic;
                        i += 3;
                    } else {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == b'\\' {
                                i += 2;
                            } else if bytes[i] == b'"' {
                                i += 1;
                                break;
                            } else {
                                i += 1;
                            }
                        }
                    }
                }
                b'\'' => {
                    if bytes[i..].starts_with(b"'''") {
                        state = TomlScanState::MultiLiteral;
                        i += 3;
                    } else {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == b'\'' {
                                i += 1;
                                break;
                            }
                            i += 1;
                        }
                    }
                }
                _ => i += 1,
            },
            TomlScanState::MultiBasic => {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i..].starts_with(b"\"\"\"") {
                    i += 3;
                    // Consume up to 2 extra quotes in 4- or 5-quote closing sequences (`""""` / `"""""`).
                    let mut extra = 0;
                    while extra < 2 && i < bytes.len() && bytes[i] == b'"' {
                        i += 1;
                        extra += 1;
                    }
                    state = TomlScanState::Normal;
                } else {
                    i += 1;
                }
            }
            TomlScanState::MultiLiteral => {
                if bytes[i..].starts_with(b"'''") {
                    i += 3;
                    let mut extra = 0;
                    while extra < 2 && i < bytes.len() && bytes[i] == b'\'' {
                        i += 1;
                        extra += 1;
                    }
                    state = TomlScanState::Normal;
                } else {
                    i += 1;
                }
            }
        }
    }

    state
}

/// Checks if `trimmed_line` (known to be in `TomlScanState::Normal` outside any `[table]`)
/// assigns a complete single-line value to `target_key` (including single-line `"""..."""`).
fn matches_top_level_single_line_key(trimmed_line: &str, target_key: &str) -> bool {
    let Some(after_key) = trimmed_line.strip_prefix(target_key) else {
        return false;
    };
    let after_ws = after_key.trim_start_matches([' ', '\t']);
    if !after_ws.starts_with('=') {
        return false;
    }
    advance_toml_scan_state(trimmed_line, TomlScanState::Normal) == TomlScanState::Normal
}

fn format_toml_string_assignment(key: &str, value: &str) -> String {
    let encoded = toml::Value::String(value.to_string()).to_string();
    format!("{key} = {encoded}")
}

/// Safely updates or inserts a top-level string key in `CHARACTER.toml` while preserving
/// comments, key order, and multi-line `"""..."""` prompts, and validates that the patched
/// document still parses as a valid [`CharacterToml`].
pub(super) fn patch_top_level_string_key(
    content: &str,
    key: &str,
    new_value: &str,
    extra_insert_after: Option<(&str, &str)>,
    prepend_comment_if_inserted: Option<&str>,
) -> Result<String> {
    let mut state = TomlScanState::Normal;
    let mut in_subtable = false;
    let mut replaced = false;
    let mut out_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim_start();
        if state == TomlScanState::Normal && trimmed.starts_with('[') {
            in_subtable = true;
        }

        if !replaced
            && state == TomlScanState::Normal
            && !in_subtable
            && matches_top_level_single_line_key(trimmed, key)
        {
            replaced = true;
            out_lines.push(format_toml_string_assignment(key, new_value));
            if let Some((extra_k, extra_v)) = extra_insert_after {
                out_lines.push(format_toml_string_assignment(extra_k, extra_v));
            }
            // Matched line closed any quotes on the same line, so `state` stays `Normal`.
            continue;
        }

        state = advance_toml_scan_state(line, state);
        out_lines.push(line.to_string());
    }

    let mut out = if replaced {
        let mut joined = out_lines.join("\n");
        if content.ends_with('\n') || joined.is_empty() {
            joined.push('\n');
        }
        joined
    } else {
        let mut prefix = String::new();
        if let Some(comment) = prepend_comment_if_inserted {
            prefix.push_str(comment);
            prefix.push('\n');
        }
        prefix.push_str(&format_toml_string_assignment(key, new_value));
        if let Some((extra_k, extra_v)) = extra_insert_after {
            prefix.push('\n');
            prefix.push_str(&format_toml_string_assignment(extra_k, extra_v));
        }
        prefix.push_str("\n\n");
        prefix.push_str(content);
        if !prefix.ends_with('\n') {
            prefix.push('\n');
        }
        prefix
    };

    // Fallback safety net: if an exotic TOML construct ever prevented clean line-patching,
    // serialize via `toml::to_string_pretty` rather than writing invalid TOML to disk.
    if toml::from_str::<CharacterToml>(&out).is_err() {
        let mut parsed: CharacterToml =
            toml::from_str(content).context("Original CHARACTER.toml is invalid")?;
        match key {
            "id" => parsed.id = Some(new_value.to_string()),
            "color" => parsed.color = Some(new_value.to_string()),
            _ => bail!("Unsupported fallback key: {key}"),
        }
        if let Some(("default_color", extra_v)) = extra_insert_after {
            parsed.default_color = Some(extra_v.to_string());
        }
        out = toml::to_string_pretty(&parsed).context("Failed to serialize fallback TOML")?;
    }

    Ok(out)
}
