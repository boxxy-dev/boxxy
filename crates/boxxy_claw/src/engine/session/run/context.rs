use std::collections::VecDeque;
use std::sync::Arc;

use crate::engine::{ClawEnvironment, ClawMessage};
use crate::registry::workspace::WorkspaceRegistry;

/// Loop-local state for `ClawSession::run()`, kept separate from the shared
/// `Arc<Mutex<SessionState>>` so turn orchestration does not require locking.
pub struct RunContext {
    pub env: Arc<dyn ClawEnvironment>,
    pub workspace: Arc<WorkspaceRegistry>,
    pub is_initialized: bool,
    pub current_dir: String,
    pub current_turn: Option<tokio::task::JoinHandle<()>>,
    pub active_btw_tasks: std::collections::HashMap<uuid::Uuid, tokio::task::JoinHandle<()>>,
    pub backlog: VecDeque<ClawMessage>,
    pub urgent_backlog: VecDeque<ClawMessage>,
}

impl RunContext {
    pub fn new(env: Arc<dyn ClawEnvironment>, workspace: Arc<WorkspaceRegistry>) -> Self {
        Self {
            env,
            workspace,
            is_initialized: false,
            current_dir: String::from("/"),
            current_turn: None,
            active_btw_tasks: std::collections::HashMap::new(),
            backlog: VecDeque::new(),
            urgent_backlog: VecDeque::new(),
        }
    }

    pub fn abort_btw_tasks(&mut self, request_id: Option<uuid::Uuid>) {
        self.active_btw_tasks.retain(|_, handle| !handle.is_finished());
        if let Some(id) = request_id {
            if let Some(handle) = self.active_btw_tasks.remove(&id) {
                handle.abort();
            }
        } else {
            for (_, handle) in self.active_btw_tasks.drain() {
                handle.abort();
            }
        }
    }
}
