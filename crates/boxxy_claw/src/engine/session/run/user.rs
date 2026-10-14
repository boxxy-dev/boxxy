use log::debug;

use crate::engine::session::ClawSession;
use crate::engine::{ClawEngineEvent, ClawMessage, turn::spawn_turn};

use super::context::RunContext;

impl ClawSession {
    pub(super) async fn handle_claw_query(
        &mut self,
        ctx: &mut RunContext,
        session_ctx: &str,
        query: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
    ) {
        ctx.current_dir = cwd.clone();
        self.send_ui(ClawEngineEvent::UserMessage {
            content: query.clone(),
        })
        .await;

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

        ctx.workspace
            .update_pane_state(
                self.pane_id.clone(),
                Some(self.session_id.clone()),
                Some(self.name.clone()),
                ctx.current_dir.clone(),
                Some(query.clone()),
                Some(snapshot.clone()),
            )
            .await;

        debug!(
            "Pane {} ({}): Direct Claw query: {query}. Starting analysis.",
            self.pane_id, self.name
        );
        ctx.current_turn = Some(spawn_turn(
            self.pane_id.clone(),
            self.session_id.clone(),
            self.name.clone(),
            &query,
            &snapshot,
            session_ctx,
            cwd,
            false,
            ctx.env.clone(),
            self.db.clone(),
            self.state.clone(),
            self.tx_ui.clone(),
            self.tx_self.clone(),
            None,
            image_attachments,
        ));
    }

    pub(super) async fn handle_user_message(
        &mut self,
        ctx: &mut RunContext,
        session_ctx: &str,
        message: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
    ) {
        ctx.current_dir = cwd.clone();
        self.send_ui(ClawEngineEvent::UserMessage {
            content: message.clone(),
        })
        .await;

        debug!(
            "Pane {} ({}): User reply: {message}. Checking for pending tools.",
            self.pane_id, self.name
        );

        ctx.workspace
            .update_pane_state(
                self.pane_id.clone(),
                Some(self.session_id.clone()),
                Some(self.name.clone()),
                ctx.current_dir.clone(),
                None,
                Some(snapshot.clone()),
            )
            .await;

        let mut state_lock = self.state.lock().await;
        let fulfilled = state_lock.drain_pending_with_interruption(&message);

        if fulfilled {
            debug!(
                "Pane {} ({}): Fulfilled pending tool with user feedback.",
                self.pane_id, self.name
            );
        } else {
            drop(state_lock);
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
            ctx.current_turn = Some(spawn_turn(
                self.pane_id.clone(),
                self.session_id.clone(),
                self.name.clone(),
                &message,
                &snapshot,
                session_ctx,
                cwd,
                false,
                ctx.env.clone(),
                self.db.clone(),
                self.state.clone(),
                self.tx_ui.clone(),
                self.tx_self.clone(),
                None,
                image_attachments,
            ));
        }
    }

    pub(super) async fn handle_delegated_task(
        &mut self,
        ctx: &mut RunContext,
        source_agent_name: String,
        prompt: String,
        request_id: uuid::Uuid,
    ) {
        if ctx.current_turn.is_some() {
            debug!(
                "Pane {} ({}): Agent is busy. Queueing delegated task from {}.",
                self.pane_id, self.name, source_agent_name
            );
            ctx.backlog.push_back(ClawMessage::DelegatedTask {
                source_agent_name,
                prompt,
                request_id,
            });
            return;
        }

        let snapshot = ctx
            .workspace
            .get_pane_snapshot(self.pane_id.clone())
            .await
            .unwrap_or_default();

        let full_prompt = format!(
            "*Task delegated from agent '{}'*: {}",
            source_agent_name, prompt
        );

        debug!(
            "Pane {} ({}): Received delegated task from {}.",
            self.pane_id, self.name, source_agent_name
        );

        ctx.current_turn = Some(spawn_turn(
            self.pane_id.clone(),
            self.session_id.clone(),
            self.name.clone(),
            &full_prompt,
            &snapshot,
            &self.session_context,
            ctx.current_dir.clone(),
            false,
            ctx.env.clone(),
            self.db.clone(),
            self.state.clone(),
            self.tx_ui.clone(),
            self.tx_self.clone(),
            Some(request_id),
            vec![],
        ));
    }
}
