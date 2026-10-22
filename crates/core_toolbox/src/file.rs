use crate::ApprovalHandler;
use crate::utils::resolve_path;
use boxxy_claw_protocol::{ClawEnvironment, FileDiff};
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// --- FILE READ ---

#[derive(Deserialize)]
pub struct FileReadArgs {
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

#[derive(Serialize)]
pub struct FileReadOutput {
    pub content: String,
}

pub struct FileReadTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for FileReadTool {
    const NAME: &'static str = "file_read";

    type Error = std::io::Error;
    type Args = FileReadArgs;
    type Output = FileReadOutput;

    fn description(&self) -> String {
        "Read the contents of a file on the host system. Use this to inspect code, configuration, or logs before modifying them. By default reads up to 500 lines starting at line 1; pass `start_line` and `end_line` to inspect specific sections of large files.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to read."
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional: The 1-based line number to start reading from (inclusive, default: 1)."
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional: The 1-based line number to end reading at (inclusive, default: start_line + 499)."
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
        const DEFAULT_MAX_LINES: u32 = 500;
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let base_dir = self
            .approval
            .current_dir()
            .await
            .unwrap_or_else(|| self.current_dir.clone());
        let path = resolve_path(&base_dir, &args.path);
        let start_line = args.start_line.unwrap_or(1).max(1);
        let end_line = args
            .end_line
            .unwrap_or_else(|| start_line.saturating_add(DEFAULT_MAX_LINES - 1));
        match self.env.read_file(path, start_line, end_line).await {
            Ok(content) => {
                let out = FileReadOutput { content };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&out).unwrap_or_default(),
                    )
                    .await;
                Ok(out)
            }
            Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
        }
    }
}

// --- LIST DIRECTORY ---

#[derive(Deserialize)]
pub struct ListDirectoryArgs {
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Serialize)]
pub struct ListDirectoryOutput {
    pub entries: Vec<DirectoryEntry>,
    pub total_entries: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

pub struct ListDirectoryTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for ListDirectoryTool {
    const NAME: &'static str = "list_directory";

    type Error = std::io::Error;
    type Args = ListDirectoryArgs;
    type Output = ListDirectoryOutput;

    fn description(&self) -> String {
        "List the contents of a directory on the host system (sorted with directories first, capped at 250 entries). Use this to discover files and subdirectories.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list (default: current directory)."
                }
            }
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        const MAX_DIR_ENTRIES: usize = 250;
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let base_dir = self
            .approval
            .current_dir()
            .await
            .unwrap_or_else(|| self.current_dir.clone());
        let path = resolve_path(&base_dir, &args.path.unwrap_or_default());
        match self.env.list_directory(path).await {
            Ok(mut entries) => {
                entries.sort_by(|a, b| {
                    b.1.cmp(&a.1)
                        .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
                });
                let total_entries = entries.len();
                let truncated = total_entries > MAX_DIR_ENTRIES;
                let output = ListDirectoryOutput {
                    entries: entries
                        .into_iter()
                        .take(MAX_DIR_ENTRIES)
                        .map(|(name, is_dir, size)| DirectoryEntry { name, is_dir, size })
                        .collect(),
                    total_entries,
                    truncated,
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&output).unwrap_or_default(),
                    )
                    .await;
                Ok(output)
            }
            Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
        }
    }
}

// --- FILE WRITE ---

#[derive(Deserialize)]
pub struct FileWriteArgs {
    pub path: String,
    pub content: String,
}

#[derive(Serialize)]
pub struct FileWriteOutput {
    pub success: bool,
    pub total_lines: usize,
    /// Explicit error feedback for the LLM. When the user rejects the write,
    /// we pass a clear message so the LLM knows not to retry infinitely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct FileWriteTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for FileWriteTool {
    const NAME: &'static str = "file_write";

    type Error = std::io::Error;
    type Args = FileWriteArgs;
    type Output = FileWriteOutput;

    fn description(&self) -> String {
        "Write content to a file on the host system. This will overwrite any existing content. Always prompts the user for approval.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to write."
                },
                "content": {
                    "type": "string",
                    "description": "The new content for the file."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let base_dir = self
            .approval
            .current_dir()
            .await
            .unwrap_or_else(|| self.current_dir.clone());
        let path = resolve_path(&base_dir, &args.path);

        let existing_res = self.env.read_file(path.clone(), 0, 0).await;
        let path_for_diff = path.clone();
        let content_for_diff = args.content.clone();
        let diff = tokio::task::spawn_blocking(move || match existing_res {
            Ok(existing) => FileDiff::compute(&path_for_diff, Some(&existing), &content_for_diff),
            Err(e) if e.to_string().contains("appears to be binary") => {
                FileDiff::binary(&path_for_diff)
            }
            Err(_) => FileDiff::compute(&path_for_diff, None, &content_for_diff),
        })
        .await
        .unwrap_or_else(|_| FileDiff::fallback_new(&path, &args.content));

        self.approval.set_thinking(false).await;
        let approved = self
            .approval
            .propose_file_write(path.clone(), args.content.clone(), diff)
            .await;
        self.approval.set_thinking(true).await;

        if approved {
            let total_lines = args.content.lines().count();
            match self.env.write_file(path, args.content).await {
                Ok(()) => {
                    let out = FileWriteOutput {
                        success: true,
                        total_lines,
                        error: None,
                    };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            let out = FileWriteOutput {
                success: false,
                total_lines: 0,
                error: Some("[USER_EXPLICIT_REJECT]".to_string()),
            };
            self.approval
                .report_tool_result(
                    Self::NAME.to_string(),
                    serde_json::to_string(&out).unwrap_or_default(),
                )
                .await;
            Ok(out)
        }
    }
}

// --- REPLACE IN FILE ---

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

pub struct ReplaceInFileTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for ReplaceInFileTool {
    const NAME: &'static str = "replace_in_file";

    type Error = std::io::Error;
    type Args = ReplaceInFileArgs;
    type Output = ReplaceInFileOutput;

    fn description(&self) -> String {
        "Surgically edit an existing file by replacing exact text segments. PREFER this tool over `file_write` whenever modifying existing files — it is much faster and avoids rewriting the entire file. For a single change, pass `old_string` and `new_string`. For multiple non-contiguous changes in the same file, pass all of them in `edits` so the user only gets ONE approval prompt. Returns `replacements` and `total_lines`.".to_string()
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
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let mut edit_list = args.edits.unwrap_or_default();
        if let (Some(old_s), Some(new_s)) = (args.old_string, args.new_string) {
            edit_list.insert(
                0,
                FileEditItem {
                    old_string: old_s,
                    new_string: new_s,
                    replace_all: args.replace_all,
                },
            );
        }

        if edit_list.is_empty() {
            let out = ReplaceInFileOutput {
                success: false,
                replacements: 0,
                total_lines: 0,
                error: Some(
                    "Provide either (`old_string` and `new_string`) or a non-empty `edits` array."
                        .to_string(),
                ),
            };
            self.approval
                .report_tool_result(
                    Self::NAME.to_string(),
                    serde_json::to_string(&out).unwrap_or_default(),
                )
                .await;
            return Ok(out);
        }

        let base_dir = self
            .approval
            .current_dir()
            .await
            .unwrap_or_else(|| self.current_dir.clone());
        let path = resolve_path(&base_dir, &args.path);

        let original = match self.env.read_file(path.clone(), 0, 0).await {
            Ok(c) => c,
            Err(e) => {
                let out = ReplaceInFileOutput {
                    success: false,
                    replacements: 0,
                    total_lines: 0,
                    error: Some(format!("Failed to read file '{path}': {e}")),
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&out).unwrap_or_default(),
                    )
                    .await;
                return Ok(out);
            }
        };

        let uses_crlf = original.contains("\r\n");
        let original_for_diff = original.clone();
        let mut working = original;
        let mut total_replacements = 0;

        for (idx, edit) in edit_list.iter().enumerate() {
            if edit.old_string.is_empty() {
                let out = ReplaceInFileOutput {
                    success: false,
                    replacements: 0,
                    total_lines: 0,
                    error: Some(format!("Edit #{}: `old_string` must not be empty.", idx + 1)),
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&out).unwrap_or_default(),
                    )
                    .await;
                return Ok(out);
            }
            if edit.old_string == edit.new_string {
                continue;
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

            let match_count = working.matches(&target_old).count();
            if match_count == 0 {
                let out = ReplaceInFileOutput {
                    success: false,
                    replacements: 0,
                    total_lines: 0,
                    error: Some(format!(
                        "Edit #{} (`{}`): `old_string` was not found in the file. Read the target section with `file_read` first and ensure exact indentation and whitespace.",
                        idx + 1,
                        edit.old_string.lines().next().unwrap_or("")
                    )),
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&out).unwrap_or_default(),
                    )
                    .await;
                return Ok(out);
            }

            let replace_all = edit.replace_all.unwrap_or(false);
            if match_count > 1 && !replace_all {
                let out = ReplaceInFileOutput {
                    success: false,
                    replacements: 0,
                    total_lines: 0,
                    error: Some(format!(
                        "`old_string` matched {match_count} locations in the file. Include more surrounding context lines in `old_string` to uniquely identify the target, or set `replace_all: true`."
                    )),
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&out).unwrap_or_default(),
                    )
                    .await;
                return Ok(out);
            }

            if replace_all {
                working = working.replace(&target_old, &target_new);
                total_replacements += match_count;
            } else {
                working = working.replacen(&target_old, &target_new, 1);
                total_replacements += 1;
            }
        }

        let total_lines = working.lines().count();
        let path_for_diff = path.clone();
        let working_for_diff = working.clone();
        let diff = tokio::task::spawn_blocking(move || {
            FileDiff::compute(&path_for_diff, Some(&original_for_diff), &working_for_diff)
        })
        .await
        .unwrap_or_else(|_| FileDiff::fallback_new(&path, &working));

        self.approval.set_thinking(false).await;
        let approved = self
            .approval
            .propose_file_write(path.clone(), working.clone(), diff)
            .await;
        self.approval.set_thinking(true).await;

        if approved {
            match self.env.write_file(path, working).await {
                Ok(()) => {
                    let out = ReplaceInFileOutput {
                        success: true,
                        replacements: total_replacements,
                        total_lines,
                        error: None,
                    };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            let out = ReplaceInFileOutput {
                success: false,
                replacements: 0,
                total_lines: 0,
                error: Some("[USER_EXPLICIT_REJECT]".to_string()),
            };
            self.approval
                .report_tool_result(
                    Self::NAME.to_string(),
                    serde_json::to_string(&out).unwrap_or_default(),
                )
                .await;
            Ok(out)
        }
    }
}

// --- FILE DELETE ---

#[derive(Deserialize)]
pub struct FileDeleteArgs {
    pub path: String,
}

#[derive(Serialize)]
pub struct FileDeleteOutput {
    pub success: bool,
    /// Explicit error feedback for the LLM to prevent retry loops on user rejection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct FileDeleteTool {
    pub env: Arc<dyn ClawEnvironment>,
    pub current_dir: String,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for FileDeleteTool {
    const NAME: &'static str = "file_delete";

    type Error = std::io::Error;
    type Args = FileDeleteArgs;
    type Output = FileDeleteOutput;

    fn description(&self) -> String {
        "Delete a file from the host system. This action is permanent and prompts the user for approval.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to delete."
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
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let base_dir = self
            .approval
            .current_dir()
            .await
            .unwrap_or_else(|| self.current_dir.clone());
        let path = resolve_path(&base_dir, &args.path);
        self.approval.set_thinking(false).await;
        let approved = self.approval.propose_file_delete(path.clone()).await;
        self.approval.set_thinking(true).await;

        if approved {
            match self.env.delete_file(path).await {
                Ok(()) => {
                    let out = FileDeleteOutput {
                        success: true,
                        error: None,
                    };
                    self.approval
                        .report_tool_result(
                            Self::NAME.to_string(),
                            serde_json::to_string(&out).unwrap_or_default(),
                        )
                        .await;
                    Ok(out)
                }
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            let out = FileDeleteOutput {
                success: false,
                error: Some("[USER_EXPLICIT_REJECT]".to_string()),
            };
            self.approval
                .report_tool_result(
                    Self::NAME.to_string(),
                    serde_json::to_string(&out).unwrap_or_default(),
                )
                .await;
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockEnv {
        last_read_path: Mutex<Option<String>>,
        file_content: Mutex<String>,
        written_content: Mutex<Option<String>>,
        dir_entries: Mutex<Vec<(String, bool, u64)>>,
    }

    impl MockEnv {
        fn new() -> Self {
            Self {
                last_read_path: Mutex::new(None),
                file_content: Mutex::new(String::new()),
                written_content: Mutex::new(None),
                dir_entries: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ClawEnvironment for MockEnv {
        async fn exec_shell(&self, _command: String) -> anyhow::Result<(i32, String, String)> {
            Ok((0, String::new(), String::new()))
        }
        async fn spawn_detached(&self, _command: String, _cwd: String) -> anyhow::Result<u32> {
            Ok(1234)
        }
        async fn read_file(
            &self,
            path: String,
            start_line: u32,
            end_line: u32,
        ) -> anyhow::Result<String> {
            *self.last_read_path.lock().unwrap() = Some(path.clone());
            let content = self.file_content.lock().unwrap().clone();
            if start_line == 0 && end_line == 0 {
                if content.is_empty() {
                    Err(anyhow::anyhow!("No such file or directory"))
                } else {
                    Ok(content)
                }
            } else {
                Ok(format!("read:{path}:{start_line}-{end_line}"))
            }
        }
        async fn write_file(&self, _path: String, content: String) -> anyhow::Result<()> {
            *self.written_content.lock().unwrap() = Some(content);
            Ok(())
        }
        async fn list_directory(&self, _path: String) -> anyhow::Result<Vec<(String, bool, u64)>> {
            Ok(self.dir_entries.lock().unwrap().clone())
        }
        async fn delete_file(&self, _path: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get_system_info(&self) -> anyhow::Result<String> {
            Ok("{}".to_string())
        }
        async fn list_processes(&self) -> anyhow::Result<Vec<(u32, String, f64, u64, u64, u64)>> {
            Ok(vec![])
        }
        async fn kill_process(&self, _pid: u32, _signal: i32) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get_clipboard(&self) -> anyhow::Result<String> {
            Ok(String::new())
        }
        async fn set_clipboard(&self, _text: String) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct MockApproval {
        approve: bool,
        live_cwd: Mutex<Option<String>>,
        results: Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl ApprovalHandler for MockApproval {
        async fn propose_file_write(
            &self,
            _path: String,
            _content: String,
            _diff: FileDiff,
        ) -> bool {
            self.approve
        }
        async fn propose_file_delete(&self, _path: String) -> bool {
            self.approve
        }
        async fn propose_background_command(&self, _command: String, _explanation: String) -> bool {
            self.approve
        }
        async fn propose_kill_process(&self, _pid: u32, _process_name: String) -> bool {
            self.approve
        }
        async fn propose_get_clipboard(&self) -> bool {
            self.approve
        }
        async fn propose_set_clipboard(&self, _text: String) -> bool {
            self.approve
        }
        async fn report_tool_result(&self, tool_name: String, result: String) {
            self.results.lock().unwrap().push((tool_name, result));
        }
        async fn report_tool_started(&self, _tool_name: String) {}
        async fn set_thinking(&self, _thinking: bool) {}
        async fn current_dir(&self) -> Option<String> {
            self.live_cwd.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn file_read_uses_live_current_dir_when_updated() {
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval {
            approve: true,
            live_cwd: Mutex::new(Some("/workspace/initial".to_string())),
            results: Mutex::new(Vec::new()),
        });
        let tool = FileReadTool {
            env: env.clone(),
            current_dir: "/workspace/stale".to_string(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        let out1 = tool
            .call(
                &mut ctx,
                FileReadArgs {
                    path: "Cargo.toml".to_string(),
                    start_line: None,
                    end_line: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(out1.content, "read:/workspace/initial/Cargo.toml:1-500");

        // Simulate user `cd` in terminal between turns without rebuilding tool
        *approval.live_cwd.lock().unwrap() = Some("/workspace/subdir".to_string());
        let out2 = tool
            .call(
                &mut ctx,
                FileReadArgs {
                    path: "src/lib.rs".to_string(),
                    start_line: Some(10),
                    end_line: Some(30),
                },
            )
            .await
            .unwrap();
        assert_eq!(out2.content, "read:/workspace/subdir/src/lib.rs:10-30");
    }

    #[tokio::test]
    async fn file_write_and_delete_return_explicit_reject_token() {
        let mut ctx = ToolContext::default();
        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(MockApproval {
            approve: false,
            live_cwd: Mutex::new(Some("/workspace".to_string())),
            results: Mutex::new(Vec::new()),
        });

        let write_tool = FileWriteTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };
        let write_out = write_tool
            .call(
                &mut ctx,
                FileWriteArgs {
                    path: "a.txt".to_string(),
                    content: "hello".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(!write_out.success);
        assert_eq!(write_out.error.as_deref(), Some("[USER_EXPLICIT_REJECT]"));

        let delete_tool = FileDeleteTool {
            env,
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };
        let delete_out = delete_tool
            .call(
                &mut ctx,
                FileDeleteArgs {
                    path: "a.txt".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(!delete_out.success);
        assert_eq!(delete_out.error.as_deref(), Some("[USER_EXPLICIT_REJECT]"));
        assert_eq!(approval.results.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn replace_in_file_handles_unique_multiple_and_crlf_matches() {
        let mut ctx = ToolContext::default();
        let env = Arc::new(MockEnv::new());
        *env.file_content.lock().unwrap() = "fn a() {\r\n    let x = 1;\r\n    let y = 1;\r\n}\r\n".to_string();

        let approval = Arc::new(MockApproval {
            approve: true,
            live_cwd: Mutex::new(Some("/workspace".to_string())),
            results: Mutex::new(Vec::new()),
        });
        let tool = ReplaceInFileTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval,
        };

        // 1. Multiple matches with replace_all: false fails safely
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

        // 2. Single match with LF input preserves CRLF in target file
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
            env.written_content.lock().unwrap().as_deref(),
            Some("fn a() {\r\n    let x = 42;\r\n    let y = 1;\r\n}\r\n")
        );

        // 3. Batch `edits` array applies multiple non-contiguous edits in a single approval
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
                            old_string: "let x = 1;".to_string(),
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
            env.written_content.lock().unwrap().as_deref(),
            Some("fn a() {\r\n    let x = 10;\r\n    let y = 20;\r\n}\r\n")
        );
    }

    #[tokio::test]
    async fn file_write_computes_created_and_modified_diffs() {
        struct DiffCapturingApproval {
            diffs: Mutex<Vec<FileDiff>>,
        }

        #[async_trait::async_trait]
        impl ApprovalHandler for DiffCapturingApproval {
            async fn propose_file_write(
                &self,
                _path: String,
                _content: String,
                diff: FileDiff,
            ) -> bool {
                self.diffs.lock().unwrap().push(diff);
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
        }

        let env = Arc::new(MockEnv::new());
        let approval = Arc::new(DiffCapturingApproval {
            diffs: Mutex::new(Vec::new()),
        });
        let tool = FileWriteTool {
            env: env.clone(),
            current_dir: "/workspace".to_string(),
            approval: approval.clone(),
        };

        let mut ctx = ToolContext::default();
        // 1. New file (MockEnv has empty file_content -> returns NotFound)
        tool.call(
            &mut ctx,
            FileWriteArgs {
                path: "src/new_mod.rs".to_string(),
                content: "pub fn hello() {}\n".to_string(),
            },
        )
        .await
        .unwrap();

        // 2. Existing file (MockEnv has existing content -> returns Modified diff)
        *env.file_content.lock().unwrap() = "pub fn hello() {\n    old();\n}\n".to_string();
        tool.call(
            &mut ctx,
            FileWriteArgs {
                path: "src/new_mod.rs".to_string(),
                content: "pub fn hello() {\n    new();\n}\n".to_string(),
            },
        )
        .await
        .unwrap();

        let captured = approval.diffs.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].kind, boxxy_claw_protocol::DiffKind::Created);
        assert_eq!(captured[0].stats.added_lines, 1);
        assert_eq!(captured[1].kind, boxxy_claw_protocol::DiffKind::Modified);
        assert_eq!(captured[1].stats.added_lines, 1);
        assert_eq!(captured[1].stats.removed_lines, 1);
    }

    #[tokio::test]
    async fn list_directory_sorts_dirs_first_and_truncates_at_250() {
        let env = Arc::new(MockEnv::new());
        let mut raw = vec![
            ("zebra.txt".to_string(), false, 10),
            ("alpha.txt".to_string(), false, 20),
            ("src".to_string(), true, 0),
            ("assets".to_string(), true, 0),
        ];
        for i in 0..260 {
            raw.push((format!("file_{i:03}.txt"), false, 1));
        }
        *env.dir_entries.lock().unwrap() = raw;

        let approval = Arc::new(MockApproval {
            approve: true,
            live_cwd: Mutex::new(Some("/workspace".to_string())),
            results: Mutex::new(Vec::new()),
        });
        let tool = ListDirectoryTool {
            env,
            current_dir: "/workspace".to_string(),
            approval,
        };

        let mut ctx = ToolContext::default();
        let out = tool
            .call(&mut ctx, ListDirectoryArgs { path: None })
            .await
            .unwrap();
        assert_eq!(out.total_entries, 264);
        assert!(out.truncated);
        assert_eq!(out.entries.len(), 250);
        // Directories must come first, sorted alphabetically
        assert_eq!(out.entries[0].name, "assets");
        assert!(out.entries[0].is_dir);
        assert_eq!(out.entries[1].name, "src");
        assert!(out.entries[1].is_dir);
        assert_eq!(out.entries[2].name, "alpha.txt");
        assert!(!out.entries[2].is_dir);
    }
}
