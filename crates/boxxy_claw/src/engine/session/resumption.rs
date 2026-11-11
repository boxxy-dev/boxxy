use boxxy_db::Db;
use log::info;
use rig::message::Message;

use super::ClawSession;
use crate::engine::{ClawEngineEvent, PersistentClawRow, context::load_session_context};

impl ClawSession {
    #[allow(clippy::too_many_lines)]
    pub(super) async fn handle_resume_session(
        &mut self,
        session_id: String,
        override_character_id: Option<String>,
        current_dir: &str,
        is_initialized: &mut bool,
    ) {
        info!("Resuming session {} in pane {}...", session_id, self.pane_id);

        let workspace = crate::registry::workspace::global_workspace().await;
        workspace.evict_session(&session_id).await;

        {
            let db_guard = self.db.lock().await;
            if db_guard.is_none()
                && let Ok(db) = Db::new().await
            {
                drop(db_guard);
                *self.db.lock().await = Some(db);
            }
        }

        let db_opt = self.db.lock().await.clone();
        if let Some(db) = db_opt {
            let store = boxxy_db::store::Store::new(db.pool());
            if let Ok(Some(session)) = store.get_session(&session_id).await {
                self.session_id.clone_from(&session_id);
                let cached_registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
                let registry: Vec<boxxy_claw_protocol::characters::CharacterInfo> =
                    if cached_registry.is_empty() {
                        boxxy_claw_protocol::character_loader::load_characters().unwrap_or_default()
                    } else {
                        (**cached_registry).clone()
                    };

                let prev_char_id = session.character_id.clone();
                let prev_name = if !session.character_display_name.trim().is_empty() {
                    session.character_display_name.clone()
                } else if let Some(n) =
                    session.agent_name.as_deref().filter(|s| !s.trim().is_empty())
                {
                    n.to_string()
                } else {
                    session.name.clone()
                };

                let fallback_names = [
                    session.character_display_name.as_str(),
                    session.agent_name.as_deref().unwrap_or_default(),
                    session.name.as_str(),
                ];
                let char_info = if let Some(target_id) =
                    override_character_id.as_deref().filter(|s| !s.is_empty())
                {
                    boxxy_claw_protocol::characters::resolve_character(&registry, target_id, &[])
                } else {
                    boxxy_claw_protocol::characters::resolve_character(
                        &registry,
                        &session.character_id,
                        &fallback_names,
                    )
                };

                let switched_character = char_info
                    .is_some_and(|c| !prev_char_id.is_empty() && c.config.id != prev_char_id);
                let needs_db_update = char_info.is_some_and(|c| {
                    c.config.id != session.character_id
                        || c.config.display_name != session.character_display_name
                });

                if let Some(info) = char_info {
                    self.name.clone_from(&info.config.display_name);
                    self.character_id.clone_from(&info.config.id);
                    if needs_db_update {
                        info!(
                            "Character '{}' reassigned for session {session_id} -> '{}' ({})",
                            session.character_id, info.config.display_name, info.config.id
                        );
                        let _ = store
                            .update_session_character(
                                &session_id,
                                &info.config.id,
                                &info.config.display_name,
                            )
                            .await;
                    }
                } else if !session.character_display_name.is_empty() {
                    self.name.clone_from(&session.character_display_name);
                } else if let Some(n) = session.agent_name.as_deref() {
                    self.name = n.to_string();
                }

                // Load the resumed character's duties and personality context
                self.session_context = load_session_context(&self.character_id);

                let has_existing_title = session
                    .title
                    .as_deref()
                    .is_some_and(|t| !t.trim().is_empty());
                self.pinned = session.pinned;
                self.total_tokens = u64::try_from(session.total_tokens).unwrap_or(0);
                self.context_tokens = if session.context_tokens > 0 {
                    u64::try_from(session.context_tokens).unwrap_or(0)
                } else {
                    self.total_tokens
                };
                self.total_cost_usd = session.total_cost_usd;

                let (agent_name_clone, tasks_clone, web_search_enabled, vibe_enabled) = {
                    let mut state_lock = self.state.lock().await;
                    state_lock.session_id.clone_from(&session_id);
                    state_lock.agent_name.clone_from(&self.name);
                    state_lock.character_id.clone_from(&self.character_id);
                    state_lock.character_display_name.clone_from(&self.name);
                    state_lock.pinned = self.pinned;
                    state_lock.title_requested = has_existing_title;
                    state_lock.total_tokens = self.total_tokens;
                    state_lock.context_tokens = self.context_tokens;
                    state_lock.total_cost_usd = self.total_cost_usd;

                    if let Some(history_json) = session.history_json {
                        match serde_json::from_str::<Vec<Message>>(&history_json) {
                            Ok(mut history) => {
                                if !prev_char_id.is_empty() || !prev_name.is_empty() {
                                    for msg in &mut history {
                                        if let Message::Assistant(turn) = msg {
                                            for item in &mut turn.content {
                                                if let rig::message::AssistantContent::Text(t) =
                                                    item
                                                    && crate::engine::turn::history::parse_speaker_tag(&t.text)
                                                        .is_none()
                                                {
                                                    t.text = crate::engine::turn::history::tag_assistant_text(
                                                        &prev_char_id,
                                                        &prev_name,
                                                        &t.text,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                state_lock.flushed_watermark = history.len();
                                state_lock.history = history;
                            }
                            Err(e) => {
                                log::warn!(
                                    "Session {session_id} history could not be deserialized (likely pre-0.44 format: {e}); starting with fresh LLM history while retaining visual logs."
                                );
                                state_lock.history.clear();
                                state_lock.flushed_watermark = 0;
                            }
                        }
                    }

                    if let Some(tasks_json) = session.pending_tasks_json
                        && let Ok(tasks) =
                            serde_json::from_str::<Vec<crate::engine::ScheduledTask>>(&tasks_json)
                    {
                        state_lock.pending_tasks = tasks;
                    }

                    state_lock.persistent_agent = None;
                    (
                        state_lock.agent_name.clone(),
                        state_lock.pending_tasks.clone(),
                        state_lock.web_search_enabled,
                        state_lock.vibe_enabled,
                    )
                };

                self.clear_ui_history().await;

                if let Ok(event_jsons) = store.get_claw_events(&session_id).await {
                    let mut rows: Vec<PersistentClawRow> = event_jsons
                        .into_iter()
                        .filter_map(|json| serde_json::from_str::<PersistentClawRow>(&json).ok())
                        .map(|mut row| {
                            match &mut row {
                                PersistentClawRow::Diagnosis {
                                    agent_name,
                                    character_id,
                                    ..
                                }
                                | PersistentClawRow::Suggested {
                                    agent_name,
                                    character_id,
                                    ..
                                }
                                | PersistentClawRow::ToolCall {
                                    agent_name,
                                    character_id,
                                    ..
                                }
                                | PersistentClawRow::ProcessList {
                                    agent_name,
                                    character_id,
                                    ..
                                }
                                | PersistentClawRow::FileDiff {
                                    agent_name,
                                    character_id,
                                    ..
                                } => {
                                    if character_id
                                        .as_deref()
                                        .is_none_or(|s| s.trim().is_empty())
                                        && !prev_char_id.is_empty()
                                    {
                                        *character_id = Some(prev_char_id.clone());
                                    }
                                    if agent_name
                                        .as_deref()
                                        .is_none_or(|s| s.trim().is_empty())
                                        && !prev_name.is_empty()
                                    {
                                        *agent_name = Some(prev_name.clone());
                                    }
                                }
                                _ => {}
                            }
                            row
                        })
                        .collect();

                    if switched_character && !rows.is_empty() {
                        let old_pretty =
                            crate::engine::session::roundtable::format_character_title(&prev_name);
                        let new_pretty =
                            crate::engine::session::roundtable::format_character_title(&self.name);
                        let switch_text = format!("Character Switch: {old_pretty} → {new_pretty}");
                        let already_appended = matches!(
                            rows.last(),
                            Some(PersistentClawRow::SystemMessage { content, .. })
                                if content == &switch_text
                        );
                        if !already_appended && !old_pretty.eq_ignore_ascii_case(&new_pretty) {
                            let switch_row = PersistentClawRow::SystemMessage {
                                pane_id: self.pane_id.clone(),
                                content: switch_text,
                            };
                            if let Ok(json) = serde_json::to_string(&switch_row) {
                                let _ = store.add_claw_event(&session_id, &json).await;
                            }
                            rows.push(switch_row);
                        }
                    }

                    if !rows.is_empty() {
                        let _ = self
                            .tx_ui
                            .send(ClawEngineEvent::RestoreHistory(rows))
                            .await;
                    }
                }

                workspace
                    .register_pane_tx(self.pane_id.clone(), self.tx_self.clone())
                    .await;
                workspace
                    .update_pane_state(
                        self.pane_id.clone(),
                        Some(self.session_id.clone()),
                        Some(self.name.clone()),
                        current_dir.to_string(),
                        None,
                        None,
                    )
                    .await;

                let settings = boxxy_preferences::Settings::load();
                let context_window = boxxy_ai_core::model_context_window_with_endpoints(
                    &settings.claw_model,
                    &settings.openai_compatible_endpoints,
                );

                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::Identity {
                        agent_name: self.name.clone(),
                        character_id: self.character_id.clone(),
                        pinned: self.pinned,
                        web_search_enabled,
                        vibe_enabled,
                        total_tokens: self.total_tokens,
                        context_tokens: self.context_tokens,
                        context_window,
                        total_cost_usd: self.total_cost_usd,
                    })
                    .await;

                workspace
                    .update_pane_tasks(self.pane_id.clone(), tasks_clone.clone())
                    .await;

                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::TaskStatusChanged {
                        agent_name: agent_name_clone,
                        character_id: self.character_id.clone(),
                        tasks: tasks_clone,
                    })
                    .await;

                let session_type = if self.pinned { "pinned" } else { "normal" };
                boxxy_telemetry::track_session_resume(session_type).await;

                let banner = crate::engine::agent_config::format_session_models_banner(
                    &settings,
                    web_search_enabled,
                );
                let search_label =
                    crate::engine::agent_config::resolve_active_search_label(
                        &settings,
                        web_search_enabled,
                    )
                    .to_string();

                {
                    let mut state_lock = self.state.lock().await;
                    state_lock
                        .current_claw_model
                        .clone_from(&settings.claw_model);
                    state_lock
                        .current_memory_model
                        .clone_from(&settings.memory_model);
                    state_lock.current_search_label = search_label;
                }

                self.send_ui(ClawEngineEvent::SystemMessage { text: banner })
                    .await;

                if let Some(last_cwd) = session.last_cwd {
                    let _ = self
                        .tx_ui
                        .send(ClawEngineEvent::RequestCwdSwitch { path: last_cwd })
                        .await;
                }
                *is_initialized = true;
            } else {
                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::SystemMessage {
                        text: "Failed to load session from database.".to_string(),
                    })
                    .await;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn handle_rebind_pane(
        &mut self,
        ctx: &super::run::context::RunContext,
        new_pane_id: String,
    ) {
        let old_pane_id = self.pane_id.clone();
        info!(
            "Rebinding live ClawSession {} from pane {} -> {}",
            self.session_id, old_pane_id, new_pane_id
        );
        self.pane_id.clone_from(&new_pane_id);

        let (
            agent_name,
            character_id,
            pinned,
            web_search_enabled,
            vibe_enabled,
            total_tokens,
            context_tokens,
            total_cost_usd,
            status,
            tasks,
        ) = {
            let mut state_lock = self.state.lock().await;
            state_lock.pane_id.clone_from(&new_pane_id);
            if state_lock.status == crate::engine::AgentStatus::Off {
                state_lock.status = crate::engine::AgentStatus::Waiting;
            }
            (
                state_lock.agent_name.clone(),
                state_lock.character_id.clone(),
                state_lock.pinned,
                state_lock.web_search_enabled,
                state_lock.vibe_enabled,
                state_lock.total_tokens,
                state_lock.context_tokens,
                state_lock.total_cost_usd,
                state_lock.status.clone(),
                state_lock.pending_tasks.clone(),
            )
        };

        ctx.workspace
            .rebind_pane(&old_pane_id, new_pane_id.clone())
            .await;
        ctx.workspace
            .register_pane_tx(new_pane_id.clone(), self.tx_self.clone())
            .await;

        let settings = boxxy_preferences::Settings::load();
        let context_window = boxxy_ai_core::model_context_window_with_endpoints(
            &settings.claw_model,
            &settings.openai_compatible_endpoints,
        );

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::Identity {
                agent_name: agent_name.clone(),
                character_id: character_id.clone(),
                pinned,
                web_search_enabled,
                vibe_enabled,
                total_tokens,
                context_tokens,
                context_window,
                total_cost_usd,
            })
            .await;

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::SessionStateChanged {
                agent_name: agent_name.clone(),
                character_id: character_id.clone(),
                status,
            })
            .await;

        {
            let db_guard = self.db.lock().await;
            if db_guard.is_none()
                && let Ok(db) = Db::new().await
            {
                drop(db_guard);
                *self.db.lock().await = Some(db);
            }
        }

        let db_opt = self.db.lock().await.clone();
        if let Some(db) = db_opt {
            let store = boxxy_db::store::Store::new(db.pool());
            if let Ok(event_jsons) = store.get_claw_events(&self.session_id).await {
                let rows: Vec<PersistentClawRow> = event_jsons
                    .into_iter()
                    .filter_map(|json| serde_json::from_str::<PersistentClawRow>(&json).ok())
                    .map(|mut row| {
                        match &mut row {
                            PersistentClawRow::Diagnosis { pane_id, .. }
                            | PersistentClawRow::User { pane_id, .. }
                            | PersistentClawRow::Suggested { pane_id, .. }
                            | PersistentClawRow::ProcessList { pane_id, .. }
                            | PersistentClawRow::ToolCall { pane_id, .. }
                            | PersistentClawRow::SystemMessage { pane_id, .. }
                            | PersistentClawRow::FileDiff { pane_id, .. } => {
                                pane_id.clone_from(&new_pane_id);
                            }
                            PersistentClawRow::Command { .. } => {}
                        }
                        row
                    })
                    .collect();
                if !rows.is_empty() {
                    let _ = self
                        .tx_ui
                        .send(ClawEngineEvent::RestoreHistory(rows))
                        .await;
                }
            }
        }

        ctx.workspace
            .update_pane_tasks(new_pane_id, tasks.clone())
            .await;

        let _ = self
            .tx_ui
            .send(ClawEngineEvent::TaskStatusChanged {
                agent_name,
                character_id,
                tasks,
            })
            .await;
    }
}
