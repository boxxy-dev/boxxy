//! Agent finite-state-machine states, spawn placement targets, and inter-agent pub/sub events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ContextQuality {
    Full,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentStatus {
    Off,
    Sleep,
    Waiting,
    Working,
    Locking { resource: String },
    Faulted { reason: String },
}

impl AgentStatus {
    /// Returns `true` when Claw is enabled for the pane (anything other than `Off`).
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Returns `true` when the agent is actively executing a turn or holding a lock.
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        matches!(self, Self::Working | Self::Locking { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TriggerSource {
    User,
    Swarm { trace_id: Vec<Uuid> },
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransitionRequest {
    pub target_state: AgentStatus,
    pub source: TriggerSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpawnLocation {
    NewTab,
    VerticalSplit,
    HorizontalSplit,
}

/// Internal events for agent-to-agent orchestration via the Workspace `EventBus`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClawEvent {
    /// A process exited in a specific pane.
    ProcessExited { pane_id: String, exit_code: i32 },
    /// A specific regex matched the terminal output of a pane.
    OutputMatch {
        pane_id: String,
        matched_text: String,
        regex: String,
    },
    /// A custom event published by an agent.
    Custom {
        source_agent: String,
        name: String,
        payload: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_status_predicates_and_event_roundtrip() {
        assert!(!AgentStatus::Off.is_enabled());
        assert!(AgentStatus::Sleep.is_enabled());
        assert!(!AgentStatus::Waiting.is_busy());
        assert!(AgentStatus::Working.is_busy());
        assert!(
            AgentStatus::Locking {
                resource: "src/lib.rs".to_string()
            }
            .is_busy()
        );

        let event = ClawEvent::ProcessExited {
            pane_id: "pane-2".to_string(),
            exit_code: 0,
        };
        let encoded = serde_json::to_string(&event).unwrap();
        let decoded: ClawEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, event);
    }
}
