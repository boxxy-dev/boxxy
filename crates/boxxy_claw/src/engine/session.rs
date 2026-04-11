use crate::utils::load_prompt_fallback;
use crate::engine::{
    ClawEngineEvent, ClawMessage, PersistentClawRow, TaskStatus, TaskType,
    context::load_session_context, context::retrieve_memories, context::summarize_and_store,
    dispatcher::extract_command_and_clean, persist_visual_event,
};
use boxxy_agent::ipc::AgentClawProxy;
use boxxy_db::Db;
use log::{debug, info};
use rig::message::Message;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct SessionState {
    pub session_id: String,
    pub pane_id: String,
    pub agent_name: String,
    pub pinned: bool,
    pub web_search_enabled: bool,
    pub total_tokens: u64,
    pub pending_terminal_reply: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
    pub pending_file_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_file_delete_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_kill_process_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_get_clipboard_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_set_clipboard_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub history: Vec<Message>,
    pub pending_lazy_diagnosis: Option<(String, String, String)>,
    pub persistent_agent: Option<crate::engine::agent::ClawAgent>,
    pub last_tools: Option<Vec<String>>,
    pub pending_tasks: Vec<crate::engine::ScheduledTask>,
    pub last_snapshot_hash: Option<u64>,
    pub is_suspended: bool,
    pub awaiting_tasks: Vec<uuid::Uuid>,
}

pub struct ClawSession {
    pub pane_id: String,
    pub session_id: String,
    pub name: String,
    pub pinned: bool,
    pub total_tokens: u64,
    pub rx: async_channel::Receiver<ClawMessage>,
    pub tx_self: async_channel::Sender<ClawMessage>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub session_context: String,
    pub db: Arc<Mutex<Option<Db>>>,
    pub state: Arc<Mutex<SessionState>>,
    pub diagnosis_mode: boxxy_preferences::config::ClawAutoDiagnosisMode,
}

impl ClawSession {
    pub fn new(
        pane_id: String,
    ) -> (
        Self,
        async_channel::Sender<ClawMessage>,
        async_channel::Receiver<ClawEngineEvent>,
    ) {
        let (tx, rx) = async_channel::unbounded();
        let (tx_ui, rx_ui) = async_channel::unbounded();

        let settings = boxxy_preferences::Settings::load();

        // Generate a funny name for this agent
        let name = petname::petname(2, " ").unwrap_or_else(|| "Unknown Agent".to_string());
        let session_id = uuid::Uuid::new_v4().to_string();

        // We defer loading session context and skills until the first use
        let session = Self {
            pane_id: pane_id.clone(),
            session_id: session_id.clone(),
            name: name.clone(),
            pinned: false,
            total_tokens: 0,
            rx,
            tx_self: tx.clone(),
            tx_ui,
            session_context: String::new(),
            db: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(SessionState {
                session_id,
                pane_id: pane_id.clone(),
                agent_name: name,
                pinned: false,
                web_search_enabled: settings.enable_web_search,
                total_tokens: 0,
                pending_terminal_reply: None,
                pending_file_reply: None,
                pending_file_delete_reply: None,
                pending_kill_process_reply: None,
                pending_get_clipboard_reply: None,
                pending_set_clipboard_reply: None,
                history: Vec::new(),
                pending_lazy_diagnosis: None,
                persistent_agent: None,
                last_tools: None,
                pending_tasks: Vec::new(),
                last_snapshot_hash: None,
                is_suspended: false,
                awaiting_tasks: Vec::new(),
            })),
            diagnosis_mode: boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy,
        };

        (session, tx, rx_ui)
    }

    pub fn start(self, claw_proxy: AgentClawProxy<'static>) {
        tokio::spawn(async move {
            self.run(claw_proxy).await;
        });
    }

    async fn send_ui(&self, event: ClawEngineEvent) {
        persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );
        let _ = self.tx_ui.send(event).await;
    }

    pub async fn run(mut self, claw_proxy: AgentClawProxy<'static>) {
        // LAZY LOADING: We don't initialize the DB or load skills until we receive a message.
        // This ensures BoxxyClaw doesn't consume memory/CPU if the user doesn't use the AI.

        let mut is_initialized = false;
        let mut current_dir = String::from("/");
        let mut current_turn: Option<tokio::task::JoinHandle<()>> = None;
        let mut backlog: VecDeque<ClawMessage> = VecDeque::new();

        let workspace = crate::registry::workspace::global_workspace().await;

        let mut task_interval = tokio::time::interval(std::time::Duration::from_secs(1));

        loop {
            tokio::select! {
                msg_res = self.rx.recv() => {
                    let msg = match msg_res {
                        Ok(m) => m,
                        Err(_) => break,
                    };

                    let needs_initialization = match &msg {
                        ClawMessage::ClawQuery { .. }
                        | ClawMessage::UserMessage { .. }
                        | ClawMessage::DelegatedTask { .. }
                        | ClawMessage::RequestLazyDiagnosis
                        | ClawMessage::Initialize => true,
                        ClawMessage::CommandFinished { exit_code, .. }
                            if *exit_code != 0 && *exit_code != 130 && *exit_code != 131 =>
                        {
                            self.diagnosis_mode != boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
                        }
                        _ => false,
                    };

                    if !is_initialized && needs_initialization {
                        info!(
                            "Initializing Claw Session for pane {} ({}) upon first request...",
                            self.pane_id, self.name
                        );

                        // If this is an explicit Initialize message, we'll handle the identity
                        // announcement in the match block below to avoid double announcements.
                        if !matches!(&msg, ClawMessage::Initialize) {
                            let web_search_enabled = self.state.lock().await.web_search_enabled;
                            let _ = self
                                .tx_ui
                                .send(ClawEngineEvent::Identity {
                                    agent_name: self.name.clone(),
                                    pinned: self.pinned,
                                    web_search_enabled,
                                    total_tokens: self.total_tokens,
                                })
                                .await;
                        }

                        let session_context = load_session_context();
                        self.session_context = session_context;

                        if let Ok(db) = Db::new().await {
                            *self.db.lock().await = Some(db);
                            info!(
                                "Claw Memory Database initialized for pane {} ({}).",
                                self.pane_id, self.name
                            );

                            // Sync any manual edits from MEMORY.md back to the DB
                            let _ = crate::memories::db::sync_markdown_to_db(self.db.clone()).await;

                            // Run Memory Hygiene
                            let _ = crate::memories::hygiene::run_hygiene(self.db.clone()).await;
                        } else {
                            log::error!("Failed to initialize Claw Memory Database.");
                        }

                        // Register our Sender to allow other agents/windows to delegate tasks or evict us
                        let workspace = crate::registry::workspace::global_workspace().await;
                        workspace
                            .register_pane_tx(self.pane_id.clone(), self.tx_self.clone())
                            .await;

                        is_initialized = true;
                    }

                    let session_ctx = self.session_context.clone();

                    match msg {
                        ClawMessage::Abort => {
                            if let Some(handle) = current_turn.take() {
                                handle.abort();
                                let _ = self
                                    .tx_ui
                                    .send(ClawEngineEvent::AgentThinking {
                                        agent_name: self.name.clone(),
                                        is_thinking: false,
                                    })
                                    .await;
                            }
                            backlog.clear();

                            let mut state_lock = self.state.lock().await;
                            if let Some(reply) = state_lock.pending_terminal_reply.take() {
                                let _ = reply.send(Err("[ABORT]".to_string()));
                            }
                            if let Some(reply) = state_lock.pending_file_reply.take() {
                                let _ = reply.send(false);
                            }
                            if let Some(reply) = state_lock.pending_file_delete_reply.take() {
                                let _ = reply.send(false);
                            }
                            if let Some(reply) = state_lock.pending_kill_process_reply.take() {
                                let _ = reply.send(false);
                            }
                            if let Some(reply) = state_lock.pending_get_clipboard_reply.take() {
                                let _ = reply.send(false);
                            }
                            if let Some(reply) = state_lock.pending_set_clipboard_reply.take() {
                                let _ = reply.send(false);
                            }
                        }
                        ClawMessage::TurnFinished => {
                            current_turn = None;
                            if let Some(next_msg) = backlog.pop_front() {
                                let _ = self.tx_self.send(next_msg).await;
                            }
                        }
                        ClawMessage::Deactivate => {
                            info!("Deactivating Claw Session for pane {}...", self.pane_id);
                            let _ = self.tx_self.send(ClawMessage::Abort).await;
                            is_initialized = false;
                            *self.db.lock().await = None;

                            let mut state_lock = self.state.lock().await;
                            state_lock.history.clear();
                            state_lock.pending_tasks.clear();
                            drop(state_lock);

                            // Update Workspace Radar to indicate no active agent
                            workspace.unregister_pane(self.pane_id.clone()).await;
                        }
                        ClawMessage::Evict => {
                            info!("Agent in pane {} was EVICTED.", self.pane_id);
                            let _ = self.tx_self.send(ClawMessage::Abort).await;
                            is_initialized = false;
                            *self.db.lock().await = None;

                            let mut state_lock = self.state.lock().await;
                            state_lock.history.clear();
                            state_lock.pending_tasks.clear();
                            drop(state_lock);

                            let _ = self.tx_ui.send(ClawEngineEvent::Evicted).await;

                            // Update Workspace Radar to indicate no active agent
                            workspace.unregister_pane(self.pane_id.clone()).await;
                        }
                        ClawMessage::ResumeSession { session_id } => {
                            info!(
                                "Resuming session {} in pane {}...",
                                session_id, self.pane_id
                            );

                            // 1. Evict session if active elsewhere
                            workspace.evict_session(&session_id).await;

                            // 2. Load session from DB
                            {
                                let db_guard = self.db.lock().await;
                                if db_guard.is_none() {
                                    if let Ok(db) = Db::new().await {
                                        drop(db_guard);
                                        *self.db.lock().await = Some(db);
                                    }
                                }
                            }

                            let db_guard = self.db.lock().await;
                            if let Some(db) = &*db_guard {
                                let store = boxxy_db::store::Store::new(db.pool());
                                match store.get_session(&session_id).await {
                                    Ok(Some(session)) => {
                                        // 3. Rehydrate state
                                        self.session_id = session_id.clone();
                                        if let Some(agent_name) = session.agent_name {
                                            self.name = agent_name.clone();
                                        }
                                        self.pinned = session.pinned;
                                        self.total_tokens = session.total_tokens as u64;

                                        let mut state_lock = self.state.lock().await;
                                        state_lock.session_id = session_id.clone();
                                        state_lock.agent_name = self.name.clone();
                                        state_lock.pinned = self.pinned;
                                        state_lock.total_tokens = self.total_tokens;

                                        if let Some(history_json) = session.history_json
                                            && let Ok(history) =
                                                serde_json::from_str::<Vec<Message>>(&history_json)
                                            {
                                                state_lock.history = history;
                                            }

                                        if let Some(tasks_json) = session.pending_tasks_json
                                            && let Ok(tasks) =
                                                serde_json::from_str::<Vec<crate::engine::ScheduledTask>>(&tasks_json)
                                            {
                                                state_lock.pending_tasks = tasks;
                                            }

                                        // Force agent rebuild
                                        state_lock.persistent_agent = None;
                                        let agent_name_clone = state_lock.agent_name.clone();
                                        let tasks_clone = state_lock.pending_tasks.clone();
                                        let web_search_enabled = state_lock.web_search_enabled;
                                        drop(state_lock);

                                        // 4. Load visual history from DB
                                        if let Ok(event_jsons) = store.get_claw_events(&session_id).await {
                                            let rows: Vec<PersistentClawRow> = event_jsons
                                                .into_iter()
                                                .filter_map(|json| serde_json::from_str(&json).ok())
                                                .collect();
                                            if !rows.is_empty() {
                                                let _ = self.tx_ui.send(ClawEngineEvent::RestoreHistory(rows)).await;
                                            }
                                        }

                                        // 5. Update Workspace Radar
                                        workspace
                                            .update_pane_state(
                                                self.pane_id.clone(),
                                                Some(self.session_id.clone()),
                                                Some(self.name.clone()),
                                                current_dir.clone(),
                                                None,
                                                None,
                                            )
                                            .await;

                                        // 6. Announce Identity to UI
                                        let _ = self
                                            .tx_ui
                                            .send(ClawEngineEvent::Identity {
                                                agent_name: self.name.clone(),
                                                pinned: self.pinned,
                                                web_search_enabled,
                                                total_tokens: self.total_tokens,
                                            })
                                            .await;

                                        let _ = self.tx_ui.send(ClawEngineEvent::TaskStatusChanged {
                                            agent_name: agent_name_clone,
                                            tasks: tasks_clone.clone(),
                                        }).await;

                                        workspace.update_pane_tasks(self.pane_id.clone(), tasks_clone).await;

                                        let session_type = if self.pinned { "pinned" } else { "normal" };
                                        boxxy_telemetry::track_session_resume(session_type).await;

                                        self.send_ui(ClawEngineEvent::SystemMessage {
                                                text: "Session resumed.".to_string(),
                                            })
                                            .await;

                                        // 6. Handle CWD Switch
                                        if let Some(last_cwd) = session.last_cwd {
                                            let _ = self
                                                .tx_ui
                                                .send(ClawEngineEvent::RequestCwdSwitch { path: last_cwd })
                                                .await;
                                        }
                                        is_initialized = true;
                                    }
                                    _ => {
                                        let _ = self
                                            .tx_ui
                                            .send(ClawEngineEvent::SystemMessage {
                                                text: "Failed to load session from database.".to_string(),
                                            })
                                            .await;
                                    }
                                }
                            }
                            drop(db_guard);
                        }
                        ClawMessage::Initialize => {
                            info!("Initializing NEW Claw Session for pane {}...", self.pane_id);

                            // 1. Pick a new name
                            let name =
                                petname::petname(2, " ").unwrap_or_else(|| "Unknown Agent".to_string());
                            self.name = name.clone();

                            // Check if database was reset (due to update in Preview Phase)
                            // We use swap(false) to ensure only the first agent to initialize shows the notification
                            if boxxy_db::DATABASE_WAS_RESET.swap(false, std::sync::atomic::Ordering::SeqCst)
                            {
                                self.send_ui(ClawEngineEvent::SystemMessage {
                                    text: "⚠️ Database reset for update. This only happens during the Preview.".to_string()
                                }).await;
                            }

                            // 2. Clear history and update agent name in state
                            let mut state_lock = self.state.lock().await;
                            state_lock.agent_name = name.clone();
                            state_lock.history.clear();
                            state_lock.pending_tasks.clear();
                            drop(state_lock);

                            // Clear visual history in DB
                            let db_guard = self.db.lock().await;
                            if db_guard.is_none()
                                && let Ok(db) = Db::new().await {
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

                            // 3. Update Workspace Radar
                            workspace
                                .update_pane_state(
                                    self.pane_id.clone(),
                                    Some(self.session_id.clone()),
                                    Some(name.clone()),
                                    current_dir.clone(),
                                    None,
                                    None,
                                )
                                .await;

                            // 4. Announce Identity to UI
                            let web_search_enabled = self.state.lock().await.web_search_enabled;
                            let _ = self
                                .tx_ui
                                .send(ClawEngineEvent::Identity {
                                    agent_name: name,
                                    pinned: false,
                                    web_search_enabled,
                                    total_tokens: 0,
                                })
                                .await;
                        }
                        ClawMessage::Reload => {
                            info!("Reloading Claw Session state...");
                            let new_ctx = load_session_context();
                            self.session_context = new_ctx;
                            if let Ok(db) = Db::new().await {
                                *self.db.lock().await = Some(db);
                            }
                        }
                        ClawMessage::TogglePin(pinned) => {
                            self.pinned = pinned;
                            let mut state_lock = self.state.lock().await;
                            state_lock.pinned = pinned;

                            let history_json = serde_json::to_string(&state_lock.history).unwrap_or_default();
                            let pending_tasks_json =
                                serde_json::to_string(&state_lock.pending_tasks).unwrap_or_default();
                            let agent_name_for_db = state_lock.agent_name.clone();
                            let session_id_for_db = self.session_id.clone();
                            let cwd_for_db = current_dir.clone();
                            let total_tokens_for_db = state_lock.total_tokens as i64;
                            let settings = boxxy_preferences::Settings::load();
                            let model_id = settings
                                .claw_model
                                .as_ref()
                                .map(|m| format!("{:?}", m))
                                .unwrap_or_default();

                            let db_for_persistence = self.db.clone();
                            tokio::spawn(async move {
                                let db_guard = db_for_persistence.lock().await;
                                if let Some(db) = &*db_guard {
                                    let store = boxxy_db::store::Store::new(db.pool());
                                    let _ = store
                                        .upsert_session_state(
                                            &session_id_for_db,
                                            &agent_name_for_db,
                                            "", // title not updated here
                                            &history_json,
                                            &pending_tasks_json,
                                            &agent_name_for_db,
                                            &cwd_for_db,
                                            &model_id,
                                            pinned,
                                            total_tokens_for_db,
                                        )
                                        .await;
                                }
                            });
                            drop(state_lock);
                            let _ = self.tx_ui.send(ClawEngineEvent::PinStatusChanged(pinned)).await;
                        }
                        ClawMessage::ToggleWebSearch(enabled) => {
                            let mut state_lock = self.state.lock().await;
                            if state_lock.web_search_enabled != enabled {
                                state_lock.web_search_enabled = enabled;
                                // Force agent rebuild to update tools
                                state_lock.persistent_agent = None;
                                let _ = self.tx_ui.send(ClawEngineEvent::WebSearchStatusChanged(enabled)).await;
                            }
                        }
                        ClawMessage::ForegroundProcessChanged { process_name } => {
                            let status = if process_name.is_empty() {
                                None
                            } else {
                                Some(format!("Running: {}", process_name))
                            };
                            workspace
                                .set_pane_status(self.pane_id.clone(), status)
                                .await;
                        }
                        ClawMessage::CommandFinished {
                            exit_code,
                            snapshot,
                            cwd,
                        } => {
                            current_dir = cwd.clone();

                            // Update workspace state
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
                                if exit_code == 0 {
                                    let _ = reply.send(Ok(snapshot.clone()));
                                } else {
                                    let _ = reply.send(Err(format!(
                                        "Command failed with exit code {exit_code}:\n{snapshot}"
                                    )));
                                }
                            } else if exit_code != 0 {
                                if exit_code == 130 || exit_code == 131 {
                                    continue;
                                }

                                let prompt = format!(
                                    "The user's command failed with exit code {exit_code}. Please analyze the terminal snapshot and suggest a fix."
                                );

                                if self.diagnosis_mode
                                    == boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
                                {
                                    state_lock.pending_lazy_diagnosis = Some((prompt, snapshot, cwd));
                                    drop(state_lock);

                                    let tx_ui = self.tx_ui.clone();
                                    let agent_name = self.name.clone();
                                    tokio::spawn(async move {
                                        let _ = tx_ui
                                            .send(ClawEngineEvent::LazyErrorIndicator {
                                                agent_name,
                                                visible: true,
                                            })
                                            .await;
                                    });
                                } else {
                                    drop(state_lock);
                                    if current_turn.is_some() {
                                        backlog.push_back(ClawMessage::CommandFinished {
                                            exit_code,
                                            snapshot,
                                            cwd,
                                        });
                                        continue;
                                    }

                                    current_turn = Some(spawn_turn(
                                        self.pane_id.clone(),
                                        self.session_id.clone(),
                                        self.name.clone(),
                                        &prompt,
                                        &snapshot,
                                        &session_ctx,
                                        cwd,
                                        false,
                                        claw_proxy.clone(),
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
                        ClawMessage::RequestLazyDiagnosis => {
                            let mut state_lock = self.state.lock().await;
                            if let Some((prompt, snapshot, cwd)) = state_lock.pending_lazy_diagnosis.take()
                            {
                                drop(state_lock);
                                if let Some(handle) = current_turn.take() {
                                    handle.abort();
                                    let _ = self
                                        .tx_ui
                                        .send(ClawEngineEvent::AgentThinking {
                                            agent_name: self.name.clone(),
                                            is_thinking: false,
                                        })
                                        .await;
                                }
                                backlog.clear();
                                current_turn = Some(spawn_turn(
                                    self.pane_id.clone(),
                                    self.session_id.clone(),
                                    self.name.clone(),
                                    &prompt,
                                    &snapshot,
                                    &session_ctx,
                                    cwd,
                                    false,
                                    claw_proxy.clone(),
                                    self.db.clone(),
                                    self.state.clone(),
                                    self.tx_ui.clone(),
                                    self.tx_self.clone(),
                                    None,
                                    vec![],
                                ));
                            }
                        }
                        ClawMessage::ClawQuery {
                            query,
                            snapshot,
                            cwd,
                            image_attachments,
                        } => {
                            current_dir = cwd.clone();
                            if let Some(handle) = current_turn.take() {
                                handle.abort();
                                let _ = self
                                    .tx_ui
                                    .send(ClawEngineEvent::AgentThinking {
                                        agent_name: self.name.clone(),
                                        is_thinking: false,
                                    })
                                    .await;
                            }
                            backlog.clear();

                            // Update workspace state
                            workspace
                                .update_pane_state(
                                    self.pane_id.clone(),
                                    Some(self.session_id.clone()),
                                    Some(self.name.clone()),
                                    current_dir.clone(),
                                    Some(query.clone()),
                                    Some(snapshot.clone()),
                                )
                                .await;

                            debug!(
                                "Pane {} ({}): Direct Claw query: {query}. Starting analysis.",
                                self.pane_id, self.name
                            );
                            current_turn = Some(spawn_turn(
                                self.pane_id.clone(),
                                self.session_id.clone(),
                                self.name.clone(),
                                &query,
                                &snapshot,
                                &session_ctx,
                                cwd,
                                false,
                                claw_proxy.clone(),
                                self.db.clone(),
                                self.state.clone(),
                                self.tx_ui.clone(),
                                self.tx_self.clone(),
                                None,
                                image_attachments,
                            ));
                        }
                        ClawMessage::FileWriteReply { approved } => {
                            let mut state_lock = self.state.lock().await;
                            if let Some(reply) = state_lock.pending_file_reply.take() {
                                let _ = reply.send(approved);
                            }
                        }
                        ClawMessage::FileDeleteReply { approved } => {
                            let mut state_lock = self.state.lock().await;
                            if let Some(reply) = state_lock.pending_file_delete_reply.take() {
                                let _ = reply.send(approved);
                            }
                        }
                        ClawMessage::KillProcessReply { approved } => {
                            let mut state_lock = self.state.lock().await;
                            if let Some(reply) = state_lock.pending_kill_process_reply.take() {
                                let _ = reply.send(approved);
                            }
                        }
                        ClawMessage::GetClipboardReply { approved } => {
                            let mut state_lock = self.state.lock().await;
                            if let Some(reply) = state_lock.pending_get_clipboard_reply.take() {
                                let _ = reply.send(approved);
                            }
                        }
                        ClawMessage::SetClipboardReply { approved } => {
                            let mut state_lock = self.state.lock().await;
                            if let Some(reply) = state_lock.pending_set_clipboard_reply.take() {
                                let _ = reply.send(approved);
                            }
                        }
                        ClawMessage::UserMessage {
                            message,
                            snapshot,
                            cwd,
                            image_attachments,
                        } => {
                            current_dir = cwd.clone();
                            debug!(
                                "Pane {} ({}): User reply: {message}. Checking for pending tools.",
                                self.pane_id, self.name
                            );

                            // Update workspace state
                            workspace
                                .update_pane_state(
                                    self.pane_id.clone(),
                                    Some(self.session_id.clone()),
                                    Some(self.name.clone()),
                                    current_dir.clone(),
                                    None,
                                    Some(snapshot.clone()),
                                )
                                .await;

                            let mut state_lock = self.state.lock().await;
                            let mut fulfilled = false;

                            if let Some(reply) = state_lock.pending_terminal_reply.take() {
                                let _ = reply.send(Err(format!(
                                    "[USER_INTERRUPTION]: {message}"
                                )));
                                state_lock.history.push(rig::message::Message::user(message.clone()));
                                fulfilled = true;
                            } else if let Some(reply) = state_lock.pending_file_reply.take() {
                                let _ = reply.send(false);
                                state_lock.history.push(rig::message::Message::user(message.clone()));
                                fulfilled = true;
                            }


                            if fulfilled {
                                debug!(
                                    "Pane {} ({}): Fulfilled pending tool with user feedback.",
                                    self.pane_id, self.name
                                );
                            } else {
                                drop(state_lock);
                                if let Some(handle) = current_turn.take() {
                                    handle.abort();
                                    let _ = self
                                        .tx_ui
                                        .send(ClawEngineEvent::AgentThinking {
                                            agent_name: self.name.clone(),
                                            is_thinking: false,
                                        })
                                        .await;
                                }
                                backlog.clear();
                                current_turn = Some(spawn_turn(
                                    self.pane_id.clone(),
                                    self.session_id.clone(),
                                    self.name.clone(),
                                    &message,
                                    &snapshot,
                                    &session_ctx,
                                    cwd,
                                    false,
                                    claw_proxy.clone(),
                                    self.db.clone(),
                                    self.state.clone(),
                                    self.tx_ui.clone(),
                                    self.tx_self.clone(),
                                    None,
                                    image_attachments,
                                ));
                            }
                        }
                        ClawMessage::DelegatedTask {
                            source_agent_name,
                            prompt,
                            reply_tx,
                        } => {
                            if current_turn.is_some() {
                                debug!(
                                    "Pane {} ({}): Agent is busy. Queueing delegated task from {}.",
                                    self.pane_id, self.name, source_agent_name
                                );
                                backlog.push_back(ClawMessage::DelegatedTask {
                                    source_agent_name,
                                    prompt,
                                    reply_tx,
                                });
                                continue;
                            }

                            let snapshot = workspace
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

                            current_turn = Some(spawn_turn(
                                self.pane_id.clone(),
                                self.session_id.clone(),
                                self.name.clone(),
                                &full_prompt,
                                &snapshot,
                                &session_ctx,
                                current_dir.clone(),
                                false,
                                claw_proxy.clone(),
                                self.db.clone(),
                                self.state.clone(),
                                self.tx_ui.clone(),
                                self.tx_self.clone(),
                                Some(reply_tx),
                                vec![],
                            ));
                        }
                        ClawMessage::CancelPending => {
                            let mut state_lock = self.state.lock().await;
                            if let Some(reply) = state_lock.pending_terminal_reply.take() {
                                let _ = reply.send(Err("[USER_EXPLICIT_REJECT]".to_string()));
                            }
                            if let Some(reply) = state_lock.pending_file_reply.take() {
                                let _ = reply.send(false);
                            }
                            if let Some(reply) = state_lock.pending_file_delete_reply.take() {
                                let _ = reply.send(false);
                            }
                            if let Some(reply) = state_lock.pending_kill_process_reply.take() {
                                let _ = reply.send(false);
                            }
                            if let Some(reply) = state_lock.pending_get_clipboard_reply.take() {
                                let _ = reply.send(false);
                            }
                            if let Some(reply) = state_lock.pending_set_clipboard_reply.take() {
                                let _ = reply.send(false);
                            }
                            debug!("Pane {}: User cancelled pending proposals.", self.pane_id);
                            let _ = self
                                .tx_ui
                                .send(ClawEngineEvent::ProposalResolved {
                                    agent_name: self.name.clone(),
                                    approved: false,
                                })
                                .await;
                        }
                        ClawMessage::SoftClearHistory => {
                            info!("Soft-clearing Claw Session visual history for pane {}...", self.pane_id);
                            let db_guard = self.db.lock().await;
                            if let Some(db) = &*db_guard {
                                let store = boxxy_db::store::Store::new(db.pool());
                                let _ = store.mark_session_cleared(&self.session_id).await;
                            }
                        }
                        ClawMessage::CancelTask { task_id } => {
                            let mut state_lock = self.state.lock().await;
                            state_lock.pending_tasks.retain(|t| t.id != task_id);
                            let agent_name = state_lock.agent_name.clone();
                            let tasks = state_lock.pending_tasks.clone();
                            drop(state_lock);

                            let _ = self.tx_ui.send(ClawEngineEvent::TaskStatusChanged {
                                agent_name,
                                tasks: tasks.clone(),
                            }).await;

                            workspace.update_pane_tasks(self.pane_id.clone(), tasks).await;
                        }
                        ClawMessage::UpdateDiagnosisMode(mode) => {
                            self.diagnosis_mode = mode;
                        }
                        ClawMessage::SubscriptionEvent { event } => {
                            let mut state_lock = self.state.lock().await;
                            
                            // Handle external sleep requests
                            if let crate::engine::ClawEvent::Custom { name, .. } = &event {
                                if name == "request_sleep" && !state_lock.is_suspended {
                                    log::info!("Agent {} received external sleep request. Suspending.", self.name);
                                    state_lock.is_suspended = true;
                                    let agent_name = self.name.clone();
                                    let tx_ui = self.tx_ui.clone();
                                    
                                    tokio::spawn(async move {
                                        let _ = tx_ui.send(crate::engine::ClawEngineEvent::SessionStateChanged {
                                            agent_name,
                                            status: crate::engine::AgentStatus::Suspended,
                                        }).await;
                                    });
                                    continue;
                                }
                            }

                            if state_lock.is_suspended {
                                state_lock.is_suspended = false;
                                drop(state_lock);

                                // Broadcast to UI
                                let _ = self.tx_ui.send(crate::engine::ClawEngineEvent::SessionStateChanged {
                                    agent_name: self.name.clone(),
                                    status: crate::engine::AgentStatus::Active,
                                }).await;

                                let snapshot = workspace.get_pane_snapshot(self.pane_id.clone()).await.unwrap_or_default();
                                let prompt = format!("WAKEUP: An event you subscribed to has occurred: {:?}", event);

                                if current_turn.is_some() {
                                    backlog.push_back(ClawMessage::SubscriptionEvent { event });
                                    continue;
                                }

                                current_turn = Some(spawn_turn(
                                    self.pane_id.clone(),
                                    self.session_id.clone(),
                                    self.name.clone(),
                                    &prompt,
                                    &snapshot,
                                    &session_ctx,
                                    current_dir.clone(),
                                    false,
                                    claw_proxy.clone(),
                                    self.db.clone(),
                                    self.state.clone(),
                                    self.tx_ui.clone(),
                                    self.tx_self.clone(),
                                    None,
                                    vec![],
                                ));
                            }
                        }
                        ClawMessage::TaskCompletedEvent { task_id, result } => {
                            let mut state_lock = self.state.lock().await;
                            state_lock.awaiting_tasks.retain(|id| id != &task_id);

                            if state_lock.is_suspended && state_lock.awaiting_tasks.is_empty() {
                                state_lock.is_suspended = false;
                                drop(state_lock);

                                // Broadcast to UI
                                let _ = self.tx_ui.send(crate::engine::ClawEngineEvent::SessionStateChanged {
                                    agent_name: self.name.clone(),
                                    status: crate::engine::AgentStatus::Active,
                                }).await;

                                let snapshot = workspace.get_pane_snapshot(self.pane_id.clone()).await.unwrap_or_default();
                                let prompt = format!("WAKEUP: Async task {} has completed with result: {}", task_id, result);

                                if current_turn.is_some() {
                                    backlog.push_back(ClawMessage::TaskCompletedEvent { task_id, result });
                                    continue;
                                }

                                current_turn = Some(spawn_turn(
                                    self.pane_id.clone(),
                                    self.session_id.clone(),
                                    self.name.clone(),
                                    &prompt,
                                    &snapshot,
                                    &session_ctx,
                                    current_dir.clone(),
                                    false,
                                    claw_proxy.clone(),
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
                }
                _ = task_interval.tick() => {
                    // 1. Process Due Tasks
                    let now = chrono::Utc::now();
                    let mut state_lock = self.state.lock().await;
                    let mut completed_indices = Vec::new();

                    for (i, task) in state_lock.pending_tasks.iter_mut().enumerate() {
                        if task.due_at <= now && task.status == TaskStatus::Pending {
                            // Execute task
                            match task.task_type {
                                TaskType::Notification => {
                                    let text = task.payload.clone();
                                    let tx_ui = self.tx_ui.clone();
                                    let db = self.db.clone();
                                    let session_id = self.session_id.clone();
                                    let pane_id = self.pane_id.clone();
                                    tokio::spawn(async move {
                                        // First persist the visual message
                                        let event = ClawEngineEvent::SystemMessage { text: text.clone() };
                                        persist_visual_event(db, session_id, pane_id, &event);
                                        let _ = tx_ui.send(event).await;

                                        // Then emit the global desktop notification event
                                        let _ = tx_ui.send(ClawEngineEvent::PushGlobalNotification {
                                            title: "Boxxy Reminder".to_string(),
                                            message: text,
                                        }).await;
                                    });
                                }
                                TaskType::Command | TaskType::Query => {
                                    let message = task.payload.clone();
                                    let tx_self = self.tx_self.clone();
                                    let pane_id = self.pane_id.clone();
                                    let workspace = workspace.clone();
                                    tokio::spawn(async move {
                                        if let Some(snapshot) = workspace.get_pane_snapshot(pane_id.clone()).await {
                                            let cwd = workspace.get_pane_cwd(pane_id).await.unwrap_or_else(|| "/".to_string());
                                            let _ = tx_self.send(ClawMessage::UserMessage {
                                                message,
                                                snapshot,
                                                cwd,
                                                image_attachments: vec![],
                                            }).await;
                                        }
                                    });
                                }
                            }
                            task.status = TaskStatus::Completed;
                            completed_indices.push(i);
                        }
                    }

                    if !completed_indices.is_empty() {
                        // Prune completed tasks
                        state_lock.pending_tasks.retain(|t| t.status == TaskStatus::Pending);
                        let agent_name = state_lock.agent_name.clone();
                        let agent_name_for_event = agent_name.clone();
                        let tasks = state_lock.pending_tasks.clone();
                        drop(state_lock);

                        let _ = self.tx_ui.send(ClawEngineEvent::TaskStatusChanged {
                            agent_name,
                            tasks: tasks.clone(),
                        }).await;

                        let _ = self.tx_ui.send(ClawEngineEvent::TaskCompleted {
                            agent_name: agent_name_for_event,
                            task_id: uuid::Uuid::nil(), // Placeholder for now as we don't have it easily available here
                        }).await;

                        workspace.update_pane_tasks(self.pane_id.clone(), tasks).await;
                    }
                }
            }
        }

        // Unregister on loop exit
        workspace.unregister_pane(self.pane_id).await;
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_turn(
    pane_id: String,
    session_id: String,
    agent_name: String,
    prompt: &str,
    snapshot: &str,
    _session_ctx: &str,
    cwd: String,
    is_new_task: bool,
    claw_proxy: AgentClawProxy<'static>,
    db: Arc<Mutex<Option<Db>>>,
    state: Arc<Mutex<SessionState>>,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    tx_self: async_channel::Sender<ClawMessage>,
    delegate_reply_tx: Option<tokio::sync::oneshot::Sender<String>>,
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
            let skill_names: Vec<String> = active_skills.iter().map(|s| s.frontmatter.name.clone()).collect();
            workspace.update_pane_skills(pane_id.clone(), skill_names).await;

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

        let system_prompt_template = load_prompt_fallback(
            "/dev/boxxy/BoxxyTerminal/prompts/claw.md",
            "claw.md",
        );

        let system_prompt =
            system_prompt_template.replace("{{available_skills}}", &available_skills_text);

        let creds = boxxy_ai_core::AiCredentials::new(
            settings.get_effective_api_keys(),
            settings.ollama_base_url.clone(),
        );

        // --- PHASE 3: PERSISTENT AGENT ---
        // We check if we already have an agent instance for this session.
        // If not, or if the model changed, we create a new one.
        let mut agent_opt = state_lock.persistent_agent.take();

        if agent_opt.is_none() {
            agent_opt = Some(
                crate::engine::agent::create_claw_agent(
                    &settings.claw_model,
                    &creds,
                    &system_prompt,
                    &claw_proxy,
                    &cwd_clone,
                    tx_ui.clone(),
                    state.clone(),
                    db.clone(),
                    &settings,
                    session_id_clone.clone(),
                    pane_id.clone(),
                    state_lock.web_search_enabled,
                )
                .await,
            );
        }

        let agent = agent_opt.unwrap();

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

                // Generate title if it's the first turn (System + User + Assistant = 3 messages)
                let mut title = String::new();
                if state_lock.history.len() == 3 {
                    title = prompt_clone.chars().take(50).collect();
                }

                let db_for_persistence = db.clone();
                tokio::spawn(async move {
                    let db_guard = db_for_persistence.lock().await;
                    if let Some(db) = &*db_guard {
                        let store = boxxy_db::store::Store::new(db.pool());
                        let _ = store
                            .upsert_session_state(
                                &session_id_for_db,
                                &agent_name_for_db,
                                &title,
                                &history_json,
                                &pending_tasks_json,
                                &agent_name_for_db,
                                &cwd_for_db,
                                &model_id,
                                pinned_for_db,
                                total_tokens_for_db,
                            )
                            .await;
                    }
                });

                // Optional: Trigger Memory Flush if history is too long
                let creds = boxxy_ai_core::AiCredentials::new(
                    settings.get_effective_api_keys(),
                    settings.ollama_base_url.clone(),
                );

                let _ = crate::memories::flush::flush_history(
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
                let db_for_summary = db.clone();
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
                tokio::spawn(async move {
                    let db_guard = db_for_summary.lock().await;
                    if db_guard.is_some() {
                        summarize_and_store(
                            &db_guard,
                            &session_id_for_summary,
                            &prompt_for_db,
                            &resp_for_db,
                            &cwd_for_db,
                            creds.clone(),
                        )
                        .await;
                    }
                    drop(db_guard);

                    let _ = crate::memories::extraction::extract_implicit_memory(
                        db_for_summary.clone(),
                        prompt_for_db,
                        resp_for_db,
                        mem_model,
                        creds,
                        cwd_for_db,
                    )
                    .await;
                });
                let mut command_opt = None;
                let mut clean_diagnosis = response.clone();

                let extracted = extract_command_and_clean(&response);
                if extracted.0.is_some() {
                    command_opt = extracted.0;
                    clean_diagnosis = extracted.1;
                }

                if clean_diagnosis.trim() == "[SILENT_ACK]" {
                    info!(
                        "Pane {} ({}): Agent acknowledged rejection silently. Not sending UI event.",
                        pane_id, agent_name
                    );
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
                        usage: usage,
                    };
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
                        usage: usage,
                    };
                    persist_visual_event(
                        db.clone(),
                        session_id_clone.clone(),
                        pane_id.clone(),
                        &event,
                    );
                    let _ = tx_ui.send(event).await;
                }
                if let Some(tx) = delegate_reply_tx {
                    let _ = tx.send(response.clone());
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

                if let Some(tx) = delegate_reply_tx {
                    let _ = tx.send(format!("Error: {}", e));
                }
            }
        }

        let _ = tx_self.send(ClawMessage::TurnFinished).await;
    })
}
