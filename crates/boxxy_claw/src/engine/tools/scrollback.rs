use crate::engine::ClawEngineEvent;
use rig::tool::{Tool, ToolContext};
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
    pub approval: std::sync::Arc<crate::engine::tools::ClawApprovalHandler>,
}

impl Tool for ReadScrollbackTool {
    const NAME: &'static str = "read_scrollback_page";

    type Error = std::io::Error;
    type Args = ReadScrollbackArgs;
    type Output = ReadScrollbackOutput;

    fn description(&self) -> String {
        "Read older lines from the terminal's scrollback history as structured semantic blocks (PROMPT, COMMAND, OUTPUT). The bottom ~100 lines are already in `Terminal Snapshot`, so pass `offset_lines >= 100` to page further back."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "max_lines": {
                    "type": "integer",
                    "description": "The number of lines to fetch (e.g. 100)."
                },
                "offset_lines": {
                    "type": "integer",
                    "description": "How many lines back from the bottom to start reading (use >= 100 since the bottom 100 lines are already in Terminal Snapshot)."
                }
            },
            "required": ["max_lines", "offset_lines"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;
        let request_id = uuid::Uuid::new_v4();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        let (agent_name, character_id) = {
            let mut state = self.state.lock().await;
            state.pending_scrollbacks.insert(request_id, reply_tx);
            (state.agent_name.clone(), state.character_id.clone())
        };

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
            Ok(text) => {
                let out = ReadScrollbackOutput { text };
                self.approval.finish_tool(Self::NAME, &out).await;
                Ok(out)
            }
            Err(e) => Err(std::io::Error::other(format!(
                "Failed to receive scrollback: {e}"
            ))),
        }
    }
}
