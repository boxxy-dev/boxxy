use crate::engine::ClawEngineEvent;
use crate::engine::session::SessionState;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct TerminalCommandArgs {
    pub command: String,
    pub explanation: String,
    #[serde(alias = "target", alias = "agent_name")]
    pub target_pane: Option<String>,
}

#[derive(Serialize)]
pub struct TerminalCommandOutput {
    pub success: bool,
    pub output: String,
}

use boxxy_db::Db;

pub struct TerminalCommandTool {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
    pub db: std::sync::Arc<tokio::sync::Mutex<Option<Db>>>,
    pub session_id: String,
    pub pane_id: String,
}

impl Tool for TerminalCommandTool {
    const NAME: &'static str = "terminal_exec";

    type Error = std::io::Error;
    type Args = TerminalCommandArgs;
    type Output = TerminalCommandOutput;

    fn description(&self) -> String {
        "Propose a command to run live in the user's active terminal or a companion workspace pane (prompts 'Accept & Run' in your drawer) and return its output once it finishes. Pass `target_pane` (e.g. 'pane-2') to execute the command inside another open terminal pane while keeping your home pane free. For detached GUI apps or long-running servers, use `run_background_command` instead. Do not repeat the executed command inside a ```bash block in your final response.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute in the terminal."
                },
                "explanation": {
                    "type": "string",
                    "description": "A brief explanation of why this command is being run."
                },
                "target_pane": {
                    "type": "string",
                    "description": "Optional alias (e.g. 'pane-2'), petname, character name, or pane ID of another open terminal pane to execute the command in. Omit to execute in your own pane."
                }
            },
            "required": ["command", "explanation"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let (tx, rx) = tokio::sync::oneshot::channel();

        let (agent_name, character_id, vibe_enabled, home_cwd) = {
            let state = self.state.lock().await;
            let (name, cid) = state.effective_speaker();
            (
                name,
                cid,
                state.vibe_enabled,
                state.current_dir.clone(),
            )
        };

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::ToolCallStarted {
                agent_name: agent_name.clone(),
                character_id: character_id.clone(),
                tool_name: Self::NAME.to_string(),
            })
            .await;

        let workspace = crate::registry::workspace::global_workspace().await;
        let mut resolved_target_id: Option<String> = None;
        let mut resolved_target_label: Option<String> = None;

        if let Some(ref target_raw) = args.target_pane
            && !target_raw.trim().is_empty()
        {
            match workspace.resolve_pane_id_by_name(target_raw).await {
                Some(tid) if tid != self.pane_id => {
                    let alias = workspace
                        .get_pane_alias(&tid)
                        .await
                        .unwrap_or_else(|| target_raw.trim().to_string());
                    resolved_target_id = Some(tid);
                    resolved_target_label = Some(alias);
                }
                Some(_) => {
                    // Points to self.pane_id; execute locally.
                }
                None => {
                    return Ok(TerminalCommandOutput {
                        success: false,
                        output: format!("Target pane '{}' not found in workspace.", target_raw),
                    });
                }
            }
        }

        if let Some(ref target_id) = resolved_target_id {
            workspace
                .register_pending_command(target_id.clone(), Some(self.pane_id.clone()), tx)
                .await;
            let mut state = self.state.lock().await;
            let target_lbl = resolved_target_label.as_deref().unwrap_or(target_id);
            state.pending_proposal_summary = Some(format!(
                "terminal_exec on {target_lbl} `{}` ({})",
                args.command, args.explanation
            ));
        } else {
            let mut state = self.state.lock().await;
            state.pending_terminal_reply = Some(tx);
            state.pending_proposal_summary =
                Some(format!("terminal_exec `{}` ({})", args.command, args.explanation));
        }

        let target_cwd = if let Some(ref target_id) = resolved_target_id {
            workspace
                .get_pane_cwd(target_id.clone())
                .await
                .filter(|c| !c.trim().is_empty())
                .unwrap_or_else(|| home_cwd.clone())
        } else {
            home_cwd
        };
        let settings = boxxy_preferences::Settings::load();
        let auto_approve = vibe_enabled
            && crate::engine::vibe_guard::is_safe_vibe_command(
                &args.command,
                &target_cwd,
                &settings.vibe_trusted_dirs,
            );

        if !auto_approve {
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::AgentThinking {
                    agent_name: agent_name.clone(),
                    character_id: character_id.clone(),
                    is_thinking: false,
                })
                .await;
        }

        let explanation_display = if let Some(ref lbl) = resolved_target_label {
            format!("**[Target: `{lbl}`]** {}", args.explanation)
        } else {
            args.explanation.clone()
        };

        let event = ClawEngineEvent::ProposeTerminalCommand {
            agent_name: agent_name.clone(),
            character_id: character_id.clone(),
            command: args.command.clone(),
            explanation: explanation_display,
            target_pane_id: resolved_target_id,
            target_pane_label: resolved_target_label,
            auto_approve,
            usage: None,
        };

        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        if let Err(e) = self.tx_ui.send(event).await {
            return Err(std::io::Error::other(format!(
                "Failed to send terminal command proposal to UI: {e}"
            )));
        }

        let result = rx.await;
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name,
                character_id,
                is_thinking: true,
            })
            .await;

        match result {
            Ok(Ok(output)) => Ok(TerminalCommandOutput {
                success: true,
                output,
            }),
            Ok(Err(err)) => {
                // If it's a specific user reject, pass it directly
                if err == "[USER_EXPLICIT_REJECT]" {
                    Ok(TerminalCommandOutput {
                        success: false,
                        output: err,
                    })
                } else {
                    Ok(TerminalCommandOutput {
                        success: false,
                        output: err,
                    })
                }
            }
            Err(_) => Ok(TerminalCommandOutput {
                success: false,
                output: "Internal error waiting for terminal command result.".to_string(),
            }),
        }
    }
}
