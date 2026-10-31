pub mod history;
pub mod persistence;
pub mod skills;
pub mod snapshot;

use crate::engine::{
    ClawEngineEvent, ClawEnvironment, ClawMessage, dispatcher::extract_command_and_clean,
    persist_visual_event, session::SessionState,
};
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
        let (speaker, host_agent_name, _host_character_id) = {
            let state_lock = state.lock().await;
            let host_name = if state_lock.agent_name.is_empty() {
                agent_name.clone()
            } else {
                state_lock.agent_name.clone()
            };
            let host_cid = state_lock.character_id.clone();
            let spk = state_lock.active_speaker.clone().unwrap_or_else(|| {
                crate::engine::session::roundtable::SpeakerIdentity {
                    character_id: host_cid.clone(),
                    display_name: host_name.clone(),
                    slug: host_name.to_lowercase().replace(' ', "-"),
                    is_guest: false,
                    home_pane_id: None,
                }
            });
            (spk, host_name, host_cid)
        };
        let agent_name = speaker.display_name.clone();
        let character_id = speaker.character_id.clone();

        let _ = tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name: agent_name.clone(),
                character_id: character_id.clone(),
                is_thinking: true,
            })
            .await;

        // Cleanup R4: Simplified history clear
        if is_new_task {
            let mut state_lock = state.lock().await;
            state_lock.history.clear();
            state_lock.flushed_watermark = 0;
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

        // 2. Fetch Global Workspace Radar + Detached Background Shells
        let workspace = crate::registry::workspace::global_workspace().await;
        let mut radar = workspace.get_global_radar(pane_id.clone()).await;
        if let Ok(detached) = env.list_detached_shells().await
            && !detached.is_empty()
        {
            radar.push_str("\n\n--- DETACHED BACKGROUND SHELLS ---\n");
            radar.push_str(
                "The following terminal sessions are running in the background (detached in boxxy-agent). You can restore any of them into a new tab using `restore_background_shell`:\n",
            );
            for (pid, _pane_id, idle_secs, shell_cwd, fg_proc, waiting_for_input) in detached {
                let proc_label = if fg_proc.is_empty() {
                    "shell".to_string()
                } else {
                    fg_proc
                };
                let wait_note = if waiting_for_input {
                    " [WAITING FOR USER INPUT]"
                } else {
                    ""
                };
                radar.push_str(&format!(
                    "- PID {}: `{}` in `{}` (idle {}s){}\n",
                    pid, proc_label, shell_cwd, idle_secs, wait_note
                ));
            }
            radar.push_str("----------------------------------");
        }

        let mut tui_warning = String::new();
        let all_agents = workspace.get_all_agents().await;
        if let Some(me) = all_agents.iter().find(|a| a.id == pane_id) {
            let status = &me.status;
            if status.starts_with("Running: ") {
                tui_warning.push_str(&format!(
                    "\n--- TUI WARNING ---\nYour pane is currently running an interactive application: {}.\nYou cannot run standard bash commands or delegate tasks to yourself to run bash commands. To interact with this application, you MUST use `send_keystrokes_to_pane` with your own agent name ('{}'). Note: You do not know the exact internal state (e.g. insert mode vs normal mode in vim), so send escape characters (\\e) first if necessary.\n-------------------\n",
                    status, me.name
                ));
            }
        }
        for peer in all_agents.iter().filter(|a| a.id != pane_id) {
            if peer.status.starts_with("Running: ") {
                tui_warning.push_str(&format!(
                    "\n--- COMPANION PANE TUI / ACTIVE PROCESS NOTICE ---\nPane `{}` (ID: `{}`) is currently `{}`. If it is running an interactive TUI (`top`, `htop`, `vim`, `less`, etc.) or you need to quit/interrupt it, use `send_keystrokes_to_pane(agent_name=\"{}\", keys=\"q\")` (or `\\x03` for Ctrl+C) instead of `terminal_exec`.\n--------------------------------------------------\n",
                    peer.alias, peer.id, peer.status, peer.alias
                ));
            }
        }

        // 2b. Swarm Peer Home-Pane Context Bridge (if Guest character is claimed in another pane)
        let mut guest_home_context = String::new();
        if let Some(ref home_pane_id) = speaker.home_pane_id {
            let alias = workspace
                .get_pane_alias(home_pane_id)
                .await
                .unwrap_or_else(|| home_pane_id.clone());
            let home_cwd = workspace
                .get_pane_cwd(home_pane_id.clone())
                .await
                .unwrap_or_default();
            let home_snap = workspace
                .get_pane_snapshot(home_pane_id.clone())
                .await
                .unwrap_or_default();
            let home_status = all_agents
                .iter()
                .find(|a| &a.id == home_pane_id)
                .map(|a| a.status.as_str())
                .unwrap_or("Idle");
            guest_home_context = format!(
                "\n--- YOUR HOME PANE CONTEXT (SWARM PEER BRIDGE) ---\n\
                You are actively claimed in your own home terminal pane `{alias}` (ID: `{home_pane_id}`, CWD: `{home_cwd}`, Status: `{home_status}`).\n\
                You are currently participating as a Guest in **{host_agent_name}**'s overlay in pane `{pane_id}`.\n\
                Latest terminal snapshot from YOUR home pane (`{alias}`):\n```\n{home_snap}\n```\n\
                --------------------------------------------------\n"
            );
        }

        // --- ATOMIC STATE SNAPSHOT ---
        // Lock state once to extract ALL needed fields and update history/hash.
        // CRITICAL: Do NOT call create_claw_agent while holding this lock!
        let (
            persistent_agent,
            mcp_handle,
            web_search_enabled,
            vibe_enabled,
            drama_mode,
            history,
            history_len,
            full_prompt,
            character_id,
        ) = {
            let mut state_lock = state.lock().await;
            state_lock.current_dir = cwd_clone.clone();
            if !state_lock.current_turn_snapshots.is_empty() {
                state_lock.last_turn_snapshots =
                    std::mem::take(&mut state_lock.current_turn_snapshots);
            }
            let drama_mode = state_lock.roundtable.drama_mode;

            // 3. Process Snapshot
            let final_snapshot_text =
                snapshot::process_snapshot(&snapshot_clone, &mut state_lock.last_snapshot_hash);

            let mut roundtable_context = String::new();
            if speaker.is_guest || state_lock.roundtable.participants.len() >= 2 {
                let roster: Vec<String> = state_lock
                    .roundtable
                    .participants
                    .iter()
                    .map(|p| {
                        let role = if p.is_guest { "Guest" } else { "Host" };
                        format!("{} (@{}) [{}]", p.display_name, p.slug, role)
                    })
                    .collect();
                let other_mentions: Vec<String> = state_lock
                    .roundtable
                    .participants
                    .iter()
                    .filter(|p| p.character_id != character_id)
                    .map(|p| format!("`@{}`", p.slug))
                    .collect();
                let other_mentions_str = if other_mentions.is_empty() {
                    "their `@character-slug`".to_string()
                } else {
                    other_mentions.join(", ")
                };
                roundtable_context = format!(
                    "\n--- ROUNDTABLE / MULTI-CHARACTER OVERLAY CHAT ---\n\
                    You are speaking as **{agent_name}** (`@{slug}`) in a live multi-character conversation inside **{host_agent_name}**'s overlay.\n\
                    Participants ALREADY in this room: {}.\n\
                    CRITICAL RULES FOR MULTI-CHARACTER CHAT:\n\
                    1. Speak ONLY as **{agent_name}** in your own distinct personality, tone, and technical role. NEVER write dialogue lines on behalf of other characters or prefix your reply with `[Character ... said]:`.\n\
                    2. The participants listed above are ALREADY in the room and queued to speak right after you — do NOT call `invite_character` for them!\n\
                    3. Directly engage with the other character(s) and the user — agree, challenge their arguments, banter in-character, or collaborate on the problem.\n\
                    4. Use inline code or plain ` ```text ` blocks for example commands during a roundtable discussion instead of ` ```bash ` blocks unless the user explicitly asked you to execute a command right now (since ` ```bash ` blocks pause the roundtable for command approval).\n\
                    5. End your message by directly addressing the other character(s) in the room ({other_mentions_str}) with a sharp question, take, or counter-argument so they can reply to you immediately on the next turn!\n\
                    -------------------------------------------------\n",
                    if roster.is_empty() {
                        format!("{host_agent_name} [Host], {agent_name} [Guest]")
                    } else {
                        roster.join(", ")
                    },
                    slug = speaker.slug,
                );
            }

            if drama_mode {
                roundtable_context.push_str(
                    "\n--- VISUAL NOVEL / DRAMA MODE ACTIVE (OVERRIDE ALL LENGTH & LIST FORMATTING) ---\n\
                    1. Speak in ONLY 1 to 2 short, punchy spoken sentences (maximum ~35 words total)!\n\
                    2. STRICTLY FORBIDDEN: Do NOT use bullet points, numbered lists, markdown headers, or multi-paragraph essays!\n\
                    3. Do NOT call routine inspection tools (`get_system_info`, `list_processes`, `sys_shell_exec`) unless the user explicitly asked you to execute a command or inspect a file right now.\n\
                    4. Write pure, dramatic, casual spoken dialogue as if in an anime / visual novel scene or fast group chat!\n\
                    --------------------------------------------------------------------------------\n",
                );
            }

            let now = chrono::Local::now();
            let identity = format!(
                "Agent Name: **{}** (`@{}`)\nUnique ID: `{}`",
                agent_name, speaker.slug, pane_id
            );
            let vibe_ctx = if state_lock.vibe_enabled {
                if crate::engine::vibe_guard::is_cwd_vibe_trusted(
                    &cwd_clone,
                    &settings.vibe_trusted_dirs,
                ) {
                    "Vibe Mode: `ON` (Trusted folder: safe terminal commands via `terminal_exec` and workspace file edits via `replace_in_file`/`file_write` execute automatically without pausing for user confirmation; destructive or out-of-workspace commands still prompt)"
                } else {
                    "Vibe Mode: `ON` (Current folder is not in Trusted Vibe Locations, so commands and edits will still prompt)"
                }
            } else {
                "Vibe Mode: `OFF` (Supervised)"
            };

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
                {}\n\
                {}\n\
                {}\n\
                {}\n",
                identity,
                now.to_rfc3339(),
                cwd_clone,
                vibe_ctx,
                roundtable_context,
                guest_home_context,
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
            let vibe = state_lock.vibe_enabled;
            let agent = if speaker.is_guest {
                None
            } else {
                state_lock.persistent_agent.take()
            };
            let char_id = character_id.clone();

            (
                agent,
                mcp,
                web,
                vibe,
                drama_mode,
                h,
                l,
                full_prompt,
                char_id,
            )
        };

        // 4. Resolve Agent (after lock is released to prevent deadlock)
        let effective_preamble = if speaker.is_guest {
            crate::engine::context::load_session_context(&character_id)
        } else {
            session_ctx_clone.clone()
        };
        let config = crate::engine::agent_config::AgentConfig::from_env(
            &settings,
            web_search_enabled,
            effective_preamble,
        );
        let agent = match persistent_agent {
            Some(agent) if agent.matches_config(&config) => agent,
            _ => crate::engine::agent::create_claw_agent(
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
            .expect("Failed to create claw agent"),
        };

        // 5. Prepare History (Perspective-flipped for the current speaker)
        let final_history = history::prepare_query_history_for_speaker(
            history,
            history_len,
            &character_id,
            &host_agent_name,
        );

        let query_for_chat = if !images_clone.is_empty() {
            let mut contents = vec![rig::message::UserContent::text(full_prompt.clone())];
            for b64 in images_clone {
                contents.push(rig::message::UserContent::image_base64(
                    b64,
                    Some(rig::message::ImageMediaType::PNG),
                    None,
                ));
            }
            rig::message::Message::User { content: contents }
        } else {
            rig::message::Message::user(full_prompt.clone())
        };

        let start_time = std::time::Instant::now();
        match agent.chat(query_for_chat, final_history).await {
            Ok((response, usage, messages)) => {
                let duration = start_time.elapsed();
                let _ = tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        is_thinking: false,
                    })
                    .await;

                if let Some(ref msgs) = messages
                    && let Some(search_summary) =
                        boxxy_ai_core::extract_native_search_summary(msgs)
                {
                    boxxy_telemetry::track_tool_use("native_web_search").await;
                    let tool_event = ClawEngineEvent::ToolResult {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        tool_name: "native_web_search".to_string(),
                        result: search_summary,
                        usage: None,
                    };
                    persist_visual_event(
                        db.clone(),
                        session_id_clone.clone(),
                        pane_id.clone(),
                        &tool_event,
                    );
                    let _ = tx_ui.send(tool_event).await;
                }

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

                let display_response = history::strip_speaker_tag(&response)
                    .replace(
                        crate::engine::session::roundtable::YIELD_TO_USER_MARKER,
                        "",
                    )
                    .trim()
                    .to_string();

                {
                    let mut state_lock = state.lock().await;
                    if !speaker.is_guest {
                        state_lock.persistent_agent = Some(agent.clone());
                    }

                    if let Some(msgs) = messages {
                        // `PromptResponse::messages` in Rig 0.44 contains only the messages
                        // committed by this run (its prompt + tool calls/results + final assistant turn).
                        // Since we already pushed the user prompt to `state_lock.history` before the run,
                        // skip the leading user prompt and append the remaining committed turns.
                        let mut new_tail: Vec<rig::message::Message> = msgs
                            .into_iter()
                            .skip_while(|m| matches!(m, rig::message::Message::User { .. }))
                            .collect();
                        if let Some(rig::message::Message::Assistant(turn)) = new_tail.last_mut() {
                            for item in turn.content.iter_mut() {
                                if let rig::message::AssistantContent::Text(t) = item {
                                    t.text = history::tag_assistant_text(
                                        &character_id,
                                        &agent_name,
                                        &display_response,
                                    );
                                }
                            }
                        }
                        state_lock.history.extend(new_tail);
                    } else {
                        state_lock.history.push(rig::message::Message::assistant(
                            history::tag_assistant_text(
                                &character_id,
                                &agent_name,
                                &display_response,
                            ),
                        ));
                    }

                    if let Some(ref usage) = usage {
                        state_lock.total_tokens += usage.total_tokens();
                        let ctx = usage.effective_context_tokens();
                        if ctx > 0 {
                            state_lock.context_tokens = ctx;
                        }
                        if let Some(cost) = usage.cost_usd
                            && cost.is_finite()
                            && cost > 0.0
                        {
                            state_lock.total_cost_usd += cost;
                        }
                    }
                }

                let p_ctx = persistence::PersistenceContext {
                    session_id: session_id_clone.clone(),
                    agent_name: host_agent_name.clone(),
                    user_query: prompt_clone.clone(),
                    cwd: cwd_clone.clone(),
                    settings: (*settings).clone(),
                    db: db.clone(),
                    tx_self: tx_self.clone(),
                };
                persistence::perform_persistence(p_ctx, state.clone()).await;

                let mut command_opt = None;
                let mut clean_diagnosis = display_response.clone();

                let extracted = extract_command_and_clean(&display_response);
                if extracted.0.is_some() {
                    command_opt = extracted.0;
                    clean_diagnosis = extracted.1;
                }

                let mut had_blocking_proposal = false;
                if clean_diagnosis.trim() == "[SILENT_ACK]" {
                    info!(
                        "Pane {} ({}): Agent acknowledged rejection silently.",
                        pane_id, agent_name
                    );
                    had_blocking_proposal = true;
                    let _ = tx_ui.send(ClawEngineEvent::DismissDrawer).await;
                } else if clean_diagnosis.trim().is_empty() && command_opt.is_none() {
                    info!(
                        "Pane {} ({}): Agent response was empty.",
                        pane_id, agent_name
                    );
                } else if let Some(command) = command_opt {
                    let latest_settings = boxxy_preferences::Settings::load();
                    let auto_approve = vibe_enabled
                        && crate::engine::vibe_guard::is_safe_vibe_command(
                            &command,
                            &cwd_clone,
                            &latest_settings.vibe_trusted_dirs,
                        );
                    had_blocking_proposal = !auto_approve;
                    let event = ClawEngineEvent::InjectCommand {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        command,
                        diagnosis: clean_diagnosis.clone(),
                        target_pane_id: None,
                        target_pane_label: None,
                        auto_approve,
                        usage,
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
                        diagnosis: clean_diagnosis.clone(),
                        usage,
                        typewriter: drama_mode,
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
                            result: display_response.clone(),
                        })
                        .await;
                }

                // Evaluate multi-character roundtable / debate handoff
                let next_thinking_speaker = {
                    let catalog = crate::engine::session::roundtable::get_active_catalog();
                    let mut state_lock = state.lock().await;
                    state_lock.active_speaker = None;
                    let blocking = had_blocking_proposal || state_lock.has_pending_approval();
                    state_lock.next_roundtable_turn = state_lock.roundtable.on_turn_completed(
                        &speaker,
                        &response,
                        blocking,
                        &pane_id,
                        &catalog,
                    );
                    state_lock
                        .next_roundtable_turn
                        .as_ref()
                        .map(|(next_spk, _)| (next_spk.display_name.clone(), next_spk.character_id.clone()))
                };

                // If another character is queued to speak next, immediately transition the UI
                // back into Thinking for the next speaker so the input bar stays inactive and
                // the Cancel (×) button remains available to interrupt the conversation.
                if let Some((next_name, next_cid)) = next_thinking_speaker {
                    if drama_mode {
                        // In Visual Novel Drama Mode, wait for the current character's typewriter
                        // animation to finish revealing on screen before the next character speaks!
                        let char_count = clean_diagnosis.chars().count() as u64;
                        let delay_ms = (char_count * 16 + 550).clamp(1100, 3400);
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    let _ = tx_ui
                        .send(ClawEngineEvent::AgentThinking {
                            agent_name: next_name,
                            character_id: next_cid,
                            is_thinking: true,
                        })
                        .await;
                }

                let _ = tx_self.send(ClawMessage::TurnFinished).await;
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
                            message: "Agent turn ended with an error after a long duration."
                                .to_string(),
                        })
                        .await;
                }

                {
                    let mut state_lock = state.lock().await;
                    if !speaker.is_guest {
                        state_lock.persistent_agent = Some(agent);
                    }
                    state_lock.active_speaker = None;
                    state_lock.next_roundtable_turn = None;
                    state_lock.roundtable.clear();
                }

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
                    typewriter: false,
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
