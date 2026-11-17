//! 3-tier `OpenCode`-style text matching and dominant line-ending detection.

/// Returns `true` when `content` predominantly uses `\r\n` (CRLF) line endings,
/// preventing a single stray `\r\n` literal inside an LF file from flipping edits to CRLF.
#[must_use]
pub(super) fn uses_crlf_line_endings(content: &str) -> bool {
    let bytes = content.as_bytes();
    let mut lf_total = 0usize;
    let mut crlf_total = 0usize;
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            lf_total += 1;
            if idx > 0 && bytes[idx - 1] == b'\r' {
                crlf_total += 1;
            }
        }
    }
    lf_total > 0 && crlf_total * 2 >= lf_total
}

/// Maps typographic quotes, dashes, and non-breaking spaces to ASCII equivalents
/// without heap allocation.
#[inline]
const fn normalize_char(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        other => other,
    }
}

/// Finds non-overlapping byte ranges `(start, end)` in `content` matching `search`
/// using a 3-tier fallback strategy:
/// 1. Exact byte match.
/// 2. Line-by-line match ignoring trailing horizontal whitespace on each line.
/// 3. Line-by-line match ignoring both trailing whitespace and typographic quote/dash/space variants.
#[must_use]
pub(super) fn find_match_ranges(content: &str, search: &str) -> Vec<(usize, usize)> {
    if search.is_empty() {
        return Vec::new();
    }

    let exact: Vec<(usize, usize)> = content
        .match_indices(search)
        .map(|(start, m)| (start, start + m.len()))
        .collect();
    if !exact.is_empty() {
        return exact;
    }

    let trailing = find_line_occurrences(content, search, false);
    if !trailing.is_empty() {
        return trailing;
    }

    find_line_occurrences(content, search, true)
}

fn find_line_occurrences(
    content: &str,
    search: &str,
    normalize_typography: bool,
) -> Vec<(usize, usize)> {
    struct LineSpan<'a> {
        start: usize,
        end_with_newline: usize,
        content_end: usize,
        trimmed_text: &'a str,
        has_newline: bool,
    }

    let trailing_newline = search.ends_with('\n');
    let search_trimmed = if trailing_newline {
        search
            .strip_suffix("\r\n")
            .or_else(|| search.strip_suffix('\n'))
            .unwrap_or(search)
    } else {
        search
    };
    let expected: Vec<&str> = search_trimmed
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).trim_end_matches([' ', '\t']))
        .collect();
    if expected.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut offset = 0;
    for raw_line in content.split_inclusive('\n') {
        let start = offset;
        let end_with_newline = offset + raw_line.len();
        offset = end_with_newline;
        let without_lf = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let text = without_lf.strip_suffix('\r').unwrap_or(without_lf);
        let content_end = start + text.len();
        lines.push(LineSpan {
            start,
            end_with_newline,
            content_end,
            trimmed_text: text.trim_end_matches([' ', '\t']),
            has_newline: raw_line.ends_with('\n'),
        });
    }
    if offset < content.len() {
        let rem = &content[offset..];
        let text = rem.strip_suffix('\r').unwrap_or(rem);
        lines.push(LineSpan {
            start: offset,
            end_with_newline: content.len(),
            content_end: offset + text.len(),
            trimmed_text: text.trim_end_matches([' ', '\t']),
            has_newline: false,
        });
    }

    let mut matches: Vec<(usize, usize)> = Vec::new();
    if lines.len() < expected.len() {
        return matches;
    }

    for i in 0..=(lines.len() - expected.len()) {
        let window = &lines[i..i + expected.len()];
        let all_eq = window.iter().zip(expected.iter()).all(|(actual, exp)| {
            if normalize_typography {
                actual
                    .trimmed_text
                    .chars()
                    .map(normalize_char)
                    .eq(exp.chars().map(normalize_char))
            } else {
                actual.trimmed_text == *exp
            }
        });
        if !all_eq {
            continue;
        }
        let last = &window[window.len() - 1];
        if trailing_newline && !last.has_newline {
            continue;
        }
        let start = window[0].start;
        let end = if trailing_newline {
            last.end_with_newline
        } else {
            last.content_end
        };
        if matches.last().is_none_or(|&(_, prev_end)| start >= prev_end) {
            matches.push((start, end));
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominant_line_ending_ignores_single_stray_crlf_in_lf_file() {
        let lf_with_stray = "line1\nline2\nlet s = \"\\r\\n\";\r\nline4\nline5\n";
        assert!(!uses_crlf_line_endings(lf_with_stray));

        let crlf_file = "line1\r\nline2\r\nline3\r\n";
        assert!(uses_crlf_line_endings(crlf_file));
    }

    #[test]
    fn tier_3_typography_normalization_matches_without_allocating() {
        let content = "let quote = \u{2018}single\u{2019};\nlet nbsp =\u{00A0}1;\n";
        let ranges = find_match_ranges(content, "let quote = 'single';\nlet nbsp = 1;");
        assert_eq!(ranges.len(), 1);
    }
}
