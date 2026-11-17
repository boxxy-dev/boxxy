//! Pure, headless trigger matching and UTF-8 completion text splicing.

/// Converts a Unicode scalar (`char`) offset into a valid UTF-8 byte index in `text`,
/// clamping offsets past the end of `text` to `text.len()`.
#[must_use]
pub(super) fn char_to_byte_idx(text: &str, char_pos: usize) -> usize {
    if char_pos == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_pos)
        .map_or(text.len(), |(idx, _)| idx)
}

/// Candidate trigger match ordered by proximity to the cursor and trigger specificity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TriggerCandidate<'a> {
    pub(super) provider_idx: usize,
    pub(super) start_byte_idx: usize,
    pub(super) query: &'a str,
}

/// Scans backwards from `cursor_char_pos` to find all valid `(provider_idx, trigger)` candidates,
/// sorted with the trigger ending closest to the cursor first (breaking ties by longer trigger).
#[must_use]
pub(super) fn find_trigger_candidates<'a, I>(
    text: &'a str,
    cursor_char_pos: usize,
    triggers: I,
) -> Vec<TriggerCandidate<'a>>
where
    I: IntoIterator<Item = (usize, &'a str)>,
{
    if cursor_char_pos == 0 || text.is_empty() {
        return Vec::new();
    }

    let byte_cursor = char_to_byte_idx(text, cursor_char_pos);
    let text_before = &text[..byte_cursor];
    let mut candidates = Vec::new();

    for (provider_idx, trigger) in triggers {
        if trigger.is_empty() {
            continue;
        }
        let Some(idx) = text_before.rfind(trigger) else {
            continue;
        };

        // Slash commands ("/", "/resume ", "/debate ", etc.) are only valid at the start of input.
        if trigger.starts_with('/') && idx != 0 {
            continue;
        }

        // Non-start triggers must follow ASCII whitespace (' ', '\t', '\n', '\r').
        let valid_boundary = idx == 0
            || matches!(
                text_before.as_bytes().get(idx - 1),
                Some(b' ' | b'\t' | b'\n' | b'\r')
            );
        if !valid_boundary {
            continue;
        }

        let query = &text_before[idx + trigger.len()..];

        // Queries never span across line breaks; space-terminated command triggers (e.g. "/resume ")
        // allow multi-word queries on the same line, while token triggers ("@", ":") stop at whitespace.
        if query.contains(['\n', '\r']) {
            continue;
        }
        let allow_spaces = trigger.ends_with(' ');
        if !allow_spaces && query.contains([' ', '\t']) {
            continue;
        }

        candidates.push((provider_idx, idx, trigger.len(), query));
    }

    candidates.sort_by(|a, b| {
        let end_a = a.1 + a.2;
        let end_b = b.1 + b.2;
        end_b.cmp(&end_a).then_with(|| b.2.cmp(&a.2))
    });

    candidates
        .into_iter()
        .map(
            |(provider_idx, start_byte_idx, _, query)| TriggerCandidate {
                provider_idx,
                start_byte_idx,
                query,
            },
        )
        .collect()
}

/// Result of splicing a selected completion into an input buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletionEdit {
    pub(super) new_text: String,
    pub(super) new_cursor_char_pos: usize,
    pub(super) should_retrigger: bool,
}

/// Computes the spliced buffer text and new cursor character offset for `replacement`.
///
/// Returns `None` if `start_byte_idx` is no longer a valid UTF-8 boundary at or before the
/// current cursor (for example, if the cursor moved left of the trigger before activation).
#[must_use]
pub(super) fn compute_completion_edit(
    text: &str,
    cursor_char_pos: usize,
    start_byte_idx: usize,
    replacement: &str,
) -> Option<CompletionEdit> {
    let byte_cursor_pos = char_to_byte_idx(text, cursor_char_pos);
    if start_byte_idx > byte_cursor_pos
        || byte_cursor_pos > text.len()
        || !text.is_char_boundary(start_byte_idx)
    {
        return None;
    }

    let prefix = &text[..start_byte_idx];
    let tail = &text[byte_cursor_pos..];

    let is_command = replacement.starts_with('/');
    let is_dir_drill = replacement.starts_with('@') && replacement.ends_with('/');
    let already_spaced = replacement.ends_with(' ');
    let tail_starts_with_space = tail.starts_with(' ');
    let tail_starts_with_ws = tail.starts_with([' ', '\t', '\n', '\r']);
    let add_space = !is_command && !is_dir_drill && !already_spaced && !tail_starts_with_ws;

    let mut new_text = String::with_capacity(
        prefix.len() + replacement.len() + usize::from(add_space) + tail.len(),
    );
    new_text.push_str(prefix);
    new_text.push_str(replacement);
    if add_space {
        new_text.push(' ');
    }
    new_text.push_str(tail);

    let start_char_pos = prefix.chars().count();
    let replacement_char_len = replacement.chars().count();
    let advance_past_space =
        usize::from(add_space || (tail_starts_with_space && !is_command && !is_dir_drill));
    let new_cursor_char_pos = start_char_pos + replacement_char_len + advance_past_space;

    // Only keep the completion popup active when intentionally drilling into a secondary picker
    // (such as `@crates/window/` or `/resume `).
    let should_retrigger = is_dir_drill || already_spaced;

    Some(CompletionEdit {
        new_text,
        new_cursor_char_pos,
        should_retrigger,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_priority_and_boundaries() {
        let providers = [(0, "/resume "), (1, "/"), (2, "@"), (3, ":")];

        // "/resume " beats "/" at index 0 because its trigger end (8) > (1)
        let cands = find_trigger_candidates("/resume proj", 12, providers);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].provider_idx, 0);
        assert_eq!(cands[0].query, "proj");

        // Slash commands ignored when not at index 0
        let cands = find_trigger_candidates("hello /resume", 13, providers);
        assert_eq!(cands, []);

        // Mid-word '@' (email) ignored; tab/newline-preceded '@' accepted
        assert_eq!(
            find_trigger_candidates("user@example.com", 16, providers),
            []
        );
        let cands = find_trigger_candidates("check\t@src/lib.rs", 17, providers);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].provider_idx, 2);
        assert_eq!(cands[0].query, "src/lib.rs");

        // Nearest valid trigger before cursor wins when multiple triggers exist
        let text = "/resume old @src/mod";
        let cands = find_trigger_candidates(text, text.chars().count(), providers);
        assert_eq!(cands[0].provider_idx, 2);
        assert_eq!(cands[0].query, "src/mod");
    }

    #[test]
    fn test_utf8_multibyte_cursor_and_splicing() {
        let text = "こんにちは @ファ";
        let cursor = text.chars().count();
        let cands = find_trigger_candidates(text, cursor, [(0, "@")]);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].query, "ファ");

        let edit =
            compute_completion_edit(text, cursor, cands[0].start_byte_idx, "@ファイル.rs").unwrap();
        assert_eq!(edit.new_text, "こんにちは @ファイル.rs ");
        assert_eq!(
            edit.new_cursor_char_pos,
            "こんにちは @ファイル.rs ".chars().count()
        );
        assert!(!edit.should_retrigger);
    }

    #[test]
    fn test_directory_drill_and_existing_trailing_space() {
        // Directory drill neither appends a space nor suppresses retrigger
        let drill = compute_completion_edit("@cr", 3, 0, "@crates/").unwrap();
        assert_eq!(drill.new_text, "@crates/");
        assert_eq!(drill.new_cursor_char_pos, 8);
        assert!(drill.should_retrigger);

        // Existing space in tail is reused without inserting a duplicate space
        let mid = compute_completion_edit("check @Car please", 10, 6, "@Cargo.toml").unwrap();
        assert_eq!(mid.new_text, "check @Cargo.toml please");
        assert_eq!(
            mid.new_cursor_char_pos,
            "check @Cargo.toml ".chars().count()
        );
        assert!(!mid.should_retrigger);
    }

    #[test]
    fn test_stale_cursor_before_start_idx_returns_none_safely() {
        // Simulates cursor moving left of `start_byte_idx` before completion activation
        assert!(compute_completion_edit("hello @world", 2, 6, "@workspace").is_none());
        // Non-char-boundary `start_byte_idx` inside multibyte char returns `None` instead of panicking
        assert!(compute_completion_edit("🦀 @file", 7, 1, "@fixed").is_none());
    }
}
