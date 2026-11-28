use super::SessionState;
use boxxy_core_toolbox::ApprovalOutcome;

impl SessionState {
    /// Resolve any waiting tool approval channel with a user-interruption payload.
    /// Returns `true` if a tool was waiting for approval (meaning the active turn is
    /// still alive and will immediately consume the interruption via the tool output,
    /// so the caller must NOT abort or spawn a duplicate turn).
    pub fn drain_pending_with_interruption(&mut self, message: &str) -> bool {
        self.terminal_command_accepted = false;
        self.pending_inject_command = false;
        self.pending_unaccepted_inject_cmd = None;
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
    /// Returns `true` if any pending tool approval channel was active and rejected.
    pub fn drain_pending_cancel(&mut self) -> bool {
        self.terminal_command_accepted = false;
        self.pending_inject_command = false;
        let mut cancelled = if let Some(cmd) = self.pending_unaccepted_inject_cmd.take() {
            let note = format!("\n\n[Proposed command `{cmd}` — rejected by the user]");
            let mut appended = false;
            if let Some(rig::message::Message::Assistant(turn)) = self.history.last_mut() {
                for item in turn.content.iter_mut().rev() {
                    if let rig::message::AssistantContent::Text(t) = item {
                        t.text.push_str(&note);
                        appended = true;
                        break;
                    }
                }
            }
            if !appended {
                let (spk_name, spk_cid) = self.effective_speaker();
                let tagged = crate::engine::turn::history::tag_assistant_text(
                    &spk_cid,
                    &spk_name,
                    note.trim_start(),
                );
                self.history.push(rig::message::Message::assistant(tagged));
            }
            true
        } else {
            false
        };
        if let Some(summary) = self.pending_proposal_summary.take() {
            let (spk_name, spk_cid) = self.effective_speaker();
            let note = format!("[Proposed {summary} — rejected by the user]");
            let tagged = crate::engine::turn::history::tag_assistant_text(&spk_cid, &spk_name, &note);
            self.history.push(rig::message::Message::assistant(tagged));
            cancelled = true;
        }
        if let Some(reply) = self.pending_terminal_reply.take() {
            let _ = reply.send(Err("[USER_EXPLICIT_REJECT]".to_string()));
            cancelled = true;
        }
        if let Some(reply) = self.pending_file_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
            cancelled = true;
        }
        if let Some(reply) = self.pending_file_delete_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
            cancelled = true;
        }
        if let Some(reply) = self.pending_kill_process_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
            cancelled = true;
        }
        if let Some(reply) = self.pending_background_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
            cancelled = true;
        }
        if let Some(reply) = self.pending_get_clipboard_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
            cancelled = true;
        }
        if let Some(reply) = self.pending_set_clipboard_reply.take() {
            let _ = reply.send(ApprovalOutcome::Rejected);
            cancelled = true;
        }
        cancelled
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
            incognito: false,
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
            terminal_command_accepted: false,
            pending_inject_command: false,
            pending_unaccepted_inject_cmd: None,
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
            title_requested: false,
            pending_lazy_diagnosis: None,
            persistent_agent: None,
            current_claw_model: None,
            current_context_window: None,
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
            sleep_commands: Vec::new(),
            awaiting_tasks: Vec::new(),
            tx_self: tx,
            mcp_handle: None,
            active_speaker: None,
            roundtable: crate::engine::session::roundtable::RoundtableState::default(),
            next_roundtable_turn: None,
            read_file_hashes: HashMap::new(),
            current_turn_snapshots: Vec::new(),
            last_turn_snapshots: Vec::new(),
            current_turn_trace: Vec::new(),
            generated_images_this_turn: Vec::new(),
            current_turn_user_images: Vec::new(),
            in_flight_snapshot: Arc::new(std::sync::Mutex::new(
                crate::engine::session::InFlightSnapshot::default(),
            )),
            last_turn_paused_queue: false,
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
