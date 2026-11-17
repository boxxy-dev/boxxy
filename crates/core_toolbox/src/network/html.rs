//! Single-pass structure-preserving HTML-to-Markdown text extractor.

use std::borrow::Cow;

fn is_raw_skip_tag(tag_name: &str) -> Option<&'static str> {
    match tag_name {
        "script" => Some("script"),
        "style" => Some("style"),
        "svg" => Some("svg"),
        "noscript" => Some("noscript"),
        "head" => Some("head"),
        _ => None,
    }
}

fn is_container_skip_tag(tag_name: &str) -> bool {
    matches!(tag_name, "nav" | "footer")
}

/// Finds the byte index right after the closing `</tag_name...>` for raw-text elements
/// (`<script>`, `<style>`, `<svg>`), ignoring `<` comparisons or string literals inside JS/CSS.
fn find_raw_closing_tag(rest: &str, tag_name: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let needle_len = tag_name.len();
    let mut i = 0;
    while i + 2 + needle_len < bytes.len() {
        if bytes[i] == b'<'
            && bytes[i + 1] == b'/'
            && bytes[i + 2..i + 2 + needle_len].eq_ignore_ascii_case(tag_name.as_bytes())
        {
            let after_name = bytes[i + 2 + needle_len];
            if after_name == b'>' || after_name == b'/' || after_name.is_ascii_whitespace() {
                if let Some(gt_rel) = bytes[i + 2 + needle_len..].iter().position(|&b| b == b'>') {
                    return Some(i + 2 + needle_len + gt_rel + 1);
                }
                return Some(rest.len());
            }
        }
        i += 1;
    }
    None
}

/// Decodes HTML character entities in a single pass without double-unescaping `&amp;lt;`.
fn decode_html_entities(input: &str) -> Cow<'_, str> {
    if !input.contains('&') {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp_idx) = rest.find('&') {
        out.push_str(&rest[..amp_idx]);
        let after_amp = &rest[amp_idx + 1..];
        let max_scan = after_amp.floor_char_boundary(after_amp.len().min(12));
        if let Some(semi_idx) = after_amp[..max_scan].find(';') {
            let entity = &after_amp[..semi_idx];
            if let Some(decoded) = resolve_entity(entity) {
                out.push(decoded);
                rest = &after_amp[semi_idx + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after_amp;
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn resolve_entity(entity: &str) -> Option<char> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    if let Some(dec) = entity.strip_prefix('#') {
        return dec.parse::<u32>().ok().and_then(char::from_u32);
    }
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "rsquo" => Some('’'),
        "lsquo" => Some('‘'),
        "rdquo" => Some('”'),
        "ldquo" => Some('“'),
        "hellip" => Some('…'),
        "copy" => Some('©'),
        _ => None,
    }
}

fn emit_structural_tag(
    out: &mut String,
    is_closing: bool,
    tag_name: &str,
    pre_depth: &mut u32,
    last_was_space: &mut bool,
) {
    match (is_closing, tag_name) {
        (false, "pre") => {
            if *pre_depth == 0 {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n");
            }
            *pre_depth += 1;
            *last_was_space = true;
        }
        (true, "pre") => {
            *pre_depth = pre_depth.saturating_sub(1);
            if *pre_depth == 0 {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n");
                *last_was_space = true;
            }
        }
        (false, "h1") if *pre_depth == 0 => {
            out.push_str("\n# ");
            *last_was_space = true;
        }
        (false, "h2") if *pre_depth == 0 => {
            out.push_str("\n## ");
            *last_was_space = true;
        }
        (false, "h3") if *pre_depth == 0 => {
            out.push_str("\n### ");
            *last_was_space = true;
        }
        (false, "h4" | "h5" | "h6") if *pre_depth == 0 => {
            out.push_str("\n#### ");
            *last_was_space = true;
        }
        (false, "li") if *pre_depth == 0 => {
            out.push_str("\n- ");
            *last_was_space = true;
        }
        (_, "code") if *pre_depth == 0 => {
            out.push('`');
        }
        (
            _,
            "p" | "div" | "section" | "article" | "tr" | "br" | "h1" | "h2" | "h3" | "h4" | "h5"
            | "h6" | "li",
        ) if *pre_depth == 0 => {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            *last_was_space = true;
        }
        _ => {}
    }
}

fn clean_extracted_lines(decoded: &str) -> String {
    let mut clean_lines = Vec::new();
    let mut in_fence = false;
    let mut prev_empty = false;

    for line in decoded.lines() {
        if line.trim() == "```" {
            in_fence = !in_fence;
            clean_lines.push("```");
            prev_empty = false;
            continue;
        }
        if in_fence {
            clean_lines.push(line.trim_end());
            prev_empty = false;
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_empty {
                clean_lines.push("");
                prev_empty = true;
            }
        } else {
            clean_lines.push(trimmed);
            prev_empty = false;
        }
    }

    clean_lines.join("\n").trim().to_string()
}

#[must_use]
pub(super) fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 4);
    let mut container_skip_depth: u32 = 0;
    let mut pre_depth: u32 = 0;
    let mut last_was_space = true;

    let mut pos = 0;
    while pos < html.len() {
        let rem = &html[pos..];

        if let Some(after_comment_start) = rem.strip_prefix("<!--") {
            if let Some(end_comment) = after_comment_start.find("-->") {
                pos += 4 + end_comment + 3;
            } else {
                break;
            }
            continue;
        }

        if let Some(after_lt) = rem.strip_prefix('<') {
            let Some(gt_offset) = after_lt.find('>') else {
                break;
            };
            let raw_tag = after_lt[..gt_offset].trim();
            pos += gt_offset + 2;

            if raw_tag.starts_with('!') || raw_tag.starts_with('?') {
                continue;
            }

            let is_closing = raw_tag.starts_with('/');
            let is_self_closing = raw_tag.ends_with('/');
            let tag_name = raw_tag
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();

            if !is_closing
                && !is_self_closing
                && let Some(raw_skip) = is_raw_skip_tag(&tag_name)
            {
                if let Some(skip_end) = find_raw_closing_tag(&html[pos..], raw_skip) {
                    pos += skip_end;
                } else {
                    break;
                }
                continue;
            }

            if is_container_skip_tag(&tag_name) {
                if is_closing {
                    container_skip_depth = container_skip_depth.saturating_sub(1);
                } else if !is_self_closing {
                    container_skip_depth += 1;
                }
                continue;
            }

            if container_skip_depth == 0 {
                emit_structural_tag(
                    &mut out,
                    is_closing,
                    &tag_name,
                    &mut pre_depth,
                    &mut last_was_space,
                );
            }
            continue;
        }

        let Some(ch) = rem.chars().next() else {
            break;
        };
        pos += ch.len_utf8();

        if container_skip_depth > 0 {
            continue;
        }

        if pre_depth > 0 {
            if ch != '\r' {
                out.push(ch);
            }
        } else if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }

    let decoded = decode_html_entities(out.trim());
    clean_extracted_lines(&decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_tags_removes_noise_and_preserves_markdown_structure() {
        let html = r#"
            <html>
            <head><title>Ignored Title</title><style>.a { color: red; }</style></head>
            <body>
                <!-- comment with > bracket inside -->
                <nav><a href="/">Home</a></nav>
                <script>if (a < b) { const x = "<style>"; }</script>
                <svg><path d="M0 0"/></svg>
                <h1>Main Heading</h1>
                <p>Use <code>cargo test</code> &amp;lt;check&amp;gt;:</p>
                <ul>
                    <li>First item</li>
                    <li>Second item</li>
                </ul>
                <pre><code>fn main() {
    let x = 1;
}</code></pre>
                <footer>Copyright 2026</footer>
            </body>
            </html>
        "#;

        let text = strip_html_tags(html);
        assert!(!text.contains("Ignored Title"));
        assert!(!text.contains("color: red"));
        assert!(!text.contains("Home"));
        assert!(!text.contains("Copyright"));
        assert!(!text.contains("const x"));
        assert!(text.contains("# Main Heading"));
        assert!(text.contains("`cargo test`"));
        assert!(text.contains("&lt;check&gt;"));
        assert!(text.contains("- First item"));
        assert!(text.contains("```\nfn main() {\n    let x = 1;\n}\n```"));
    }

    #[test]
    fn utf8_multibyte_boundary_truncation_never_panics() {
        let repeated = "—".repeat(20_000);
        let html = format!("<p>{repeated}</p>");
        let text = strip_html_tags(&html);
        let cut = text.floor_char_boundary(super::super::MAX_EXTRACTED_HTML_CHARS);
        assert!(cut <= super::super::MAX_EXTRACTED_HTML_CHARS);
        assert!(text.is_char_boundary(cut));
    }
}
