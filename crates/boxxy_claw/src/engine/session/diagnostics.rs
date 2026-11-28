use std::collections::VecDeque;
use std::sync::Arc;

use crate::engine::{ClawEngineEvent, ClawEnvironment, ClawMessage, turn::spawn_turn};

use super::ClawSession;

impl ClawSession {
    /// Checks whether an executed shell command matches an unaccepted `InjectCommand` preview
    /// (`short_unaccepted_cmd` from `turn/mod.rs`, which truncates > 72-byte lines with `…`).
    fn command_matches_proposed(executed_cmd: &str, proposed_short: &str) -> bool {
        let exec_first = executed_cmd.lines().next().unwrap_or(executed_cmd).trim();
        let prop_trimmed = proposed_short.trim();
        if exec_first.is_empty() || prop_trimmed.is_empty() {
            return false;
        }
        if let Some(prefix) = prop_trimmed.strip_suffix('…') {
            exec_first.starts_with(prefix)
        } else {
            exec_first == prop_trimmed
        }
    }

    /// Returns `true` if a non-zero exit code represents a normal non-error outcome
    /// (such as `SIGINT`/`SIGQUIT` or `grep`/`rg`/`diff`/`test`/`[` returning 1).
    pub(super) fn should_ignore_exit_code(exit_code: i32, snapshot: &str) -> bool {
        if exit_code == 0 || exit_code == 130 || exit_code == 131 {
            return true;
        }
        if exit_code == 1 {
            let cmd = boxxy_claw_protocol::extract_last_command_from_snapshot(snapshot);
            let first_word = cmd
                .split_whitespace()
                .find(|tok| {
                    // Skip leading `VAR=val` inline environment assignments before the binary name.
                    !tok.split_once('=').is_some_and(|(k, _)| {
                        !k.is_empty()
                            && k.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                            && !k.bytes().next().unwrap_or(b'0').is_ascii_digit()
                    })
                })
                .unwrap_or("");
            if matches!(
                first_word,
                "grep" | "egrep" | "fgrep" | "rg" | "diff" | "test" | "[" | "[["
            ) {
                return true;
            }
        }
        false
    }

    pub(super) async fn handle_command_finished(
        &self,
        exit_code: i32,
        snapshot: String,
        cwd: String,
        session_ctx: &str,
        env: &Arc<dyn ClawEnvironment>,
        current_dir: &mut String,
        current_turn: &mut Option<tokio::task::JoinHandle<()>>,
        backlog: &mut VecDeque<ClawMessage>,
    ) {
        if self.incognito {
            return;
        }
        *current_dir = cwd.clone();

        let workspace = crate::registry::workspace::global_workspace().await;
        let last_cmd =
            boxxy_claw_protocol::extract_last_command_from_snapshot(&snapshot).to_string();
        let cmd_opt = (!last_cmd.is_empty()).then_some(last_cmd.clone());
        workspace
            .update_pane_state(
                self.pane_id.clone(),
                Some(self.session_id.clone()),
                Some(self.name.clone()),
                current_dir.clone(),
                cmd_opt,
                Some(snapshot.clone()),
            )
            .await;

        let resolved_cross_pane = workspace
            .complete_pending_command(&self.pane_id, exit_code, snapshot.clone())
            .await;
        workspace
            .publish_event(crate::engine::ClawEvent::ProcessExited {
                pane_id: self.pane_id.clone(),
                exit_code,
            })
            .await;
        if resolved_cross_pane {
            return;
        }

        let mut state_lock = self.state.lock().await;
        state_lock.terminal_command_accepted = false;
        let ghost_dismissed_proposal = if let Some(unaccepted) =
            state_lock.pending_unaccepted_inject_cmd.take()
        {
            if Self::command_matches_proposed(&last_cmd, &unaccepted) {
                false
            } else {
                // User never clicked "Accept & Run" and instead ran a different command manually
                // in the terminal ("ghost dismiss" per State Machine Rule 3): record that the
                // proposed command was skipped and do NOT misclassify user commands as agent-owned.
                state_lock.pending_inject_command = false;
                let note = format!("\n\n[Proposed command `{unaccepted}` — dismissed without running]");
                let mut appended = false;
                if let Some(rig::message::Message::Assistant(turn)) = state_lock.history.last_mut()
                {
                    for item in turn.content.iter_mut().rev() {
                        if let rig::message::AssistantContent::Text(t) = item {
                            t.text.push_str(&note);
                            appended = true;
                            break;
                        }
                    }
                }
                if !appended {
                    let (spk_name, spk_cid) = state_lock.effective_speaker();
                    let tagged = crate::engine::turn::history::tag_assistant_text(
                        &spk_cid,
                        &spk_name,
                        note.trim_start(),
                    );
                    state_lock
                        .history
                        .push(rig::message::Message::assistant(tagged));
                }
                true
            }
        } else {
            false
        };
        if state_lock.status == crate::engine::AgentStatus::Sleep {
            let trimmed_cmd = last_cmd.trim();
            if !trimmed_cmd.is_empty() {
                if state_lock.sleep_commands.len() >= 64 {
                    state_lock.sleep_commands.remove(0);
                }
                state_lock
                    .sleep_commands
                    .push((trimmed_cmd.to_string(), exit_code));
            }
        }
        if let Some(reply) = state_lock.pending_terminal_reply.take() {
            state_lock.pending_proposal_summary = None;
            state_lock.pending_inject_command = false;
            if exit_code == 0 {
                let _ = reply.send(Ok(snapshot));
            } else {
                let _ = reply.send(Err(format!(
                    "Command failed with exit code {exit_code}:\n{snapshot}"
                )));
            }
            drop(state_lock);
            if ghost_dismissed_proposal {
                self.persist_history_if_present().await;
            }
        } else if Self::should_ignore_exit_code(exit_code, &snapshot) {
            state_lock.pending_inject_command = false;
            drop(state_lock);
            if ghost_dismissed_proposal {
                self.persist_history_if_present().await;
            }
        } else {
            let prompt = format!(
                "The user's command failed with exit code {exit_code}. Please analyze the terminal snapshot and suggest a fix."
            );

            // Sleep mode gate — short-circuit before we decide anything else.
            if state_lock.status != crate::engine::AgentStatus::Waiting {
                drop(state_lock);
                if ghost_dismissed_proposal {
                    self.persist_history_if_present().await;
                }
                return;
            }

            let agent_originated = state_lock.pending_inject_command;
            state_lock.pending_inject_command = false;

            if !agent_originated {
                state_lock.pending_lazy_diagnosis = Some((prompt, snapshot, cwd));
                drop(state_lock);
                if ghost_dismissed_proposal {
                    self.persist_history_if_present().await;
                }
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
            if ghost_dismissed_proposal {
                self.persist_history_if_present().await;
            }
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
                vec![],
            ));
        }
    }

    pub(super) async fn handle_request_lazy_diagnosis(
        &self,
        session_ctx: &str,
        env: &Arc<dyn ClawEnvironment>,
        current_turn: &mut Option<tokio::task::JoinHandle<()>>,
        backlog: &mut VecDeque<ClawMessage>,
    ) {
        let mut state_lock = self.state.lock().await;
        if let Some((prompt, snapshot, cwd)) = state_lock.pending_lazy_diagnosis.take() {
            if let Some(handle) = current_turn.take() {
                let _ = state_lock.salvage_in_flight_tool_steps();
                handle.abort();
                let spk_name = self.name.clone();
                let spk_cid = self.character_id.clone();
                drop(state_lock);
                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: spk_name,
                        character_id: spk_cid,
                        is_thinking: false,
                    })
                    .await;
            } else {
                drop(state_lock);
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
                vec![],
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_benign_exit_codes_across_osc133_semantic_and_plain_snapshots() {
        let osc133_grep = "\n[PROMPT]\n```text\nuser@host:~/proj$ \n```\n\n[COMMAND]\n```bash\ncargo build\n```\n\n[OUTPUT]\n```text\nFinished\n```\n\n[PROMPT]\n```text\nuser@host:~/proj$ \n```\n\n[COMMAND]\n```bash\nLC_ALL=C grep missing_symbol src/lib.rs\n```\n\n[OUTPUT]\n```text\n```";
        assert!(ClawSession::should_ignore_exit_code(1, osc133_grep));

        let osc133_rg = "\n[PROMPT]\n```text\n$ \n```\n\n[COMMAND]\n```bash\nrg needle crates/\n```\n\n[OUTPUT]\n```text\n```";
        assert!(ClawSession::should_ignore_exit_code(1, osc133_rg));

        // Real error (`exit 2` on grep syntax/missing file, or `cargo check` exit 1/101) must NOT be ignored
        assert!(!ClawSession::should_ignore_exit_code(2, osc133_grep));
        let osc133_cargo_fail = "\n[COMMAND]\n```bash\ncargo test\n```\n\n[OUTPUT]\n```text\ntest failed\n```";
        assert!(!ClawSession::should_ignore_exit_code(1, osc133_cargo_fail));
        assert!(ClawSession::should_ignore_exit_code(130, osc133_cargo_fail));
    }
}
