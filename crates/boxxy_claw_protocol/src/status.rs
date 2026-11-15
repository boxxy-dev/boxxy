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
