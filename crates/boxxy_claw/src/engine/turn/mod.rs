pub mod file_refs;
pub mod history;
mod incognito;
pub mod persistence;
pub mod skills;
pub mod snapshot;
mod whats_new;

pub const WHATS_NEW_CONTEXT_MARKER: &str = "\n\n--- UPDATE CHANGELOG CONTEXT ---";
pub const ATTACHMENTS_CONTEXT_MARKER: &str = "\n\n--- ATTACHMENTS ---";

/// Returns `true` when `prompt` is a post-update "Read What's New" changelog summary request.
#[must_use]
pub fn is_whats_new_prompt(prompt: &str) -> bool {
    prompt.contains(WHATS_NEW_CONTEXT_MARKER)
}

/// Strips synthetic appended payload sections and roundtable handoff wrappers from a prompt.
///
/// Ensures skill matching, memory recall, `@file` extraction, and `:character` mention
/// parsing inspect only the actual user/task topic rather than synthetic boilerplate words.
#[must_use]
pub fn extract_user_authored_prompt(prompt: &str) -> &str {
    let base = prompt
        .split(WHATS_NEW_CONTEXT_MARKER)
        .next()
        .unwrap_or(prompt)
        .split(ATTACHMENTS_CONTEXT_MARKER)
        .next()
        .unwrap_or(prompt);

    if (base.starts_with("[ROUNDTABLE TURN") || base.starts_with("[COLLABORATIVE HANDOFF"))
        && let Some(after_topic) = base.split("Original Topic / Context: ").nth(1)
    {
        return after_topic
            .split("\nINSTRUCTIONS FOR YOUR TURN:")
            .next()
            .unwrap_or(after_topic)
            .trim();
    }

    if (base.starts_with("[ROUNDTABLE DEBATE STARTED]")
        || base.starts_with("[VISUAL NOVEL DRAMA MODE STARTED]"))
        && let Some(after_topic) = base.split("\nTopic: ").nth(1)
    {
        return after_topic.lines().next().unwrap_or(after_topic).trim();
    }

    base
}

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
    let session_id_clone = session_id;
    let snapshot_clone = snapshot.to_string();
    let cwd_clone = cwd;
    let mut images_clone = image_attachments;
    let session_ctx_clone = _session_ctx.to_string();

    tokio::spawn(async move {
        let (speaker, host_agent_name, _host_character_id, is_incognito) = {
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
            (spk, host_name, host_cid, state_lock.incognito)
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

        if is_incognito {
            incognito::run_incognito_turn(
                pane_id,
                agent_name,
                character_id,
                prompt_clone,
                images_clone,
                session_ctx_clone,
                state,
                tx_ui,
                tx_self,
            )
            .await;
            return;
        }

        if is_whats_new_prompt(&prompt_clone) {
            let effective_preamble = if speaker.is_guest {
                crate::engine::context::load_session_context(&character_id)
            } else {
                session_ctx_clone
            };
            whats_new::run_whats_new_turn(
                pane_id,
                session_id_clone,
                agent_name,
                host_agent_name,
                character_id,
                speaker.slug,
                prompt_clone,
                cwd_clone,
                is_new_task,
                effective_preamble,
                db,
                state,
                tx_ui,
                tx_self,
            )
            .await;
            return;
        }

        // Cleanup R4: Simplified history clear
        if is_new_task {
            let mut state_lock = state.lock().await;
            state_lock.history.clear();
            state_lock.flushed_watermark = 0;
            state_lock.read_file_hashes.clear();
        }

        let user_authored_prompt = extract_user_authored_prompt(&prompt_clone).to_string();

        // --- DEADLOCK PREVENTION: Brief DB lock to clone handle ---
        let db_opt = {
            let db_guard = db.lock().await;
            db_guard.clone()
        };
        let past_memories =
            crate::engine::context::retrieve_memories(&db_opt, &user_authored_prompt, &cwd_clone)
                .await;

        let settings = boxxy_preferences::Settings::load();

        // 1. Build Skills Context & Pre-Load `@file` References (only from user-authored text)
        let (active_skills_text, available_skills_text, triggered_skill_names) =
            skills::build_skills_context(&user_authored_prompt, &pane_id).await;
        let skills_context = format!("{active_skills_text}{available_skills_text}");
        let resolved_file_refs =
            file_refs::resolve_prompt_file_refs(&user_authored_prompt, &cwd_clone).await;
        for b64 in resolved_file_refs.image_attachments {
            if !images_clone.contains(&b64) {
                images_clone.push(b64);
            }
        }
        let prior_read_hashes = state.lock().await.read_file_hashes.clone();
        let (stale_files_notice, deleted_tracked_paths) =
            file_refs::check_externally_modified_files(
                env.as_ref(),
                &prior_read_hashes,
                &resolved_file_refs.read_hashes,
            )
            .await;

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
                    "- PID {pid}: `{proc_label}` in `{shell_cwd}` (idle {idle_secs}s){wait_note}\n"
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
                .map_or("Idle", |a| a.status.as_str());
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
            for stale_deleted in &deleted_tracked_paths {
                state_lock.read_file_hashes.remove(stale_deleted);
            }
            for (path, hash) in &resolved_file_refs.read_hashes {
                state_lock.read_file_hashes.insert(path.clone(), *hash);
            }
            if !state_lock.current_turn_snapshots.is_empty() {
                state_lock.last_turn_snapshots =
                    std::mem::take(&mut state_lock.current_turn_snapshots);
            }
            state_lock.generated_images_this_turn.clear();
            state_lock.current_turn_user_images.clone_from(&images_clone);
            state_lock.current_turn_trace.clear();
            for skill_name in &triggered_skill_names {
                state_lock
                    .current_turn_trace
                    .push(boxxy_claw_protocol::TurnTraceItem::Skill {
                        name: skill_name.clone(),
                    });
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
                let max_rounds = state_lock.roundtable.max_rounds.max(1);
                let rounds_taken = state_lock.roundtable.rounds_taken;
                let remaining_rounds = state_lock.roundtable.remaining_rounds;
                let is_final_round =
                    rounds_taken > 0 && (remaining_rounds == 0 || rounds_taken >= max_rounds);
                let turn_limit_rule = if is_final_round && !state_lock.roundtable.auto_alternate {
                    format!(
                        "5. SINGLE-TURN COLLABORATIVE HANDOFF: Complete the requested task or assistance (using your tools such as `generate_image` if applicable), present your result directly to the user in-character, and conclude cleanly — do NOT ask {other_mentions_str} a follow-up question and do NOT call `invite_character`."
                    )
                } else if is_final_round {
                    format!(
                        "5. FINAL TURN ({rounds_taken}/{max_rounds} — TURN LIMIT REACHED): You have reached the configured Character Chat Turn Limit ({max_rounds} turns). Deliver your concluding point and address the user directly — do NOT ask {other_mentions_str} another follow-up question and do NOT call `invite_character`."
                    )
                } else {
                    format!(
                        "5. Turn Limit ({rounds_taken}/{max_rounds} autonomous turns used, {remaining_rounds} remaining): Strictly respect the turn limit (and if the user asked for fewer turns or the discussion has reached a natural conclusion, append `{}` at the end of your message to stop early). Otherwise, end your message by directly addressing the other character(s) in the room ({other_mentions_str}) with a sharp question, take, or counter-argument so they can reply immediately on the next turn!",
                        crate::engine::session::roundtable::YIELD_TO_USER_MARKER
                    )
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
                    {turn_limit_rule}\n\
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
                {}\n\
                {}\n\
                {}\n",
                identity,
                now.to_rfc3339(),
                cwd_clone,
                vibe_ctx,
                roundtable_context,
                guest_home_context,
                stale_files_notice,
                resolved_file_refs.context_block,
                skills_context,
                past_memories,
                radar,
                tui_warning
            );

            let full_prompt = format!(
                "{prompt_clone}\n\n{turn_context}\n\nTerminal Snapshot:\n```\n{final_snapshot_text}\n```"
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
        let config = crate::engine::agent_config::AgentConfig::from_env_for_character(
            &settings,
            Some(&character_id),
            web_search_enabled,
            effective_preamble,
        );
        let agent = match persistent_agent {
            Some(agent) if agent.matches_config(&config) => agent,
            _ => match crate::engine::agent::create_claw_agent(
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
            {
                Ok(created) => created,
                Err(err) => {
                    let _ = tx_ui
                        .send(ClawEngineEvent::AgentThinking {
                            agent_name: agent_name.clone(),
                            character_id: character_id.clone(),
                            is_thinking: false,
                        })
                        .await;
                    {
                        let mut state_lock = state.lock().await;
                        state_lock.current_turn_user_images.clear();
                        state_lock.generated_images_this_turn.clear();
                        state_lock.active_speaker = None;
                        state_lock.next_roundtable_turn = None;
                        state_lock.roundtable.clear();
                    }
                    log::warn!(
                        "Pane {pane_id} ({agent_name}): Failed to create Claw agent: {err}"
                    );
                    let err_card = ClawEngineEvent::DiagnosisComplete {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        diagnosis: format!("**Configuration Error:** {err}"),
                        trace: Vec::new(),
                        usage: None,
                        typewriter: false,
                    };
                    persist_visual_event(
                        db.clone(),
                        session_id_clone.clone(),
                        pane_id.clone(),
                        &err_card,
                    );
                    let _ = tx_ui.send(err_card).await;
                    if let Some(request_id) = delegate_reply_id {
                        let err_str = format!("Error: {err}");
                        workspace
                            .complete_delegated_task(request_id, err_str.clone())
                            .await;
                        let _ = tx_ui
                            .send(ClawEngineEvent::DelegatedTaskReply {
                                request_id,
                                result: err_str,
                            })
                            .await;
                    }
                    let _ = tx_self.send(ClawMessage::TurnFinished).await;
                    return;
                }
            },
        };

        if !speaker.is_guest {
            state.lock().await.persistent_agent = Some(agent.clone());
        }

        // 5. Prepare History (Perspective-flipped for the current speaker)
        let final_history = history::prepare_query_history_for_speaker(
            history,
            history_len,
            &character_id,
            &host_agent_name,
        );

        for skill_name in &triggered_skill_names {
            let _ = tx_ui
                .send(ClawEngineEvent::TurnTraceProgress {
                    agent_name: agent_name.clone(),
                    character_id: character_id.clone(),
                    item: Some(boxxy_claw_protocol::TurnTraceItem::Skill {
                        name: skill_name.clone(),
                    }),
                    active_status: None,
                })
                .await;
        }

        let query_for_chat = build_user_message_with_images(full_prompt.clone(), images_clone);
        let stream_ctx = crate::engine::agent::TurnStreamContext {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            agent_name: agent_name.clone(),
            character_id: character_id.clone(),
        };

        let start_time = std::time::Instant::now();
        match agent
            .chat_streaming(query_for_chat, final_history, Some(stream_ctx))
            .await
        {
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
                    if let Some(item) = boxxy_claw_protocol::tool_result_to_trace_item(
                        "native_web_search",
                        &search_summary,
                    ) {
                        state.lock().await.current_turn_trace.push(item.clone());
                        let _ = tx_ui
                            .send(ClawEngineEvent::TurnTraceProgress {
                                agent_name: agent_name.clone(),
                                character_id: character_id.clone(),
                                item: Some(item),
                                active_status: None,
                            })
                            .await;
                    }
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
                    let summary = summarize_long_task_completion(&response);

                    let _ = tx_ui
                        .send(ClawEngineEvent::LongTaskCompleted {
                            agent_name: agent_name.clone(),
                            character_id: character_id.clone(),
                            duration_secs: duration.as_secs(),
                            message: summary,
                        })
                        .await;
                }

                let mut display_response = history::strip_speaker_tag(&response)
                    .replace(
                        crate::engine::session::roundtable::YIELD_TO_USER_MARKER,
                        "",
                    )
                    .trim()
                    .to_string();

                {
                    let mut state_lock = state.lock().await;
                    state_lock.current_turn_user_images.clear();
                    let generated_images =
                        std::mem::take(&mut state_lock.generated_images_this_turn);
                    display_response =
                        append_missing_generated_images(display_response, &generated_images);
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
                            for item in &mut turn.content {
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

                let completed_trace = {
                    let mut state_lock = state.lock().await;
                    std::mem::take(&mut state_lock.current_turn_trace)
                };

                let mut had_blocking_proposal = false;
                if clean_diagnosis.trim() == "[SILENT_ACK]" {
                    info!(
                        "Pane {pane_id} ({agent_name}): Agent acknowledged rejection silently."
                    );
                    had_blocking_proposal = true;
                    let _ = tx_ui.send(ClawEngineEvent::DismissDrawer).await;
                } else if clean_diagnosis.trim().is_empty()
                    && command_opt.is_none()
                    && completed_trace.is_empty()
                {
                    info!(
                        "Pane {pane_id} ({agent_name}): Agent response was empty."
                    );
                } else if let Some(command) = command_opt {
                    let latest_settings = boxxy_preferences::Settings::load();
                    // If this turn already created or modified files (`file_write` / `replace_in_file`),
                    // a trailing markdown ```bash block in prose is almost always an optional
                    // "Run it with:" suggestion rather than an explicit execution tool call —
                    // present it as "Accept & Run" instead of silently auto-running it in Vibe Mode.
                    let turn_modified_files =
                        !state.lock().await.current_turn_snapshots.is_empty();
                    let auto_approve = vibe_enabled
                        && !turn_modified_files
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
                        trace: completed_trace,
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
                        trace: completed_trace,
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
                    workspace
                        .complete_delegated_task(request_id, display_response.clone())
                        .await;
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

                let completed_trace = {
                    let mut state_lock = state.lock().await;
                    state_lock.current_turn_user_images.clear();
                    state_lock.generated_images_this_turn.clear();
                    if !speaker.is_guest {
                        state_lock.persistent_agent = Some(agent);
                    }
                    state_lock.active_speaker = None;
                    state_lock.next_roundtable_turn = None;
                    state_lock.roundtable.clear();
                    std::mem::take(&mut state_lock.current_turn_trace)
                };

                log::error!(
                    "Pane {pane_id} ({agent_name}): Boxxy-Claw agent failed: {e}"
                );

                let friendly_msg = format_turn_error(&e);

                let event = ClawEngineEvent::DiagnosisComplete {
                    agent_name: agent_name.clone(),
                    character_id: character_id.clone(),
                    diagnosis: friendly_msg,
                    trace: completed_trace,
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
                    let err_res = format!("Error: {e}");
                    workspace
                        .complete_delegated_task(request_id, err_res.clone())
                        .await;
                    let _ = tx_ui
                        .send(ClawEngineEvent::DelegatedTaskReply {
                            request_id,
                            result: err_res,
                        })
                        .await;
                }
                let _ = tx_self.send(ClawMessage::TurnFinished).await;
            }
        }
    })
}

const NO_TOOLS_GUIDANCE: &str = "**Error:** The selected Ollama model does not support tool calling.\n\nBoxxy-Claw requires a highly capable reasoning model with native tool support (like `llama3.2`, `qwen2.5`, or `mistral`) to interact with your system.\n\nPlease select a different model for Boxxy-Claw in the Model Selection menu (Ctrl+Shift+P).";

/// Maps structured [`rig::completion::PromptError`] variants and [`rig::error::ErrorReport`]
/// metadata into actionable user-facing diagnostics.
pub(super) fn format_turn_error(err: &rig::completion::PromptError) -> String {
    match err {
        rig::completion::PromptError::MaxTurns { max_turns, .. } => format!(
            "**Turn Limit Reached:** Reached the {max_turns}-step tool limit — reply `\"continue\"` to keep going."
        ),
        rig::completion::PromptError::Provider(rig::error::ProviderError::Stalled(idle)) => {
            format!(
                "**Stream Timed Out:** No response frames arrived for {}s. Please retry or switch models.",
                idle.as_secs()
            )
        }
        rig::completion::PromptError::Provider(
            rig::error::ProviderError::InvalidAuthentication(resp),
        ) => format!(
            "**Authentication Failed:** The provider rejected your API key or credentials.\n\nPlease verify your API key in **Preferences -> APIs**.\n```\n{resp}\n```"
        ),
        _ => {
            let report_opt: Option<std::borrow::Cow<'_, rig::error::ErrorReport>> = match err {
                rig::completion::PromptError::Provider(p) => {
                    Some(std::borrow::Cow::Owned(p.report()))
                }
                rig::completion::PromptError::Report(r) => Some(std::borrow::Cow::Borrowed(r)),
                _ => None,
            };

            if let Some(report) = report_opt {
                if report.message.contains("does not support tools")
                    || report
                        .provider_response_body()
                        .is_some_and(|b| b.contains("does not support tools"))
                {
                    return NO_TOOLS_GUIDANCE.to_string();
                }

                if matches!(report.http_status, Some(401 | 403)) {
                    let status = report.http_status.unwrap_or(401);
                    return format!(
                        "**Authentication Failed (HTTP {status}):** Please verify your API key and permissions in **Preferences -> APIs**.\n```\n{}\n```",
                        report.message
                    );
                }

                if report.is_context_overflow() {
                    return format!(
                        "**Context Window Exceeded:** This conversation or prompt exceeds the model's context limit.\n\nRun `/clear` to start a fresh context or switch to a larger-context model.\n```\n{}\n```",
                        report.message
                    );
                }

                if let Some(wait) = report.retry_after(std::time::SystemTime::now()) {
                    let secs = wait.as_secs().max(1);
                    return format!(
                        "**Rate Limited:** Provider asked to retry after {secs}s.\n```\n{}\n```",
                        report.message
                    );
                }
            }

            let raw = format!("{err}");
            if raw.contains("does not support tools") {
                NO_TOOLS_GUIDANCE.to_string()
            } else {
                format!("**Boxxy-Claw encountered an error:**\n```\n{raw}\n```")
            }
        }
    }
}

fn sniff_base64_image_media_type(b64: &str) -> rig::message::ImageMediaType {
    use base64::Engine as _;

    let trimmed = b64.trim();
    // Decode up to 16 base64 chars (12 raw bytes — covers PNG, JPEG, GIF, and RIFF/WEBP headers)
    // onto the stack without heap allocation.
    let take = trimmed.len().min(16) & !3;
    let mut buf = [0_u8; 12];
    if take >= 4
        && let Ok(n) = base64::engine::general_purpose::STANDARD
            .decode_slice(&trimmed.as_bytes()[..take], &mut buf)
        && let Some(media) = rig::message::ImageMediaType::sniff(&buf[..n])
    {
        return media;
    }
    rig::message::ImageMediaType::PNG
}

fn build_user_message_with_images(prompt: String, images: Vec<String>) -> rig::message::Message {
    if images.is_empty() {
        return rig::message::Message::user(prompt);
    }
    let mut contents = vec![rig::message::UserContent::text(prompt)];
    for b64 in images {
        let media_type = sniff_base64_image_media_type(&b64);
        contents.push(rig::message::UserContent::image_base64(
            b64,
            Some(media_type),
            None,
        ));
    }
    rig::message::Message::User { content: contents }
}

fn summarize_long_task_completion(response: &str) -> String {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return "Task completed silently.".to_string();
    }
    let first_line = trimmed
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(trimmed)
        .trim();
    if first_line.len() > 100 {
        let cut = first_line.floor_char_boundary(97);
        format!("{}...", &first_line[..cut])
    } else {
        first_line.to_string()
    }
}

/// Ensures every `(caption, img_path)` generated by `generate_image` during the turn
/// is present in `display_response` so images always render inline in the Claw overlay,
/// without duplicating Markdown tags the model already included or altering `[SILENT_ACK]`.
pub(crate) fn append_missing_generated_images(
    mut display_response: String,
    generated_images: &[(String, String)],
) -> String {
    if generated_images.is_empty() || display_response.trim() == "[SILENT_ACK]" {
        return display_response;
    }

    for (caption, img_path) in generated_images {
        if !img_path.is_empty() && !display_response.contains(img_path.as_str()) {
            if !display_response.is_empty() {
                display_response.push_str("\n\n");
            }
            display_response
                .push_str(&crate::engine::tools::image::format_markdown_image(caption, img_path));
        }
    }

    display_response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_long_task_completion_handles_multibyte_utf8_at_boundary() {
        // Place a 4-byte emoji right across byte index 97
        let prefix = "a".repeat(95);
        let input = format!("{prefix}🦀🔥 extra trailing text\nsecond line");
        let summary = summarize_long_task_completion(&input);
        assert!(summary.ends_with("..."));
        assert!(summary.len() <= 100);
    }

    #[test]
    fn sniffs_jpeg_webp_gif_and_png_base64_prefixes() {
        assert_eq!(
            sniff_base64_image_media_type("/9j/4AAQSkZJRgABAQ"),
            rig::message::ImageMediaType::JPEG
        );
        assert_eq!(
            sniff_base64_image_media_type("UklGRh4AAABXRUJQVlA4"),
            rig::message::ImageMediaType::WEBP
        );
        assert_eq!(
            sniff_base64_image_media_type("R0lGODlhAQABAIAAAP"),
            rig::message::ImageMediaType::GIF
        );
        assert_eq!(
            sniff_base64_image_media_type("iVBORw0KGgoAAAANSUhEUg"),
            rig::message::ImageMediaType::PNG
        );
    }

    #[test]
    fn extracts_user_authored_prompt_without_changelog_or_attachment_payloads() {
        let whats_new = "Read me what's new from `a1b2c3d` to `e4f5a6b`!\n\n--- UPDATE CHANGELOG CONTEXT ---\n- commit mentioning themes and bugs";
        assert!(is_whats_new_prompt(whats_new));
        assert_eq!(
            extract_user_authored_prompt(whats_new),
            "Read me what's new from `a1b2c3d` to `e4f5a6b`!"
        );

        let with_attachments =
            "Explain this snippet\n\n--- ATTACHMENTS ---\n### Snippet 1:\n```\nfn demo() {}\n```";
        assert!(!is_whats_new_prompt(with_attachments));
        assert_eq!(
            extract_user_authored_prompt(with_attachments),
            "Explain this snippet"
        );

        let handoff = "[COLLABORATIVE HANDOFF — 1 Turn]\n**Host** (`@host`) just spoke in the overlay conversation (see their latest message in history).\nOther participants in this conversation: Host (@host).\nOriginal Topic / Context: Generate a geometric blue cube\nINSTRUCTIONS FOR YOUR TURN:\n- Complete the requested task.";
        assert_eq!(
            extract_user_authored_prompt(handoff),
            "Generate a geometric blue cube"
        );
    }

    #[test]
    fn format_turn_error_maps_structured_rig_errors_to_actionable_diagnostics() {
        use rig::completion::PromptError;
        use rig::error::{ErrorKind, ErrorReport, ProviderError};
        use std::time::Duration;

        let max_turns = PromptError::MaxTurns {
            max_turns: 100,
            chat_history: Vec::new(),
            prompt: rig::message::Message::user("go"),
        };
        let max_msg = format_turn_error(&max_turns);
        assert!(max_msg.contains("100-step tool limit"));
        assert!(max_msg.contains("continue"));

        let stalled = PromptError::Provider(ProviderError::Stalled(Duration::from_secs(300)));
        let stalled_msg = format_turn_error(&stalled);
        assert!(stalled_msg.contains("Stream Timed Out"));
        assert!(stalled_msg.contains("300s"));

        let auth_report = ErrorReport::new(ErrorKind::ProviderResponse, "invalid api key")
            .with_http_status(401);
        let auth_msg = format_turn_error(&PromptError::Report(auth_report));
        assert!(auth_msg.contains("Authentication Failed"));
        assert!(auth_msg.contains("Preferences -> APIs"));

        let overflow_report = ErrorReport::new(
            ErrorKind::ProviderResponse,
            "This model's maximum context length is 128000 tokens",
        )
        .with_code("context_length_exceeded");
        let overflow_msg = format_turn_error(&PromptError::Report(overflow_report));
        assert!(overflow_msg.contains("Context Window Exceeded"));
        assert!(overflow_msg.contains("/clear"));

        let rate_report = ErrorReport::new(
            ErrorKind::ProviderResponse,
            "Rate limit reached, please try again in 12s",
        )
        .with_retryable(true)
        .with_http_status(429);
        let rate_msg = format_turn_error(&PromptError::Report(rate_report));
        assert!(rate_msg.contains("Rate Limited"));
        assert!(rate_msg.contains("retry after 12s"));

        let no_tools = PromptError::Provider(ProviderError::Provider(
            "registry.ollama.ai/library/gemma2 does not support tools".to_string(),
        ));
        assert!(format_turn_error(&no_tools).contains("does not support tool calling"));
    }

    #[test]
    fn append_missing_generated_images_appends_only_absent_paths_and_respects_silent_ack() {
        let images = vec![
            (
                "A geometric blue cube".to_string(),
                "/tmp/synthetic/cube_1.png".to_string(),
            ),
            (
                "A silver sphere".to_string(),
                "/tmp/synthetic/sphere_2.png".to_string(),
            ),
        ];

        // Appends both when model omitted Markdown tags
        let appended =
            append_missing_generated_images("Here are both renders!".to_string(), &images);
        assert_eq!(
            appended,
            "Here are both renders!\n\n![A geometric blue cube](/tmp/synthetic/cube_1.png)\n\n![A silver sphere](/tmp/synthetic/sphere_2.png)"
        );

        // Does NOT duplicate an image already present in prose, while appending the missing second one
        let partial = append_missing_generated_images(
            "First one:\n\n![cube](/tmp/synthetic/cube_1.png)".to_string(),
            &images,
        );
        assert_eq!(
            partial.matches("/tmp/synthetic/cube_1.png").count(),
            1,
            "must not duplicate already-embedded image path"
        );
        assert!(partial.contains("![A silver sphere](/tmp/synthetic/sphere_2.png)"));

        // Never mutates `[SILENT_ACK]`
        assert_eq!(
            append_missing_generated_images("[SILENT_ACK]".to_string(), &images),
            "[SILENT_ACK]"
        );
    }
}
