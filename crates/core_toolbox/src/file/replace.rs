//! Surgical text replacement tool (`replace_in_file`).

use super::matcher::{find_match_ranges, uses_crlf_line_endings};
use crate::ApprovalHandler;
use crate::approval::{await_proposal, begin_tool, finish_tool, resolve_live_cwd};
use crate::utils::resolve_path;
use boxxy_claw_protocol::{ClawEnvironment, FileDiff};
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize, Clone)]
pub struct FileEditItem {
    pub old_string: String,
    pub new_string: String,
    pub replace_all: Option<bool>,
}

#[derive(Deserialize)]
pub struct ReplaceInFileArgs {
    pub path: String,
    pub old_string: Option<String>,
    pub new_string: Option<String>,
    pub replace_all: Option<bool>,
    pub edits: Option<Vec<FileEditItem>>,
}

#[derive(Serialize)]
pub struct ReplaceInFileOutput {
    pub success: bool,
    pub replacements: usize,
    pub total_lines: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ReplaceInFileOutput {
    fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            replacements: 0,
            total_lines: 0,
            error: Some(msg.into()),
        }
    }
}

pub struct ReplaceInFileTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

fn collect_edit_items(args: ReplaceInFileArgs) -> Result<(String, Vec<FileEditItem>), String> {
    let raw_path = args.path.trim().to_string();
    if raw_path.is_empty() {
        return Err("`path` must not be empty.".to_string());
    }

    let mut edit_list = args.edits.unwrap_or_default();
    match (args.old_string, args.new_string) {
        (Some(old_s), Some(new_s)) => {
            edit_list.insert(
                0,
                FileEditItem {
                    old_string: old_s,
                    new_string: new_s,
                    replace_all: args.replace_all,
                },
            );
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err("Both `old_string` and `new_string` must be provided together.".to_string());
        }
        (None, None) => {}
    }

    if edit_list.is_empty() {
        return Err(
            "Provide either (`old_string` and `new_string`) or a non-empty `edits` array."
                .to_string(),
        );
    }

    Ok((raw_path, edit_list))
}

fn apply_edits(original: &str, edit_list: &[FileEditItem]) -> Result<(String, usize), String> {
    let uses_crlf = uses_crlf_line_endings(original);
    let mut working = original.to_string();
    let mut total_replacements = 0usize;

    for (idx, edit) in edit_list.iter().enumerate() {
        if edit.old_string.is_empty() {
            return Err(format!("Edit #{}: `old_string` must not be empty.", idx + 1));
        }
        if edit.old_string == edit.new_string {
            return Err(format!(
                "Edit #{}: `old_string` and `new_string` are identical; no changes to apply.",
                idx + 1
            ));
        }

        let target_old = if uses_crlf && !edit.old_string.contains("\r\n") {
            edit.old_string.replace('\n', "\r\n")
        } else {
            edit.old_string.clone()
        };
        let target_new = if uses_crlf && !edit.new_string.contains("\r\n") {
            edit.new_string.replace('\n', "\r\n")
        } else {
            edit.new_string.clone()
        };

        let ranges = find_match_ranges(&working, &target_old);
        let match_count = ranges.len();
        if match_count == 0 {
            return Err(format!(
                "Edit #{} (`{}`): `old_string` was not found in the file. Read the target section with `file_read` first and ensure exact indentation and whitespace.",
                idx + 1,
                edit.old_string.lines().next().unwrap_or("")
            ));
        }

        let replace_all = edit.replace_all.unwrap_or(false);
        if match_count > 1 && !replace_all {
            return Err(format!(
                "`old_string` matched {match_count} locations in the file. Include more surrounding context lines in `old_string` to uniquely identify the target, or set `replace_all: true`."
            ));
        }

        if replace_all {
            for (start, end) in ranges.into_iter().rev() {
                working.replace_range(start..end, &target_new);
            }
            total_replacements += match_count;
        } else {
            let (start, end) = ranges[0];
            working.replace_range(start..end, &target_new);
            total_replacements += 1;
        }
    }

    if working == original {
        return Err("Edit produced no effective changes to the file.".to_string());
    }

    Ok((working, total_replacements))
}

impl Tool for ReplaceInFileTool {
    const NAME: &'static str = "replace_in_file";

    type Error = std::io::Error;
    type Args = ReplaceInFileArgs;
    type Output = ReplaceInFileOutput;

    fn description(&self) -> String {
        "Surgically edit an existing file by replacing exact text segments. Prefer this over `file_write` when modifying existing files. Pass `old_string` and `new_string` for a single change, or pass `edits` to apply multiple non-contiguous changes in one approval prompt.".to_string()
    }

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        rig::completion::ToolAnnotations::default().with_read_only(false)
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact existing text to find and replace (for a single edit)."
                },
                "new_string": {
                    "type": "string",
                    "description": "The new text to replace `old_string` with (for a single edit)."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Optional: Whether to replace all occurrences of `old_string` (default: false, which requires a unique match)."
                },
                "edits": {
                    "type": "array",
                    "description": "Optional batch of multiple edits to apply atomically to the file in one approval prompt.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string" },
                            "new_string": { "type": "string" },
                            "replace_all": { "type": "boolean" }
                        },
                        "required": ["old_string", "new_string"]
                    }
                }
            },
            "required": ["path"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let (raw_path, edit_list) = match collect_edit_items(args) {
            Ok(pair) => pair,
            Err(e) => {
                let out = ReplaceInFileOutput::err(e);
                finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let base_dir = resolve_live_cwd(self.approval.as_ref(), &self.current_dir).await;
        let path = resolve_path(&base_dir, &raw_path);

        let original = match self.env.read_file(path.clone(), 0, 0).await {
            Ok(c) => c,
            Err(e) => {
                let out = ReplaceInFileOutput::err(format!("Failed to read file '{path}': {e}"));
                finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
                return Ok(out);
            }
        };

        if let Some(stale_err) = self
            .approval
            .check_stale_file_before_write(&path, Some(&original))
            .await
        {
            let out = ReplaceInFileOutput::err(stale_err);
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let (working, total_replacements) = match apply_edits(&original, &edit_list) {
            Ok(res) => res,
            Err(e) => {
                let out = ReplaceInFileOutput::err(e);
                finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let path_for_diff = path.clone();
        let (diff, original, working) = match tokio::task::spawn_blocking(move || {
            let d = FileDiff::compute(&path_for_diff, Some(&original), &working);
            (d, original, working)
        })
        .await
        {
            Ok(tuple) => tuple,
            Err(e) => {
                let out = ReplaceInFileOutput::err(format!("Diff computation failed: {e}"));
                finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let outcome = await_proposal(
            self.approval.as_ref(),
            self.approval
                .propose_file_write(path.clone(), working.clone(), diff),
        )
        .await;

        let out = if outcome.is_approved() {
            let total_lines = working.lines().count();
            let new_hash = crate::hash_file_content(&working);
            match self.env.write_file(path.clone(), working).await {
                Ok(()) => {
                    self.approval
                        .record_file_snapshot_before_write(path, Some(original), new_hash)
                        .await;
                    ReplaceInFileOutput {
                        success: true,
                        replacements: total_replacements,
                        total_lines,
                        error: None,
                    }
                }
                Err(e) => ReplaceInFileOutput::err(format!("Environment Error: {e}")),
            }
        } else {
            ReplaceInFileOutput::err(outcome.rejection_message())
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockApproval, MockEnv};

    #[tokio::test]
    async fn replace_in_file_handles_unique_multiple_batch_and_crlf_matches() {
        let mut ctx = ToolContext::default();
        let env = Arc::new(MockEnv::new());
        *env.file_content.lock() =
            "fn a() {\r\n    let x = 1;\r\n    let y = 1;\r\n}\r\n".to_string();

        let approval = Arc::new(MockApproval::approved(Some("/workspace")));
        let tool = ReplaceInFileTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval,
        };

        let multi_err = tool
            .call(
                &mut ctx,
                ReplaceInFileArgs {
                    path: "main.rs".to_string(),
                    old_string: Some("= 1;".to_string()),
                    new_string: Some("= 2;".to_string()),
                    replace_all: Some(false),
                    edits: None,
                },
            )
            .await
            .unwrap();
        assert!(!multi_err.success);
        assert!(multi_err.error.unwrap().contains("matched 2 locations"));

        let single_ok = tool
            .call(
                &mut ctx,
                ReplaceInFileArgs {
                    path: "main.rs".to_string(),
                    old_string: Some("    let x = 1;\n    let y = 1;".to_string()),
                    new_string: Some("    let x = 42;\n    let y = 1;".to_string()),
                    replace_all: None,
                    edits: None,
                },
            )
            .await
            .unwrap();
        assert!(single_ok.success);
        assert_eq!(single_ok.replacements, 1);
        assert_eq!(single_ok.total_lines, 4);
        assert_eq!(
            env.written_content.lock().as_deref(),
            Some("fn a() {\r\n    let x = 42;\r\n    let y = 1;\r\n}\r\n")
        );

        *env.file_content.lock() =
            "fn a() {\r\n    let x = 42;\r\n    let y = 1;\r\n}\r\n".to_string();

        let batch_ok = tool
            .call(
                &mut ctx,
                ReplaceInFileArgs {
                    path: "main.rs".to_string(),
                    old_string: None,
                    new_string: None,
                    replace_all: None,
                    edits: Some(vec![
                        FileEditItem {
                            old_string: "let x = 42;".to_string(),
                            new_string: "let x = 10;".to_string(),
                            replace_all: None,
                        },
                        FileEditItem {
                            old_string: "let y = 1;".to_string(),
                            new_string: "let y = 20;".to_string(),
                            replace_all: None,
                        },
                    ]),
                },
            )
            .await
            .unwrap();
        assert!(batch_ok.success);
        assert_eq!(batch_ok.replacements, 2);
        assert_eq!(
            env.written_content.lock().as_deref(),
            Some("fn a() {\r\n    let x = 10;\r\n    let y = 20;\r\n}\r\n")
        );
    }

    #[tokio::test]
    async fn replace_in_file_handles_tier2_and_tier3_typography_fallback() {
        let mut ctx = ToolContext::default();
        let env = Arc::new(MockEnv::new());
        let fuzzy_disk =
            "fn greet() {   \n    let msg = \u{201C}hello \u{2014} world\u{201D};  \n}\n"
                .to_string();
        *env.file_content.lock() = fuzzy_disk.clone();

        let approval = Arc::new(MockApproval::approved(Some("/workspace")));
        approval
            .record_file_read(
                "/workspace/greet.rs".to_string(),
                crate::hash_file_content(&fuzzy_disk),
            )
            .await;

        let tool = ReplaceInFileTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval,
        };

        let fuzzy_ok = tool
            .call(
                &mut ctx,
                ReplaceInFileArgs {
                    path: "greet.rs".to_string(),
                    old_string: Some(
                        "fn greet() {\n    let msg = \"hello - world\";\n}".to_string(),
                    ),
                    new_string: Some("fn greet() {\n    let msg = \"hi\";\n}".to_string()),
                    replace_all: None,
                    edits: None,
                },
            )
            .await
            .unwrap();
        assert!(fuzzy_ok.success);
        assert_eq!(fuzzy_ok.replacements, 1);
        assert_eq!(
            env.written_content.lock().as_deref(),
            Some("fn greet() {\n    let msg = \"hi\";\n}\n")
        );
    }

    #[tokio::test]
    async fn replace_in_file_detects_stale_disk_edits_and_rejects_noop_edits() {
        let mut ctx = ToolContext::default();
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval::approved(Some("/workspace")));

        approval
            .record_file_read(
                "/workspace/src/parser.rs".to_string(),
                crate::hash_file_content("fn parse() { let a = 1; }\n"),
            )
            .await;
        *env.file_content.lock() =
            "fn parse() { let a = 1; /* external manual change */ }\n".to_string();

        let tool = ReplaceInFileTool {
            env,
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };

        let stale_out = tool
            .call(
                &mut ctx,
                ReplaceInFileArgs {
                    path: "src/parser.rs".to_string(),
                    old_string: Some("let a = 1;".to_string()),
                    new_string: Some("let a = 2;".to_string()),
                    replace_all: None,
                    edits: None,
                },
            )
            .await
            .unwrap();
        assert!(!stale_out.success);
        assert!(stale_out.error.unwrap().contains("modified on disk"));
        assert!(approval.diffs.lock().is_empty());

        let noop_out = tool
            .call(
                &mut ctx,
                ReplaceInFileArgs {
                    path: "src/untracked.rs".to_string(),
                    old_string: Some("same".to_string()),
                    new_string: Some("same".to_string()),
                    replace_all: None,
                    edits: None,
                },
            )
            .await
            .unwrap();
        assert!(!noop_out.success);
    }
}
