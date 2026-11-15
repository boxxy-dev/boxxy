//! Local text & binary diff computation (`FileDiff`) powered by `similar` Histogram diffing.

use serde::{Deserialize, Serialize};
use similar::{Algorithm, ChangeTag, DiffOp, InlineChangeOptions, TextDiff};
use std::borrow::Cow;
use std::fmt::Write as _;
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
        self.write_plain_text(&mut out);
        out
    }

    /// Appends all span text directly into `out` without allocating an intermediate `String`.
    pub fn write_plain_text(&self, out: &mut String) {
        for span in &self.spans {
            out.push_str(&span.text);
        }
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

#[inline]
fn saturating_u32(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

#[inline]
fn op_pair_rows(op: &DiffOp) -> usize {
    match *op {
        DiffOp::Equal { len, .. } => len,
        DiffOp::Delete { old_len, .. } => old_len,
        DiffOp::Insert { new_len, .. } => new_len,
        DiffOp::Replace {
            old_len, new_len, ..
        } => old_len.max(new_len),
    }
}

struct HunkRenderBudget {
    rendered_rows: usize,
    truncated: bool,
    omitted_lines: usize,
}

impl FileDiff {
    /// Computes a structured `FileDiff` between optional `old_content` (`None` if creating a new file)
    /// and `new_content`.
    #[must_use]
    pub fn compute(path: &str, old_content: Option<&str>, new_content: &str) -> Self {
        let language = infer_language(path);

        if is_binary_path(path)
            || is_binary_bytes(new_content.as_bytes())
            || old_content.is_some_and(|old| is_binary_bytes(old.as_bytes()))
        {
            return Self::binary(path);
        }

        let Some(old_raw) = old_content else {
            return Self::compute_created(path, language, new_content);
        };

        // Guard against multi-megabyte buffers BEFORE allocating CRLF-normalized copies.
        if old_raw.len() > MAX_DIFF_FILE_BYTES || new_content.len() > MAX_DIFF_FILE_BYTES {
            return Self::compute_oversized(path, language, old_raw, new_content);
        }

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
                    old_total_lines,
                    new_total_lines,
                    ..DiffStats::default()
                },
                hunks: Vec::new(),
            };
        }

        Self::compute_modified(
            path,
            language,
            old_norm.as_ref(),
            new_norm.as_ref(),
            old_total_lines,
            new_total_lines,
        )
    }

    fn compute_oversized(
        path: &str,
        language: String,
        old_raw: &str,
        new_content: &str,
    ) -> Self {
        let old_total_lines = count_lines(old_raw);
        let new_total_lines = count_lines(new_content);
        if old_raw == new_content {
            return Self {
                path: path.to_string(),
                language,
                kind: DiffKind::Unchanged,
                stats: DiffStats {
                    old_total_lines,
                    new_total_lines,
                    ..DiffStats::default()
                },
                hunks: Vec::new(),
            };
        }
        Self {
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
        }
    }

    fn compute_modified(
        path: &str,
        language: String,
        old_text: &str,
        new_text: &str,
        old_total_lines: usize,
        new_total_lines: usize,
    ) -> Self {
        let diff = TextDiff::configure()
            .algorithm(Algorithm::Histogram)
            .timeout(Duration::from_millis(250))
            .diff_lines(old_text, new_text);

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
            old_text.len() <= MAX_INLINE_DIFF_BYTES && new_text.len() <= MAX_INLINE_DIFF_BYTES;

        let mut hunks = Vec::with_capacity(groups.len());
        let mut budget = HunkRenderBudget {
            rendered_rows: 0,
            truncated: false,
            omitted_lines: 0,
        };

        for group in groups {
            if let Some(hunk) = build_group_hunk(&diff, &group, enable_inline, &mut budget) {
                hunks.push(hunk);
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
                truncated: budget.truncated,
                omitted_lines: budget.omitted_lines,
            },
            hunks,
        }
    }

    fn compute_created(path: &str, language: String, new_content: &str) -> Self {
        let mut rows = Vec::new();
        let mut total = 0usize;

        for raw_line in new_content.lines() {
            total += 1;
            if rows.len() < MAX_DIFF_RENDER_ROWS {
                let ln = saturating_u32(total);
                rows.push(DiffPairRow {
                    left: DiffCell::empty(),
                    right: DiffCell::plain(Some(ln), DiffLineKind::Insert, raw_line),
                });
            }
        }

        let truncated = total > MAX_DIFF_RENDER_ROWS;
        let omitted_lines = total.saturating_sub(rows.len());

        let hunks = if rows.is_empty() {
            Vec::new()
        } else {
            vec![DiffHunk {
                header: format!("@@ -0,0 +1,{total} @@"),
                old_start: 0,
                old_count: 0,
                new_start: 1,
                new_count: saturating_u32(total),
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

    /// Formats the diff as a standard Git-compatible unified patch (`git apply` ready).
    ///
    /// Groups contiguous `-` lines before `+` lines within replacement blocks and
    /// writes span slices directly without per-line `String` allocations.
    #[must_use]
    pub fn to_unified_patch(&self) -> String {
        let mut out = String::new();
        if self.kind == DiffKind::Created {
            let _ = writeln!(out, "--- /dev/null\n+++ b/{}", self.path);
        } else {
            let _ = writeln!(out, "--- a/{0}\n+++ b/{0}", self.path);
        }

        let mut pending_inserts: Vec<&DiffCell> = Vec::new();
        let flush_inserts = |out: &mut String, pending_inserts: &mut Vec<&DiffCell>| {
            for cell in pending_inserts.drain(..) {
                out.push('+');
                cell.write_plain_text(out);
                out.push('\n');
            }
        };

        for hunk in &self.hunks {
            out.push_str(&hunk.header);
            out.push('\n');
            for row in &hunk.rows {
                match (row.left.kind, row.right.kind) {
                    (DiffLineKind::Context, _) => {
                        flush_inserts(&mut out, &mut pending_inserts);
                        out.push(' ');
                        row.right.write_plain_text(&mut out);
                        out.push('\n');
                    }
                    (DiffLineKind::Delete, DiffLineKind::Empty) => {
                        out.push('-');
                        row.left.write_plain_text(&mut out);
                        out.push('\n');
                    }
                    (DiffLineKind::Empty, DiffLineKind::Insert) => {
                        pending_inserts.push(&row.right);
                    }
                    (DiffLineKind::Delete, DiffLineKind::Insert) => {
                        out.push('-');
                        row.left.write_plain_text(&mut out);
                        out.push('\n');
                        pending_inserts.push(&row.right);
                    }
                    _ => {
                        flush_inserts(&mut out, &mut pending_inserts);
                    }
                }
            }
            flush_inserts(&mut out, &mut pending_inserts);
        }
        out
    }
}

fn build_group_hunk(
    diff: &TextDiff<'_, '_, str>,
    group: &[DiffOp],
    enable_inline: bool,
    budget: &mut HunkRenderBudget,
) -> Option<DiffHunk> {
    let first = group.first()?;
    let last = group.last()?;

    let old_start_idx = first.old_range().start;
    let old_end_idx = last.old_range().end;
    let new_start_idx = first.new_range().start;
    let new_end_idx = last.new_range().end;

    let old_count = saturating_u32(old_end_idx.saturating_sub(old_start_idx));
    let new_count = saturating_u32(new_end_idx.saturating_sub(new_start_idx));
    let old_start = if old_count == 0 {
        saturating_u32(old_start_idx)
    } else {
        saturating_u32(old_start_idx.saturating_add(1))
    };
    let new_start = if new_count == 0 {
        saturating_u32(new_start_idx)
    } else {
        saturating_u32(new_start_idx.saturating_add(1))
    };

    if budget.rendered_rows >= MAX_DIFF_RENDER_ROWS {
        budget.truncated = true;
        budget.omitted_lines += group.iter().map(op_pair_rows).sum::<usize>();
        return None;
    }

    let header = format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@");
    let mut rows = Vec::new();

    for op in group {
        append_op_rows(diff, op, enable_inline, &mut rows, budget);
    }

    (!rows.is_empty()).then_some(DiffHunk {
        header,
        old_start,
        old_count,
        new_start,
        new_count,
        rows,
    })
}

fn append_op_rows(
    diff: &TextDiff<'_, '_, str>,
    op: &DiffOp,
    enable_inline: bool,
    rows: &mut Vec<DiffPairRow>,
    budget: &mut HunkRenderBudget,
) {
    if budget.rendered_rows >= MAX_DIFF_RENDER_ROWS {
        budget.truncated = true;
        budget.omitted_lines += op_pair_rows(op);
        return;
    }

    if matches!(op, DiffOp::Equal { .. }) {
        for change in diff.iter_changes(op) {
            if budget.rendered_rows >= MAX_DIFF_RENDER_ROWS {
                budget.truncated = true;
                budget.omitted_lines += 1;
                continue;
            }
            let text = trim_line_end(change.value());
            let old_ln = change.old_index().map(|i| saturating_u32(i + 1));
            let new_ln = change.new_index().map(|i| saturating_u32(i + 1));
            rows.push(DiffPairRow {
                left: DiffCell::plain(old_ln, DiffLineKind::Context, text),
                right: DiffCell::plain(new_ln, DiffLineKind::Context, text),
            });
            budget.rendered_rows += 1;
        }
        return;
    }

    let remaining_budget = MAX_DIFF_RENDER_ROWS - budget.rendered_rows;
    let (left_cells, right_cells, total_pairs) =
        collect_change_cells(diff, op, enable_inline, remaining_budget);

    let emit_pairs = total_pairs.min(remaining_budget);
    let mut left_iter = left_cells.into_iter();
    let mut right_iter = right_cells.into_iter();

    for _ in 0..emit_pairs {
        let left = left_iter.next().unwrap_or_else(DiffCell::empty);
        let right = right_iter.next().unwrap_or_else(DiffCell::empty);
        rows.push(DiffPairRow { left, right });
        budget.rendered_rows += 1;
    }

    if total_pairs > emit_pairs {
        budget.truncated = true;
        budget.omitted_lines += total_pairs - emit_pairs;
    }
}

fn collect_change_cells(
    diff: &TextDiff<'_, '_, str>,
    op: &DiffOp,
    enable_inline: bool,
    remaining_budget: usize,
) -> (Vec<DiffCell>, Vec<DiffCell>, usize) {
    let mut left_cells = Vec::new();
    let mut right_cells = Vec::new();
    let mut left_seen = 0usize;
    let mut right_seen = 0usize;

    if enable_inline && matches!(op, DiffOp::Replace { .. }) {
        let mut inline_opts = InlineChangeOptions::new();
        inline_opts
            .algorithm(Algorithm::Histogram)
            .semantic_cleanup(true);
        for change in diff.iter_inline_changes_with_options(op, inline_opts) {
            match change.tag() {
                ChangeTag::Delete => {
                    left_seen += 1;
                    if left_cells.len() < remaining_budget {
                        left_cells.push(DiffCell {
                            line_no: change.old_index().map(|i| saturating_u32(i + 1)),
                            kind: DiffLineKind::Delete,
                            spans: collect_inline_spans(&change),
                        });
                    }
                }
                ChangeTag::Insert => {
                    right_seen += 1;
                    if right_cells.len() < remaining_budget {
                        right_cells.push(DiffCell {
                            line_no: change.new_index().map(|i| saturating_u32(i + 1)),
                            kind: DiffLineKind::Insert,
                            spans: collect_inline_spans(&change),
                        });
                    }
                }
                ChangeTag::Equal => {}
            }
        }
    } else {
        for change in diff.iter_changes(op) {
            match change.tag() {
                ChangeTag::Delete => {
                    left_seen += 1;
                    if left_cells.len() < remaining_budget {
                        left_cells.push(DiffCell::plain(
                            change.old_index().map(|i| saturating_u32(i + 1)),
                            DiffLineKind::Delete,
                            trim_line_end(change.value()),
                        ));
                    }
                }
                ChangeTag::Insert => {
                    right_seen += 1;
                    if right_cells.len() < remaining_budget {
                        right_cells.push(DiffCell::plain(
                            change.new_index().map(|i| saturating_u32(i + 1)),
                            DiffLineKind::Insert,
                            trim_line_end(change.value()),
                        ));
                    }
                }
                ChangeTag::Equal => {}
            }
        }
    }

    (left_cells, right_cells, left_seen.max(right_seen))
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

fn normalize_newlines(s: &str) -> Cow<'_, str> {
    if s.contains("\r\n") {
        Cow::Owned(s.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(s)
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

const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "avif", "jxl", "gif", "bmp", "ico", "pdf", "zip", "tar", "gz",
    "bz2", "xz", "lz4", "zst", "7z", "wasm", "so", "dylib", "dll", "exe", "bin", "o", "a", "pyc",
    "class", "jar", "whl", "ttf", "otf", "woff", "woff2", "mp3", "wav", "ogg", "flac", "mp4",
    "webm", "mkv", "avi", "mov", "sqlite", "db", "parquet", "bxt",
];

fn is_binary_path(path: &str) -> bool {
    let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    BINARY_EXTENSIONS
        .iter()
        .any(|candidate| ext.eq_ignore_ascii_case(candidate))
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
        .map(|n| n.trim_start_matches('.'))
        .filter(|n| !n.is_empty())
        .unwrap_or("text")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_file_displays_all_lines_as_additions_and_emits_dev_null_patch() {
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
            assert_eq!(row.right.line_no, Some(u32::try_from(i + 1).unwrap()));
        }

        let patch = diff.to_unified_patch();
        assert!(
            patch.starts_with("--- /dev/null\n+++ b/src/main.rs\n@@ -0,0 +1,3 @@\n+fn main()")
        );
    }

    #[test]
    fn existing_file_computes_contextual_hunks_inline_spans_and_grouped_unified_patch() {
        let old = "line 1\nline 2\nline 3\nlet count = 10;\nlet flag = false;\nline 6\nline 7\n";
        let new = "line 1\nline 2\nline 3\nlet count = 42;\nlet flag = true;\nlet extra = 1;\nline 6\nline 7\n";

        let diff = FileDiff::compute("src/lib.rs", Some(old), new);

        assert_eq!(diff.kind, DiffKind::Modified);
        assert_eq!(diff.stats.added_lines, 3);
        assert_eq!(diff.stats.removed_lines, 2);
        assert_eq!(diff.hunks.len(), 1);

        let hunk = &diff.hunks[0];
        assert_eq!(hunk.rows.len(), 8);

        let mod_row = &hunk.rows[3];
        assert_eq!(mod_row.left.kind, DiffLineKind::Delete);
        assert_eq!(mod_row.right.kind, DiffLineKind::Insert);
        assert!(
            mod_row
                .left
                .spans
                .iter()
                .any(|s| s.emphasized && s.text.contains("10"))
        );
        assert!(
            mod_row
                .right
                .spans
                .iter()
                .any(|s| s.emphasized && s.text.contains("42"))
        );

        let extra_row = &hunk.rows[5];
        assert_eq!(extra_row.left.kind, DiffLineKind::Empty);
        assert_eq!(extra_row.right.kind, DiffLineKind::Insert);
        assert_eq!(extra_row.right.plain_text(), "let extra = 1;");

        let patch = diff.to_unified_patch();
        assert!(patch.contains(
            "-let count = 10;\n-let flag = false;\n+let count = 42;\n+let flag = true;\n+let extra = 1;\n"
        ));
    }

    #[test]
    fn binary_files_and_dotfiles_are_detected_cleanly() {
        let binary_data = "PNG\0\r\n\x1a\n";
        let diff = FileDiff::compute("assets/logo.dat", None, binary_data);
        assert_eq!(diff.kind, DiffKind::Binary);
        assert_eq!(diff.hunks, []);

        let ext_diff = FileDiff::compute("assets/ICON.PNG", None, "not nul but png");
        assert_eq!(ext_diff.kind, DiffKind::Binary);
        assert_eq!(ext_diff.hunks, []);

        let dotfile_diff = FileDiff::compute(".gitignore", None, "target/\n");
        assert_eq!(dotfile_diff.language, "gitignore");
    }

    #[test]
    fn crlf_and_lf_normalize_cleanly() {
        let old = "alpha\r\nbeta\r\ngamma\r\n";
        let new = "alpha\nbeta\ngamma\n";
        let diff = FileDiff::compute("config.toml", Some(old), new);
        assert_eq!(diff.kind, DiffKind::Unchanged);
        assert_eq!(diff.hunks, []);
    }

    #[test]
    fn large_created_and_modified_diffs_cap_at_max_render_rows() {
        let mut old_text = String::new();
        let mut new_text = String::new();
        for i in 0..400 {
            let _ = writeln!(old_text, "old line {i}");
            let _ = writeln!(new_text, "new line {i}");
        }

        let created = FileDiff::compute("big_created.txt", None, &new_text);
        assert_eq!(created.kind, DiffKind::Created);
        assert_eq!(created.stats.added_lines, 400);
        assert!(created.stats.truncated);
        assert_eq!(created.stats.omitted_lines, 400 - MAX_DIFF_RENDER_ROWS);
        assert_eq!(created.total_rendered_rows(), MAX_DIFF_RENDER_ROWS);

        let modified = FileDiff::compute("big_modified.txt", Some(&old_text), &new_text);
        assert_eq!(modified.kind, DiffKind::Modified);
        assert_eq!(modified.stats.added_lines, 400);
        assert_eq!(modified.stats.removed_lines, 400);
        assert!(modified.stats.truncated);
        assert_eq!(modified.stats.omitted_lines, 400 - MAX_DIFF_RENDER_ROWS);
        assert_eq!(modified.total_rendered_rows(), MAX_DIFF_RENDER_ROWS);
    }
}
