use crate::ApprovalHandler;
use crate::utils::resolve_path;
use boxxy_claw_protocol::ClawEnvironment;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read the contents of a file on the host system. Use this to inspect code, configuration, or logs before modifying them.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The absolute or relative path to the file to read."
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "Optional: The 1-based line number to start reading from (inclusive)."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Optional: The 1-based line number to end reading at (inclusive)."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let path = resolve_path(&self.current_dir, &args.path);
        let start_line = args.start_line.unwrap_or(0);
        let end_line = args.end_line.unwrap_or(0);
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List the contents of a directory on the host system. Use this to discover files and subdirectories.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The directory path to list (default: current directory)."
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let path = resolve_path(&self.current_dir, &args.path.unwrap_or_default());
        match self.env.list_directory(path).await {
            Ok(entries) => {
                let output = ListDirectoryOutput {
                    entries: entries
                        .into_iter()
                        .map(|(name, is_dir, size)| DirectoryEntry { name, is_dir, size })
                        .collect(),
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Write content to a file on the host system. This will overwrite any existing content. Always prompts the user for approval.".to_string(),
            parameters: serde_json::json!({
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
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let path = resolve_path(&self.current_dir, &args.path);
        self.approval.set_thinking(false).await;
        let approved = self
            .approval
            .propose_file_write(path.clone(), args.content.clone())
            .await;
        self.approval.set_thinking(true).await;

        if approved {
            match self.env.write_file(path, args.content).await {
                Ok(_) => Ok(FileWriteOutput {
                    success: true,
                    error: None,
                }),
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            Ok(FileWriteOutput {
                success: false,
                error: Some("The user explicitly rejected this file write operation. Do not attempt it again.".to_string()),
            })
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Delete a file from the host system. This action is permanent and prompts the user for approval.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to delete."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let path = resolve_path(&self.current_dir, &args.path);
        self.approval.set_thinking(false).await;
        let approved = self.approval.propose_file_delete(path.clone()).await;
        self.approval.set_thinking(true).await;

        if approved {
            match self.env.delete_file(path).await {
                Ok(_) => Ok(FileDeleteOutput {
                    success: true,
                    error: None,
                }),
                Err(e) => Err(std::io::Error::other(format!("Environment Error: {e}"))),
            }
        } else {
            Ok(FileDeleteOutput {
                success: false,
                error: Some(
                    "The user explicitly rejected this file deletion. Do not attempt it again."
                        .to_string(),
                ),
            })
        }
    }
}
