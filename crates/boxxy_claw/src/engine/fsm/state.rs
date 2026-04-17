#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ContextQuality {
    Full,
    Degraded,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum AgentStatus {
    Off,
    Sleep,
    Waiting,
    Working,
    Locking { resource: String },
    Faulted { reason: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum TriggerSource {
    User,
    Swarm { trace_id: Vec<uuid::Uuid> },
    System,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransitionRequest {
    pub target_state: AgentStatus,
    pub source: TriggerSource,
}
