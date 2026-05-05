pub mod history;
pub mod persistence;
pub mod skills;
pub mod snapshot;

use crate::engine::{
    ClawEngineEvent, ClawEnvironment, ClawMessage, dispatcher::extract_command_and_clean,
    persist_visual_event, session::SessionState,
};
use boxxy_claw_protocol::UsageWrapper;
use boxxy_db::Db;
use log::info;
use std::sync::Arc;
use tokio::sync::Mutex;

pub fn spawn_turn(
    pane_id: String,
    session_id: String,
    agent_name: String,
    prompt: &str,
    snapshot: &str,
    _session_ctx: &str,
    cwd: String,
    is_new_task: bool,
    env: Arc<dyn ClawEnvironment>,
    db: Arc<Mutex<Option<Db>>>,
    state: Arc<Mutex<SessionState>>,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    tx_self: async_channel::Sender<ClawMessage>,
    delegate_reply_id: Option<uuid::Uuid>,
    image_attachments: Vec<String>,
) -> tokio::task::JoinHandle<()> {
    let prompt_clone = prompt.to_string();
    let session_id_clone = session_id.clone();
    let snapshot_clone = snapshot.to_string();
    let cwd_clone = cwd.clone();
    let images_clone = image_attachments.clone();
    let session_ctx_clone = _session_ctx.to_string();

    tokio::spawn(async move {
        let character_id = {
            let state_lock = state.lock().await;
            state_lock.character_id.clone()
        };

        let _ = tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name: agent_name.clone(),
                character_id,
                is_thinking: true,
            })
            .await;

        // Cleanup R4: Simplified history clear
        if is_new_task {
            state.lock().await.history.clear();
        }

        // --- DEADLOCK PREVENTION: Brief DB lock to clone handle ---
        let db_opt = {
            let db_guard = db.lock().await;
            db_guard.clone()
        };
        let past_memories =
            crate::engine::context::retrieve_memories(&db_opt, &prompt_clone, &cwd_clone).await;

        let settings = boxxy_preferences::Settings::load();

        // 1. Build Skills Context
        let (active_skills_text, _available_skills_text) =
            skills::build_skills_context(&prompt_clone, &pane_id).await;

        // 2. Fetch Global Workspace Radar
        let workspace = crate::registry::workspace::global_workspace().await;
        let radar = workspace.get_global_radar(pane_id.clone()).await;

        let mut tui_warning = String::new();
        let all_agents = workspace.get_all_agents().await;
        if let Some(me) = all_agents.iter().find(|a| a.id == pane_id) {
            let status = &me.status;
            if status.starts_with("Running: ") {
                tui_warning = format!(
                    "\n--- TUI WARNING ---\nYour pane is currently running an interactive application: {}.\nYou cannot run standard bash commands or delegate tasks to yourself to run bash commands. To interact with this application, you MUST use `send_keystrokes_to_pane` with your own agent name ('{}'). Note: You do not know the exact internal state (e.g. insert mode vs normal mode in vim), so send escape characters (\\e) first if necessary.\n-------------------\n",
                    status, me.name
                );
            }
        }

        // --- ATOMIC STATE SNAPSHOT ---
        // Lock state once to extract ALL needed fields and update history/hash.
        // CRITICAL: Do NOT call create_claw_agent while holding this lock!
        let (
            persistent_agent,
            mcp_handle,
            web_search_enabled,
            history,
            history_len,
            full_prompt,
            character_id,
        ) = {
            let mut state_lock = state.lock().await;

            // 3. Process Snapshot
            let final_snapshot_text =
                snapshot::process_snapshot(&snapshot_clone, &mut state_lock.last_snapshot_hash);

            let now = chrono::Local::now();
            let identity = format!("Agent Name: **{}**\nUnique ID: `{}`", agent_name, pane_id);

            let turn_context = format!(
                "## YOUR IDENTITY\n\
                {}\n\
                \n\
                ## CURRENT TURN CONTEXT\n\
                System Time: `{}`\n\
                CWD: `{}`\n\
                {}\n\
                {}\n\
                {}\n\
                {}\n",
                identity,
                now.to_rfc3339(),
                cwd_clone,
                active_skills_text,
                past_memories,
                radar,
                tui_warning
            );

            let full_prompt = format!(
                "{}\n\n{}\n\nTerminal Snapshot:\n```\n{}\n```",
                prompt_clone, turn_context, final_snapshot_text
            );

            // --- AMNESIA FIX: IMMEDIATE PUSH ---
            state_lock
                .history
                .push(rig::message::Message::user(full_prompt.clone()));

            let h = state_lock.history.clone();
            let l = h.len();
            let mcp = state_lock.mcp_handle.clone();
            let web = state_lock.web_search_enabled;
            let agent = state_lock.persistent_agent.take();
            let char_id = state_lock.character_id.clone();

            (agent, mcp, web, h, l, full_prompt, char_id)
        };

        // 4. Resolve Agent (after lock is released to prevent deadlock)
        let agent = if let Some(agent) = persistent_agent {
            agent
        } else {
            let config = crate::engine::agent_config::AgentConfig::from_env(
                &settings,
                web_search_enabled,
                session_ctx_clone.clone(),
            );
            crate::engine::agent::create_claw_agent(
                &config,
                &env,
                &cwd_clone,
                tx_ui.clone(),
                state.clone(),
                db.clone(),
                session_id_clone.clone(),
                pane_id.clone(),
                mcp_handle,
            )
            .await
            .expect("Failed to create claw agent")
        };

        // 5. Prepare History
        let mut final_history = history::prepare_query_history(history, history_len);

        let mut is_multimodal = false;
        let mut user_msg = vec![rig::message::Message::User {
            content: rig::OneOrMany::one(rig::message::UserContent::text(full_prompt.clone())),
        }];

        if !images_clone.is_empty() {
            let mut contents = vec![rig::message::UserContent::text(full_prompt.clone())];
            for b64 in images_clone {
                contents.push(rig::message::UserContent::image_base64(
                    b64,
                    Some(rig::message::ImageMediaType::PNG),
                    None,
                ));
            }
            if let Ok(many) = rig::OneOrMany::many(contents) {
                user_msg = vec![rig::message::Message::User { content: many }];
                is_multimodal = true;
            }
        }

        let query_for_chat = if is_multimodal {
            final_history.push(user_msg.into_iter().next().unwrap());
            ""
        } else {
            &full_prompt
        };

        let start_time = std::time::Instant::now();
        match agent.chat(query_for_chat, final_history).await {
            Ok((response, usage)) => {
                let duration = start_time.elapsed();
                let _ = tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        is_thinking: false,
                    })
                    .await;

                if duration.as_secs() >= 7 {
                    let mut summary = response.trim().to_string();
                    if summary.is_empty() {
                        summary = "Task completed silently.".to_string();
                    } else if summary.lines().count() > 1 || summary.len() > 100 {
                        // Extract first non-empty line
                        if let Some(first_line) = summary.lines().find(|l| !l.trim().is_empty()) {
                            summary = first_line.trim().to_string();
                            if summary.len() > 100 {
                                summary.truncate(97);
                                summary.push_str("...");
                            }
                        }
                    }

                    let _ = tx_ui
                        .send(ClawEngineEvent::LongTaskCompleted {
                            agent_name: agent_name.clone(),
                            character_id: character_id.clone(),
                            duration_secs: duration.as_secs(),
                            message: summary,
                        })
                        .await;
                }

                {
                    let mut state_lock = state.lock().await;
                    state_lock.persistent_agent = Some(agent.clone());
                    state_lock
                        .history
                        .push(rig::message::Message::assistant(response.clone()));

                    if let Some(usage) = usage {
                        state_lock.total_tokens += usage.total_tokens as u64;
                    }
                }

                let p_ctx = persistence::PersistenceContext {
                    pane_id: pane_id.clone(),
                    session_id: session_id_clone.clone(),
                    agent_name: agent_name.clone(),
                    full_prompt: full_prompt.clone(),
                    response: response.clone(),
                    cwd: cwd_clone.clone(),
                    settings: settings.clone(),
                    db: db.clone(),
                    tx_self: tx_self.clone(),
                };
                persistence::perform_persistence(p_ctx, state.clone()).await;

                let mut command_opt = None;
                let mut clean_diagnosis = response.clone();

                let extracted = extract_command_and_clean(&response);
                if extracted.0.is_some() {
                    command_opt = extracted.0;
                    clean_diagnosis = extracted.1;
                }

                if clean_diagnosis.trim() == "[SILENT_ACK]" {
                    info!(
                        "Pane {} ({}): Agent acknowledged rejection silently.",
                        pane_id, agent_name
                    );
                    let _ = tx_ui.send(ClawEngineEvent::DismissDrawer).await;
                } else if clean_diagnosis.trim().is_empty() && command_opt.is_none() {
                    info!(
                        "Pane {} ({}): Agent response was empty.",
                        pane_id, agent_name
                    );
                } else if let Some(command) = command_opt {
                    let event = ClawEngineEvent::InjectCommand {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        command,
                        diagnosis: clean_diagnosis,
                        usage: usage.map(UsageWrapper::from),
                    };
                    state.lock().await.pending_inject_command = true;
                    persist_visual_event(
                        db.clone(),
                        session_id_clone.clone(),
                        pane_id.clone(),
                        &event,
                    );
                    let _ = tx_ui.send(event).await;
                } else {
                    let event = ClawEngineEvent::DiagnosisComplete {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        diagnosis: clean_diagnosis,
                        usage: usage.map(UsageWrapper::from),
                    };
                    persist_visual_event(
                        db.clone(),
                        session_id_clone.clone(),
                        pane_id.clone(),
                        &event,
                    );
                    let _ = tx_ui.send(event).await;
                }

                if let Some(request_id) = delegate_reply_id {
                    let _ = tx_ui
                        .send(ClawEngineEvent::DelegatedTaskReply {
                            request_id,
                            result: response.clone(),
                        })
                        .await;
                }
            }
            Err(e) => {
                let duration = start_time.elapsed();
                let _ = tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        is_thinking: false,
                    })
                    .await;

                if duration.as_secs() >= 7 {
                    let _ = tx_ui
                        .send(ClawEngineEvent::LongTaskCompleted {
                            agent_name: agent_name.clone(),
                            character_id: character_id.clone(),
                            duration_secs: duration.as_secs(),
                            message: "Agent turn ended with an error after a long duration.".to_string(),
                        })
                        .await;
                }

                state.lock().await.persistent_agent = Some(agent);

                log::error!(
                    "Pane {} ({}): Boxxy-Claw agent failed: {e}",
                    pane_id,
                    agent_name
                );

                let error_msg = format!("{}", e);
                let friendly_msg = if error_msg.contains("does not support tools") {
                    "**Error:** The selected Ollama model does not support tool calling.\n\nBoxxy-Claw requires a highly capable reasoning model with native tool support (like `llama3.2`, `qwen2.5`, or `mistral`) to interact with your system.\n\nPlease select a different model for Boxxy-Claw in the Model Selection menu (Ctrl+Shift+P).".to_string()
                } else {
                    format!("**Boxxy-Claw encountered an error:**\n```\n{}\n```", e)
                };

                let event = ClawEngineEvent::DiagnosisComplete {
                    agent_name: agent_name.clone(),
                    character_id: character_id.clone(),
                    diagnosis: friendly_msg,
                    usage: None,
                };
                persist_visual_event(
                    db.clone(),
                    session_id_clone.clone(),
                    pane_id.clone(),
                    &event,
                );
                let _ = tx_ui.send(event).await;

                if let Some(request_id) = delegate_reply_id {
                    let _ = tx_ui
                        .send(ClawEngineEvent::DelegatedTaskReply {
                            request_id,
                            result: format!("Error: {}", e),
                        })
                        .await;
                }
                let _ = tx_self.send(ClawMessage::TurnFinished).await;
            }
        }
    })
}
