//! Persisted visual interaction rows stored in SQLite (`claw_events`) and rendered in the UI.

use crate::{FileDiff, TurnTraceItem, UsageWrapper};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PersistentClawRow {
    Diagnosis {
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        trace: Vec<TurnTraceItem>,
        usage: Option<UsageWrapper>,
    },
    User {
        pane_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
    },
    Suggested {
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        diagnosis: String,
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        trace: Vec<TurnTraceItem>,
        usage: Option<UsageWrapper>,
    },
    ProcessList {
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        result_json: String,
        usage: Option<UsageWrapper>,
    },
    ToolCall {
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        tool_name: String,
        result: String,
        usage: Option<UsageWrapper>,
    },
    Command {
        command: String,
        exit_code: i32,
    },
    SystemMessage {
        pane_id: String,
        content: String,
    },
    FileDiff {
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        diff: FileDiff,
        #[serde(default)]
        auto_approved: bool,
        usage: Option<UsageWrapper>,
    },
}

impl PersistentClawRow {
    #[must_use]
    pub const fn is_overlay_visible(&self) -> bool {
        match self {
            Self::User { .. }
            | Self::Diagnosis { .. }
            | Self::Suggested { .. }
            | Self::SystemMessage { .. }
            | Self::Command { .. }
            | Self::FileDiff { .. } => true,
            Self::ToolCall { .. } | Self::ProcessList { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_visibility_separates_debug_rows_from_conversation_rows() {
        let user = PersistentClawRow::User {
            pane_id: "pane-1".to_string(),
            content: "Hello".to_string(),
            images: Vec::new(),
        };
        assert!(user.is_overlay_visible());

        let tool = PersistentClawRow::ToolCall {
            pane_id: "pane-1".to_string(),
            agent_name: None,
            character_id: None,
            tool_name: "file_read".to_string(),
            result: "ok".to_string(),
            usage: None,
        };
        assert!(!tool.is_overlay_visible());

        let procs = PersistentClawRow::ProcessList {
            pane_id: "pane-1".to_string(),
            agent_name: None,
            character_id: None,
            result_json: "[]".to_string(),
            usage: None,
        };
        assert!(!procs.is_overlay_visible());
    }
}
