pub mod agent;
pub mod agent_config;
pub mod context;
pub mod dispatcher;
pub mod fsm;
pub mod history_utils;
pub mod session;
pub mod summarization;
pub mod tools;
pub mod turn;

pub use boxxy_claw_protocol::{
    ClawEngineEvent, ClawEnvironment, ClawEvent, ClawMessage, PersistentClawRow, ScheduledTask,
    SpawnLocation, TaskStatus, TaskType,
};
pub use fsm::state::*;

use boxxy_db::Db;
pub use session::ClawSession;
use std::sync::Arc;
use tokio::sync::Mutex;

pub fn persist_visual_event(
    db_cell: Arc<Mutex<Option<Db>>>,
    session_id: String,
    pane_id: String,
    event: &ClawEngineEvent,
) {
    if let Some(row) = PersistentClawRowExt::from_engine_event(pane_id, event) {
        tokio::spawn(async move {
            let db_val = {
                let db_guard = db_cell.lock().await;
                db_guard.as_ref().cloned()
            };
            if let Some(db) = db_val {
                let store = boxxy_db::store::Store::new(db.pool());
                if let Ok(json) = serde_json::to_string(&row) {
                    let _ = store.add_claw_event(&session_id, &json).await;
                }
            }
        });
    }
}

/// Extension trait to keep the from_engine_event logic in the engine crate for now
pub struct PersistentClawRowExt;

impl PersistentClawRowExt {
    #[must_use]
    pub fn from_engine_event(
        pane_id: String,
        event: &ClawEngineEvent,
    ) -> Option<PersistentClawRow> {
        match event {
            ClawEngineEvent::UserMessage { content, .. } => Some(PersistentClawRow::User {
                pane_id,
                content: content.clone(),
            }),
            ClawEngineEvent::DiagnosisComplete {
                agent_name,
                diagnosis,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: diagnosis.clone(),
                usage: usage.clone(),
            }),
            ClawEngineEvent::InjectCommand {
                agent_name,
                command,
                diagnosis,
                usage,
                ..
            } => Some(PersistentClawRow::Suggested {
                pane_id,
                agent_name: Some(agent_name.clone()),
                diagnosis: diagnosis.clone(),
                command: command.clone(),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeFileWrite {
                agent_name,
                path,
                content,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed file write to `{path}`:\n```\n{content}\n```"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeFileDelete {
                agent_name,
                path,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed file deletion: `{path}`"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeKillProcess {
                agent_name,
                pid,
                process_name,
                usage,
                ..
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed killing process: {process_name} (PID: {pid})"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeGetClipboard { agent_name, usage } => {
                Some(PersistentClawRow::Diagnosis {
                    pane_id,
                    agent_name: Some(agent_name.clone()),
                    content: "Proposed reading from clipboard.".to_string(),
                    usage: usage.clone(),
                })
            }
            ClawEngineEvent::ProposeSetClipboard {
                agent_name,
                text,
                usage,
            } => Some(PersistentClawRow::Diagnosis {
                pane_id,
                agent_name: Some(agent_name.clone()),
                content: format!("Proposed writing to clipboard:\n```\n{text}\n```"),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ProposeTerminalCommand {
                command,
                explanation,
                agent_name,
                usage,
            } => Some(PersistentClawRow::Suggested {
                pane_id,
                agent_name: Some(agent_name.clone()),
                diagnosis: explanation.clone(),
                command: command.clone(),
                usage: usage.clone(),
            }),
            ClawEngineEvent::ToolResult {
                agent_name,
                tool_name,
                result,
                usage,
            } => {
                if tool_name == "list_processes" {
                    Some(PersistentClawRow::ProcessList {
                        pane_id,
                        agent_name: Some(agent_name.clone()),
                        result_json: result.clone(),
                        usage: usage.clone(),
                    })
                } else {
                    Some(PersistentClawRow::ToolCall {
                        pane_id,
                        agent_name: Some(agent_name.clone()),
                        tool_name: tool_name.clone(),
                        result: result.clone(),
                        usage: usage.clone(),
                    })
                }
            }
            ClawEngineEvent::SystemMessage { text } => Some(PersistentClawRow::SystemMessage {
                pane_id,
                content: text.clone(),
            }),
            ClawEngineEvent::ToolCallStarted { .. } => None,
            _ => None,
        }
    }
}
