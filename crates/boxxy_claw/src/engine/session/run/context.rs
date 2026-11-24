use std::collections::VecDeque;
use std::sync::Arc;

use crate::engine::{ClawEnvironment, ClawMessage};
use crate::registry::workspace::WorkspaceRegistry;

/// User follow-up prompt admitted into the session queue while a turn is running.
#[derive(Debug, Clone)]
pub struct QueuedPrompt {
    pub id: uuid::Uuid,
    pub raw_query: String,
    pub display_query: String,
    pub snapshot: String,
    pub cwd: String,
    pub image_attachments: Vec<String>,
    pub image_attachment_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
    pub thumb_paths: Vec<String>,
    pub text_attachments: Vec<String>,
    pub is_user_message_variant: bool,
}

impl QueuedPrompt {
    #[must_use]
    pub fn to_summary(&self) -> boxxy_claw_protocol::QueuedPromptSummary {
        boxxy_claw_protocol::QueuedPromptSummary {
            id: self.id,
            display_text: self.display_query.clone(),
            images: self.thumb_paths.clone(),
            text_attachments: self.text_attachments.clone(),
        }
    }
}

/// Loop-local state for `ClawSession::run()`, kept separate from the shared
/// `Arc<Mutex<SessionState>>` so turn orchestration does not require locking.
pub struct RunContext {
    pub env: Arc<dyn ClawEnvironment>,
    pub workspace: Arc<WorkspaceRegistry>,
    pub is_initialized: bool,
    pub current_dir: String,
    pub current_turn: Option<tokio::task::JoinHandle<()>>,
    pub active_btw_tasks: std::collections::HashMap<uuid::Uuid, tokio::task::JoinHandle<()>>,
    pub queued_prompts: VecDeque<QueuedPrompt>,
    pub queue_paused: bool,
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
            queued_prompts: VecDeque::new(),
            queue_paused: false,
            backlog: VecDeque::new(),
            urgent_backlog: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn queue_summaries(&self) -> Vec<boxxy_claw_protocol::QueuedPromptSummary> {
        self.queued_prompts
            .iter()
            .map(QueuedPrompt::to_summary)
            .collect()
    }

    pub fn clear_queue(&mut self) -> bool {
        let was_non_empty = !self.queued_prompts.is_empty() || self.queue_paused;
        self.queued_prompts.clear();
        self.queue_paused = false;
        was_non_empty
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
