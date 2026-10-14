use log::debug;

use crate::engine::ClawEngineEvent;
use crate::engine::session::ClawSession;

use super::context::RunContext;

impl ClawSession {
    pub(super) async fn handle_abort(&mut self, ctx: &mut RunContext) {
        if let Some(handle) = ctx.current_turn.take() {
            handle.abort();
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::AgentThinking {
                    agent_name: self.name.clone(),
                    character_id: self.character_id.clone(),
                    is_thinking: false,
                })
                .await;
        }
        ctx.backlog.clear();

        let mut state_lock = self.state.lock().await;
        state_lock.status = crate::engine::AgentStatus::Waiting;

        if let Some(reply) = state_lock.pending_terminal_reply.take() {
            let _ = reply.send(Err("[ABORT]".to_string()));
        }
        if let Some(reply) = state_lock.pending_file_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = state_lock.pending_file_delete_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = state_lock.pending_kill_process_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = state_lock.pending_get_clipboard_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = state_lock.pending_set_clipboard_reply.take() {
            let _ = reply.send(false);
        }
    }

    pub(super) async fn handle_cancel_pending(&mut self) {
        let mut state_lock = self.state.lock().await;
        state_lock.drain_pending_cancel();
        drop(state_lock);

        debug!("Pane {}: User cancelled pending proposals.", self.pane_id);
        let character_id = self.character_id.clone();
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ProposalResolved {
                agent_name: self.name.clone(),
                character_id,
                approved: false,
            })
            .await;
    }
}
