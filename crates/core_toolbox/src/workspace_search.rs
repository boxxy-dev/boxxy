use crate::ApprovalHandler;
use crate::utils::resolve_path;
use regex::{Regex, RegexBuilder};
use rig::tool::PortableTool;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_GLOB_RESULTS: usize = 100;
const MAX_GREP_MATCHES: usize = 100;
const MAX_GREP_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LINE_PREVIEW_CHARS: usize = 300;

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

// --- GLOB SEARCH ---

#[derive(Deserialize)]
pub struct GlobSearchArgs {
    pub pattern: String,
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct GlobSearchOutput {
    pub matches: Vec<String>,
    pub total_found: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

pub struct GlobSearchTool {
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl PortableTool for GlobSearchTool {
    const NAME: &'static str = "glob_search";

    type Error = std::io::Error;
    type Args = GlobSearchArgs;
    type Output = GlobSearchOutput;

    fn description(&self) -> String {
        "Find files by glob pattern (e.g. '**/*.rs', 'src/**/*.ui', '**/Cargo.toml') starting from `path` (default: current directory). Automatically skips build/VCS directories (.git, target, node_modules). Use this instead of chaining multiple `list_directory` calls or running `find`.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match relative file paths (e.g. '**/*.rs', '**/mod.rs', '*.toml')."
                },
                "path": {
                    "type": "string",
                    "description": "Optional base directory to search in (defaults to current working directory)."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let base_dir = self
            .approval
            .current_dir()
            .await
            .unwrap_or_else(|| self.current_dir.clone());
        let root_str = resolve_path(&base_dir, args.pattern_base(&args.path));
        let pattern = args.pattern.clone();

        let output = tokio::task::spawn_blocking(move || run_glob_search(&root_str, &pattern))
            .await
            .map_err(|e| std::io::Error::other(format!("Glob search task panicked: {e}")))??;

        self.approval
            .report_tool_result(
                Self::NAME.to_string(),
                serde_json::to_string(&output).unwrap_or_default(),
            )
            .await;

        Ok(output)
    }
}

impl GlobSearchArgs {
    fn pattern_base<'a>(&self, path: &'a Option<String>) -> &'a str {
        path.as_deref().unwrap_or("")
    }
}

// --- GREP SEARCH ---

#[derive(Deserialize)]
pub struct GrepSearchArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub include: Option<String>,
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
}

pub struct GrepSearchTool {
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl PortableTool for GrepSearchTool {
    const NAME: &'static str = "grep_search";

    type Error = std::io::Error;
    type Args = GrepSearchArgs;
    type Output = GrepSearchOutput;

    fn description(&self) -> String {
        "Search file contents using a regular expression across a file or directory (default: current directory). Automatically skips binary files and build/VCS directories (.git, target, node_modules). Returns matching file paths, 1-based line numbers, and line previews (capped at 100 matches). PREFER this tool over running `grep` or `rg` via shell tools.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for in file contents."
                },
                "path": {
                    "type": "string",
                    "description": "Optional file or directory path to search within (defaults to current working directory)."
                },
                "include": {
                    "type": "string",
                    "description": "Optional file glob filter (e.g. '*.rs', '*.toml', '*.ui')."
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Optional: Whether the regex search is case-sensitive (default: true)."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let base_dir = self
            .approval
            .current_dir()
            .await
            .unwrap_or_else(|| self.current_dir.clone());
        let target_str = resolve_path(&base_dir, args.path.as_deref().unwrap_or(""));
        let pattern = args.pattern;
        let include = args.include;
        let case_sensitive = args.case_sensitive.unwrap_or(true);

        let output = tokio::task::spawn_blocking(move || {
            run_grep_search(&target_str, &pattern, include.as_deref(), case_sensitive)
        })
        .await
        .map_err(|e| std::io::Error::other(format!("Grep search task panicked: {e}")))??;

        self.approval
            .report_tool_result(
                Self::NAME.to_string(),
                serde_json::to_string(&output).unwrap_or_default(),
            )
            .await;

        Ok(output)
    }
}

fn load_blacklist() -> Vec<String> {
    let mut blacklisted = vec![
        "/etc/shadow".to_string(),
        "/etc/gshadow".to_string(),
        "/root/.ssh".to_string(),
        ".ssh/id_rsa".to_string(),
        ".ssh/id_ed25519".to_string(),
    ];

    let blacklist_path = boxxy_sys_utils::get_config_dir()
        .join("boxxyclaw")
        .join("BLACKLIST.md");
    if let Ok(content) = std::fs::read_to_string(&blacklist_path) {
        let custom: Vec<String> = content
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(ToString::to_string)
            .collect();
        if !custom.is_empty() {
            blacklisted = custom;
        }
    }
    blacklisted
}

fn is_blacklisted(path: &Path, blacklist: &[String]) -> bool {
    let s = path.to_string_lossy();
    blacklist.iter().any(|b| s.contains(b))
}

/// Converts a glob pattern (`**/*.rs`, `src/*.toml`, `*. {rs,ui}`) into a compiled `Regex`.
pub(crate) fn glob_to_regex(glob: &str) -> Result<Regex, std::io::Error> {
    let trimmed = glob.trim();
    // If pattern has no path separator (e.g. "*.rs" or "Cargo.toml"), match either at root or in any subdirectory
    let effective = if !trimmed.contains('/') {
        format!("**/{trimmed}")
    } else {
        trimmed.trim_start_matches("./").to_string()
    };

    let mut re = String::from("^");
    let chars: Vec<char> = effective.chars().collect();
    let mut i = 0;

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
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '[' | ']' | '\\' => {
                re.push('\\');
                re.push(chars[i]);
            }
            '{' => re.push_str("(?:"),
            '}' => re.push(')'),
            ',' => re.push('|'),
            c => re.push(c),
        }
        i += 1;
    }
    re.push('$');

    Regex::new(&re).map_err(|e| std::io::Error::other(format!("Invalid glob pattern '{glob}': {e}")))
}

fn collect_candidate_files(root: &Path, blacklist: &[String], out: &mut Vec<PathBuf>) {
    if is_blacklisted(root, blacklist) {
        return;
    }
    if root.is_file() {
        out.push(root.to_path_buf());
        return;
    }

    let Ok(read_dir) = std::fs::read_dir(root) else {
        return;
    };

    let mut entries: Vec<PathBuf> = read_dir
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();

    for path in entries {
        if is_blacklisted(&path, blacklist) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if IGNORED_DIRS.contains(&dir_name) {
                continue;
            }
            collect_candidate_files(&path, blacklist, out);
        } else if meta.is_file() {
            out.push(path);
        }
    }
}

fn run_glob_search(root_str: &str, pattern: &str) -> Result<GlobSearchOutput, std::io::Error> {
    let root = Path::new(if root_str.is_empty() { "." } else { root_str });
    let blacklist = load_blacklist();
    if is_blacklisted(root, &blacklist) {
        return Err(std::io::Error::other(
            "Path is blacklisted for security reasons",
        ));
    }

    let matcher = glob_to_regex(pattern)?;
    let mut files = Vec::new();
    collect_candidate_files(root, &blacklist, &mut files);

    let mut matches = Vec::new();
    let mut total_found = 0;

    for file in files {
        let rel = file.strip_prefix(root).unwrap_or(&file);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if matcher.is_match(&rel_str) {
            total_found += 1;
            if matches.len() < MAX_GLOB_RESULTS {
                matches.push(file.to_string_lossy().to_string());
            }
        }
    }

    Ok(GlobSearchOutput {
        truncated: total_found > MAX_GLOB_RESULTS,
        matches,
        total_found,
    })
}

fn run_grep_search(
    target_str: &str,
    pattern: &str,
    include: Option<&str>,
    case_sensitive: bool,
) -> Result<GrepSearchOutput, std::io::Error> {
    let root = Path::new(if target_str.is_empty() { "." } else { target_str });
    let blacklist = load_blacklist();
    if is_blacklisted(root, &blacklist) {
        return Err(std::io::Error::other(
            "Path is blacklisted for security reasons",
        ));
    }

    let regex = RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|e| std::io::Error::other(format!("Invalid regex pattern '{pattern}': {e}")))?;

    let include_matcher = match include.map(str::trim).filter(|s| !s.is_empty()) {
        Some(glob) => Some(glob_to_regex(glob)?),
        None => None,
    };

    let mut files = Vec::new();
    collect_candidate_files(root, &blacklist, &mut files);

    let mut matches = Vec::new();
    let mut total_found = 0;

    for file in files {
        if let Some(ref inc) = include_matcher {
            let rel = file.strip_prefix(root).unwrap_or(&file);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if !inc.is_match(&rel_str) {
                continue;
            }
        }

        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        if meta.len() > MAX_GREP_FILE_BYTES {
            continue;
        }

        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        let probe_len = bytes.len().min(8192);
        if bytes[..probe_len].contains(&0) {
            continue;
        }

        let Ok(content) = std::str::from_utf8(&bytes) else {
            continue;
        };

        let file_path_str = file.to_string_lossy().to_string();
        for (idx, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                total_found += 1;
                if matches.len() < MAX_GREP_MATCHES {
                    let preview = if line.len() > MAX_LINE_PREVIEW_CHARS {
                        let cut = line.floor_char_boundary(MAX_LINE_PREVIEW_CHARS);
                        format!("{}...", &line[..cut])
                    } else {
                        line.to_string()
                    };
                    matches.push(GrepMatch {
                        path: file_path_str.clone(),
                        line_number: idx + 1,
                        line_text: preview,
                    });
                }
            }
        }
    }

    Ok(GrepSearchOutput {
        truncated: total_found > MAX_GREP_MATCHES,
        matches,
        total_found,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopApproval {
        cwd: String,
    }

    #[async_trait::async_trait]
    impl ApprovalHandler for NoopApproval {
        async fn propose_file_write(
            &self,
            _: String,
            _: String,
            _: crate::FileDiff,
        ) -> bool {
            true
        }
        async fn propose_file_delete(&self, _: String) -> bool {
            true
        }
        async fn propose_background_command(&self, _: String, _: String) -> bool {
            true
        }
        async fn propose_kill_process(&self, _: u32, _: String) -> bool {
            true
        }
        async fn propose_get_clipboard(&self) -> bool {
            true
        }
        async fn propose_set_clipboard(&self, _: String) -> bool {
            true
        }
        async fn report_tool_result(&self, _: String, _: String) {}
        async fn report_tool_started(&self, _: String) {}
        async fn set_thinking(&self, _: bool) {}
        async fn current_dir(&self) -> Option<String> {
            Some(self.cwd.clone())
        }
    }

    #[tokio::test]
    async fn glob_and_grep_search_work_and_skip_ignored_and_binary_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();

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
        // Should be ignored (inside target/ and .git/)
        std::fs::write(root.join("target/debug/ignored.rs"), "compute_magic_value").unwrap();
        std::fs::write(root.join(".git/config.rs"), "compute_magic_value").unwrap();
        // Binary file with NUL byte should be skipped by grep_search
        std::fs::write(root.join("src/binary.dat"), b"compute_magic_value\0binary").unwrap();

        let approval = Arc::new(NoopApproval {
            cwd: root.to_string_lossy().to_string(),
        });

        let glob_tool = GlobSearchTool {
            current_dir: String::new(),
            approval: approval.clone(),
        };

        let rs_files = glob_tool
            .call(GlobSearchArgs {
                pattern: "**/*.rs".to_string(),
                path: None,
            })
            .await
            .unwrap();
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
            .call(GrepSearchArgs {
                pattern: "magic".to_string(),
                path: None,
                include: Some("*.rs".to_string()),
                case_sensitive: Some(false),
            })
            .await
            .unwrap();

        assert_eq!(grep_out.total_found, 2);
        assert_eq!(grep_out.matches.len(), 2);
        assert!(grep_out.matches.iter().any(|m| m.line_number == 1 && m.line_text.contains("compute_magic_value")));
        assert!(grep_out.matches.iter().any(|m| m.line_number == 1 && m.line_text.contains("MAGIC_TOKEN")));
    }
}
