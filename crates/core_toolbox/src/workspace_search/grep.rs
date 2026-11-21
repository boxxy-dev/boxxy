//! Fast workspace content search tool (`grep_search`) with SIMD whole-file prefiltering,
//! reusable read buffers, 8 KiB binary header probing, and literal syntax fallback.

use super::walker::{glob_to_regex, relative_match_path, walk_candidate_files};
use crate::ApprovalHandler;
use crate::approval::{begin_tool, finish_tool, resolve_live_cwd};
use crate::utils::resolve_dir_path;
use regex::{Regex, RegexBuilder};
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;

const MAX_GREP_MATCHES: usize = 100;
const MAX_GREP_FILE_BYTES: u64 = 2 * 1024 * 1024;
const BINARY_PROBE_BYTES: u64 = 8 * 1024;
const MAX_LINE_PREVIEW_CHARS: usize = 300;
const REGEX_SIZE_LIMIT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Deserialize)]
pub struct GrepSearchArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub include: Option<String>,
    pub literal: Option<bool>,
    pub case_sensitive: Option<bool>,
}

#[derive(Serialize)]
pub struct GrepMatch {
    pub path: String,
    pub line_number: usize,
    pub line_text: String,
}

#[derive(Serialize)]
pub struct GrepSearchOutput {
    pub matches: Vec<GrepMatch>,
    pub total_found: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl GrepSearchOutput {
    fn err(msg: impl Into<String>) -> Self {
        Self {
            matches: Vec::new(),
            total_found: 0,
            truncated: false,
            error: Some(msg.into()),
        }
    }
}

pub struct GrepSearchTool {
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for GrepSearchTool {
    const NAME: &'static str = "grep_search";

    type Error = std::io::Error;
    type Args = GrepSearchArgs;
    type Output = GrepSearchOutput;

    fn description(&self) -> String {
        "Search file contents using a regular expression or literal text across a file or directory (default: current directory). Automatically respects `.gitignore` and skips binary files and build/VCS directories (.git, target, node_modules). Returns matching file paths, 1-based line numbers, and line previews (capped at 100 matches). Prefer this tool over running `grep` or `rg` via shell tools.".to_string()
    }

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        rig::completion::ToolAnnotations::default()
            .with_read_only(true)
            .with_open_world(false)
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression or literal text to search for in file contents."
                },
                "path": {
                    "type": "string",
                    "description": "Optional file or directory path to search within (defaults to current working directory)."
                },
                "include": {
                    "type": "string",
                    "description": "Optional file glob filter (e.g. '*.rs', '*.toml', '*.ui')."
                },
                "literal": {
                    "type": "boolean",
                    "description": "Optional: Treat `pattern` as exact literal text instead of a regular expression (default: false)."
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Optional: Whether the search is case-sensitive (default: true)."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let base_dir = resolve_live_cwd(self.approval.as_ref(), &self.current_dir).await;
        let target_str = resolve_dir_path(&base_dir, args.path.as_deref());
        let pattern = args.pattern;
        let include = args.include;
        let literal = args.literal.unwrap_or(false);
        let case_sensitive = args.case_sensitive.unwrap_or(true);

        let output = match tokio::task::spawn_blocking(move || {
            run_grep_search(
                &target_str,
                &pattern,
                include.as_deref(),
                literal,
                case_sensitive,
            )
        })
        .await
        {
            Ok(out) => out,
            Err(e) => GrepSearchOutput::err(format!("Grep search task panicked: {e}")),
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &output).await;
        Ok(output)
    }
}

fn compile_grep_regex(
    pattern: &str,
    literal: bool,
    case_sensitive: bool,
) -> Result<(Regex, bool), String> {
    let effective = if literal {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    };

    let build_re = |pat: &str| {
        RegexBuilder::new(pat)
            .case_insensitive(!case_sensitive)
            .multi_line(true)
            .crlf(true)
            .size_limit(REGEX_SIZE_LIMIT_BYTES)
            .build()
    };

    let (regex, used_fallback) = match build_re(&effective) {
        Ok(re) => (re, false),
        Err(_) if !literal => match build_re(&regex::escape(pattern)) {
            Ok(re) => (re, true),
            Err(e) => return Err(format!("Invalid regex pattern '{pattern}': {e}")),
        },
        Err(e) => return Err(format!("Invalid regex pattern '{pattern}': {e}")),
    };

    let can_fast_reject = literal
        || used_fallback
        || (!effective.contains(r"\A") && !effective.contains(r"\z") && !effective.contains(r"\Z"));

    Ok((regex, can_fast_reject))
}

fn scan_file_for_matches(
    file: &Path,
    known_size: Option<u64>,
    regex: &Regex,
    can_fast_reject: bool,
    scratch: &mut Vec<u8>,
    matches: &mut Vec<GrepMatch>,
    total_found: &mut usize,
) {
    if known_size.is_some_and(|len| len > MAX_GREP_FILE_BYTES) {
        return;
    }

    let Ok(mut f) = std::fs::File::open(file) else {
        return;
    };

    let file_len = match known_size {
        Some(len) => len,
        None => match f.metadata() {
            Ok(m) if m.len() <= MAX_GREP_FILE_BYTES => m.len(),
            _ => return,
        },
    };

    scratch.clear();
    let probe_cap = file_len.min(BINARY_PROBE_BYTES);
    if (&mut f).take(probe_cap).read_to_end(scratch).is_err() || scratch.contains(&0) {
        return;
    }

    if file_len > u64::try_from(scratch.len()).unwrap_or(u64::MAX) && f.read_to_end(scratch).is_err()
    {
        return;
    }

    let Ok(content) = std::str::from_utf8(scratch) else {
        return;
    };

    if can_fast_reject && !regex.is_match(content) {
        return;
    }

    let mut file_path_cached: Option<String> = None;
    for (idx, line) in content.lines().enumerate() {
        if regex.is_match(line) {
            *total_found += 1;
            if matches.len() < MAX_GREP_MATCHES {
                let preview = if line.len() > MAX_LINE_PREVIEW_CHARS {
                    let cut = line.floor_char_boundary(MAX_LINE_PREVIEW_CHARS);
                    format!("{}...", &line[..cut])
                } else {
                    line.to_string()
                };
                let path_str = file_path_cached
                    .get_or_insert_with(|| file.to_string_lossy().into_owned())
                    .clone();
                matches.push(GrepMatch {
                    path: path_str,
                    line_number: idx + 1,
                    line_text: preview,
                });
            }
        }
    }
}

fn run_grep_search(
    target_str: &str,
    pattern: &str,
    include: Option<&str>,
    literal: bool,
    case_sensitive: bool,
) -> GrepSearchOutput {
    if pattern.is_empty() {
        return GrepSearchOutput::err("`pattern` must not be empty.");
    }

    let (regex, can_fast_reject) = match compile_grep_regex(pattern, literal, case_sensitive) {
        Ok(pair) => pair,
        Err(e) => return GrepSearchOutput::err(e),
    };

    let include_re = match include.map(str::trim).filter(|s| !s.is_empty()) {
        Some(glob) => match glob_to_regex(glob) {
            Ok(re) => Some(re),
            Err(e) => return GrepSearchOutput::err(e),
        },
        None => None,
    };

    let root = Path::new(if target_str.is_empty() { "." } else { target_str });
    let root_is_file = std::fs::metadata(root).is_ok_and(|m| m.is_file());

    let mut found_matches = Vec::new();
    let mut total_found = 0usize;
    let mut scratch = Vec::with_capacity(64 * 1024);

    if let Err(e) = walk_candidate_files(root, |file, known_size| {
        if let Some(ref inc) = include_re {
            let rel_str = relative_match_path(root, root_is_file, file);
            if !inc.is_match(&rel_str) {
                return ControlFlow::Continue(());
            }
        }
        scan_file_for_matches(
            file,
            known_size,
            &regex,
            can_fast_reject,
            &mut scratch,
            &mut found_matches,
            &mut total_found,
        );
        ControlFlow::Continue(())
    }) {
        return GrepSearchOutput::err(e);
    }

    GrepSearchOutput {
        truncated: total_found > MAX_GREP_MATCHES,
        matches: found_matches,
        total_found,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockApproval;
    use crate::workspace_search::{GlobSearchArgs, GlobSearchTool};

    #[tokio::test]
    async fn glob_and_grep_search_work_and_skip_ignored_and_binary_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("custom_out")).unwrap();

        std::fs::write(root.join(".gitignore"), "custom_out/\n").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn compute_magic_value() -> u32 {\n    42\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/nested/mod.rs"),
            "pub const MAGIC_TOKEN: &str = \"boxxy\";\n",
        )
        .unwrap();
        std::fs::write(root.join("target/debug/ignored.rs"), "compute_magic_value").unwrap();
        std::fs::write(root.join(".git/config.rs"), "compute_magic_value").unwrap();
        std::fs::write(root.join("custom_out/gen.rs"), "compute_magic_value").unwrap();
        std::fs::write(root.join("src/binary.dat"), b"compute_magic_value\0binary").unwrap();

        let root_str = root.to_string_lossy().into_owned();
        let approval = Arc::new(MockApproval::approved(Some(&root_str)));

        let glob_tool = GlobSearchTool {
            current_dir: String::new(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        let rs_files = glob_tool
            .call(
                &mut ctx,
                GlobSearchArgs {
                    pattern: "**/*.rs".to_string(),
                    path: None,
                },
            )
            .await
            .unwrap();
        assert!(rs_files.error.is_none());
        assert_eq!(rs_files.total_found, 2);
        assert!(rs_files.matches.iter().any(|p| p.ends_with("src/lib.rs")));
        assert!(
            rs_files
                .matches
                .iter()
                .any(|p| p.ends_with("src/nested/mod.rs"))
        );

        let grep_tool = GrepSearchTool {
            current_dir: String::new(),
            approval,
        };

        let grep_out = grep_tool
            .call(
                &mut ctx,
                GrepSearchArgs {
                    pattern: "magic".to_string(),
                    path: None,
                    include: Some("*.rs".to_string()),
                    literal: None,
                    case_sensitive: Some(false),
                },
            )
            .await
            .unwrap();

        assert!(grep_out.error.is_none());
        assert_eq!(grep_out.total_found, 2);
        assert_eq!(grep_out.matches.len(), 2);

        let single_file_out = grep_tool
            .call(
                &mut ctx,
                GrepSearchArgs {
                    pattern: "compute_magic_value(".to_string(),
                    path: Some("src/lib.rs".to_string()),
                    include: Some("*.rs".to_string()),
                    literal: None,
                    case_sensitive: Some(true),
                },
            )
            .await
            .unwrap();
        assert_eq!(single_file_out.total_found, 1);

        let missing_out = grep_tool
            .call(
                &mut ctx,
                GrepSearchArgs {
                    pattern: "magic".to_string(),
                    path: Some("does_not_exist".to_string()),
                    include: None,
                    literal: None,
                    case_sensitive: None,
                },
            )
            .await
            .unwrap();
        assert!(missing_out.error.is_some());
    }
}
