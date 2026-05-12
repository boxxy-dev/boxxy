use crate::proposal::Proposal;

/// The exclusive interaction state of the Claw overlay drawer.
#[derive(Debug, Clone)]
pub enum OverlayState {
    /// Agent is ready. No pending proposal, no active task visible to the drawer.
    Idle,
    /// Agent is actively processing — LLM turn or blocking tool execution.
    /// The user can type but should not expect an immediate response.
    Thinking,
    /// A non-blocking task is in flight (e.g., background process spawning).
    /// No user action is required. The drawer remains interactive.
    Pending,
    /// Waiting for the user to resolve a proposal before the agent can continue.
    Action(Proposal),
}
