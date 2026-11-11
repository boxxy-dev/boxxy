use boxxy_db::Db;
use log::info;
use rig::message::Message;

use super::ClawSession;
use crate::engine::{ClawEngineEvent, ClawMessage, context::load_session_context};

impl ClawSession {
    #[must_use]
    pub fn new_headless(
        parent_pane_id: String,
        parent_session_id: uuid::Uuid,
        parent_character_name: &str,
        profile: String,
    ) -> (Self, async_channel::Sender<ClawMessage>) {
        let pane_id = format!("{}-headless-{}", parent_pane_id, uuid::Uuid::new_v4());
        let (mut session, tx, _) = Self::new(pane_id);

        let name = format!(
            "{} (Worker {})",
            parent_character_name,
            uuid::Uuid::new_v4()
                .to_string()
                .chars()
                .take(4)
                .collect::<String>()
        );

        // Mutate for headless execution
        {
            let mut state = session.state.try_lock().unwrap();
            state.parent_id = Some(parent_session_id);
            state.is_headless = true;
            state.agent_name = name.clone();
            state.status = crate::engine::AgentStatus::Working;
        }

        session.name = name;
        session.session_context = format!(
            "You are a specialized transient background worker. {profile}\n"
        );

        (session, tx)
    }

    /// Spawns a background flush of any unflushed messages in `state.history` before
    /// the session enters Sleep, Deactivate, or Evict.
    pub(super) async fn flush_unflushed_history(&self, cwd: &str) {
        if self.incognito {
            return;
        }
        let unflushed: Vec<Message> = {
            let mut state_lock = self.state.lock().await;
            let items: Vec<Message> = state_lock
                .history
                .iter()
                .skip(state_lock.flushed_watermark)
                .cloned()
                .collect();
            state_lock.flushed_watermark = state_lock.history.len();
            items
        };

        if unflushed.is_empty() {
            return;
        }

        let db_opt = {
            let db_guard = self.db.lock().await;
            db_guard.clone()
        };
        let Some(db_val) = db_opt else {
            return;
        };

        let db_cell = std::sync::Arc::new(tokio::sync::Mutex::new(Some(db_val)));
        let settings = boxxy_preferences::Settings::load();
        let model = settings
            .memory_model
            .clone()
            .or(settings.claw_model.clone());
        let creds = boxxy_ai_core::AiCredentials::new(
            settings.get_effective_api_keys(),
            settings.ollama_base_url.clone(),
        )
        .with_custom_endpoints(settings.openai_compatible_endpoints.clone());
        let cwd_owned = if cwd.is_empty() {
            "global".to_string()
        } else {
            cwd.to_string()
        };
        let session_id_owned = self.session_id.clone();

        tokio::spawn(async move {
            let _ = crate::engine::summarization::history::flush_history(
                db_cell,
                &session_id_owned,
                unflushed,
                &model,
                &creds,
                &cwd_owned,
            )
            .await;
        });
    }

    pub(super) async fn ensure_initialized(&mut self, msg: &ClawMessage, is_initialized: &mut bool) {
        if *is_initialized {
            return;
        }

        let needs_initialization = match msg {
            ClawMessage::ClawQuery { .. }
            | ClawMessage::UserMessage { .. }
            | ClawMessage::DelegatedTask { .. }
            | ClawMessage::RequestLazyDiagnosis
            | ClawMessage::SleepToggle { .. }
            | ClawMessage::Transition { .. }
            | ClawMessage::Initialize => true,
            ClawMessage::CommandFinished { exit_code, .. }
                if *exit_code != 0 && *exit_code != 130 && *exit_code != 131 =>
            {
                self.state.lock().await.status == crate::engine::AgentStatus::Waiting
            }
            _ => false,
        };

        if !needs_initialization {
            return;
        }

        info!(
            "Initializing Claw Session for pane {} ({}) upon first request...",
            self.pane_id, self.name
        );

        if !matches!(msg, ClawMessage::Initialize) {
            let endpoints = &boxxy_preferences::Settings::load().openai_compatible_endpoints;
            let (web_search_enabled, vibe_enabled, char_id, context_window) = {
                let state_lock = self.state.lock().await;
                (
                    state_lock.web_search_enabled,
                    state_lock.vibe_enabled,
                    state_lock.character_id.clone(),
                    boxxy_ai_core::model_context_window_with_endpoints(
                        &state_lock.current_claw_model,
                        endpoints,
                    ),
                )
            };
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::Identity {
                    agent_name: self.name.clone(),
                    character_id: char_id,
                    pinned: self.pinned,
                    web_search_enabled,
                    vibe_enabled,
                    total_tokens: self.total_tokens,
                    context_tokens: self.context_tokens,
                    context_window,
                    total_cost_usd: self.total_cost_usd,
                })
                .await;
        }

        if self.incognito {
            self.session_context = crate::utils::load_prompt_fallback(
                "/dev/boxxy/BoxxyTerminal/prompts/ai_chat.md",
                "ai_chat.md",
            );
            *is_initialized = true;
            return;
        }

        let session_context = load_session_context(&self.character_id);
        self.session_context = session_context;

        if let Ok(db) = Db::new().await {
            *self.db.lock().await = Some(db);
            info!(
                "Claw Memory Database initialized for pane {} ({}).",
                self.pane_id, self.name
            );

            let _ = crate::memories::db::sync_markdown_to_db(self.db.clone()).await;
            let _ = crate::memories::hygiene::run_hygiene(self.db.clone()).await;
        } else {
            log::error!("Failed to initialize Claw Memory Database.");
        }

        let workspace = crate::registry::workspace::global_workspace().await;
        workspace
            .register_pane_tx(self.pane_id.clone(), self.tx_self.clone())
            .await;

        *is_initialized = true;
    }

    pub(super) async fn handle_transition(
        &self,
        req: crate::engine::TransitionRequest,
        current_turn: &mut Option<tokio::task::JoinHandle<()>>,
        urgent_backlog: &mut std::collections::VecDeque<crate::engine::ClawMessage>,
    ) {
        let mut state_lock = self.state.lock().await;
        let current_status = state_lock.status.clone();
        let parent_id = state_lock.parent_id;

        match crate::engine::fsm::router::FsmRouter::evaluate_transition(
            &current_status,
            parent_id,
            &req,
        ) {
            Ok(crate::engine::fsm::router::FsmAction::AbortCurrentTurn) => {
                if let Some(handle) = current_turn.take() {
                    handle.abort();

                    // Notify UI that thinking has stopped
                    let agent_name = state_lock.agent_name.clone();
                    let character_id = state_lock.character_id.clone();
                    let tx_ui = self.tx_ui.clone();
                    tokio::spawn(async move {
                        let _ = tx_ui
                            .send(crate::engine::ClawEngineEvent::AgentThinking {
                                agent_name,
                                character_id,
                                is_thinking: false,
                            })
                            .await;
                    });
                }

                // Immediately pop urgent if we were interrupted
                if let Some(next_msg) = urgent_backlog.pop_front() {
                    let _ = self.tx_self.send(next_msg).await;
                }
            }
            Ok(crate::engine::fsm::router::FsmAction::Proceed) => {}
            Err(e) => {
                log::warn!("{e}");
                return;
            }
        }

        let current_quality = state_lock.context_quality.clone();

        // --- HARD WAKE INTERCEPT ---
        // If User is waking from Sleep to Waiting, we enter Working to summarize history
        if current_status == crate::engine::AgentStatus::Sleep
            && req.target_state == crate::engine::AgentStatus::Waiting
            && req.source == crate::engine::TriggerSource::User
        {
            state_lock.status = crate::engine::AgentStatus::Working;
            let agent_name = self.name.clone();
            let character_id = self.character_id.clone();
            let sleep_timestamp = state_lock.sleep_timestamp.take();
            let db_cell = self.db.clone();
            let session_id = self.session_id.clone();
            let tx_self = self.tx_self.clone();
            drop(state_lock);

            let _ = self
                .tx_ui
                .send(crate::engine::ClawEngineEvent::SessionStateChanged {
                    agent_name,
                    character_id,
                    status: crate::engine::AgentStatus::Working,
                })
                .await;

            // Spawn the Dreamer Task
            let task_handle = tokio::spawn(async move {
                crate::engine::summarization::wake::summarize_wake_delta(
                    db_cell,
                    session_id,
                    sleep_timestamp,
                    tx_self,
                )
                .await;
            });
            *current_turn = Some(task_handle);
            return;
        }

        // Handle entering sleep: flush any unflushed active history in the background
        let entering_sleep = req.target_state == crate::engine::AgentStatus::Sleep;
        if entering_sleep {
            state_lock.sleep_timestamp = Some(chrono::Utc::now().timestamp_millis());
        }

        state_lock.status = req.target_state.clone();
        let agent_name = self.name.clone();
        let character_id = self.character_id.clone();
        drop(state_lock);

        if entering_sleep {
            let workspace = crate::registry::workspace::global_workspace().await;
            let cwd = workspace
                .get_pane_cwd(self.pane_id.clone())
                .await
                .unwrap_or_else(|| "global".to_string());
            self.flush_unflushed_history(&cwd).await;
        }

        let _ = self
            .tx_ui
            .send(crate::engine::ClawEngineEvent::SessionStateChanged {
                agent_name,
                character_id,
                status: req.target_state,
            })
            .await;

        crate::registry::workspace::global_workspace()
            .await
            .set_pane_quality(self.pane_id.clone(), Some(current_quality))
            .await;
    }

    pub(super) async fn ensure_session_persisted(&self, cwd: &str) {
        if self.incognito {
            return;
        }
        let db_val = {
            let db_guard = self.db.lock().await;
            db_guard.clone()
        };
        if let Some(db) = db_val {
            let store = boxxy_db::store::Store::new(db.pool());
            let _ = store
                .ensure_session_exists(&self.session_id, &self.character_id, &self.name, cwd)
                .await;
        }
    }

    pub(super) async fn handle_settings_invalidated(&mut self) {
        info!(
            "[{}] Settings changed. Next turn will trigger a hot-swap.",
            self.name
        );

        if self.incognito {
            boxxy_preferences::Settings::reload();
            let after = boxxy_preferences::Settings::load();
            let mut state_lock = self.state.lock().await;
            state_lock.persistent_agent = None;
            state_lock.current_claw_model = after.claw_model.clone();
            state_lock.current_memory_model = after.memory_model.clone();
            return;
        }

        // Check if the character was migrated or renamed during a catalog hot-reload
        let cached_catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let catalog: Vec<boxxy_claw_protocol::characters::CharacterInfo> =
            if cached_catalog.is_empty() {
                boxxy_claw_protocol::character_loader::load_characters().unwrap_or_default()
            } else {
                (**cached_catalog).clone()
            };

        let claimed_char_id = boxxy_claw_protocol::characters::CLAIMS_CACHE
            .load()
            .iter()
            .find(|c| c.holder_id == self.pane_id)
            .map(|c| c.character_id.clone());

        let lookup_id = claimed_char_id.as_deref().unwrap_or(&self.character_id);
        let fallback_names = [self.name.as_str()];
        let old_char_id = self.character_id.clone();
        let old_char_name = self.name.clone();
        let mut character_changed = false;
        let mut character_id_switched = false;

        if let Some(info) =
            boxxy_claw_protocol::characters::resolve_character(&catalog, lookup_id, &fallback_names)
            && (info.config.id != self.character_id || info.config.display_name != self.name)
        {
            character_id_switched =
                !self.character_id.is_empty() && info.config.id != self.character_id;
            self.character_id = info.config.id.clone();
            self.name = info.config.display_name.clone();
            character_changed = true;
        }

        // Always reload character duties & personality so edits to CHARACTER.toml apply on next turn
        self.session_context = load_session_context(&self.character_id);

        let (total_tokens, context_tokens, total_cost_usd, had_messages) = {
            let mut state_lock = self.state.lock().await;
            if character_changed {
                if character_id_switched {
                    for msg in &mut state_lock.history {
                        if let Message::Assistant(turn) = msg {
                            for item in &mut turn.content {
                                if let rig::message::AssistantContent::Text(t) = item
                                    && crate::engine::turn::history::parse_speaker_tag(&t.text)
                                        .is_none()
                                {
                                    t.text = crate::engine::turn::history::tag_assistant_text(
                                        &old_char_id,
                                        &old_char_name,
                                        &t.text,
                                    );
                                }
                            }
                        }
                    }
                }
                state_lock.character_id = self.character_id.clone();
                state_lock.character_display_name = self.name.clone();
                state_lock.agent_name = self.name.clone();
            }
            self.total_tokens = state_lock.total_tokens;
            self.context_tokens = state_lock.context_tokens;
            self.total_cost_usd = state_lock.total_cost_usd;
            (
                state_lock.total_tokens,
                state_lock.context_tokens,
                state_lock.total_cost_usd,
                !state_lock.history.is_empty(),
            )
        };
        let (web_search_enabled, vibe_enabled) = {
            let state_lock = self.state.lock().await;
            (state_lock.web_search_enabled, state_lock.vibe_enabled)
        };

        let model_label = |m: &Option<_>| -> String {
            m.as_ref().map_or_else(|| "Default".to_string(), |p: &boxxy_model_selection::ModelProvider| p.format_label())
        };

        let (old_claw, old_memory, old_search) = {
            let state_lock = self.state.lock().await;
            (
                model_label(&state_lock.current_claw_model),
                model_label(&state_lock.current_memory_model),
                state_lock.current_search_label.clone(),
            )
        };

        boxxy_preferences::Settings::reload();
        crate::registry::skills::reload_if_initialized().await;

        let after = boxxy_preferences::Settings::load();
        let (new_claw, new_memory, new_search) = (
            model_label(&after.claw_model),
            model_label(&after.memory_model),
            crate::engine::agent_config::resolve_active_search_label(&after, web_search_enabled)
                .to_string(),
        );
        let new_context_window = boxxy_ai_core::model_context_window_with_endpoints(
            &after.claw_model,
            &after.openai_compatible_endpoints,
        );

        {
            let mut state_lock = self.state.lock().await;
            state_lock.persistent_agent = None;
            state_lock
                .roundtable
                .sync_max_rounds(after.max_roundtable_turns.clamp(1, 100) as u8);
        }

        if character_changed {
            let db_guard = self.db.lock().await;
            if let Some(db) = &*db_guard {
                let store = boxxy_db::store::Store::new(db.pool());
                let _ = store
                    .update_session_character(&self.session_id, &self.character_id, &self.name)
                    .await;
            }
            drop(db_guard);

            let _ = self
                .tx_ui
                .send(ClawEngineEvent::Identity {
                    agent_name: self.name.clone(),
                    character_id: self.character_id.clone(),
                    pinned: self.pinned,
                    web_search_enabled,
                    vibe_enabled,
                    total_tokens,
                    context_tokens,
                    context_window: new_context_window,
                    total_cost_usd,
                })
                .await;

            if character_id_switched && had_messages {
                let switch_event = ClawEngineEvent::SystemMessage {
                    text: format!(
                        "Character Switch: {} → {}",
                        crate::engine::session::roundtable::format_character_title(&old_char_name),
                        crate::engine::session::roundtable::format_character_title(&self.name),
                    ),
                };
                crate::engine::persist_visual_event(
                    self.db.clone(),
                    self.session_id.clone(),
                    self.pane_id.clone(),
                    &switch_event,
                );
                let _ = self.tx_ui.send(switch_event).await;
            }
        }

        if new_claw == old_claw && new_memory == old_memory && new_search == old_search {
            return;
        }

        {
            let mut state_lock = self.state.lock().await;
            state_lock.current_claw_model = after.claw_model.clone();
            state_lock.current_memory_model = after.memory_model.clone();
            state_lock.current_search_label = new_search;
        }

        if !character_changed && new_claw != old_claw {
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::Identity {
                    agent_name: self.name.clone(),
                    character_id: self.character_id.clone(),
                    pinned: self.pinned,
                    web_search_enabled,
                    vibe_enabled,
                    total_tokens,
                    context_tokens,
                    context_window: new_context_window,
                    total_cost_usd,
                })
                .await;
        }

        let event = ClawEngineEvent::SystemMessage {
            text: crate::engine::agent_config::format_session_models_banner(
                &after,
                web_search_enabled,
            ),
        };
        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );
        let _ = self.tx_ui.send(event).await;
    }

    pub(super) async fn handle_initialize(&mut self, current_dir: &str) {
        info!("Initializing NEW Claw Session for pane {}...", self.pane_id);

        let name = self.name.clone();

        if boxxy_db::DATABASE_WAS_RESET.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.send_ui(ClawEngineEvent::SystemMessage {
                text: "⚠️ Database reset for update. This only happens during the Preview."
                    .to_string(),
            })
            .await;
        }

        self.total_tokens = 0;
        self.context_tokens = 0;
        self.total_cost_usd = 0.0;

        let mut state_lock = self.state.lock().await;
        state_lock.agent_name = name.clone();
        state_lock.history.clear();
        state_lock.flushed_watermark = 0;
        state_lock.title_requested = false;
        state_lock.pending_tasks.clear();
        state_lock.total_tokens = 0;
        state_lock.context_tokens = 0;
        state_lock.total_cost_usd = 0.0;
        state_lock.status = crate::engine::AgentStatus::Waiting;
        drop(state_lock);

        if self.incognito {
            self.clear_ui_history().await;
            let settings = boxxy_preferences::Settings::load();
            let context_window = boxxy_ai_core::model_context_window_with_endpoints(
                &settings.claw_model,
                &settings.openai_compatible_endpoints,
            );
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::Identity {
                    agent_name: name.clone(),
                    character_id: self.character_id.clone(),
                    pinned: false,
                    web_search_enabled: false,
                    vibe_enabled: false,
                    total_tokens: 0,
                    context_tokens: 0,
                    context_window,
                    total_cost_usd: 0.0,
                })
                .await;
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::SessionStateChanged {
                    agent_name: name,
                    character_id: self.character_id.clone(),
                    status: crate::engine::AgentStatus::Waiting,
                })
                .await;
            let tx_ui = self.tx_ui.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let _ = tx_ui
                    .send(ClawEngineEvent::SystemMessage {
                        text: "🕵️ Incognito Mode • No context sent • Nothing saved".to_string(),
                    })
                    .await;
            });
            return;
        }

        let db_guard = self.db.lock().await;
        if db_guard.is_none()
            && let Ok(db) = Db::new().await
        {
            drop(db_guard);
            *self.db.lock().await = Some(db);
        } else {
            drop(db_guard);
        }

        let db_guard = self.db.lock().await;
        if let Some(db) = &*db_guard {
            let store = boxxy_db::store::Store::new(db.pool());
            let _ = store.clear_claw_events(&self.session_id).await;
        }
        drop(db_guard);

        self.clear_ui_history().await;

        let workspace = crate::registry::workspace::global_workspace().await;
        workspace
            .update_pane_state(
                self.pane_id.clone(),
                Some(self.session_id.clone()),
                Some(name.clone()),
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
        let (web_search_enabled, vibe_enabled) = {
            let state_lock = self.state.lock().await;
            (state_lock.web_search_enabled, state_lock.vibe_enabled)
        };
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::Identity {
                agent_name: name,
                character_id: self.character_id.clone(),
                pinned: false,
                web_search_enabled,
                vibe_enabled,
                total_tokens: 0,
                context_tokens: 0,
                context_window,
                total_cost_usd: 0.0,
            })
            .await;

        let banner = crate::engine::agent_config::format_session_models_banner(
            &settings,
            web_search_enabled,
        );
        let search_label =
            crate::engine::agent_config::resolve_active_search_label(&settings, web_search_enabled)
                .to_string();

        {
            let mut state_lock = self.state.lock().await;
            state_lock.current_claw_model = settings.claw_model.clone();
            state_lock.current_memory_model = settings.memory_model.clone();
            state_lock.current_search_label = search_label;
        }

        let event = ClawEngineEvent::SystemMessage { text: banner };
        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );

        let tx_ui = self.tx_ui.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = tx_ui.send(event).await;
        });
    }
}
