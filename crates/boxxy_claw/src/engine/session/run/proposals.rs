use log::debug;

use crate::engine::ClawEngineEvent;
use crate::engine::session::ClawSession;

use super::context::RunContext;

impl ClawSession {
    pub(super) async fn handle_abort(&self, ctx: &mut RunContext) {
        if let Some(handle) = ctx.current_turn.take() {
            handle.abort();
            let (thinking_name, thinking_cid) = self.state.lock().await.effective_speaker();
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::AgentThinking {
                    agent_name: thinking_name,
                    character_id: thinking_cid,
                    is_thinking: false,
                })
                .await;
        }
        ctx.backlog.clear();

        {
            let mut state_lock = self.state.lock().await;
            state_lock.status = crate::engine::AgentStatus::Waiting;
            state_lock.active_speaker = None;
            state_lock.next_roundtable_turn = None;
            state_lock.roundtable.pause_for_interjection();
            state_lock.drain_pending_cancel();
        }
        let _ = ctx
            .workspace
            .drain_pending_commands_for_source(&self.pane_id, "[USER_EXPLICIT_REJECT]".to_string())
            .await;
        ctx.workspace.release_all_locks(&self.pane_id).await;
    }

    pub(super) async fn handle_cancel_pending(&self, ctx: &mut RunContext) {
        self.handle_abort(ctx).await;

        debug!("Pane {}: User cancelled pending proposals.", self.pane_id);
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ProposalResolved {
                agent_name: self.name.clone(),
                character_id: self.character_id.clone(),
                approved: false,
            })
            .await;
    }
}
