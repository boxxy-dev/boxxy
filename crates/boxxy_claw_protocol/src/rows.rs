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

    #[must_use]
    pub fn pane_id(&self) -> Option<&str> {
        match self {
            Self::Diagnosis { pane_id, .. }
            | Self::User { pane_id, .. }
            | Self::Suggested { pane_id, .. }
            | Self::ProcessList { pane_id, .. }
            | Self::ToolCall { pane_id, .. }
            | Self::SystemMessage { pane_id, .. }
            | Self::FileDiff { pane_id, .. } => Some(pane_id),
            Self::Command { .. } => None,
        }
    }

    #[must_use]
    pub fn agent_name(&self) -> Option<&str> {
        match self {
            Self::Diagnosis { agent_name, .. }
            | Self::Suggested { agent_name, .. }
            | Self::ProcessList { agent_name, .. }
            | Self::ToolCall { agent_name, .. }
            | Self::FileDiff { agent_name, .. } => agent_name.as_deref(),
            Self::User { .. } | Self::Command { .. } | Self::SystemMessage { .. } => None,
        }
    }

    #[must_use]
    pub fn character_id(&self) -> Option<&str> {
        match self {
            Self::Diagnosis { character_id, .. }
            | Self::Suggested { character_id, .. }
            | Self::ProcessList { character_id, .. }
            | Self::ToolCall { character_id, .. }
            | Self::FileDiff { character_id, .. } => character_id.as_deref(),
            Self::User { .. } | Self::Command { .. } | Self::SystemMessage { .. } => None,
        }
    }

    #[must_use]
    pub fn trace(&self) -> &[TurnTraceItem] {
        match self {
            Self::Diagnosis { trace, .. } | Self::Suggested { trace, .. } => trace,
            _ => &[],
        }
    }

    #[must_use]
    pub const fn usage(&self) -> Option<&UsageWrapper> {
        match self {
            Self::Diagnosis { usage, .. }
            | Self::Suggested { usage, .. }
            | Self::ProcessList { usage, .. }
            | Self::ToolCall { usage, .. }
            | Self::FileDiff { usage, .. } => usage.as_ref(),
            Self::User { .. } | Self::Command { .. } | Self::SystemMessage { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_visibility_and_accessors_work_across_variants() {
        let user = PersistentClawRow::User {
            pane_id: "pane-1".to_string(),
            content: "Hello".to_string(),
            images: Vec::new(),
        };
        assert!(user.is_overlay_visible());
        assert_eq!(user.pane_id(), Some("pane-1"));
        assert_eq!(user.character_id(), None);

        let tool = PersistentClawRow::ToolCall {
            pane_id: "pane-1".to_string(),
            agent_name: Some("red-pony".to_string()),
            character_id: Some("char-1".to_string()),
            tool_name: "file_read".to_string(),
            result: "ok".to_string(),
            usage: None,
        };
        assert!(!tool.is_overlay_visible());
        assert_eq!(tool.agent_name(), Some("red-pony"));
        assert_eq!(tool.character_id(), Some("char-1"));

        let procs = PersistentClawRow::ProcessList {
            pane_id: "pane-1".to_string(),
            agent_name: None,
            character_id: None,
            result_json: "[]".to_string(),
            usage: None,
        };
        assert!(!procs.is_overlay_visible());

        let cmd = PersistentClawRow::Command {
            command: "cargo test".to_string(),
            exit_code: 0,
        };
        assert!(cmd.is_overlay_visible());
        assert_eq!(cmd.pane_id(), None);
    }
}
