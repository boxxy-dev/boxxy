//! Workspace file-path glob search tool (`glob_search`).

use super::walker::{glob_to_regex, relative_match_path, walk_candidate_files};
use crate::ApprovalHandler;
use crate::approval::{begin_tool, finish_tool, resolve_live_cwd};
use crate::utils::resolve_dir_path;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;

const MAX_GLOB_RESULTS: usize = 100;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl GlobSearchOutput {
    fn err(msg: impl Into<String>) -> Self {
        Self {
            matches: Vec::new(),
            total_found: 0,
            truncated: false,
            error: Some(msg.into()),
        }
    }
}

pub struct GlobSearchTool {
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for GlobSearchTool {
    const NAME: &'static str = "glob_search";

    type Error = std::io::Error;
    type Args = GlobSearchArgs;
    type Output = GlobSearchOutput;

    fn description(&self) -> String {
        "Find files by glob pattern (e.g. '**/*.rs', 'src/**/*.ui', '**/Cargo.toml') starting from `path` (default: current directory). Automatically respects `.gitignore` and skips build/VCS directories (.git, target, node_modules). Use this instead of chaining multiple `list_directory` calls or running `find`.".to_string()
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

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        rig::completion::ToolAnnotations::default()
            .with_read_only(true)
            .with_open_world(false)
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let base_dir = resolve_live_cwd(self.approval.as_ref(), &self.current_dir).await;
        let root_str = resolve_dir_path(&base_dir, args.path.as_deref());
        let pattern = args.pattern;

        let output = match tokio::task::spawn_blocking(move || run_glob_search(&root_str, &pattern))
            .await
        {
            Ok(out) => out,
            Err(e) => GlobSearchOutput::err(format!("Glob search task panicked: {e}")),
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &output).await;
        Ok(output)
    }
}

fn run_glob_search(root_str: &str, pattern: &str) -> GlobSearchOutput {
    let glob_re = match glob_to_regex(pattern) {
        Ok(re) => re,
        Err(e) => return GlobSearchOutput::err(e),
    };

    let root = Path::new(if root_str.is_empty() { "." } else { root_str });
    let root_is_file = std::fs::metadata(root).is_ok_and(|m| m.is_file());

    let mut matched_paths = Vec::new();
    let mut total_found = 0usize;

    if let Err(e) = walk_candidate_files(root, |file, _size| {
        let rel_str = relative_match_path(root, root_is_file, file);
        if glob_re.is_match(&rel_str) {
            total_found += 1;
            if matched_paths.len() < MAX_GLOB_RESULTS {
                matched_paths.push(file.to_string_lossy().into_owned());
            }
        }
        ControlFlow::Continue(())
    }) {
        return GlobSearchOutput::err(e);
    }

    GlobSearchOutput {
        truncated: total_found > MAX_GLOB_RESULTS,
        matches: matched_paths,
        total_found,
        error: None,
    }
}
