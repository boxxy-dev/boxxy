use crate::engine::ClawEngineEvent;
use crate::engine::session::SessionState;
use boxxy_agent::ipc::AgentClawProxy;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct FileReadArgs {
    pub path: String,
}

#[derive(Serialize)]
pub struct FileReadOutput {
    pub content: String,
}

fn resolve_path(base: &str, path: &str) -> String {
    if base.is_empty() {
        path.to_string()
    } else {
        std::path::Path::new(base)
            .join(path)
            .to_string_lossy()
            .to_string()
    }
}

pub struct FileReadTool {
    pub proxy: AgentClawProxy<'static>,
    pub current_dir: String,
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
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = resolve_path(&self.current_dir, &args.path);
        match self.proxy.read_file(path).await {
            Ok(content) => Ok(FileReadOutput { content }),
            Err(e) => Err(std::io::Error::other(format!("IPC Error: {e}"))),
        }
    }
}

#[derive(Deserialize)]
pub struct FileWriteArgs {
    pub path: String,
    pub content: String,
}

#[derive(Serialize)]
pub struct FileWriteOutput {
    pub success: bool,
    pub message: String,
}

pub struct FileWriteTool {
    pub proxy: AgentClawProxy<'static>,
    pub current_dir: String,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
}

impl Tool for FileWriteTool {
    const NAME: &'static str = "file_write";

    type Error = std::io::Error;
    type Args = FileWriteArgs;
    type Output = FileWriteOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Propose to write or overwrite a file on the host system. This will prompt the user for approval. You MUST wait for the result. You MUST use this tool instead of outputting bash `cat` or `echo` commands when writing scripts or files.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to write."
                    },
                    "content": {
                        "type": "string",
                        "description": "The exact full content to write to the file. Do NOT use placeholders."
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let path = resolve_path(&self.current_dir, &args.path);

        let agent_name = {
            let state = self.state.lock().await;
            state.agent_name.clone()
        };

        {
            let mut state = self.state.lock().await;
            state.pending_file_reply = Some(tx);
        }

        if let Err(e) = self
            .tx_ui
            .send(ClawEngineEvent::ProposeFileWrite {
                agent_name: agent_name.clone(),
                path: path.clone(),
                content: args.content.clone(),
            })
            .await
        {
            return Err(std::io::Error::other(format!(
                "Failed to send proposal to UI: {e}"
            )));
        }

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name: agent_name.clone(),
                is_thinking: false,
            })
            .await;
        let approved = rx.await.unwrap_or(false);
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name,
                is_thinking: true,
            })
            .await;

        if approved {
            match self.proxy.write_file(path.clone(), args.content).await {
                Ok(()) => Ok(FileWriteOutput {
                    success: true,
                    message: format!("Successfully wrote to {path}"),
                }),
                Err(e) => Err(std::io::Error::other(format!(
                    "IPC Error writing file: {e}"
                ))),
            }
        } else {
            Ok(FileWriteOutput {
                success: false,
                message: "[USER_EXPLICIT_REJECT]".to_string(),
            })
        }
    }
}
