use crate::engine::fsm::state::{AgentStatus, TriggerSource, TransitionRequest};

pub struct FsmRouter;

pub enum FsmAction {
    Proceed,
    AbortCurrentTurn,
}

impl FsmRouter {
    pub fn evaluate_transition(
        current_status: &AgentStatus,
        parent_id: Option<uuid::Uuid>,
        req: &TransitionRequest,
    ) -> Result<FsmAction, String> {
        // 1. Cycle Detection (DAG)
        if let TriggerSource::Swarm { trace_id } = &req.source {
            if let Some(my_id) = parent_id {
                if trace_id.contains(&my_id) {
                    return Err(format!("Cycle detected in FSM transition: {:?}", trace_id));
                }
            }
        }

        // 2. Lock Integrity
        if matches!(current_status, AgentStatus::Locking { .. }) {
            if matches!(
                req.target_state,
                AgentStatus::Sleep | AgentStatus::Off
            ) {
                return Err(format!("Rejected transition to {:?} while Locking", req.target_state));
            }
        }

        // 3. Working Exit Paths
        if matches!(
            current_status,
            AgentStatus::Working | AgentStatus::Locking { .. }
        ) {
            if matches!(
                req.target_state,
                AgentStatus::Waiting | AgentStatus::Faulted { .. } | AgentStatus::Sleep | AgentStatus::Off
            ) {
                return Ok(FsmAction::AbortCurrentTurn);
            }
        }

        Ok(FsmAction::Proceed)
    }
}
