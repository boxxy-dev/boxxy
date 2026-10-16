use super::SessionState;

impl SessionState {
    /// Reject all waiting tool channels with a user-interruption payload.
    /// Returns `true` if any channel was pending (so the caller knows whether
    /// to skip spawning a new turn).
    pub fn drain_pending_with_interruption(&mut self, message: &str) -> bool {
        let mut fulfilled = false;

        if let Some(reply) = self.pending_terminal_reply.take() {
            let _ = reply.send(Err(format!("[USER_INTERRUPTION]: {message}")));
            fulfilled = true;
        }
        if let Some(reply) = self.pending_file_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }
        if let Some(reply) = self.pending_file_delete_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }
        if let Some(reply) = self.pending_kill_process_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }
        if let Some(reply) = self.pending_get_clipboard_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }
        if let Some(reply) = self.pending_set_clipboard_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }

        if fulfilled {
            self.history
                .push(rig::message::Message::user(message.to_string()));
        }

        fulfilled
    }

    /// Reject all waiting tool channels without recording history (used by CancelPending).
    pub fn drain_pending_cancel(&mut self) {
        self.pending_inject_command = false;
        if let Some(reply) = self.pending_terminal_reply.take() {
            let _ = reply.send(Err("[USER_EXPLICIT_REJECT]".to_string()));
        }
        if let Some(reply) = self.pending_file_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_file_delete_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_kill_process_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_background_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_get_clipboard_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_set_clipboard_reply.take() {
            let _ = reply.send(false);
        }
    }
}

#[cfg(test)]
mod pending_drain_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn make_state() -> SessionState {
        let (tx, _rx) = async_channel::unbounded();
        SessionState {
            session_id: "test".to_string(),
            pane_id: "test".to_string(),
            agent_name: "test".to_string(),
            character_id: "test".to_string(),
            character_display_name: "test".to_string(),
            pinned: false,
            web_search_enabled: false,
            total_tokens: 0,
            tracked_pids: Arc::new(RwLock::new(HashSet::new())),
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            ollama_url: Arc::new(RwLock::new(String::new())),
            pending_terminal_reply: None,
            pending_inject_command: false,
            pending_file_reply: None,
            pending_file_delete_reply: None,
            pending_kill_process_reply: None,
            pending_background_reply: None,
            pending_get_clipboard_reply: None,
            pending_set_clipboard_reply: None,
            pending_scrollbacks: HashMap::new(),
            pending_delegations: HashMap::new(),
            history: Vec::new(),
            flushed_watermark: 0,
            pending_lazy_diagnosis: None,
            persistent_agent: None,
            current_claw_model: None,
            current_memory_model: None,
            current_search_label: "Off".to_string(),
            last_tools: None,
            pending_tasks: Vec::new(),
            last_snapshot_hash: None,
            status: crate::engine::AgentStatus::Waiting,
            context_quality: crate::engine::ContextQuality::Full,
            parent_id: None,
            is_headless: false,
            sleep_timestamp: None,
            awaiting_tasks: Vec::new(),
            tx_self: tx,
            mcp_handle: None,
        }
    }

    #[tokio::test]
    async fn drain_with_interruption_nothing_pending_returns_false() {
        let mut state = make_state();
        assert!(!state.drain_pending_with_interruption("hi"));
        assert!(state.history.is_empty());
    }

    #[tokio::test]
    async fn drain_with_interruption_terminal_reply_pushes_history() {
        let mut state = make_state();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        state.pending_terminal_reply = Some(tx);
        assert!(state.drain_pending_with_interruption("cancel"));
        let res = rx.try_recv();
        assert!(res.is_ok());
        assert!(res.unwrap().is_err());
        assert_eq!(state.history.len(), 1);
    }

    #[tokio::test]
    async fn drain_cancel_clears_inject_command_flag() {
        let mut state = make_state();
        state.pending_inject_command = true;
        let (tx, _rx) = tokio::sync::oneshot::channel::<bool>();
        state.pending_file_reply = Some(tx);
        state.drain_pending_cancel();
        assert!(!state.pending_inject_command);
        assert!(state.pending_file_reply.is_none());
    }
}
