use log::debug;

use crate::engine::ClawEngineEvent;
use crate::engine::session::ClawSession;

use super::context::RunContext;

impl ClawSession {
    pub(super) async fn handle_abort(&self, ctx: &mut RunContext) {
        let (thinking_stop, salvaged_usage) = {
            let mut state_lock = self.state.lock().await;
            // Salvage completed mid-turn tool steps BEFORE calling `handle.abort()` so
            // `InFlightClearGuard::drop()` on multi-threaded Tokio workers never clears
            // `in_flight_snapshot.messages` ahead of salvage.
            let (_, salvaged_usage) = state_lock.salvage_in_flight_tool_steps();
            state_lock.current_turn_user_images.clear();
            state_lock.generated_images_this_turn.clear();
            let spk = state_lock.effective_speaker();
            let had_turn = ctx.current_turn.take().is_some_and(|handle| {
                handle.abort();
                true
            });
            state_lock.last_turn_paused_queue = false;
            state_lock.status = crate::engine::AgentStatus::Waiting;
            state_lock.active_speaker = None;
            state_lock.next_roundtable_turn = None;
            state_lock.roundtable.pause_for_interjection();
            state_lock.drain_pending_cancel();
            drop(state_lock);
            (had_turn.then_some(spk), salvaged_usage)
        };

        if let Some((thinking_name, thinking_cid)) = thinking_stop {
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
        if !ctx.queued_prompts.is_empty() && !ctx.queue_paused {
            ctx.queue_paused = true;
            self.emit_queue_updated(ctx).await;
        }

        self.persist_history_if_present().await;
        if salvaged_usage {
            self.emit_usage_updated().await;
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
