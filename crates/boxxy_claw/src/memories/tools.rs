use anyhow::Result;
use boxxy_core_toolbox::ApprovalHandler;
use boxxy_db::Db;
use boxxy_db::store::Store;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
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

impl Tool for MemoryStoreTool {
    const NAME: &'static str = "memory_store";

    type Args = MemoryStoreArgs;
    type Output = String;
    type Error = MemoryToolError;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "CRITICAL DIRECTIVE: You MUST use this tool immediately if the user explicitly asks you to 'remember', 'save', 'note', or store a fact, preference, path, or any other information. Do not just reply 'I will remember'. \
            Store a fact, preference, or lesson in long-term memory. \
            Use a concise snake_case key (e.g., 'favorite_editor', 'os_type'). \
            If the key already exists, the memory will be updated (overwritten). \
            If project_path is provided (or if this is project-specific info), it will be scoped to that project. \
            Defaults to 'global' if no project_path is given."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "Unique key for this memory (e.g. 'preferred_shell', 'project_stack')"
                    },
                    "content": {
                        "type": "string",
                        "description": "The information to remember"
                    },
                    "category": {
                        "type": "string",
                        "description": "Optional category: 'preference', 'system', 'project', etc."
                    },
                    "project_path": {
                        "type": "string",
                        "description": "Optional: scope this memory to a specific project directory. If 'global', it applies everywhere."
                    }
                },
                "required": ["key", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let db_guard = self.db.lock().await;
        if let Some(db) = db_guard.as_ref() {
            let store = Store::new(db.pool());
            let project_path = args.project_path.as_deref().or(Some(&self.current_dir));

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
                    self.approval.report_tool_result(Self::NAME.to_string(), result.clone()).await;
                    Ok(result)
                }
                Err(e) => {
                    let result = format!("Error storing memory: {}", e);
                    self.approval.report_tool_result(Self::NAME.to_string(), result.clone()).await;
                    Ok(result)
                }
            }
        } else {
            Err(MemoryToolError::DbUnavailable)
        }
    }
}
