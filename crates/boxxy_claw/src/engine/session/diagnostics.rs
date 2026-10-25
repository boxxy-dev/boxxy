use std::collections::VecDeque;
use std::sync::Arc;

use crate::engine::{ClawEngineEvent, ClawEnvironment, ClawMessage, turn::spawn_turn};

use super::ClawSession;

impl ClawSession {
    /// Returns `true` if a non-zero exit code represents a normal non-error outcome
    /// (such as `SIGINT`/`SIGQUIT` or `grep`/`diff`/`test`/`[` returning 1).
    pub(super) fn should_ignore_exit_code(exit_code: i32, snapshot: &str) -> bool {
        if exit_code == 0 || exit_code == 130 || exit_code == 131 {
            return true;
        }
        if exit_code == 1 {
            let cmd = snapshot.lines().next().unwrap_or("");
            let first_word = cmd.split_whitespace().next().unwrap_or("");
            if matches!(first_word, "grep" | "diff" | "test" | "[") {
                return true;
            }
        }
        false
    }

    pub(super) async fn handle_command_finished(
        &mut self,
        exit_code: i32,
        snapshot: String,
        cwd: String,
        session_ctx: &str,
        env: &Arc<dyn ClawEnvironment>,
        current_dir: &mut String,
        current_turn: &mut Option<tokio::task::JoinHandle<()>>,
        backlog: &mut VecDeque<ClawMessage>,
    ) {
        *current_dir = cwd.clone();

        let workspace = crate::registry::workspace::global_workspace().await;
        let last_cmd = snapshot.lines().next().unwrap_or("").to_string();
        workspace
            .update_pane_state(
                self.pane_id.clone(),
                Some(self.session_id.clone()),
                Some(self.name.clone()),
                current_dir.clone(),
                Some(last_cmd),
                Some(snapshot.clone()),
            )
            .await;

        let mut state_lock = self.state.lock().await;
        if let Some(reply) = state_lock.pending_terminal_reply.take() {
            state_lock.pending_proposal_summary = None;
            if exit_code == 0 {
                let _ = reply.send(Ok(snapshot));
            } else {
                let _ = reply.send(Err(format!(
                    "Command failed with exit code {exit_code}:\n{snapshot}"
                )));
            }
        } else if !Self::should_ignore_exit_code(exit_code, &snapshot) {
            let prompt = format!(
                "The user's command failed with exit code {exit_code}. Please analyze the terminal snapshot and suggest a fix."
            );

            // Sleep mode gate — short-circuit before we decide anything else.
            if state_lock.status != crate::engine::AgentStatus::Waiting {
                return;
            }

            let agent_originated = state_lock.pending_inject_command;
            state_lock.pending_inject_command = false;

            if !agent_originated {
                state_lock.pending_lazy_diagnosis =
                    Some((prompt, snapshot, cwd));
                drop(state_lock);
                let character_id = self.character_id.clone();
                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::LazyErrorIndicator {
                        visible: true,
                        agent_name: self.name.clone(),
                        character_id,
                    })
                    .await;
                return;
            }

            drop(state_lock);
            if current_turn.is_some() {
                backlog.push_back(ClawMessage::CommandFinished {
                    exit_code,
                    snapshot,
                    cwd,
                });
                return;
            }

            *current_turn = Some(spawn_turn(
                self.pane_id.clone(),
                self.session_id.clone(),
                self.name.clone(),
                &prompt,
                &snapshot,
                session_ctx,
                cwd,
                false,
                env.clone(),
                self.db.clone(),
                self.state.clone(),
                self.tx_ui.clone(),
                self.tx_self.clone(),
                None,
                vec![],
            ));
        }
    }

    pub(super) async fn handle_request_lazy_diagnosis(
        &mut self,
        session_ctx: &str,
        env: &Arc<dyn ClawEnvironment>,
        current_turn: &mut Option<tokio::task::JoinHandle<()>>,
        backlog: &mut VecDeque<ClawMessage>,
    ) {
        let mut state_lock = self.state.lock().await;
        if let Some((prompt, snapshot, cwd)) = state_lock.pending_lazy_diagnosis.take() {
            drop(state_lock);
            if let Some(handle) = current_turn.take() {
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
            backlog.clear();
            *current_turn = Some(spawn_turn(
                self.pane_id.clone(),
                self.session_id.clone(),
                self.name.clone(),
                &prompt,
                &snapshot,
                session_ctx,
                cwd,
                false,
                env.clone(),
                self.db.clone(),
                self.state.clone(),
                self.tx_ui.clone(),
                self.tx_self.clone(),
                None,
                vec![],
            ));
        }
    }
}
