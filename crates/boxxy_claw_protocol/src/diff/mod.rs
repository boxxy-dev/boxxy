//! Local text & binary diff computation (`FileDiff`) powered by `similar` Histogram diffing.

mod compute;

use crate::str_utils::truncate_chars_cow;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Write as _;

/// Maximum number of aligned diff rows stored/rendered per `FileDiff` to protect
/// D-Bus payloads, SQLite storage, and GTK4 layout passes on massive files.
pub const MAX_DIFF_RENDER_ROWS: usize = 250;

/// Maximum Unicode scalar values retained per rendered diff line to protect
/// D-Bus frames, SQLite rows, and Pango single-line layout on minified files.
pub const MAX_DIFF_LINE_CHARS: usize = 4_096;

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
    /// True if total rendered rows exceeded `MAX_DIFF_RENDER_ROWS` or `MAX_DIFF_FILE_BYTES`.
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
        let raw = text.into();
        if raw.is_empty() {
            return Self {
                line_no,
                kind,
                spans: Vec::new(),
            };
        }
        let bounded = match truncate_chars_cow(&raw, MAX_DIFF_LINE_CHARS) {
            Cow::Borrowed(_) => raw,
            Cow::Owned(clipped) => clipped,
        };
        Self {
            line_no,
            kind,
            spans: vec![DiffSpan {
                emphasized: false,
                text: bounded,
            }],
        }
    }

    pub(super) fn plain_slice(line_no: Option<u32>, kind: DiffLineKind, text: &str) -> Self {
        if text.is_empty() {
            return Self {
                line_no,
                kind,
                spans: Vec::new(),
            };
        }
        Self {
            line_no,
            kind,
            spans: vec![DiffSpan {
                emphasized: false,
                text: truncate_chars_cow(text, MAX_DIFF_LINE_CHARS).into_owned(),
            }],
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
    /// Optional `(caption, local_file_path)` image previews rendered inside `DiffKind::Binary`
    /// approval cards (e.g. `prepare_character_assets` and `generate_image` `output_path`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preview_images: Vec<(String, String)>,
}

impl FileDiff {
    /// Computes a structured `FileDiff` between optional `old_content` (`None` if creating a new file)
    /// and `new_content`.
    #[must_use]
    pub fn compute(path: &str, old_content: Option<&str>, new_content: &str) -> Self {
        compute::compute_file_diff(path, old_content, new_content)
    }

    /// Creates a `FileDiff` representing a binary or non-text file.
    #[must_use]
    pub fn binary(path: &str) -> Self {
        Self::binary_with_previews(path, Vec::new())
    }

    /// Creates a `FileDiff` representing a binary image write with live `(caption, local_path)` previews.
    #[must_use]
    pub fn binary_with_previews(path: &str, preview_images: Vec<(String, String)>) -> Self {
        Self {
            path: path.to_string(),
            language: compute::infer_language(path),
            kind: DiffKind::Binary,
            stats: DiffStats::default(),
            hunks: Vec::new(),
            preview_images,
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
    /// writes span slices directly without per-line `String` allocations. Returns an
    /// empty string for `Unchanged`, `Binary`, or hunk-less truncated files so callers
    /// never receive a header-only invalid patch.
    #[must_use]
    pub fn to_unified_patch(&self) -> String {
        if self.hunks.is_empty() || matches!(self.kind, DiffKind::Binary | DiffKind::Unchanged) {
            return String::new();
        }

        let mut out = String::with_capacity(64 + self.total_rendered_rows() * 48);
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
        assert!(patch.starts_with("--- /dev/null\n+++ b/src/main.rs\n@@ -0,0 +1,3 @@\n+fn main()"));
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
    fn binary_and_unchanged_files_emit_empty_unified_patches() {
        let binary_data = "PNG\0\r\n\x1a\n";
        let diff = FileDiff::compute("assets/logo.dat", None, binary_data);
        assert_eq!(diff.kind, DiffKind::Binary);
        assert_eq!(diff.hunks, []);
        assert_eq!(diff.to_unified_patch(), "");

        let ext_diff = FileDiff::compute("assets/ICON.PNG", None, "not nul but png");
        assert_eq!(ext_diff.kind, DiffKind::Binary);
        assert_eq!(ext_diff.hunks, []);

        let dotfile_diff = FileDiff::compute(".gitignore", None, "target/\n");
        assert_eq!(dotfile_diff.language, "gitignore");

        let unchanged =
            FileDiff::compute("config.toml", Some("alpha\r\nbeta\r\n"), "alpha\nbeta\n");
        assert_eq!(unchanged.kind, DiffKind::Unchanged);
        assert_eq!(unchanged.hunks, []);
        assert_eq!(unchanged.to_unified_patch(), "");
    }

    #[test]
    fn large_diffs_cap_rows_line_lengths_and_emit_consistent_hunk_headers() {
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
        assert_eq!(
            created.hunks[0].new_count,
            u32::try_from(MAX_DIFF_RENDER_ROWS).unwrap()
        );
        assert!(
            created
                .to_unified_patch()
                .contains(&format!("@@ -0,0 +1,{MAX_DIFF_RENDER_ROWS} @@"))
        );

        let modified = FileDiff::compute("big_modified.txt", Some(&old_text), &new_text);
        assert_eq!(modified.kind, DiffKind::Modified);
        assert_eq!(modified.stats.added_lines, 400);
        assert_eq!(modified.stats.removed_lines, 400);
        assert!(modified.stats.truncated);
        assert_eq!(modified.stats.omitted_lines, 400 - MAX_DIFF_RENDER_ROWS);
        assert_eq!(modified.total_rendered_rows(), MAX_DIFF_RENDER_ROWS);
        assert_eq!(
            modified.hunks[0].old_count,
            u32::try_from(MAX_DIFF_RENDER_ROWS).unwrap()
        );
        assert_eq!(
            modified.hunks[0].new_count,
            u32::try_from(MAX_DIFF_RENDER_ROWS).unwrap()
        );

        // Single-line minified content is bounded by MAX_DIFF_LINE_CHARS
        let minified_old = format!("const a={};", "1".repeat(MAX_DIFF_LINE_CHARS + 500));
        let minified_new = format!("const a={};", "2".repeat(MAX_DIFF_LINE_CHARS + 500));
        let min_diff = FileDiff::compute("bundle.min.js", Some(&minified_old), &minified_new);
        let right_text = min_diff.hunks[0].rows[0].right.plain_text();
        assert_eq!(right_text.chars().count(), MAX_DIFF_LINE_CHARS + 1);
        assert!(right_text.ends_with('…'));

        // Oversized created file (> 2 MiB) marks truncated without materializing rows
        let huge_created = "x\n".repeat((2 * 1024 * 1024 / 2) + 16);
        let huge_diff = FileDiff::compute("huge.log", None, &huge_created);
        assert_eq!(huge_diff.kind, DiffKind::Created);
        assert!(huge_diff.stats.truncated);
        assert_eq!(huge_diff.hunks, []);
        assert_eq!(huge_diff.to_unified_patch(), "");
    }
}
