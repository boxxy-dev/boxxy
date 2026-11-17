//! `.gitignore`-aware directory walker, glob compiler, and blacklist filter.

use crate::blacklist::{is_entry_lexically_blacklisted, is_path_blacklisted, load_blacklist};
use ignore::WalkBuilder;
use regex::Regex;
use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".flatpak-builder",
    ".direnv",
    "__pycache__",
    ".venv",
    "venv",
    ".next",
    "dist",
    "build",
];

/// Converts a workspace glob pattern (`**/*.rs`, `src/*.{rs,ui}`, `file_[0-9].txt`)
/// into an anchored `Regex`.
///
/// Patterns without `/` match either at the search root or inside any subdirectory.
pub(super) fn glob_to_regex(glob: &str) -> Result<Regex, String> {
    let trimmed = glob.trim();
    if trimmed.is_empty() {
        return Err("Glob pattern must not be empty.".to_string());
    }

    let effective = if trimmed.contains('/') {
        trimmed.trim_start_matches("./").to_string()
    } else {
        format!("**/{trimmed}")
    };

    let mut re = String::with_capacity(effective.len() * 2 + 4);
    re.push('^');

    let chars: Vec<char> = effective.chars().collect();
    let mut i = 0;
    let mut brace_depth = 0usize;

    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    if i + 2 < chars.len() && chars[i + 2] == '/' {
                        re.push_str("(?:.*/)?");
                        i += 3;
                        continue;
                    }
                    re.push_str(".*");
                    i += 2;
                    continue;
                }
                re.push_str("[^/]*");
            }
            '?' => re.push_str("[^/]"),
            '[' => {
                if let Some(close_offset) = chars[i + 1..].iter().position(|&c| c == ']')
                    && close_offset > 0
                {
                    re.push('[');
                    let mut j = i + 1;
                    if chars[j] == '!' {
                        re.push('^');
                        j += 1;
                    }
                    while j <= i + close_offset {
                        let ch = chars[j];
                        if ch == '\\' {
                            re.push_str("\\\\");
                        } else {
                            re.push(ch);
                        }
                        j += 1;
                    }
                    re.push(']');
                    i += close_offset + 2;
                    continue;
                }
                re.push_str("\\[");
            }
            ']' => re.push_str("\\]"),
            '{' => {
                if chars[i + 1..].contains(&'}') {
                    brace_depth += 1;
                    re.push_str("(?:");
                } else {
                    re.push_str("\\{");
                }
            }
            '}' if brace_depth > 0 => {
                brace_depth -= 1;
                re.push(')');
            }
            '}' => re.push_str("\\}"),
            ',' if brace_depth > 0 => {
                re.push('|');
                while i + 1 < chars.len() && chars[i + 1] == ' ' {
                    i += 1;
                }
            }
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '\\' => {
                re.push('\\');
                re.push(chars[i]);
            }
            c => re.push(c),
        }
        i += 1;
    }
    re.push('$');

    Regex::new(&re).map_err(|e| format!("Invalid glob pattern '{glob}': {e}"))
}

/// Computes the normalized slash-separated path of `file` relative to `root`.
/// When `root` itself is a single file, returns `file`'s basename so `include: "*.rs"` works.
#[must_use]
pub(super) fn relative_match_path(root: &Path, root_is_file: bool, file: &Path) -> String {
    if root_is_file {
        return file
            .file_name()
            .map_or_default(|n| n.to_string_lossy().into_owned());
    }
    let rel = file.strip_prefix(root).unwrap_or(file);
    let lossy = rel.to_string_lossy();
    if lossy.contains('\\') {
        lossy.replace('\\', "/")
    } else {
        lossy.into_owned()
    }
}

/// Walks non-ignored, non-blacklisted regular files under `root` in deterministic order.
pub(super) fn walk_candidate_files<F>(root: &Path, mut visit: F) -> Result<bool, String>
where
    F: FnMut(&Path, Option<u64>) -> ControlFlow<()>,
{
    let blacklist: Arc<Vec<String>> = load_blacklist();
    if is_path_blacklisted(root, &blacklist) {
        return Err("Path is blacklisted for security reasons.".to_string());
    }

    let root_meta = std::fs::metadata(root)
        .map_err(|e| format!("Search path '{}' cannot be accessed: {e}", root.display()))?;

    if root_meta.is_file() {
        let _ = visit(root, Some(root_meta.len()));
        return Ok(true);
    }

    let root_owned = root.to_path_buf();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .follow_links(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .sort_by_file_name(OsStr::cmp)
        .filter_entry(move |entry| {
            if entry.path() == root_owned {
                return true;
            }
            if entry.path_is_symlink() {
                return false;
            }
            if let Some(ft) = entry.file_type()
                && ft.is_dir()
            {
                let name = entry.file_name().to_str().unwrap_or("");
                if IGNORED_DIRS.contains(&name) {
                    return false;
                }
            }
            !is_entry_lexically_blacklisted(entry.path(), &blacklist)
        })
        .build();

    for entry_res in walker {
        let Ok(entry) = entry_res else {
            continue;
        };
        if entry.path() == root || entry.path_is_symlink() {
            continue;
        }
        let Some(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_file() {
            continue;
        }
        let size = entry.metadata().ok().map(|m| m.len());
        if visit(entry.path(), size).is_break() {
            break;
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_compiler_supports_braces_brackets_and_literal_commas() {
        let brace_re = glob_to_regex("src/*.{rs, ui}").unwrap();
        assert!(brace_re.is_match("src/main.rs"));
        assert!(brace_re.is_match("src/window.ui"));
        assert!(!brace_re.is_match("src/readme.md"));

        let bracket_re = glob_to_regex("part_[0-9].txt").unwrap();
        assert!(bracket_re.is_match("sub/part_4.txt"));
        assert!(!bracket_re.is_match("sub/part_a.txt"));

        let comma_re = glob_to_regex("data/a,b.csv").unwrap();
        assert!(comma_re.is_match("data/a,b.csv"));
        assert!(!comma_re.is_match("other/b.csv"));
    }
}
