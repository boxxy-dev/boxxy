use anyhow::Result;
use boxxy_core_toolbox::ApprovalHandler;
use boxxy_db::Db;
use boxxy_db::store::Store;
use rig::tool::PortableTool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Deserialize, Serialize)]
pub struct MemoryStoreArgs {
    pub key: String,
    pub content: String,
    pub category: Option<String>,
    pub project_path: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryToolError {
    #[error("Database not available")]
    DbUnavailable,
    #[error("Database error: {0}")]
    DbError(#[from] anyhow::Error),
}

pub struct MemoryStoreTool {
    pub db: Arc<Mutex<Option<Db>>>,
    pub current_dir: String,
    pub approval: Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for MemoryStoreTool {
    const NAME: &'static str = "memory_store";

    type Args = MemoryStoreArgs;
    type Output = String;
    type Error = MemoryToolError;

    fn description(&self) -> String {
        "CRITICAL DIRECTIVE: You MUST use this tool immediately if the user explicitly asks you to 'remember', 'save', 'note', or store a fact, preference, command, path, or any other information. Do not just reply 'I will remember'. \
        Store a fact, preference, or lesson in long-term memory. \
        Use a concise snake_case key (e.g., 'system_update_command', 'favorite_editor', 'os_type'). \
        If the key already exists, the memory will be updated (overwritten). \
        IMPORTANT: Defaults to 'global' (applies across all directories and sessions) unless `project_path` is explicitly provided for project-specific facts."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Unique snake_case key for this memory (e.g. 'system_update_command', 'preferred_shell', 'project_stack')"
                },
                "content": {
                    "type": "string",
                    "description": "The information to remember (include context/keywords, e.g. 'Always use cachy-update instead of pacman -Syu to update the system')"
                },
                "category": {
                    "type": "string",
                    "description": "Optional category: 'preference', 'system', 'project', etc. Defaults to 'preference'."
                },
                "project_path": {
                    "type": "string",
                    "description": "Optional: ONLY set this to a directory path if the memory is strictly specific to one codebase/project. Leave omitted or 'global' for system-wide preferences, OS tools, and user habits."
                }
            },
            "required": ["key", "content"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        let db_guard = self.db.lock().await;
        if let Some(db) = db_guard.as_ref() {
            let store = Store::new(db.pool());
            // Default to 'global' unless the LLM explicitly passed a non-empty project_path
            // or explicitly set category == "project".
            let project_path = match args.project_path.as_deref().map(str::trim) {
                Some("") | Some("global") | None => {
                    if args.category.as_deref() == Some("project") && !self.current_dir.is_empty() {
                        Some(self.current_dir.as_str())
                    } else {
                        Some("global")
                    }
                }
                Some(path) => Some(path),
            };

            // Explicit memory stores via tool are automatically considered verified and NOT pinned by default
            match store
                .add_memory(
                    &args.key,
                    project_path,
                    &args.content,
                    args.category.as_deref(),
                    true,
                    false,
                )
                .await
            {
                Ok(_) => {
                    drop(db_guard);
                    let _ = crate::memories::db::sync_memories_to_markdown(self.db.clone()).await;
                    let result = format!("Successfully stored memory: {}", args.key);
                    self.approval
                        .report_tool_result(Self::NAME.to_string(), result.clone())
                        .await;
                    Ok(result)
                }
                Err(e) => {
                    let result = format!("Error storing memory: {}", e);
                    self.approval
                        .report_tool_result(Self::NAME.to_string(), result.clone())
                        .await;
                    Ok(result)
                }
            }
        } else {
            Err(MemoryToolError::DbUnavailable)
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemoryDeleteArgs {
    pub key: String,
    pub project_path: Option<String>,
}

pub struct MemoryDeleteTool {
    pub db: Arc<Mutex<Option<Db>>>,
    pub current_dir: String,
    pub approval: Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl PortableTool for MemoryDeleteTool {
    const NAME: &'static str = "memory_delete";

    type Args = MemoryDeleteArgs;
    type Output = String;
    type Error = MemoryToolError;

    fn description(&self) -> String {
        "Delete a specific memory from the long-term database. \
        Use this to prune transient, incorrect, or outdated data (e.g. old git branches, temporary paths). \
        If project_path is not provided, it defaults to the current directory."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The unique key of the memory to delete."
                },
                "project_path": {
                    "type": "string",
                    "description": "Optional: the project path scope of the memory. Use 'global' for global memories."
                }
            },
            "required": ["key"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        let db_guard = self.db.lock().await;
        if let Some(db) = db_guard.as_ref() {
            let store = Store::new(db.pool());
            let project_path = args
                .project_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());

            // Delete from the requested scope (or both current_dir and global if unspecified)
            let res = if let Some(scope) = project_path {
                store.delete_memory(&args.key, Some(scope)).await
            } else {
                let _ = store.delete_memory(&args.key, Some(&self.current_dir)).await;
                store.delete_memory(&args.key, Some("global")).await
            };

            match res {
                Ok(_) => {
                    drop(db_guard);
                    let _ = crate::memories::db::sync_memories_to_markdown(self.db.clone()).await;
                    let result = format!("Successfully deleted memory: {}", args.key);
                    self.approval
                        .report_tool_result(Self::NAME.to_string(), result.clone())
                        .await;
                    Ok(result)
                }
                Err(e) => {
                    let result = format!("Error deleting memory: {}", e);
                    self.approval
                        .report_tool_result(Self::NAME.to_string(), result.clone())
                        .await;
                    Ok(result)
                }
            }
        } else {
            Err(MemoryToolError::DbUnavailable)
        }
    }
}
