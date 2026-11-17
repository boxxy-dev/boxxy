//! Internal zero-allocation string comparison and Unicode-safe truncation helpers.
#![expect(
    clippy::redundant_pub_crate,
    reason = "workspace lints deny unreachable_pub, requiring pub(crate) inside private root modules"
)]

use std::borrow::Cow;
use std::cmp::Ordering;

/// Zero-allocation case-insensitive equality check (ASCII fast-path + full Unicode fallback).
#[must_use]
pub(crate) fn eq_case_insensitive(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .eq(b.chars().flat_map(char::to_lowercase))
    }
}

/// Zero-allocation case-insensitive ordering (ASCII fast-path + full Unicode fallback).
#[must_use]
pub(crate) fn cmp_case_insensitive(a: &str, b: &str) -> Ordering {
    if a.is_ascii() && b.is_ascii() {
        a.bytes()
            .map(|c| c.to_ascii_lowercase())
            .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .cmp(b.chars().flat_map(char::to_lowercase))
    }
}

/// Truncates `text` to at most `max_chars` Unicode scalar values, appending `…` only when clipped.
/// Returns `Cow::Borrowed(text)` with zero allocation when `text` already fits within `max_chars`.
#[must_use]
pub(crate) fn truncate_chars_cow(text: &str, max_chars: usize) -> Cow<'_, str> {
    if text.len() <= max_chars {
        return Cow::Borrowed(text);
    }
    if text.is_ascii() {
        return Cow::Owned(format!("{}…", &text[..max_chars]));
    }
    text.char_indices()
        .nth(max_chars)
        .map_or(Cow::Borrowed(text), |(byte_idx, _)| {
            Cow::Owned(format!("{}…", &text[..byte_idx]))
        })
}

/// Owned convenience wrapper around [`truncate_chars_cow`].
#[must_use]
pub(crate) fn truncate_chars_with_ellipsis(text: &str, max_chars: usize) -> String {
    truncate_chars_cow(text, max_chars).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive_helpers_handle_ascii_and_unicode() {
        assert!(eq_case_insensitive("Levi-Kujo", "levi-kujo"));
        assert!(eq_case_insensitive("Kürö", "kürö"));
        assert!(!eq_case_insensitive("Kürö", "kuro"));

        assert_eq!(cmp_case_insensitive("alpha", "Beta"), Ordering::Less);
        assert_eq!(cmp_case_insensitive("Kürö", "kürö"), Ordering::Equal);
    }

    #[test]
    fn truncate_chars_borrows_short_strings_and_clips_on_scalar_boundaries() {
        let short = "hello world";
        assert!(matches!(truncate_chars_cow(short, 20), Cow::Borrowed(_)));

        let ascii_clipped = truncate_chars_with_ellipsis("abcdefghij", 5);
        assert_eq!(ascii_clipped, "abcde…");

        // Multi-byte UTF-8 where byte length > max_chars, scalar count <= max_chars -> Borrowed
        let box_chars = "─".repeat(8);
        assert!(box_chars.len() > 10);
        assert!(matches!(
            truncate_chars_cow(&box_chars, 10),
            Cow::Borrowed(_)
        ));

        // Multi-byte UTF-8 exceeding max_chars -> clipped cleanly on char boundary
        let over = "─".repeat(12);
        let clipped = truncate_chars_with_ellipsis(&over, 10);
        assert_eq!(clipped.chars().count(), 11);
        assert!(clipped.ends_with('…'));
    }
}
