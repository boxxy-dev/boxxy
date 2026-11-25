enum ResponseSegment {
    Prose(String),
    CodeBlock {
        raw: String,
        lang: String,
        content: String,
    },
}

#[must_use]
fn try_sanitize_shell_block(lang: &str, cmd: &str) -> Option<String> {
    let lang_lower = lang.to_ascii_lowercase();
    if !matches!(lang_lower.as_str(), "bash" | "sh" | "zsh" | "fish" | "") {
        return None;
    }

    // If an untagged block looks like source code rather than a shell command, leave it in prose.
    if lang_lower.is_empty()
        && (cmd.contains("import ")
            || cmd.contains("def ")
            || cmd.contains("fn ")
            || cmd.contains("use "))
    {
        return None;
    }

    let sanitized: Vec<&str> = cmd
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !l.trim().starts_with('#'))
        .collect();

    // Safety check: if the bash block contains literal escape characters,
    // the LLM is hallucinating a raw keystroke injection as a bash script.
    let is_raw_keystrokes = sanitized.iter().any(|l| {
        l.contains("\\u001b")
            || l.contains("\\e")
            || l.contains("\\x03")
            || l.contains("\\x04")
            || l.contains("^[")
    });
    if is_raw_keystrokes {
        return None;
    }

    let mut final_cmd = String::new();
    for (i, line) in sanitized.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(stripped) = trimmed.strip_suffix('\\') {
            final_cmd.push_str(stripped.trim_end());
            final_cmd.push(' ');
        } else {
            final_cmd.push_str(trimmed);
            if i < sanitized.len() - 1 {
                if trimmed.ends_with("&&") || trimmed.ends_with("||") || trimmed.ends_with(';') {
                    final_cmd.push(' ');
                } else {
                    final_cmd.push_str(" && ");
                }
            }
        }
    }

    (!final_cmd.is_empty()).then_some(final_cmd)
}

#[must_use]
pub fn extract_command_and_clean(text: &str) -> (Option<String>, String) {
    // IMPORTANT: Extracts a runnable shell markdown block (`bash`, `sh`, `zsh`, `fish`)
    // so the UI can present an interactive "Accept & Run" proposal card, while preserving
    // any non-command fenced code blocks (`rust`, `toml`, `python`, `text`, etc.) in `clean_text`.
    let mut segments: Vec<ResponseSegment> = Vec::new();
    let mut prose_buf = String::new();
    let mut active_block: Option<(usize, String, String, String)> = None; // (ticks, lang, content, raw)

    for line in text.lines() {
        let trimmed_start = line.trim_start();
        let ticks = trimmed_start.bytes().take_while(|&b| b == b'`').count();

        if let Some((fence_len, ref lang, ref mut content, ref mut raw)) = active_block {
            raw.push_str(line);
            raw.push('\n');
            let trimmed = line.trim();
            if ticks >= fence_len && trimmed.len() == ticks {
                let (_, block_lang, block_content, block_raw) = active_block.take().unwrap();
                segments.push(ResponseSegment::CodeBlock {
                    raw: block_raw,
                    lang: block_lang,
                    content: block_content,
                });
            } else {
                let _ = lang;
                content.push_str(line);
                content.push('\n');
            }
        } else if ticks >= 3 {
            if !prose_buf.is_empty() {
                segments.push(ResponseSegment::Prose(std::mem::take(&mut prose_buf)));
            }
            let info = trimmed_start[ticks..].trim().to_string();
            let mut raw = String::with_capacity(line.len() + 1);
            raw.push_str(line);
            raw.push('\n');
            active_block = Some((ticks, info, String::new(), raw));
        } else {
            prose_buf.push_str(line);
            prose_buf.push('\n');
        }
    }

    if let Some((_, _, _, unclosed_raw)) = active_block {
        prose_buf.push_str(&unclosed_raw);
    }
    if !prose_buf.is_empty() {
        segments.push(ResponseSegment::Prose(prose_buf));
    }

    let mut extracted_idx: Option<usize> = None;
    let mut final_cmd: Option<String> = None;
    for (idx, seg) in segments.iter().enumerate() {
        if let ResponseSegment::CodeBlock { lang, content, .. } = seg
            && let Some(cmd) = try_sanitize_shell_block(lang, content)
        {
            extracted_idx = Some(idx);
            final_cmd = Some(cmd);
            break;
        }
    }

    let mut clean_text = String::new();
    for (idx, seg) in segments.into_iter().enumerate() {
        if Some(idx) == extracted_idx {
            continue;
        }
        match seg {
            ResponseSegment::Prose(s) => clean_text.push_str(&s),
            ResponseSegment::CodeBlock { raw, .. } => clean_text.push_str(&raw),
        }
    }

    (final_cmd, clean_text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bash_command_while_preserving_explanatory_code_blocks() {
        let reply = "Add the missing semicolon in `src/parser.rs`:\n\n```rust\npub fn check() -> bool {\n    true\n}\n```\n\nThen verify the build:\n```bash\ncargo check \\\n  --workspace\n```";
        let (cmd, clean) = extract_command_and_clean(reply);
        assert_eq!(cmd.as_deref(), Some("cargo check --workspace"));
        assert!(
            clean.contains("```rust\npub fn check() -> bool {"),
            "explanatory non-bash code blocks must remain intact in clean diagnosis:\n{clean}"
        );
        assert!(!clean.contains("cargo check"));
    }

    #[test]
    fn ignores_non_shell_and_raw_keystroke_blocks() {
        let rust_only = "Here is the snippet:\n```rust\nfn main() {}\n```";
        let (cmd_none, clean_rust) = extract_command_and_clean(rust_only);
        assert_eq!(cmd_none, None);
        assert!(clean_rust.contains("```rust"));

        let keystrokes = "Send Ctrl+C:\n```bash\n\\x03\n```";
        let (cmd_keys, clean_keys) = extract_command_and_clean(keystrokes);
        assert_eq!(cmd_keys, None);
        assert!(clean_keys.contains("\\x03"));
    }
}
