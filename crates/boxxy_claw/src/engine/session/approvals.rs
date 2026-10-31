use super::SessionState;
use boxxy_core_toolbox::ApprovalOutcome;

impl SessionState {
    /// Resolve any waiting tool approval channel with a user-interruption payload.
    /// Returns `true` if a tool was waiting for approval (meaning the active turn is
    /// still alive and will immediately consume the interruption via the tool output,
    /// so the caller must NOT abort or spawn a duplicate turn).
    pub fn drain_pending_with_interruption(&mut self, message: &str) -> bool {
        self.pending_inject_command = false;
        self.pending_proposal_summary = None;
        let mut fulfilled = false;
        let outcome = ApprovalOutcome::Interrupted(message.to_string());

        if let Some(reply) = self.pending_terminal_reply.take() {
            let _ = reply.send(Err(outcome.rejection_message()));
            fulfilled = true;
        }
        if let Some(reply) = self.pending_file_reply.take() {
            let _ = reply.send(outcome.clone());
            fulfilled = true;
        }
        if let Some(reply) = self.pending_file_delete_reply.take() {
            let _ = reply.send(outcome.clone());
            fulfilled = true;
        }
        if let Some(reply) = self.pending_kill_process_reply.take() {
            let _ = reply.send(outcome.clone());
            fulfilled = true;
        }
        if let Some(reply) = self.pending_background_reply.take() {
            let _ = reply.send(outcome.clone());
            fulfilled = true;
        }
        if let Some(reply) = self.pending_get_clipboard_reply.take() {
            let _ = reply.send(outcome.clone());
            fulfilled = true;
        }
        if let Some(reply) = self.pending_set_clipboard_reply.take() {
            let _ = reply.send(outcome);
            fulfilled = true;
        }

        fulfilled
    }

    /// Reject all waiting tool channels and record a synthetic assistant summary of the
    /// rejected proposal so the next turn retains context without making an extra LLM call.
    pub fn drain_pending_cancel(&mut self) {
        self.pending_inject_command = false;
        if let Some(summary) = self.pending_proposal_summary.take() {
            self.history.push(rig::message::Message::assistant(format!(
                "[Proposed {summary} — rejected by the user]"
            )));
        }
        if let Some(reply) = self.pending_terminal_reply.take() {
            let _ = reply.send(Err("[USER_EXPLICIT_REJECT]".to_string()));
        }
        if let Some(reply) = self.pending_file_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
        }
        if let Some(reply) = self.pending_file_delete_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
        }
        if let Some(reply) = self.pending_kill_process_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
        }
        if let Some(reply) = self.pending_background_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
        }
        if let Some(reply) = self.pending_get_clipboard_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
        }
        if let Some(reply) = self.pending_set_clipboard_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
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
            current_dir: String::new(),
            pinned: false,
            web_search_enabled: false,
            vibe_enabled: false,
            total_tokens: 0,
            context_tokens: 0,
            total_cost_usd: 0.0,
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
            pending_proposal_summary: None,
            pending_scrollbacks: HashMap::new(),
            pending_spawns: HashMap::new(),
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
            active_speaker: None,
            roundtable: crate::engine::session::roundtable::RoundtableState::default(),
            next_roundtable_turn: None,
            read_file_hashes: HashMap::new(),
            current_turn_snapshots: Vec::new(),
            last_turn_snapshots: Vec::new(),
        }
    }

    #[tokio::test]
    async fn drain_with_interruption_nothing_pending_returns_false() {
        let mut state = make_state();
        assert!(!state.drain_pending_with_interruption("hi"));
        assert!(state.history.is_empty());
    }

    #[tokio::test]
    async fn drain_with_interruption_terminal_reply_sends_interruption_payload() {
        let mut state = make_state();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        state.pending_terminal_reply = Some(tx);
        assert!(state.drain_pending_with_interruption("cancel"));
        let res = rx.try_recv();
        assert!(res.is_ok());
        let err_str = res.unwrap().unwrap_err();
        assert!(err_str.contains("[USER_INTERRUPTION]"));
        assert!(err_str.contains("cancel"));
        assert!(state.history.is_empty());
    }

    #[tokio::test]
    async fn drain_cancel_clears_inject_command_flag() {
        let mut state = make_state();
        state.pending_inject_command = true;
        let (tx, _rx) = tokio::sync::oneshot::channel::<ApprovalOutcome>();
        state.pending_file_reply = Some(tx);
        state.drain_pending_cancel();
        assert!(!state.pending_inject_command);
        assert!(state.pending_file_reply.is_none());
    }

    #[tokio::test]
    async fn drain_with_interruption_drains_background_command_reply() {
        let mut state = make_state();
        let (tx, mut rx) = tokio::sync::oneshot::channel::<ApprovalOutcome>();
        state.pending_background_reply = Some(tx);
        assert!(state.drain_pending_with_interruption("hold on, wrong port"));
        assert!(state.pending_background_reply.is_none());
        assert_eq!(
            rx.try_recv(),
            Ok(ApprovalOutcome::Interrupted(
                "hold on, wrong port".to_string()
            ))
        );
    }
}
