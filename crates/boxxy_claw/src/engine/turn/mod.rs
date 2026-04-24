use crate::engine::{
    ClawEngineEvent, ClawEnvironment, ClawMessage, PersistentClawRow, TaskStatus, TaskType,
    context::retrieve_memories, dispatcher::extract_command_and_clean, persist_visual_event,
    session::SessionState, summarization::turn::summarize_and_store,
};
use crate::utils::load_prompt_fallback;
use boxxy_claw_protocol::UsageWrapper;
use boxxy_db::Db;
use log::info;
use rig::message::Message;
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

    tokio::spawn(async move {
        let _ = tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name: agent_name.clone(),
                is_thinking: true,
            })
            .await;

        if is_new_task {
            if let Ok(mut lock) = state.try_lock() {
                lock.history.clear();
            } else {
                state.lock().await.history.clear();
            }
        }

        let db_guard = db.lock().await;
        let past_memories = retrieve_memories(&db_guard, &prompt_clone, &cwd_clone).await;
        drop(db_guard);

        let mut state_lock = state.lock().await;
        let settings = boxxy_preferences::Settings::load();

        let mut active_skills_text = String::new();
        let mut available_skills_text = String::new();
        let query_lower = prompt_clone.to_lowercase();

        let registry = crate::registry::skills::global_registry().await;

        // Fetch Global Workspace Radar
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

        // 1. Semantic Search: Get only the TOP 1 most relevant skill to avoid context bloat
        let mut active_skills = registry.search_relevant_skills(&prompt_clone, 1).await;

        // 2. Keyword Fallback: Only add skills that were EXPLICITLY triggered by keywords in the query
        let all_skills = registry.get_skills().await;
        for skill in &all_skills {
            // Avoid duplicates
            if active_skills
                .iter()
                .any(|s| s.frontmatter.name == skill.frontmatter.name)
            {
                continue;
            }

            let mut should_load = false;
            // We NO LONGER load trigger-less skills by default.
            // They must be triggered or activated via tool.
            for trigger in &skill.frontmatter.triggers {
                if !trigger.is_empty() && query_lower.contains(&trigger.to_lowercase()) {
                    should_load = true;
                    break;
                }
            }

            if should_load {
                active_skills.push(skill.clone());
            }
        }

        // 3. Build Active Skills Text (Full Content)
        // We limit this to a maximum of 2 skills in full to save tokens.
        if !active_skills.is_empty() {
            let skill_names: Vec<String> = active_skills
                .iter()
                .map(|s| s.frontmatter.name.clone())
                .collect();
            workspace
                .update_pane_skills(pane_id.clone(), skill_names)
                .await;

            active_skills_text.push_str("\n--- ACTIVE SKILLS (FULL INSTRUCTIONS) ---\n");
            for skill in active_skills.iter().take(2) {
                active_skills_text.push_str(&format!("\n### SKILL: {}\n", skill.frontmatter.name));
                active_skills_text.push_str(&skill.content);
                active_skills_text.push('\n');
            }
        }

        // 4. Build Available Skills Text (Compact Toolbox)
        // Everything else is just a name and description.
        let mut toolbox_count = 0;
        for skill in &all_skills {
            if active_skills
                .iter()
                .any(|s| s.frontmatter.name == skill.frontmatter.name)
            {
                continue;
            }

            if toolbox_count == 0 {
                available_skills_text.push_str("\n--- AVAILABLE SKILLS (TOOLBOX - Compact) ---\n");
                available_skills_text.push_str("Use `activate_skill(name)` if you need the full instructions for any of these:\n");
            }

            let description = skill
                .frontmatter
                .description
                .split('.')
                .next()
                .unwrap_or("No description available.")
                .trim();
            available_skills_text
                .push_str(&format!("- {}: {}.\n", skill.frontmatter.name, description));
            toolbox_count += 1;
        }

        let system_prompt_template =
            load_prompt_fallback("/dev/boxxy/BoxxyTerminal/prompts/claw.md", "claw.md");

        let system_prompt =
            system_prompt_template.replace("{{available_skills}}", &available_skills_text);

        // --- PHASE 3: PERSISTENT AGENT ---
        // We check if we already have an agent instance for this session.
        // If not, or if the model changed, we create a new one safely.

        let (agent_opt, current_config, needs_rebuild, mcp_handle) = {
            let config = crate::engine::agent_config::AgentConfig::from_env(
                &settings,
                state_lock.web_search_enabled,
                system_prompt.clone(),
            );
            let needs_rebuild = state_lock
                .persistent_agent
                .as_ref()
                .map(|a| !a.matches_config(&config))
                .unwrap_or(true);
            let mcp_changed = state_lock
                .persistent_agent
                .as_ref()
                .map(|a| a.config.mcp_servers != config.mcp_servers)
                .unwrap_or(true);

            // Take ownership of agent out of state
            let agent = state_lock.persistent_agent.take();
            let mcp = if mcp_changed {
                None
            } else {
                state_lock.mcp_handle.clone()
            };

            (agent, config, needs_rebuild, mcp)
        };
        // Lock drops here!
        drop(state_lock);

        let agent = if needs_rebuild {
            {
                let mut state_lock = state.lock().await;
                let sanitized_history = crate::engine::history_utils::maybe_sanitize_history(
                    &state_lock.history,
                    agent_opt.as_ref().map(|a| &a.config),
                    &current_config,
                );
                // Replace history in state if it was sanitized
                if let std::borrow::Cow::Owned(sanitized) = sanitized_history {
                    state_lock.history = sanitized;
                }
            }

            match crate::engine::agent::create_claw_agent(
                &current_config,
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
                Ok(new_agent) => new_agent,
                Err(e) => {
                    let event = ClawEngineEvent::SystemMessage {
                        text: format!("⚠️ Failed to switch: {e}. Keeping previous agent."),
                    };
                    crate::engine::persist_visual_event(
                        db.clone(),
                        session_id_clone.clone(),
                        pane_id.clone(),
                        &event,
                    );
                    let _ = tx_ui.send(event).await;

                    // Restore old agent and abort turn cleanly
                    let mut state_lock = state.lock().await;
                    state_lock.persistent_agent = agent_opt;

                    let _ = tx_ui
                        .send(ClawEngineEvent::AgentThinking {
                            agent_name: agent_name.clone(),
                            is_thinking: false,
                        })
                        .await;
                    let _ = tx_self.send(ClawMessage::TurnFinished).await;
                    return; // Abort turn
                }
            }
        } else {
            agent_opt.expect("agent_opt is Some when needs_rebuild is false")
        };

        // Re-acquire lock to process the snapshot hash since we need state
        let mut state_lock = state.lock().await;

        // Clean snapshot: remove completely empty or whitespace-only lines
        let cleaned_snapshot: String = snapshot_clone
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<&str>>()
            .join("\n");

        // Hash the cleaned snapshot
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        cleaned_snapshot.hash(&mut hasher);
        let current_hash = hasher.finish();

        let final_snapshot = if state_lock.last_snapshot_hash == Some(current_hash) {
            "[TERMINAL_SNAPSHOT: NO_CHANGE]".to_string()
        } else {
            state_lock.last_snapshot_hash = Some(current_hash);
            let mut snap = cleaned_snapshot;
            // 1. Line limit (take only the last 50 lines)
            let lines: Vec<&str> = snap.lines().collect();
            if lines.len() > 50 {
                snap = lines[lines.len() - 50..].join("\n");
            }

            // 2. Character limit
            if snap.len() > 5000 {
                snap = format!(
                    "... (truncated {} chars) ...\n{}",
                    snap.len() - 5000,
                    &snap[snap.len() - 5000..]
                );
            }
            snap
        };

        // 3. Build Dynamic Turn Context (The part that changes every turn)
        let now = chrono::Utc::now();
        let identity = format!(
            "You are the Boxxy-Claw agent managing terminal pane: **{}**\n\
            Your unique internal ID is: `{}`",
            agent_name, pane_id
        );
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
            prompt_clone, turn_context, final_snapshot
        );

        let history = state_lock.history.clone();
        drop(state_lock);

        let mut user_msg = vec![rig::message::Message::User {
            content: rig::OneOrMany::one(rig::message::UserContent::text(full_prompt.clone())),
        }];

        let mut is_multimodal = false;

        if !images_clone.is_empty() {
            // Rig requires multimodal content for images
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

        let history_len = history.len();

        // We temporarily adapt the ClawAgent to accept `Vec<Message>` for the current prompt
        // instead of just `&str` since we need to send the multimodal `user_msg`.

        // Context Hygiene 2.0: Aggressively strip ALL transient context from previous turns
        // (Skills, Radar, Memories, and Snapshots) so history grows near-zero tokens per turn.
        let mut final_history: Vec<rig::message::Message> = history
            .into_iter()
            .enumerate()
            .map(|(i, mut msg)| {
                // Rule 1B: Freshness Buffer - only keep the verbatim snapshot for the last 4 turns.
                // Every user + assistant exchange is 2 messages, so 4 turns = 8 messages.
                // We strip the dynamic blocks from anything older than the last 8 messages.
                let is_old = history_len.saturating_sub(i) > 8;

                if let rig::message::Message::User { content } = &mut msg {
                    let mut items: Vec<rig::message::UserContent> =
                        content.clone().into_iter().collect();
                    for item in &mut items {
                        if let rig::message::UserContent::Text(text) = item {
                            if is_old {
                                // Find the start of the dynamic block and truncate everything after it
                                if let Some(idx) = text.text.find("\n\n## YOUR IDENTITY") {
                                    text.text.truncate(idx);
                                } else if let Some(idx) =
                                    text.text.find("\n\n## CURRENT TURN CONTEXT")
                                {
                                    text.text.truncate(idx);
                                } else if let Some(idx) = text.text.find("\n\n--- GLOBAL RADAR") {
                                    text.text.truncate(idx);
                                } else if let Some(idx) =
                                    text.text.find("\n\nTerminal Snapshot:\n```")
                                {
                                    text.text.truncate(idx);
                                }
                            }
                        }
                    }
                    if let Ok(new_content) = rig::OneOrMany::many(items) {
                        *content = new_content;
                    }
                }
                msg
            })
            .collect();

        let query_for_chat = if is_multimodal {
            final_history.push(user_msg.into_iter().next().unwrap());
            ""
        } else {
            &full_prompt
        };

        match agent.chat(query_for_chat, final_history).await {
            Ok((response, usage)) => {
                let _ = tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: agent_name.clone(),
                        is_thinking: false,
                    })
                    .await;

                let mut state_lock = state.lock().await;
                // Put the agent and tools back for the next turn
                state_lock.persistent_agent = Some(agent);

                state_lock
                    .history
                    .push(rig::message::Message::user(full_prompt.clone()));
                state_lock
                    .history
                    .push(rig::message::Message::assistant(response.clone()));

                if let Some(usage) = usage {
                    state_lock.total_tokens += usage.total_tokens as u64;
                }

                // --- ATOMIC PERSISTENCE ---
                let history_json = serde_json::to_string(&state_lock.history).unwrap_or_default();
                let pending_tasks_json =
                    serde_json::to_string(&state_lock.pending_tasks).unwrap_or_default();
                let agent_name_for_db = agent_name.clone();
                let session_id_for_db = session_id_clone.clone();
                let cwd_for_db = cwd_clone.clone();
                let pinned_for_db = state_lock.pinned;
                let total_tokens_for_db = state_lock.total_tokens as i64;
                let model_id = settings
                    .claw_model
                    .as_ref()
                    .map(|m| format!("{:?}", m))
                    .unwrap_or_default();

                // Count user messages to know when to generate a permanent LLM title
                let user_msg_count = state_lock
                    .history
                    .iter()
                    .filter(|m| matches!(m, rig::message::Message::User { .. }))
                    .count();

                // Safely extract the Db clone before spawning to prevent race conditions with Deactivate/Evict
                let db_val = {
                    let db_guard = db.lock().await;
                    db_guard.as_ref().cloned()
                };

                if let Some(db_for_persistence) = db_val.clone() {
                    tokio::spawn(async move {
                        let store = boxxy_db::store::Store::new(db_for_persistence.pool());
                        let _ = store
                            .upsert_session_state(
                                &session_id_for_db,
                                &agent_name_for_db,
                                "", // title is updated separately in the background LLM task
                                &history_json,
                                &pending_tasks_json,
                                &agent_name_for_db,
                                &cwd_for_db,
                                &model_id,
                                pinned_for_db,
                                total_tokens_for_db,
                            )
                            .await;
                    });
                }

                // Optional: Trigger Memory Flush if history is too long
                let creds = boxxy_ai_core::AiCredentials::new(
                    settings.get_effective_api_keys(),
                    settings.ollama_base_url.clone(),
                );

                let _ = crate::engine::summarization::history::flush_history(
                    db.clone(),
                    &mut state_lock.history,
                    &settings.claw_model,
                    &creds,
                    &cwd_clone,
                )
                .await;

                drop(state_lock);
                let prompt_for_db = prompt_clone.clone();
                let resp_for_db = response.clone();
                let cwd_for_db = cwd_clone.clone();
                let mem_model = settings
                    .memory_model
                    .clone()
                    .or(settings.claw_model.clone());
                let creds = boxxy_ai_core::AiCredentials::new(
                    settings.get_effective_api_keys(),
                    settings.ollama_base_url.clone(),
                );

                let session_id_for_summary = session_id_clone.clone();
                let title_model = mem_model.clone();
                let title_creds = creds.clone();
                let title_prompt = prompt_clone.clone();

                if let Some(db_for_summary) = db_val {
                    tokio::spawn(async move {
                        // --- LLM Title Generation ---
                        let store = boxxy_db::store::Store::new(db_for_summary.pool());
                        let mut needs_title = false;
                        let mut current_title = String::new();

                        if let Ok(Some(session)) = store.get_session(&session_id_for_summary).await
                        {
                            current_title = session.title.unwrap_or_default();
                            if current_title.trim().is_empty() {
                                needs_title = true;
                            }
                        }

                        // Force a beautiful LLM title specifically when the user sends their VERY FIRST prompt.
                        // This overwrites any initial greeting fallbacks.
                        if user_msg_count == 1 {
                            needs_title = true;
                        } else if user_msg_count == 2 {
                            // If the second user message provides more context, and the first title was just a greeting, upgrade it.
                            let lower_title = current_title.to_lowercase();
                            if lower_title.contains("greeting")
                                || lower_title.contains("hello")
                                || current_title.len() < 15
                            {
                                needs_title = true;
                            }
                        }

                        if needs_title && !title_prompt.trim().is_empty() {
                            let agent = boxxy_ai_core::create_agent(
                                &title_model,
                                &title_creds,
                                "You are a conversation title generator. Summarize the user's prompt in 3 to 6 words. MUST BE UNDER 40 CHARACTERS. Output ONLY the raw title, no quotes, no punctuation. Capitalize it like a title.",
                            );
                            if let Ok(res) = agent.prompt(&title_prompt).await {
                                let generated_title = res
                                    .0
                                    .trim()
                                    .trim_matches('"')
                                    .trim_matches('\'')
                                    .to_string();
                                if !generated_title.is_empty() && generated_title != current_title {
                                    let _ =
                                        sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
                                            .bind(&generated_title)
                                            .bind(&session_id_for_summary)
                                            .execute(db_for_summary.pool())
                                            .await;
                                    current_title = generated_title;
                                }
                            }
                        }

                        // Fallback if title is STILL empty (e.g. LLM failed)
                        if current_title.trim().is_empty() && !title_prompt.trim().is_empty() {
                            let first_line = title_prompt
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .unwrap_or("");
                            let fallback = first_line
                                .chars()
                                .take(40)
                                .collect::<String>()
                                .trim()
                                .to_string();
                            if !fallback.is_empty() {
                                let _ = sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
                                    .bind(fallback)
                                    .bind(&session_id_for_summary)
                                    .execute(db_for_summary.pool())
                                    .await;
                            }
                        }

                        summarize_and_store(
                            &Some(db_for_summary.clone()),
                            &session_id_for_summary,
                            &prompt_for_db,
                            &resp_for_db,
                            &cwd_for_db,
                            creds.clone(),
                        )
                        .await;

                        let _ = crate::memories::extraction::extract_implicit_memory(
                            Arc::new(Mutex::new(Some(db_for_summary.clone()))),
                            prompt_for_db,
                            resp_for_db,
                            mem_model,
                            creds,
                            cwd_for_db,
                        )
                        .await;
                    });
                }
                let mut command_opt = None;
                let mut clean_diagnosis = response.clone();

                let extracted = extract_command_and_clean(&response);
                if extracted.0.is_some() {
                    command_opt = extracted.0;
                    clean_diagnosis = extracted.1;
                }

                if clean_diagnosis.trim() == "[SILENT_ACK]" {
                    info!(
                        "Pane {} ({}): Agent acknowledged rejection silently. Sending DismissDrawer event.",
                        pane_id, agent_name
                    );
                    // Tell the UI to actively hide the drawer and clear any pending action buttons
                    // so we don't leave stale proposals on the screen after a rejection.
                    let _ = tx_ui.send(ClawEngineEvent::DismissDrawer).await;
                } else if clean_diagnosis.trim().is_empty() && command_opt.is_none() {
                    info!(
                        "Pane {} ({}): Agent response was empty (likely just tool calls). Not sending UI event.",
                        pane_id, agent_name
                    );
                } else if let Some(command) = command_opt {
                    let event = ClawEngineEvent::InjectCommand {
                        agent_name: agent_name.clone(),
                        command,
                        diagnosis: clean_diagnosis,
                        usage: usage.map(UsageWrapper::from),
                    };
                    // Mark this command as agent-originated so that when
                    // the user accepts and the shell eventually returns a
                    // non-zero exit, `CommandFinished` auto-diagnoses
                    // instead of falling to the passive Lazy Error path
                    // (which is reserved for user-typed commands).
                    // Cleared on CommandFinished consumption or explicit
                    // reject via CancelPending.
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
                let _ = tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: agent_name.clone(),
                        is_thinking: false,
                    })
                    .await;
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
            }
        }

        let _ = tx_self.send(ClawMessage::TurnFinished).await;
    })
}
