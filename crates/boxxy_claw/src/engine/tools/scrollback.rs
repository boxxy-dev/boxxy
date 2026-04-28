use crate::engine::ClawEngineEvent;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ReadScrollbackArgs {
    pub max_lines: usize,
    pub offset_lines: usize,
}

#[derive(Serialize)]
pub struct ReadScrollbackOutput {
    pub text: String,
}

pub struct ReadScrollbackTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<crate::engine::session::SessionState>>,
}

impl Tool for ReadScrollbackTool {
    const NAME: &'static str = "read_scrollback_page";

    type Error = std::io::Error;
    type Args = ReadScrollbackArgs;
    type Output = ReadScrollbackOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read older lines from your terminal's scrollback history. \
            By default you only see the last 100 lines. Use this tool if an error or context \
            you need to analyze happened further up in the terminal history. \
            Provides structured semantic blocks (PROMPT, COMMAND, OUTPUT)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "max_lines": {
                        "type": "integer",
                        "description": "The number of lines to fetch."
                    },
                    "offset_lines": {
                        "type": "integer",
                        "description": "How many lines back from the bottom to start reading. e.g. 0 is the absolute bottom, 100 reads lines ending 100 lines above the bottom."
                    }
                },
                "required": ["max_lines", "offset_lines"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let request_id = uuid::Uuid::new_v4();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        let (agent_name, character_id) = {
            let mut state = self.state.lock().await;
            state.pending_scrollbacks.insert(request_id, reply_tx);
            (state.agent_name.clone(), state.character_id.clone())
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name: agent_name.clone(),
                character_id: character_id.clone(),
                tool_name: Self::NAME.to_string(),
            })
            .await;

        let req = ClawEngineEvent::RequestScrollback {
            agent_name,
            character_id,
            max_lines: args.max_lines,
            offset_lines: args.offset_lines,
            request_id,
        };

        if let Err(e) = self.tx_ui.send(req).await {
            return Err(std::io::Error::other(format!(
                "Failed to send scrollback request: {e}"
            )));
        }

        match reply_rx.await {
            Ok(text) => Ok(ReadScrollbackOutput { text }),
            Err(e) => Err(std::io::Error::other(format!(
                "Failed to receive scrollback: {e}"
            ))),
        }
    }
}
