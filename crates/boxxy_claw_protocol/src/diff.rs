use serde::{Deserialize, Serialize};
use similar::{Algorithm, ChangeTag, DiffOp, TextDiff};
use std::path::Path;
use std::time::Duration;

/// Maximum number of aligned diff rows stored/rendered per `FileDiff` to protect
/// D-Bus payloads, SQLite storage, and GTK4 layout passes on massive files.
pub const MAX_DIFF_RENDER_ROWS: usize = 250;

/// Maximum byte size of a text buffer before we fall back to line-only diffing
/// without sub-line inline token diffing.
const MAX_INLINE_DIFF_BYTES: usize = 256 * 1024;

/// Maximum byte size of a file to diff in memory (2 MiB).
const MAX_DIFF_FILE_BYTES: usize = 2 * 1024 * 1024;

/// Number of unchanged context lines shown before and after each change cluster.
const DEFAULT_CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffKind {
    /// Existing text file modified with contextual hunks.
    Modified,
    /// Brand-new text file; all lines are additions.
    Created,
    /// Non-text / binary file (NUL bytes detected or known binary format).
    Binary,
    /// Existing file where old and new content are identical.
    Unchanged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStats {
    pub added_lines: usize,
    pub removed_lines: usize,
    pub hunk_count: usize,
    pub old_total_lines: usize,
    pub new_total_lines: usize,
    /// True if total rendered rows exceeded `MAX_DIFF_RENDER_ROWS` and were capped.
    pub truncated: bool,
    pub omitted_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSpan {
    /// True when this sub-line token was specifically changed within a replaced block.
    pub emphasized: bool,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineKind {
    Context,
    Delete,
    Insert,
    /// Alignment filler slot on the shorter side of an asymmetric replacement block.
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffCell {
    pub line_no: Option<u32>,
    pub kind: DiffLineKind,
    pub spans: Vec<DiffSpan>,
}

impl DiffCell {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            line_no: None,
            kind: DiffLineKind::Empty,
            spans: Vec::new(),
        }
    }

    #[must_use]
    pub fn plain(line_no: Option<u32>, kind: DiffLineKind, text: impl Into<String>) -> Self {
        let s = text.into();
        let spans = if s.is_empty() {
            Vec::new()
        } else {
            vec![DiffSpan {
                emphasized: false,
                text: s,
            }]
        };
        Self {
            line_no,
            kind,
            spans,
        }
    }

    #[must_use]
    pub fn plain_text(&self) -> String {
        let cap: usize = self.spans.iter().map(|s| s.text.len()).sum();
        let mut out = String::with_capacity(cap);
        for span in &self.spans {
            out.push_str(&span.text);
        }
        out
    }
}

/// A single aligned row capable of driving both Two-Column Side-by-Side view
/// and Single-Column / Drawer view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPairRow {
    pub left: DiffCell,
    pub right: DiffCell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Standard unified hunk header, e.g. `"@@ -42,8 +42,10 @@"`.
    pub header: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub rows: Vec<DiffPairRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub language: String,
    pub kind: DiffKind,
    pub stats: DiffStats,
    pub hunks: Vec<DiffHunk>,
}

impl FileDiff {
    /// Computes a structured `FileDiff` between optional `old_content` (`None` if creating a new file)
    /// and `new_content`. Designed to run inside `tokio::task::spawn_blocking`.
    #[must_use]
    pub fn compute(path: &str, old_content: Option<&str>, new_content: &str) -> Self {
        let language = infer_language(path);

        // 1. Binary guard: check extension and probe first 8 KiB for NUL bytes.
        if is_binary_path(path)
            || is_binary_bytes(new_content.as_bytes())
            || old_content.is_some_and(|old| is_binary_bytes(old.as_bytes()))
        {
            return Self::binary(path);
        }

        // 2. Brand-new text file: display all lines as additions.
        let Some(old_raw) = old_content else {
            return Self::compute_created(path, language, new_content);
        };

        // 3. Existing text file: normalize CRLF so line-ending differences don't obscure code edits.
        let old_norm = normalize_newlines(old_raw);
        let new_norm = normalize_newlines(new_content);

        let old_total_lines = count_lines(&old_norm);
        let new_total_lines = count_lines(&new_norm);

        if old_norm == new_norm {
            return Self {
                path: path.to_string(),
                language,
                kind: DiffKind::Unchanged,
                stats: DiffStats {
                    added_lines: 0,
                    removed_lines: 0,
                    hunk_count: 0,
                    old_total_lines,
                    new_total_lines,
                    truncated: false,
                    omitted_lines: 0,
                },
                hunks: Vec::new(),
            };
        }

        // Guard against extreme multi-megabyte files.
        if old_norm.len() > MAX_DIFF_FILE_BYTES || new_norm.len() > MAX_DIFF_FILE_BYTES {
            return Self {
                path: path.to_string(),
                language,
                kind: DiffKind::Modified,
                stats: DiffStats {
                    added_lines: new_total_lines,
                    removed_lines: old_total_lines,
                    hunk_count: 0,
                    old_total_lines,
                    new_total_lines,
                    truncated: true,
                    omitted_lines: new_total_lines.max(old_total_lines),
                },
                hunks: Vec::new(),
            };
        }

        let diff = TextDiff::configure()
            .algorithm(Algorithm::Patience)
            .timeout(Duration::from_millis(250))
            .diff_lines(old_norm.as_ref(), new_norm.as_ref());

        let mut added_lines = 0usize;
        let mut removed_lines = 0usize;
        for op in diff.ops() {
            match *op {
                DiffOp::Equal { .. } => {}
                DiffOp::Delete { old_len, .. } => removed_lines += old_len,
                DiffOp::Insert { new_len, .. } => added_lines += new_len,
                DiffOp::Replace {
                    old_len, new_len, ..
                } => {
                    removed_lines += old_len;
                    added_lines += new_len;
                }
            }
        }

        let groups = diff.grouped_ops(DEFAULT_CONTEXT_LINES);
        let hunk_count = groups.len();
        let enable_inline =
            old_norm.len() <= MAX_INLINE_DIFF_BYTES && new_norm.len() <= MAX_INLINE_DIFF_BYTES;

        let mut hunks = Vec::with_capacity(groups.len());
        let mut rendered_rows = 0usize;
        let mut truncated = false;
        let mut omitted_lines = 0usize;

        for group in groups {
            let Some(first) = group.first() else {
                continue;
            };
            let Some(last) = group.last() else {
                continue;
            };

            let old_start_idx = first.old_range().start;
            let old_end_idx = last.old_range().end;
            let new_start_idx = first.new_range().start;
            let new_end_idx = last.new_range().end;

            let old_count = (old_end_idx.saturating_sub(old_start_idx)) as u32;
            let new_count = (new_end_idx.saturating_sub(new_start_idx)) as u32;
            let old_start = if old_count == 0 {
                old_start_idx as u32
            } else {
                (old_start_idx + 1) as u32
            };
            let new_start = if new_count == 0 {
                new_start_idx as u32
            } else {
                (new_start_idx + 1) as u32
            };

            let header = format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@");

            if rendered_rows >= MAX_DIFF_RENDER_ROWS {
                truncated = true;
                omitted_lines += (old_count.max(new_count)) as usize;
                continue;
            }

            let mut rows = Vec::new();

            for op in &group {
                match *op {
                    DiffOp::Equal { .. } => {
                        for change in diff.iter_changes(op) {
                            if rendered_rows >= MAX_DIFF_RENDER_ROWS {
                                truncated = true;
                                omitted_lines += 1;
                                continue;
                            }
                            let text = trim_line_end(change.value());
                            let old_ln = change.old_index().map(|i| (i + 1) as u32);
                            let new_ln = change.new_index().map(|i| (i + 1) as u32);
                            rows.push(DiffPairRow {
                                left: DiffCell::plain(old_ln, DiffLineKind::Context, text),
                                right: DiffCell::plain(new_ln, DiffLineKind::Context, text),
                            });
                            rendered_rows += 1;
                        }
                    }
                    DiffOp::Delete { .. } | DiffOp::Insert { .. } | DiffOp::Replace { .. } => {
                        let mut left_cells = Vec::new();
                        let mut right_cells = Vec::new();

                        if enable_inline && matches!(op, DiffOp::Replace { .. }) {
                            for change in diff.iter_inline_changes(op) {
                                let spans = collect_inline_spans(&change);
                                match change.tag() {
                                    ChangeTag::Delete => {
                                        left_cells.push(DiffCell {
                                            line_no: change.old_index().map(|i| (i + 1) as u32),
                                            kind: DiffLineKind::Delete,
                                            spans,
                                        });
                                    }
                                    ChangeTag::Insert => {
                                        right_cells.push(DiffCell {
                                            line_no: change.new_index().map(|i| (i + 1) as u32),
                                            kind: DiffLineKind::Insert,
                                            spans,
                                        });
                                    }
                                    ChangeTag::Equal => {}
                                }
                            }
                        } else {
                            for change in diff.iter_changes(op) {
                                let text = trim_line_end(change.value());
                                match change.tag() {
                                    ChangeTag::Delete => {
                                        left_cells.push(DiffCell::plain(
                                            change.old_index().map(|i| (i + 1) as u32),
                                            DiffLineKind::Delete,
                                            text,
                                        ));
                                    }
                                    ChangeTag::Insert => {
                                        right_cells.push(DiffCell::plain(
                                            change.new_index().map(|i| (i + 1) as u32),
                                            DiffLineKind::Insert,
                                            text,
                                        ));
                                    }
                                    ChangeTag::Equal => {}
                                }
                            }
                        }

                        let pair_count = left_cells.len().max(right_cells.len());
                        let mut left_iter = left_cells.into_iter();
                        let mut right_iter = right_cells.into_iter();

                        for _ in 0..pair_count {
                            let left = left_iter.next().unwrap_or_else(DiffCell::empty);
                            let right = right_iter.next().unwrap_or_else(DiffCell::empty);
                            if rendered_rows >= MAX_DIFF_RENDER_ROWS {
                                truncated = true;
                                omitted_lines += 1;
                                continue;
                            }
                            rows.push(DiffPairRow { left, right });
                            rendered_rows += 1;
                        }
                    }
                }
            }

            if !rows.is_empty() {
                hunks.push(DiffHunk {
                    header,
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                    rows,
                });
            }
        }

        Self {
            path: path.to_string(),
            language,
            kind: DiffKind::Modified,
            stats: DiffStats {
                added_lines,
                removed_lines,
                hunk_count,
                old_total_lines,
                new_total_lines,
                truncated,
                omitted_lines,
            },
            hunks,
        }
    }

    fn compute_created(path: &str, language: String, new_content: &str) -> Self {
        let new_norm = normalize_newlines(new_content);
        let all_lines: Vec<&str> = new_norm.lines().collect();
        let total = all_lines.len();

        let cap = total.min(MAX_DIFF_RENDER_ROWS);
        let truncated = total > MAX_DIFF_RENDER_ROWS;
        let omitted_lines = total.saturating_sub(cap);

        let mut rows = Vec::with_capacity(cap);
        for (idx, &line) in all_lines.iter().take(cap).enumerate() {
            let ln = (idx + 1) as u32;
            rows.push(DiffPairRow {
                left: DiffCell::empty(),
                right: DiffCell::plain(Some(ln), DiffLineKind::Insert, line),
            });
        }

        let hunks = if rows.is_empty() {
            Vec::new()
        } else {
            vec![DiffHunk {
                header: format!("@@ -0,0 +1,{total} @@"),
                old_start: 0,
                old_count: 0,
                new_start: 1,
                new_count: total as u32,
                rows,
            }]
        };

        Self {
            path: path.to_string(),
            language,
            kind: DiffKind::Created,
            stats: DiffStats {
                added_lines: total,
                removed_lines: 0,
                hunk_count: usize::from(!hunks.is_empty()),
                old_total_lines: 0,
                new_total_lines: total,
                truncated,
                omitted_lines,
            },
            hunks,
        }
    }

    /// Creates a `FileDiff` representing a binary or non-text file.
    #[must_use]
    pub fn binary(path: &str) -> Self {
        Self {
            path: path.to_string(),
            language: infer_language(path),
            kind: DiffKind::Binary,
            stats: DiffStats::default(),
            hunks: Vec::new(),
        }
    }

    /// Fallback constructor when only the new content is available (e.g. legacy deserialization).
    #[must_use]
    pub fn fallback_new(path: &str, new_content: &str) -> Self {
        Self::compute(path, None, new_content)
    }

    /// Total number of aligned rows across all hunks.
    #[must_use]
    pub fn total_rendered_rows(&self) -> usize {
        self.hunks.iter().map(|h| h.rows.len()).sum()
    }

    /// Formats the diff as a standard unified patch for clipboard copy.
    #[must_use]
    pub fn to_unified_patch(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("--- a/{}\n+++ b/{}\n", self.path, self.path));
        for hunk in &self.hunks {
            out.push_str(&hunk.header);
            out.push('\n');
            for row in &hunk.rows {
                match (&row.left.kind, &row.right.kind) {
                    (DiffLineKind::Context, _) => {
                        out.push(' ');
                        out.push_str(&row.right.plain_text());
                        out.push('\n');
                    }
                    (DiffLineKind::Delete, DiffLineKind::Empty) => {
                        out.push('-');
                        out.push_str(&row.left.plain_text());
                        out.push('\n');
                    }
                    (DiffLineKind::Empty, DiffLineKind::Insert) => {
                        out.push('+');
                        out.push_str(&row.right.plain_text());
                        out.push('\n');
                    }
                    (DiffLineKind::Delete, DiffLineKind::Insert) => {
                        out.push('-');
                        out.push_str(&row.left.plain_text());
                        out.push('\n');
                        out.push('+');
                        out.push_str(&row.right.plain_text());
                        out.push('\n');
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

fn collect_inline_spans(change: &similar::InlineChange<'_, str>) -> Vec<DiffSpan> {
    let mut spans: Vec<DiffSpan> = Vec::new();
    for (emphasized, value) in change.iter_strings_lossy() {
        let cleaned = value.trim_end_matches(['\r', '\n']);
        if cleaned.is_empty() {
            continue;
        }
        if let Some(last) = spans.last_mut()
            && last.emphasized == emphasized
        {
            last.text.push_str(cleaned);
            continue;
        }
        spans.push(DiffSpan {
            emphasized,
            text: cleaned.to_string(),
        });
    }
    spans
}

fn normalize_newlines(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains("\r\n") {
        std::borrow::Cow::Owned(s.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

fn trim_line_end(s: &str) -> &str {
    s.trim_end_matches(['\r', '\n'])
}

fn count_lines(s: &str) -> usize {
    if s.is_empty() { 0 } else { s.lines().count() }
}

fn is_binary_bytes(bytes: &[u8]) -> bool {
    let probe = bytes.len().min(8192);
    bytes[..probe].contains(&0)
}

fn is_binary_path(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "bmp"
            | "ico"
            | "pdf"
            | "zip"
            | "tar"
            | "gz"
            | "xz"
            | "zst"
            | "7z"
            | "wasm"
            | "so"
            | "dylib"
            | "dll"
            | "exe"
            | "bin"
            | "o"
            | "a"
            | "ttf"
            | "otf"
            | "woff"
            | "woff2"
            | "mp3"
            | "wav"
            | "ogg"
            | "flac"
            | "mp4"
            | "webm"
            | "mkv"
            | "avi"
            | "mov"
            | "sqlite"
            | "db"
            | "bxt"
    )
}

fn infer_language(path: &str) -> String {
    let p = Path::new(path);
    if let Some(ext) = p.extension().and_then(|e| e.to_str())
        && !ext.is_empty()
    {
        return ext.to_ascii_lowercase();
    }
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("text")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_file_displays_all_lines_as_additions() {
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        let diff = FileDiff::compute("src/main.rs", None, content);

        assert_eq!(diff.kind, DiffKind::Created);
        assert_eq!(diff.language, "rs");
        assert_eq!(diff.stats.added_lines, 3);
        assert_eq!(diff.stats.removed_lines, 0);
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].rows.len(), 3);

        for (i, row) in diff.hunks[0].rows.iter().enumerate() {
            assert_eq!(row.left.kind, DiffLineKind::Empty);
            assert_eq!(row.right.kind, DiffLineKind::Insert);
            assert_eq!(row.right.line_no, Some((i + 1) as u32));
        }
    }

    #[test]
    fn existing_file_computes_contextual_hunks_and_inline_spans() {
        let old = "line 1\nline 2\nline 3\nlet count = 10;\nline 5\nline 6\n";
        let new = "line 1\nline 2\nline 3\nlet count = 42;\nlet extra = true;\nline 5\nline 6\n";

        let diff = FileDiff::compute("src/lib.rs", Some(old), new);

        assert_eq!(diff.kind, DiffKind::Modified);
        assert_eq!(diff.stats.added_lines, 2);
        assert_eq!(diff.stats.removed_lines, 1);
        assert_eq!(diff.hunks.len(), 1);

        let hunk = &diff.hunks[0];
        // 3 context before + 2 replacement rows + 2 context after = 7 rows
        assert_eq!(hunk.rows.len(), 7);

        // Row 3 is the modified `let count` line; check inline emphasis on `10` -> `42`
        let mod_row = &hunk.rows[3];
        assert_eq!(mod_row.left.kind, DiffLineKind::Delete);
        assert_eq!(mod_row.right.kind, DiffLineKind::Insert);
        assert!(mod_row.left.spans.iter().any(|s| s.emphasized && s.text.contains("10")));
        assert!(mod_row.right.spans.iter().any(|s| s.emphasized && s.text.contains("42")));

        // Row 4 is the unbalanced extra insertion; left side must be Empty filler
        let extra_row = &hunk.rows[4];
        assert_eq!(extra_row.left.kind, DiffLineKind::Empty);
        assert_eq!(extra_row.right.kind, DiffLineKind::Insert);
        assert_eq!(extra_row.right.plain_text(), "let extra = true;");
    }

    #[test]
    fn binary_files_are_detected_and_not_diffed_as_text() {
        let binary_data = "PNG\0\r\n\x1a\n";
        let diff = FileDiff::compute("assets/logo.dat", None, binary_data);
        assert_eq!(diff.kind, DiffKind::Binary);
        assert!(diff.hunks.is_empty());

        let ext_diff = FileDiff::compute("assets/icon.png", None, "not nul but png");
        assert_eq!(ext_diff.kind, DiffKind::Binary);
        assert!(ext_diff.hunks.is_empty());
    }

    #[test]
    fn crlf_and_lf_normalize_cleanly() {
        let old = "alpha\r\nbeta\r\ngamma\r\n";
        let new = "alpha\nbeta\ngamma\n";
        let diff = FileDiff::compute("config.toml", Some(old), new);
        assert_eq!(diff.kind, DiffKind::Unchanged);
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn large_diffs_cap_at_max_render_rows() {
        let mut new_text = String::new();
        for i in 0..400 {
            new_text.push_str(&format!("line {i}\n"));
        }
        let diff = FileDiff::compute("big.txt", None, &new_text);
        assert_eq!(diff.kind, DiffKind::Created);
        assert_eq!(diff.stats.added_lines, 400);
        assert!(diff.stats.truncated);
        assert_eq!(diff.stats.omitted_lines, 400 - MAX_DIFF_RENDER_ROWS);
        assert_eq!(diff.total_rendered_rows(), MAX_DIFF_RENDER_ROWS);
    }
}
