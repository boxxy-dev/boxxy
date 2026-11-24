use boxxy_claw_protocol::ImageAttachmentMeta;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Write as _;

/// Maximum raw file size accepted per attached image or text/code file (10 MiB,
/// matching `boxxy_claw::engine::turn::file_refs::MAX_IMAGE_BYTES`).
pub const MAX_ATTACHMENT_FILE_BYTES: usize = 10 * 1024 * 1024;
/// Maximum combined raw payload budget across all attachments in one message (20 MiB,
/// matching `OpenCode` desktop/CLI `MAX_ATTACHMENT_BYTES` and staying well inside D-Bus frame limits).
pub const MAX_TOTAL_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
/// Maximum number of attachments permitted in a single message draft.
pub const MAX_ATTACHMENTS_PER_MESSAGE: usize = 16;
/// Maximum UTF-8 text/code slice retained inline per attachment/snippet (512 KiB).
pub const MAX_TEXT_SNIPPET_BYTES: usize = 512 * 1024;
/// Single-line pastes up to this character count insert directly as editable text;
/// multi-line pastes (`>= 2` lines) or longer single-line blobs collapse into `[Pasted ~N lines]`.
pub const PASTE_INLINE_MAX_CHARS: usize = 100;
/// Maximum filename characters displayed inside an inline `[Image N · name]` / `[File N · name]` tag.
const MAX_INLINE_FILENAME_CHARS: usize = 32;

pub const TEXT_SNIPPET_LABEL: &str = "Text Snippet";
pub const PASTED_IMAGE_LABEL: &str = "Pasted Image";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Image,
    File,
    Snippet,
}

impl TokenKind {
    #[must_use]
    pub fn from_attachment(label: &str, is_image: bool) -> Self {
        if is_image {
            Self::Image
        } else if label == TEXT_SNIPPET_LABEL {
            Self::Snippet
        } else {
            Self::File
        }
    }

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::File => "File",
            Self::Snippet => "Pasted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicKeyOp {
    Backspace,
    Delete,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicKeyAction {
    DeleteSpan(CharSpan),
    MoveCursor(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    /// Inline editor token such as `"[Image 1 · photo.jpg]"`, `"[Image 2]"`,
    /// `"[File 1 · script.py]"`, or `"[Pasted ~351 lines]"`.
    /// Defaults to empty when deserializing pre-token `msgbar_history` records.
    #[serde(default)]
    pub token: String,
    pub label: String,
    pub content: String,
    pub is_image: bool,
}

impl Attachment {
    /// Approximate decoded byte footprint used to enforce `MAX_TOTAL_ATTACHMENT_BYTES`
    /// without allocating decoded buffers.
    #[must_use]
    pub const fn estimated_raw_bytes(&self) -> usize {
        if self.is_image {
            (self.content.len() / 4) * 3
        } else {
            self.content.len()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachmentPayload {
    pub prompt_id: Option<uuid::Uuid>,
    pub text: String,
    pub images: Vec<String>,
    pub image_meta: Vec<ImageAttachmentMeta>,
}

/// Normalizes CRLF/CR line endings to `\n` and drops a single trailing line break on
/// one-line terminal copies (so copying `"cargo test\n"` pastes `"cargo test"` inline).
#[must_use]
pub fn normalize_pasted_text(raw: &str) -> String {
    let lf = if raw.contains('\r') {
        raw.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        raw.to_string()
    };
    if let Some(without_trailing) = lf.strip_suffix('\n')
        && !without_trailing.contains('\n')
    {
        return without_trailing.to_string();
    }
    lf
}

/// Returns `true` when normalized pasted text should collapse into an inline `[Pasted ~N lines]`
/// chip rather than expanding raw text into the single-line input box.
#[must_use]
pub fn should_collapse_pasted_text(normalized: &str) -> bool {
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.contains('\n') || trimmed.chars().count() > PASTE_INLINE_MAX_CHARS
}

/// Sanitizes and middle-ellipsizes a filename for safe embedding inside `[Kind N · <name>]`
/// (replacing `[`/`]` with `(`/`)` so nested brackets never split token boundaries).
#[must_use]
pub fn compact_filename_for_token(label: &str) -> String {
    let clean: String = label
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '[' => '(',
            ']' => ')',
            _ => c,
        })
        .collect();
    let trimmed = clean.trim();
    let total_chars = trimmed.chars().count();
    if total_chars <= MAX_INLINE_FILENAME_CHARS {
        return trimmed.to_string();
    }
    let head_len = 16;
    let tail_len = 12;
    let head: String = trimmed.chars().take(head_len).collect();
    let tail: String = trimmed
        .chars()
        .skip(total_chars.saturating_sub(tail_len))
        .collect();
    format!("{head}…{tail}")
}

/// Parses the positive integer `N` from `"[Image N]"` / `"[Image N · name]"` / `"[File N · name]"`.
#[must_use]
pub fn parse_token_number(token: &str, kind: TokenKind) -> Option<u32> {
    let inner = token.strip_prefix('[')?.strip_suffix(']')?;
    let rest = inner.strip_prefix(kind.word())?.strip_prefix(' ')?;
    let digits_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits_len == 0 {
        return None;
    }
    let after = &rest[digits_len..];
    if !after.is_empty()
        && !after.starts_with(" · ")
        && !after.starts_with(": ")
    {
        return None;
    }
    rest[..digits_len].parse::<u32>().ok().filter(|&n| n >= 1)
}

fn max_token_number_in_text(text: &str, kind: TokenKind) -> u32 {
    let prefix = format!("[{} ", kind.word());
    let mut max_n = 0_u32;
    let mut from = 0;
    while let Some(rel) = text[from..].find(&prefix) {
        let num_start = from + rel + prefix.len();
        let bytes = text.as_bytes();
        let mut idx = num_start;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx > num_start && idx < bytes.len() {
            let tail = &text[idx..];
            let has_closing_on_line = tail
                .find(']')
                .is_some_and(|close| !tail[..close].contains('\n'));
            let valid_delim = bytes[idx] == b']'
                || tail.starts_with(" · ")
                || tail.starts_with(": ");
            if valid_delim
                && has_closing_on_line
                && let Ok(n) = text[num_start..idx].parse::<u32>()
            {
                max_n = max_n.max(n);
            }
        }
        from = idx.max(num_start + 1).min(text.len());
    }
    max_n
}

fn token_exists(
    candidate: &str,
    active: &[Attachment],
    stashed: &[Attachment],
    buffer_text: &str,
) -> bool {
    buffer_text.contains(candidate)
        || active.iter().any(|a| a.token == candidate)
        || stashed.iter().any(|a| a.token == candidate)
}

/// Allocates a descriptive inline token:
/// - Named image: `"[Image 1 · photo.jpg]"`
/// - Clipboard screenshot: `"[Image 1]"`
/// - Text/code file: `"[File 1 · script.py]"`
/// - Collapsed text paste: `"[Pasted ~351 lines]"` (or `"[Pasted ~1 line]"`, disambiguated with `#2` on line-count collision)
#[must_use]
pub fn allocate_next_token(
    kind: TokenKind,
    label: &str,
    content: &str,
    active: &[Attachment],
    stashed: &[Attachment],
    buffer_text: &str,
) -> String {
    if kind == TokenKind::Snippet {
        let lines = content.trim().bytes().filter(|&b| b == b'\n').count() + 1;
        let line_summary = if lines == 1 {
            "~1 line".to_string()
        } else {
            format!("~{lines} lines")
        };
        let primary = format!("[Pasted {line_summary}]");
        if !token_exists(&primary, active, stashed, buffer_text) {
            return primary;
        }
        let mut seq = 2_u32;
        loop {
            let candidate = format!("[Pasted #{seq} {line_summary}]");
            if !token_exists(&candidate, active, stashed, buffer_text) {
                return candidate;
            }
            seq = seq.saturating_add(1);
        }
    }

    let mut max_n = max_token_number_in_text(buffer_text, kind);
    for att in active.iter().chain(stashed.iter()) {
        if let Some(n) = parse_token_number(&att.token, kind) {
            max_n = max_n.max(n);
        }
    }
    let next_n = max_n.saturating_add(1);
    let include_name = match kind {
        TokenKind::Image => !label.trim().is_empty() && label != PASTED_IMAGE_LABEL,
        TokenKind::File => !label.trim().is_empty() && label != TEXT_SNIPPET_LABEL,
        TokenKind::Snippet => false,
    };
    if include_name {
        let short_name = compact_filename_for_token(label);
        if !short_name.is_empty() {
            return format!("[{} {next_n} · {short_name}]", kind.word());
        }
    }
    format!("[{} {next_n}]", kind.word())
}

/// Formats the header written above a non-image attachment inside `--- ATTACHMENTS ---`.
#[must_use]
pub fn text_attachment_header_label(att: &Attachment) -> String {
    if att.token.is_empty() {
        return att.label.clone();
    }
    if att.label.is_empty()
        || att.label == TEXT_SNIPPET_LABEL
        || att.token.contains(&att.label)
    {
        return att.token.clone();
    }
    // Full filename fallback when `att.token` middle-ellipsized a very long filename.
    format!("{} · {}", att.token, att.label)
}

/// Builds a hover tooltip for the inline token at `char_offset` in `text`, if any.
#[must_use]
pub fn hover_tooltip_for_offset(
    text: &str,
    active: &[Attachment],
    char_offset: usize,
) -> Option<String> {
    if active.is_empty() || text.is_empty() {
        return None;
    }
    let tokens: Vec<String> = active
        .iter()
        .filter(|a| !a.token.is_empty())
        .map(|a| a.token.clone())
        .collect();
    let spans = find_active_token_char_spans(text, &tokens);
    let hit = spans
        .into_iter()
        .find(|span| char_offset >= span.start && char_offset < span.end)?;
    let matched_token: String = text
        .chars()
        .skip(hit.start)
        .take(hit.end - hit.start)
        .collect();
    let att = active.iter().find(|a| a.token == matched_token)?;
    if att.is_image {
        if att.label.is_empty() || att.label == PASTED_IMAGE_LABEL {
            Some(format!("{} (Backspace to remove)", att.token))
        } else {
            Some(format!("{} — {} (Backspace to remove)", att.token, att.label))
        }
    } else {
        Some(build_text_preview_tooltip(&att.token, &att.content))
    }
}

/// Finds non-overlapping Unicode scalar (`char`) spans of `active_tokens` in `text`.
///
/// Longer tokens claim occurrences first. Returned spans are sorted by `start`.
#[must_use]
pub fn find_active_token_char_spans(text: &str, active_tokens: &[String]) -> Vec<CharSpan> {
    if text.is_empty() || active_tokens.is_empty() || !text.contains('[') {
        return Vec::new();
    }

    let mut unique_tokens: Vec<&str> = active_tokens
        .iter()
        .map(String::as_str)
        .filter(|t| !t.is_empty())
        .collect();
    unique_tokens.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    unique_tokens.dedup();
    if unique_tokens.is_empty() {
        return Vec::new();
    }

    let mut byte_spans: Vec<(usize, usize)> = Vec::new();
    for tok in unique_tokens {
        let mut from = 0;
        while let Some(rel) = text[from..].find(tok) {
            let b_start = from + rel;
            let b_end = b_start + tok.len();
            let overlaps = byte_spans
                .iter()
                .any(|&(s, e)| b_start < e && b_end > s);
            if !overlaps {
                byte_spans.push((b_start, b_end));
            }
            from = b_end;
        }
    }
    if byte_spans.is_empty() {
        return Vec::new();
    }

    byte_spans.sort_unstable_by_key(|&(s, _)| s);

    // Convert sorted UTF-8 byte offsets into GTK-compatible Unicode scalar (`char`) offsets in one pass.
    let mut char_spans = Vec::with_capacity(byte_spans.len());
    let mut prev_byte = 0;
    let mut cum_chars = 0;
    for (b_start, b_end) in byte_spans {
        cum_chars += text[prev_byte..b_start].chars().count();
        let start_char = cum_chars;
        cum_chars += text[b_start..b_end].chars().count();
        let end_char = cum_chars;
        prev_byte = b_end;
        char_spans.push(CharSpan {
            start: start_char,
            end: end_char,
        });
    }
    char_spans
}

/// Evaluates whether a single-cursor `Backspace`, `Delete`, `Left`, or `Right` key press
/// interacts with an inline attachment token so the token behaves as an atomic chip.
#[must_use]
pub fn evaluate_atomic_token_key(
    spans: &[CharSpan],
    cursor_char: usize,
    op: AtomicKeyOp,
) -> Option<AtomicKeyAction> {
    for &span in spans {
        match op {
            AtomicKeyOp::Backspace => {
                if cursor_char > span.start && cursor_char <= span.end {
                    return Some(AtomicKeyAction::DeleteSpan(span));
                }
            }
            AtomicKeyOp::Delete => {
                if cursor_char >= span.start && cursor_char < span.end {
                    return Some(AtomicKeyAction::DeleteSpan(span));
                }
            }
            AtomicKeyOp::Left => {
                if cursor_char > span.start && cursor_char <= span.end {
                    return Some(AtomicKeyAction::MoveCursor(span.start));
                }
            }
            AtomicKeyOp::Right => {
                if cursor_char >= span.start && cursor_char < span.end {
                    return Some(AtomicKeyAction::MoveCursor(span.end));
                }
            }
        }
    }
    None
}

/// If a mouse click lands strictly inside an inline token (`span.start < cursor < span.end`),
/// returns the nearer boundary so the caret never sits mid-token.
#[must_use]
pub fn snap_cursor_out_of_token(spans: &[CharSpan], cursor_char: usize) -> Option<usize> {
    for &span in spans {
        if cursor_char > span.start && cursor_char < span.end {
            let dist_left = cursor_char - span.start;
            let dist_right = span.end - cursor_char;
            return Some(if dist_left <= dist_right {
                span.start
            } else {
                span.end
            });
        }
    }
    None
}

/// Reconciles `active` and `stashed` attachments against the current `buffer_text`:
/// - Any inline-tracked attachment whose `token` is no longer in `buffer_text` moves to `stashed`
///   (so `Backspace` removes it while `Ctrl+Z` Undo or `Ctrl+X`/`Ctrl+V` Cut-Paste restores it).
/// - Any `stashed` attachment whose `token` reappears in `buffer_text` moves back to `active`.
///
/// Returns `true` if `active` changed.
pub fn sync_attachments_with_buffer_text<S: std::hash::BuildHasher>(
    active: &mut Vec<Attachment>,
    stashed: &mut Vec<Attachment>,
    tracked_inline_ids: &HashSet<String, S>,
    buffer_text: &str,
) -> bool {
    let mut changed = false;

    let mut i = 0;
    while i < active.len() {
        let should_stash = !active[i].token.is_empty()
            && tracked_inline_ids.contains(&active[i].id)
            && !buffer_text.contains(&active[i].token);
        if should_stash {
            let removed = active.remove(i);
            stashed.push(removed);
            changed = true;
        } else {
            i += 1;
        }
    }

    let mut j = 0;
    while j < stashed.len() {
        let should_restore = !stashed[j].token.is_empty() && buffer_text.contains(&stashed[j].token);
        if should_restore {
            let restored = stashed.remove(j);
            active.push(restored);
            changed = true;
        } else {
            j += 1;
        }
    }

    changed
}

/// Builds the outgoing `(text, images, image_meta)` payload from `base_text` and `active` attachments.
///
/// Attachments are ordered by their first inline token appearance in `base_text` (followed by any
/// mentionless legacy attachments in list order), ensuring both User-bubble thumbnails and fallback
/// sections match the exact reading order of the user's prompt.
#[must_use]
pub fn build_payload_from_attachments(base_text: &str, active: &[Attachment]) -> AttachmentPayload {
    let mut text = base_text.to_string();
    if active.is_empty() {
        return AttachmentPayload {
            prompt_id: None,
            text,
            images: Vec::new(),
            image_meta: Vec::new(),
        };
    }

    let mut ordered: Vec<(usize, usize, &Attachment)> = active
        .iter()
        .enumerate()
        .map(|(list_idx, att)| {
            let first_pos = if att.token.is_empty() {
                usize::MAX
            } else {
                base_text.find(&att.token).unwrap_or(usize::MAX)
            };
            (first_pos, list_idx, att)
        })
        .collect();
    ordered.sort_by_key(|&(first_pos, list_idx, _)| (first_pos, list_idx));

    let mut images = Vec::new();
    let mut image_meta = Vec::new();
    let mut text_attachments_present = false;

    for (_, _, att) in ordered {
        if att.is_image {
            images.push(att.content.clone());
            image_meta.push(ImageAttachmentMeta {
                token: att.token.clone(),
                label: att.label.clone(),
            });
        } else {
            if !text_attachments_present {
                text.push_str("\n\n--- ATTACHMENTS ---");
                text_attachments_present = true;
            }
            let fence = fence_for_content(&att.content);
            let header = text_attachment_header_label(att);
            let _ = write!(text, "\n\n**{header}**\n{fence}text\n{}\n{fence}", att.content);
        }
    }

    AttachmentPayload {
        prompt_id: None,
        text,
        images,
        image_meta,
    }
}

#[must_use]
pub fn build_text_preview_tooltip(label: &str, content: &str) -> String {
    const MAX_PREVIEW_LINES: usize = 8;
    const MAX_PREVIEW_CHARS: usize = 320;

    let mut lines = content.lines();
    let mut preview: String = lines
        .by_ref()
        .take(MAX_PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if preview.chars().count() > MAX_PREVIEW_CHARS {
        preview = preview.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
        preview.push('…');
    } else if lines.next().is_some() {
        preview.push_str("\n…");
    }

    if label.is_empty() || label == TEXT_SNIPPET_LABEL {
        preview
    } else if preview.is_empty() {
        label.to_string()
    } else {
        format!("{label}\n\n{preview}")
    }
}

/// Computes a Markdown code fence strictly longer than any contiguous backtick run in `content`
/// (minimum 3 backticks) so embedded code blocks never close the attachment block prematurely.
#[must_use]
pub fn fence_for_content(content: &str) -> String {
    let mut max_run = 0usize;
    let mut cur_run = 0usize;
    for b in content.bytes() {
        if b == b'`' {
            cur_run += 1;
            if cur_run > max_run {
                max_run = cur_run;
            }
        } else {
            cur_run = 0;
        }
    }
    "`".repeat(max_run.saturating_add(1).max(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_att(id: &str, token: &str, label: &str, content: &str, is_image: bool) -> Attachment {
        Attachment {
            id: id.to_string(),
            token: token.to_string(),
            label: label.to_string(),
            content: content.to_string(),
            is_image,
        }
    }

    #[test]
    fn allocates_named_file_tokens_clipboard_image_tokens_and_pasted_line_tags() {
        let mut active = Vec::new();
        let stashed = Vec::new();

        let t1 = allocate_next_token(
            TokenKind::Image,
            "chart_a.jpg",
            "AAA=",
            &active,
            &stashed,
            "",
        );
        assert_eq!(t1, "[Image 1 · chart_a.jpg]");
        active.push(sample_att("1", &t1, "chart_a.jpg", "AAA=", true));

        let t2 = allocate_next_token(
            TokenKind::Image,
            PASTED_IMAGE_LABEL,
            "BBB=",
            &active,
            &stashed,
            &t1,
        );
        assert_eq!(t2, "[Image 2]");
        active.push(sample_att("2", &t2, PASTED_IMAGE_LABEL, "BBB=", true));

        let f1 = allocate_next_token(
            TokenKind::File,
            "script.py",
            "print('ok')",
            &active,
            &stashed,
            "",
        );
        assert_eq!(f1, "[File 1 · script.py]");

        let s1 = allocate_next_token(
            TokenKind::Snippet,
            TEXT_SNIPPET_LABEL,
            "line1\nline2\nline3\nline4",
            &active,
            &stashed,
            "",
        );
        assert_eq!(s1, "[Pasted ~4 lines]");
        active.push(sample_att("3", &s1, TEXT_SNIPPET_LABEL, "line1\nline2\nline3\nline4", false));

        // Second distinct 4-line paste disambiguates with #2 so tokens never collide
        let s2 = allocate_next_token(
            TokenKind::Snippet,
            TEXT_SNIPPET_LABEL,
            "other1\nother2\nother3\nother4",
            &active,
            &stashed,
            &s1,
        );
        assert_eq!(s2, "[Pasted #2 ~4 lines]");
    }

    #[test]
    fn paste_normalization_keeps_short_single_line_inline_and_collapses_multiline_or_long_pastes() {
        // Single-line terminal copy with trailing newline stays inline plain text
        let cmd = normalize_pasted_text("cargo test --workspace\r\n");
        assert_eq!(cmd, "cargo test --workspace");
        assert!(!should_collapse_pasted_text(&cmd));

        // Multi-line paste collapses even if it mentions `[Image 1]` in prose!
        let doc = normalize_pasted_text(
            "Referencing [Image 1] multiple times in one prompt\nemits the block once.",
        );
        assert!(should_collapse_pasted_text(&doc));

        // Long single-line paste (> 100 chars) collapses into `[Pasted ~1 line]`
        let long_single = "x".repeat(120);
        assert!(should_collapse_pasted_text(&long_single));
        assert_eq!(
            allocate_next_token(
                TokenKind::Snippet,
                TEXT_SNIPPET_LABEL,
                &long_single,
                &[],
                &[],
                ""
            ),
            "[Pasted ~1 line]"
        );
    }

    #[test]
    fn atomic_key_and_click_snapping_handle_named_tokens_and_multibyte_spans() {
        let text = "🦀 [Image 1 · alpha.png][Image 2] ok";
        let tokens = vec![
            "[Image 1 · alpha.png]".to_string(),
            "[Image 2]".to_string(),
        ];
        let spans = find_active_token_char_spans(text, &tokens);
        assert_eq!(
            spans,
            vec![
                CharSpan { start: 2, end: 23 },
                CharSpan { start: 23, end: 32 }
            ]
        );

        assert_eq!(
            evaluate_atomic_token_key(&spans, 23, AtomicKeyOp::Backspace),
            Some(AtomicKeyAction::DeleteSpan(CharSpan { start: 2, end: 23 }))
        );
        assert_eq!(
            evaluate_atomic_token_key(&spans, 23, AtomicKeyOp::Delete),
            Some(AtomicKeyAction::DeleteSpan(CharSpan { start: 23, end: 32 }))
        );
        assert_eq!(
            evaluate_atomic_token_key(&spans, 32, AtomicKeyOp::Left),
            Some(AtomicKeyAction::MoveCursor(23))
        );
        assert_eq!(snap_cursor_out_of_token(&spans, 6), Some(2));
        assert_eq!(snap_cursor_out_of_token(&spans, 20), Some(23));
    }

    #[test]
    fn sync_stashes_deleted_tokens_and_restores_on_undo_or_paste() {
        let mut active = vec![
            sample_att("a1", "[Image 1 · first.png]", "first.png", "AAA=", true),
            sample_att("legacy", "[Image 2 · legacy.png]", "legacy.png", "BBB=", true),
        ];
        let mut stashed = Vec::new();
        let mut tracked = HashSet::new();
        tracked.insert("a1".to_string());

        assert!(sync_attachments_with_buffer_text(
            &mut active,
            &mut stashed,
            &tracked,
            "plain text without tokens"
        ));
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "legacy");
        assert_eq!(stashed.len(), 1);
        assert_eq!(stashed[0].id, "a1");

        assert!(sync_attachments_with_buffer_text(
            &mut active,
            &mut stashed,
            &tracked,
            "restored [Image 1 · first.png] via undo"
        ));
        assert_eq!(active.len(), 2);
        assert_eq!(stashed.len(), 0);
    }

    #[test]
    fn build_payload_orders_attachments_and_avoids_duplicate_filename_headers() {
        let active = vec![
            sample_att("1", "[Image 1 · alpha.jpg]", "alpha.jpg", "IMG_ALPHA", true),
            sample_att("2", "[Image 2]", PASTED_IMAGE_LABEL, "IMG_BETA", true),
            sample_att("3", "[File 1 · config.toml]", "config.toml", "port = 8080", false),
            sample_att("4", "[Pasted ~3 lines]", TEXT_SNIPPET_LABEL, "a\nb\nc", false),
        ];
        let payload = build_payload_from_attachments(
            "compare [Image 2] against [Image 1 · alpha.jpg] with [File 1 · config.toml] and [Pasted ~3 lines]",
            &active,
        );
        assert_eq!(payload.images, vec!["IMG_BETA", "IMG_ALPHA"]);
        assert!(payload.text.contains("**[File 1 · config.toml]**"));
        assert!(!payload.text.contains("config.toml · config.toml"));
        assert!(payload.text.contains("**[Pasted ~3 lines]**"));
    }

    #[test]
    fn legacy_attachment_deserializes_with_default_empty_token() {
        let json = r#"{"id":"u1","label":"photo.png","content":"iVBORw==","is_image":true}"#;
        let att: Attachment = serde_json::from_str(json).expect("legacy json");
        assert_eq!(att.token, "");
        assert_eq!(att.label, "photo.png");
    }
}
