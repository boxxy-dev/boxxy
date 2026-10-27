use crate::cache::{SimpleLru, syntax_set, theme_set};
use crate::parser::markdown::escape_pango;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, LazyLock};
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxSet;

const DIFF_CACHE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffPayloadKind {
    Modified,
    Created,
    Binary,
    Unchanged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPayloadStats {
    pub added_lines: usize,
    pub removed_lines: usize,
    pub hunk_count: usize,
    pub old_total_lines: usize,
    pub new_total_lines: usize,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub omitted_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPayloadSpan {
    pub emphasized: bool,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffRowTone {
    Context,
    Delete,
    Insert,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPayloadCell {
    pub line_no: Option<u32>,
    pub kind: DiffRowTone,
    pub spans: Vec<DiffPayloadSpan>,
}

impl DiffPayloadCell {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPayloadPairRow {
    pub left: DiffPayloadCell,
    pub right: DiffPayloadCell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPayloadHunk {
    pub header: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub rows: Vec<DiffPayloadPairRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPayload {
    pub path: String,
    pub language: String,
    pub kind: DiffPayloadKind,
    pub stats: DiffPayloadStats,
    pub hunks: Vec<DiffPayloadHunk>,
}

/// Pre-computed, ready-to-bind Pango markup and line tone metadata for a single hunk.
#[derive(Debug, Clone)]
pub struct PreparedHunk {
    pub header: String,
    pub row_count: usize,
    pub left_gutter_markup: String,
    pub left_code_markup: String,
    pub left_tones: Vec<DiffRowTone>,
    pub right_gutter_markup: String,
    pub right_code_markup: String,
    pub right_tones: Vec<DiffRowTone>,
    /// Compact 1-column unified view used as the primary surface in narrow/drawer mode.
    pub unified_gutter_markup: String,
    pub unified_code_markup: String,
    pub unified_tones: Vec<DiffRowTone>,
}

/// Cached, ready-to-render representation of a `DiffPayload`.
#[derive(Debug, Clone)]
pub struct PreparedDiff {
    pub path: String,
    pub dir_prefix: String,
    pub filename: String,
    pub language: String,
    pub kind: DiffPayloadKind,
    pub stats: DiffPayloadStats,
    pub total_rows: usize,
    pub unified_patch: String,
    pub hunks: Vec<PreparedHunk>,
}

type DiffCacheKey = (usize, u64, bool);

static DIFF_CACHE: LazyLock<Mutex<SimpleLru<DiffCacheKey, Arc<PreparedDiff>>>> =
    LazyLock::new(|| Mutex::new(SimpleLru::new(DIFF_CACHE_CAPACITY)));

fn hash_payload(s: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Parses and prepares a `FileDiff` JSON payload, returning a cached `Arc<PreparedDiff>` in 0 ms
/// on `GtkListView` row rebind.
pub fn get_or_prepare_diff(raw_json: &str, is_dark: bool) -> Option<Arc<PreparedDiff>> {
    let key = (raw_json.len(), hash_payload(raw_json), is_dark);
    if let Some(cached) = DIFF_CACHE.lock().get(&key) {
        return Some(cached);
    }

    let payload: DiffPayload = serde_json::from_str(raw_json).ok()?;
    let prepared = Arc::new(prepare_from_payload(payload, is_dark));
    DIFF_CACHE.lock().insert(key, prepared.clone());
    Some(prepared)
}

#[must_use]
pub fn prepare_from_payload(payload: DiffPayload, is_dark: bool) -> PreparedDiff {
    let (dir_prefix, filename) = split_path_display(&payload.path);
    let unified_patch = build_unified_patch(&payload);
    let total_rows = payload.hunks.iter().map(|h| h.rows.len()).sum();

    let ss = syntax_set();
    let ts = theme_set();
    let syntax = if !payload.language.is_empty() && payload.language != "text" {
        ss.find_syntax_by_token(&payload.language)
            .or_else(|| ss.find_syntax_by_extension(&payload.language))
    } else {
        None
    }
    .unwrap_or_else(|| ss.find_syntax_plain_text());

    let theme = if is_dark {
        &ts.themes["base16-ocean.dark"]
    } else {
        &ts.themes["base16-ocean.light"]
    };

    let mut hunks = Vec::with_capacity(payload.hunks.len());
    for hunk in &payload.hunks {
        hunks.push(prepare_hunk(hunk, syntax, theme, ss, is_dark));
    }

    PreparedDiff {
        path: payload.path,
        dir_prefix,
        filename,
        language: payload.language,
        kind: payload.kind,
        stats: payload.stats,
        total_rows,
        unified_patch,
        hunks,
    }
}

fn prepare_hunk(
    hunk: &DiffPayloadHunk,
    syntax: &syntect::parsing::SyntaxReference,
    theme: &syntect::highlighting::Theme,
    ss: &SyntaxSet,
    is_dark: bool,
) -> PreparedHunk {
    let n = hunk.rows.len();
    let mut left_gutter = String::with_capacity(n * 24);
    let mut left_code = String::with_capacity(n * 64);
    let mut left_tones = Vec::with_capacity(n);

    let mut right_gutter = String::with_capacity(n * 24);
    let mut right_code = String::with_capacity(n * 64);
    let mut right_tones = Vec::with_capacity(n);

    let mut unified_gutter = String::with_capacity(n * 32);
    let mut unified_code = String::with_capacity(n * 64);
    let mut unified_tones = Vec::with_capacity(n);

    let mut hl_left = HighlightLines::new(syntax, theme);
    let mut hl_right = HighlightLines::new(syntax, theme);

    for (idx, row) in hunk.rows.iter().enumerate() {
        if idx > 0 {
            left_gutter.push('\n');
            left_code.push('\n');
            right_gutter.push('\n');
            right_code.push('\n');
        }

        // Left cell (Original)
        append_gutter_line(&mut left_gutter, &row.left, is_dark);
        let left_line_markup = render_cell_code(&row.left, &mut hl_left, ss, is_dark);
        left_code.push_str(&left_line_markup);
        left_tones.push(row.left.kind);

        // Right cell (Changes)
        append_gutter_line(&mut right_gutter, &row.right, is_dark);
        let right_line_markup = render_cell_code(&row.right, &mut hl_right, ss, is_dark);
        right_code.push_str(&right_line_markup);
        right_tones.push(row.right.kind);

        // Unified sequence for compact single-column mode
        match (row.left.kind, row.right.kind) {
            (DiffRowTone::Context, _) => {
                if !unified_tones.is_empty() {
                    unified_gutter.push('\n');
                    unified_code.push('\n');
                }
                append_gutter_line(&mut unified_gutter, &row.right, is_dark);
                unified_code.push_str(&right_line_markup);
                unified_tones.push(DiffRowTone::Context);
            }
            (DiffRowTone::Delete, DiffRowTone::Empty) => {
                if !unified_tones.is_empty() {
                    unified_gutter.push('\n');
                    unified_code.push('\n');
                }
                append_gutter_line(&mut unified_gutter, &row.left, is_dark);
                unified_code.push_str(&left_line_markup);
                unified_tones.push(DiffRowTone::Delete);
            }
            (DiffRowTone::Empty, DiffRowTone::Insert) => {
                if !unified_tones.is_empty() {
                    unified_gutter.push('\n');
                    unified_code.push('\n');
                }
                append_gutter_line(&mut unified_gutter, &row.right, is_dark);
                unified_code.push_str(&right_line_markup);
                unified_tones.push(DiffRowTone::Insert);
            }
            (DiffRowTone::Delete, DiffRowTone::Insert) => {
                if !unified_tones.is_empty() {
                    unified_gutter.push('\n');
                    unified_code.push('\n');
                }
                append_gutter_line(&mut unified_gutter, &row.left, is_dark);
                unified_code.push_str(&left_line_markup);
                unified_tones.push(DiffRowTone::Delete);

                unified_gutter.push('\n');
                unified_code.push('\n');
                append_gutter_line(&mut unified_gutter, &row.right, is_dark);
                unified_code.push_str(&right_line_markup);
                unified_tones.push(DiffRowTone::Insert);
            }
            _ => {}
        }
    }

    PreparedHunk {
        header: hunk.header.clone(),
        row_count: n,
        left_gutter_markup: left_gutter,
        left_code_markup: left_code,
        left_tones,
        right_gutter_markup: right_gutter,
        right_code_markup: right_code,
        right_tones,
        unified_gutter_markup: unified_gutter,
        unified_code_markup: unified_code,
        unified_tones,
    }
}

fn append_gutter_line(buf: &mut String, cell: &DiffPayloadCell, is_dark: bool) {
    let (num_fg, sym_fg, sym) = match cell.kind {
        DiffRowTone::Context => (
            if is_dark { "#7a828e" } else { "#6e7781" },
            if is_dark { "#7a828e" } else { "#6e7781" },
            ' ',
        ),
        DiffRowTone::Delete => (
            if is_dark { "#ff8080" } else { "#cf222e" },
            if is_dark { "#ff6b6b" } else { "#cf222e" },
            '-',
        ),
        DiffRowTone::Insert => (
            if is_dark { "#6ee7b7" } else { "#1a7f37" },
            if is_dark { "#4ade80" } else { "#1a7f37" },
            '+',
        ),
        DiffRowTone::Empty => (
            if is_dark { "#484f58" } else { "#afb8c1" },
            if is_dark { "#484f58" } else { "#afb8c1" },
            '·',
        ),
    };

    let num_str = cell
        .line_no
        .map_or_else(|| "    ".to_string(), |n| format!("{n:>4}"));
    buf.push_str(&format!(
        "<span foreground=\"{num_fg}\">{num_str}</span> <span foreground=\"{sym_fg}\" weight=\"bold\">{sym}</span>"
    ));
}

fn render_cell_code(
    cell: &DiffPayloadCell,
    hl: &mut HighlightLines<'_>,
    ss: &SyntaxSet,
    is_dark: bool,
) -> String {
    if cell.kind == DiffRowTone::Empty || cell.spans.is_empty() {
        return " ".to_string();
    }

    let has_emphasis = cell.spans.iter().any(|s| s.emphasized);

    if has_emphasis {
        let (emp_bg, emp_fg, base_fg) = match cell.kind {
            DiffRowTone::Delete => {
                if is_dark {
                    ("#7d242b", "#ffdcd7", "#f0d6d6")
                } else {
                    ("#ff8182", "#4c0004", "#5c161a")
                }
            }
            DiffRowTone::Insert => {
                if is_dark {
                    ("#1d633f", "#d2f8e2", "#d6f0df")
                } else {
                    ("#7ee787", "#043915", "#114620")
                }
            }
            _ => {
                if is_dark {
                    ("#30363d", "#e6edf3", "#c9d1d9")
                } else {
                    ("#d0d7de", "#1f2328", "#24292f")
                }
            }
        };

        let mut out = String::new();
        for span in &cell.spans {
            let escaped = escape_pango(&span.text);
            if span.emphasized {
                out.push_str(&format!(
                    "<span background=\"{emp_bg}\" foreground=\"{emp_fg}\" weight=\"bold\">{escaped}</span>"
                ));
            } else {
                out.push_str(&format!(
                    "<span foreground=\"{base_fg}\">{escaped}</span>"
                ));
            }
        }
        if out.is_empty() {
            out.push(' ');
        }
        return out;
    }

    // Syntax-highlighted path for context lines and whole-line additions/deletions
    let plain = cell.plain_text();
    if plain.is_empty() {
        return " ".to_string();
    }

    if let Ok(ranges) = hl.highlight_line(&plain, ss) {
        let mut out = String::with_capacity(plain.len() * 2);
        for (style, token) in ranges {
            if token.is_empty() {
                continue;
            }
            let c = style.foreground;
            out.push_str(&format!(
                "<span foreground=\"#{:02x}{:02x}{:02x}\">{}</span>",
                c.r,
                c.g,
                c.b,
                escape_pango(token)
            ));
        }
        if out.is_empty() {
            out.push(' ');
        }
        out
    } else {
        escape_pango(&plain)
    }
}

fn split_path_display(path: &str) -> (String, String) {
    let p = Path::new(path);
    let filename = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string();
    let dir_prefix = p
        .parent()
        .and_then(|parent| parent.to_str())
        .filter(|s| !s.is_empty())
        .map_or_else(String::new, |s| format!("{s}/"));
    (dir_prefix, filename)
}

fn build_unified_patch(payload: &DiffPayload) -> String {
    let mut out = String::new();
    out.push_str(&format!("--- a/{}\n+++ b/{}\n", payload.path, payload.path));
    for hunk in &payload.hunks {
        out.push_str(&hunk.header);
        out.push('\n');
        for row in &hunk.rows {
            match (row.left.kind, row.right.kind) {
                (DiffRowTone::Context, _) => {
                    out.push(' ');
                    out.push_str(&row.right.plain_text());
                    out.push('\n');
                }
                (DiffRowTone::Delete, DiffRowTone::Empty) => {
                    out.push('-');
                    out.push_str(&row.left.plain_text());
                    out.push('\n');
                }
                (DiffRowTone::Empty, DiffRowTone::Insert) => {
                    out.push('+');
                    out.push_str(&row.right.plain_text());
                    out.push('\n');
                }
                (DiffRowTone::Delete, DiffRowTone::Insert) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_and_cache_diff_roundtrip() {
        let json = r#"{
            "path": "crates/app/src/main.rs",
            "language": "rs",
            "kind": "Modified",
            "stats": {
                "added_lines": 1,
                "removed_lines": 1,
                "hunk_count": 1,
                "old_total_lines": 5,
                "new_total_lines": 5,
                "truncated": false,
                "omitted_lines": 0
            },
            "hunks": [{
                "header": "@@ -1,2 +1,2 @@",
                "old_start": 1,
                "old_count": 2,
                "new_start": 1,
                "new_count": 2,
                "rows": [{
                    "left": {
                        "line_no": 1,
                        "kind": "Delete",
                        "spans": [{"emphasized": true, "text": "let a = 1;"}]
                    },
                    "right": {
                        "line_no": 1,
                        "kind": "Insert",
                        "spans": [{"emphasized": true, "text": "let a = 2;"}]
                    }
                }]
            }]
        }"#;

        let p1 = get_or_prepare_diff(json, true).expect("should parse");
        let p2 = get_or_prepare_diff(json, true).expect("should hit cache");
        assert!(Arc::ptr_eq(&p1, &p2));
        assert_eq!(p1.dir_prefix, "crates/app/src/");
        assert_eq!(p1.filename, "main.rs");
        assert_eq!(p1.hunks.len(), 1);
        assert!(p1.hunks[0].left_code_markup.contains("let a = 1;"));
        assert!(p1.hunks[0].right_code_markup.contains("let a = 2;"));
    }
}
