use crate::engine::{
    ClawEngineEvent, ClawEnvironment, ClawMessage, TaskStatus, TaskType,
    context::load_session_context, persist_visual_event, turn::spawn_turn,
};
use boxxy_db::Db;
use log::{debug, info};
use rig::message::Message;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

mod approvals;
mod diagnostics;
mod lifecycle;

pub struct SessionState {
    pub session_id: String,
    pub pane_id: String,
    pub agent_name: String,
    pub character_id: String,
    pub character_display_name: String,
    pub pinned: bool,
    pub web_search_enabled: bool,
    pub total_tokens: u64,
    pub tracked_pids: Arc<tokio::sync::RwLock<std::collections::HashSet<u32>>>,
    pub api_keys: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    pub ollama_url: Arc<tokio::sync::RwLock<String>>,
    pub pending_terminal_reply: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
    pub pending_inject_command: bool,
    pub pending_file_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_file_delete_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_kill_process_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_background_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_get_clipboard_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_set_clipboard_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_scrollbacks:
        std::collections::HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<String>>,
    pub pending_delegations:
        std::collections::HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<String>>,
    pub history: Vec<Message>,
    pub pending_lazy_diagnosis: Option<(String, String, String)>,
    pub persistent_agent: Option<crate::engine::agent::ClawAgent>,
    pub current_claw_model: Option<boxxy_model_selection::ModelProvider>,
    pub current_memory_model: Option<boxxy_model_selection::ModelProvider>,
    pub last_tools: Option<Vec<String>>,
    pub pending_tasks: Vec<crate::engine::ScheduledTask>,
    pub last_snapshot_hash: Option<u64>,
    pub status: crate::engine::AgentStatus,
    pub context_quality: crate::engine::ContextQuality,
    pub parent_id: Option<uuid::Uuid>,
    pub is_headless: bool,
    pub sleep_timestamp: Option<i64>,
    pub awaiting_tasks: Vec<uuid::Uuid>,
    pub tx_self: async_channel::Sender<ClawMessage>,
    pub mcp_handle: Option<std::sync::Arc<boxxy_mcp::manager::McpClientManager>>,
}

pub struct ClawSession {
    pub pane_id: String,
    pub session_id: String,
    pub name: String,
    pub character_id: String,
    pub pinned: bool,
    pub total_tokens: u64,
    pub rx: async_channel::Receiver<ClawMessage>,
    pub tx_self: async_channel::Sender<ClawMessage>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub session_context: String,
    pub db: Arc<Mutex<Option<Db>>>,
    pub state: Arc<Mutex<SessionState>>,
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

        let name = String::new();
        let session_id = uuid::Uuid::new_v4().to_string();

        let session = Self {
            pane_id: pane_id.clone(),
            session_id: session_id.clone(),
            name: name.clone(),
            character_id: String::new(),
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
                character_id: String::new(),
                character_display_name: String::new(),
                pinned: false,
                web_search_enabled: settings.enable_web_search,
                total_tokens: 0,
                tracked_pids: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
                api_keys: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
                ollama_url: Arc::new(tokio::sync::RwLock::new(String::new())),
                pending_terminal_reply: None,
                pending_inject_command: false,
                pending_file_reply: None,
                pending_file_delete_reply: None,
                pending_kill_process_reply: None,
                pending_background_reply: None,
                pending_get_clipboard_reply: None,
                pending_set_clipboard_reply: None,
                pending_scrollbacks: std::collections::HashMap::new(),
                pending_delegations: std::collections::HashMap::new(),
                history: Vec::new(),
                pending_lazy_diagnosis: None,
                persistent_agent: None,
                current_claw_model: settings.claw_model.clone(),
                current_memory_model: settings.memory_model.clone(),
                last_tools: None,
                pending_tasks: Vec::new(),
                last_snapshot_hash: None,
                status: crate::engine::AgentStatus::Off,
                context_quality: crate::engine::ContextQuality::Full,
                parent_id: None,
                is_headless: false,
                sleep_timestamp: None,
                awaiting_tasks: Vec::new(),
                tx_self: tx.clone(),
                mcp_handle: None,
            })),
        };

        (session, tx, rx_ui)
    }

    pub fn with_identity(
        mut self,
        character_id: String,
        character_display_name: String,
        session_id: String,
    ) -> Self {
        self.name = character_display_name.clone();
        self.character_id = character_id.clone();
        self.session_id = session_id.clone();
        if let Ok(mut state) = self.state.try_lock() {
            state.agent_name = character_display_name.clone();
            state.character_id = character_id;
            state.character_display_name = character_display_name;
            state.session_id = session_id;
        }
        self
    }

    pub fn start(self, env: Arc<dyn ClawEnvironment>) {
        tokio::spawn(async move {
            self.run(env).await;
        });
    }

    pub(super) async fn send_ui(&self, event: ClawEngineEvent) {
        persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &event,
        );
        let _ = self.tx_ui.send(event).await;
    }

    pub(super) async fn clear_ui_history(&self) {
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::RestoreHistory(vec![]))
            .await;
    }

    pub(super) fn is_urgent_msg(msg: &ClawMessage) -> bool {
        if let ClawMessage::Transition(req) = msg
            && req.source == crate::engine::TriggerSource::User
        {
            return true;
        }
        if let ClawMessage::SubscriptionEvent {
            event: crate::engine::ClawEvent::Custom { name, .. },
        } = msg
            && name == "request_sleep"
        {
            return true;
        }
        matches!(
            msg,
            ClawMessage::CancelPending
                | ClawMessage::SleepToggle(_)
                | ClawMessage::Abort
                | ClawMessage::TurnFinished
                | ClawMessage::ClawQuery { .. }
                | ClawMessage::UserMessage { .. }
                | ClawMessage::FileWriteReply { .. }
                | ClawMessage::FileDeleteReply { .. }
                | ClawMessage::KillProcessReply { .. }
                | ClawMessage::BackgroundCommandReply { .. }
                | ClawMessage::GetClipboardReply { .. }
                | ClawMessage::SetClipboardReply { .. }
                | ClawMessage::ScrollbackReply { .. }
                | ClawMessage::TaskCompletedEvent { .. }
                | ClawMessage::CommandFinished { .. }
        )
    }

    pub(super) async fn get_next_msg(
        rx: &async_channel::Receiver<ClawMessage>,
        urgent_backlog: &mut VecDeque<ClawMessage>,
        backlog: &mut VecDeque<ClawMessage>,
        current_turn_active: bool,
    ) -> Option<ClawMessage> {
        while let Ok(msg) = rx.try_recv() {
            if Self::is_urgent_msg(&msg) {
                urgent_backlog.push_back(msg);
            } else {
                backlog.push_back(msg);
            }
        }

        if let Some(m) = urgent_backlog.pop_front() {
            return Some(m);
        }
        if !current_turn_active
            && let Some(m) = backlog.pop_front()
        {
            return Some(m);
        }

        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if Self::is_urgent_msg(&msg) {
                        return Some(msg);
                    } else if current_turn_active {
                        backlog.push_back(msg);
                    } else {
                        return Some(msg);
                    }
                }
                Err(_) => return None,
            }
        }
    }

    pub async fn run(mut self, env: Arc<dyn ClawEnvironment>) {
        let mut is_initialized = false;
        let mut current_dir = String::from("/");
        let mut current_turn: Option<tokio::task::JoinHandle<()>> = None;
        let mut backlog: VecDeque<ClawMessage> = VecDeque::new();
        let mut urgent_backlog: VecDeque<ClawMessage> = VecDeque::new();

        let workspace = crate::registry::workspace::global_workspace().await;

        let mut task_interval = tokio::time::interval(std::time::Duration::from_secs(1));

        loop {
            tokio::select! {
                msg_opt = Self::get_next_msg(&self.rx, &mut urgent_backlog, &mut backlog, current_turn.is_some()) => {
                    let msg = match msg_opt {
                        Some(m) => m,
                        None => break,
                    };

                    self.ensure_initialized(&msg, &mut is_initialized).await;
                    let session_ctx = self.session_context.clone();

                    match msg {
                        ClawMessage::SettingsInvalidated => {
                            self.handle_settings_invalidated().await;
                        }
                        ClawMessage::Abort => {
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

                            let mut state_lock = self.state.lock().await;
                            state_lock.status = crate::engine::AgentStatus::Waiting;

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

                            let req = crate::engine::TransitionRequest {
                                target_state: crate::engine::AgentStatus::Waiting,
                                source: crate::engine::TriggerSource::System,
                            };
                            self.handle_transition(req, &mut current_turn, &mut urgent_backlog).await;

                            if let Some(next_msg) = urgent_backlog.pop_front() {
                                let _ = self.tx_self.send(next_msg).await;
                            } else if let Some(next_msg) = backlog.pop_front() {
                                let _ = self.tx_self.send(next_msg).await;
                            }
                        }
                        ClawMessage::WakeSummaryComplete { result } => {
                            let mut state_lock = self.state.lock().await;
                            state_lock.status = crate::engine::AgentStatus::Waiting;

                            match result {
                                Ok(summary) => {
                                    state_lock.context_quality = crate::engine::ContextQuality::Full;
                                    state_lock.history.push(rig::message::Message::user(format!("[WHILE_YOU_SLEPT]\n{}", summary)));
                                }
                                Err(e) => {
                                    log::warn!("Dreamer fallback triggered: {}", e);
                                    state_lock.context_quality = crate::engine::ContextQuality::Degraded;
                                }
                            }

                            let current_quality = state_lock.context_quality.clone();
                            let agent_name = self.name.clone();
                            drop(state_lock);

                            let character_id = self.character_id.clone();
                            let _ = self.tx_ui.send(crate::engine::ClawEngineEvent::SessionStateChanged {
                                agent_name,
                                character_id,
                                status: crate::engine::AgentStatus::Waiting,
                            }).await;

                            crate::registry::workspace::global_workspace().await.set_pane_quality(self.pane_id.clone(), Some(current_quality)).await;

                            let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
                        }
                        ClawMessage::Transition(req) => {
                            self.handle_transition(req, &mut current_turn, &mut urgent_backlog).await;
                        }
                        ClawMessage::WatchdogTimeout { task_id, state } => {
                            let state_lock = self.state.lock().await;
                            if state_lock.status == state {
                                drop(state_lock);
                                log::warn!("Watchdog expired for task {:?}. Forcing Faulted state.", task_id);
                                self.handle_transition(
                                    crate::engine::TransitionRequest {
                                        target_state: crate::engine::AgentStatus::Faulted { reason: "Watchdog Timeout".to_string() },
                                        source: crate::engine::TriggerSource::System,
                                    },
                                    &mut current_turn,
                                    &mut urgent_backlog,
                                ).await;
                            }
                        }
                        ClawMessage::SleepToggle(sleep) => {
                            let target_state = if sleep {
                                crate::engine::AgentStatus::Sleep
                            } else {
                                crate::engine::AgentStatus::Waiting
                            };
                            let req = crate::engine::TransitionRequest {
                                target_state,
                                source: crate::engine::TriggerSource::User,
                            };
                            self.handle_transition(req, &mut current_turn, &mut urgent_backlog).await;
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

                            self.clear_ui_history().await;
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

                            self.clear_ui_history().await;
                            let _ = self.tx_ui.send(ClawEngineEvent::Evicted).await;
                            workspace.unregister_pane(self.pane_id.clone()).await;
                        }
                        ClawMessage::ResumeSession { session_id } => {
                            self.handle_resume_session(session_id, &current_dir, &mut is_initialized).await;
                        }
                        ClawMessage::Initialize => {
                            self.handle_initialize(&current_dir).await;
                        }
                        ClawMessage::Reload => {
                            info!("Reloading Claw Session state...");
                            let new_ctx = load_session_context(&self.character_id);
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
                            let character_id_for_db = self.character_id.clone();
                            let character_display_name_for_db = self.name.clone();
                            tokio::spawn(async move {
                                let db_guard = db_for_persistence.lock().await;
                                if let Some(db) = &*db_guard {
                                    let store = boxxy_db::store::Store::new(db.pool());
                                    let _ = store
                                        .upsert_session_state(
                                            &session_id_for_db,
                                            &agent_name_for_db,
                                            &character_id_for_db,
                                            &character_display_name_for_db,
                                            "",
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
                            self.handle_command_finished(
                                exit_code,
                                snapshot,
                                cwd,
                                &session_ctx,
                                &env,
                                &mut current_dir,
                                &mut current_turn,
                                &mut backlog,
                            )
                            .await;
                        }
                        ClawMessage::RequestLazyDiagnosis => {
                            self.handle_request_lazy_diagnosis(
                                &session_ctx,
                                &env,
                                &mut current_turn,
                                &mut backlog,
                            )
                            .await;
                        }
                        ClawMessage::ClawQuery {
                            query,
                            snapshot,
                            cwd,
                            image_attachments,
                        } => {
                            current_dir = cwd.clone();
                            self.send_ui(ClawEngineEvent::UserMessage {
                                content: query.clone(),
                            })
                            .await;

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
                                env.clone(),
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
                        ClawMessage::BackgroundCommandReply { approved } => {
                            let mut state_lock = self.state.lock().await;
                            if let Some(reply) = state_lock.pending_background_reply.take() {
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
                            self.send_ui(ClawEngineEvent::UserMessage {
                                content: message.clone(),
                            })
                            .await;

                            debug!(
                                "Pane {} ({}): User reply: {message}. Checking for pending tools.",
                                self.pane_id, self.name
                            );

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
                            let fulfilled = state_lock.drain_pending_with_interruption(&message);

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
                                            character_id: self.character_id.clone(),
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
                                    env.clone(),
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
                            request_id,
                        } => {
                            if current_turn.is_some() {
                                debug!(
                                    "Pane {} ({}): Agent is busy. Queueing delegated task from {}.",
                                    self.pane_id, self.name, source_agent_name
                                );
                                backlog.push_back(ClawMessage::DelegatedTask {
                                    source_agent_name,
                                    prompt,
                                    request_id,
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
                                &self.session_context,
                                current_dir.clone(),
                                false,
                                env.clone(),
                                self.db.clone(),
                                self.state.clone(),
                                self.tx_ui.clone(),
                                self.tx_self.clone(),
                                Some(request_id),
                                vec![],
                            ));
                        }
                        ClawMessage::CancelPending => {
                            let mut state_lock = self.state.lock().await;
                            state_lock.drain_pending_cancel();
                            drop(state_lock);

                            debug!("Pane {}: User cancelled pending proposals.", self.pane_id);
                            let character_id = self.character_id.clone();
                            let _ = self
                                .tx_ui
                                .send(ClawEngineEvent::ProposalResolved {
                                    agent_name: self.name.clone(),
                                    character_id,
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
                            self.clear_ui_history().await;
                        }
                        ClawMessage::CancelTask { task_id } => {
                            let mut state_lock = self.state.lock().await;
                            state_lock.pending_tasks.retain(|t| t.id != task_id);
                            let agent_name = state_lock.agent_name.clone();
                            let character_id = state_lock.character_id.clone();
                            let tasks = state_lock.pending_tasks.clone();
                            drop(state_lock);

                            let _ = self.tx_ui.send(ClawEngineEvent::TaskStatusChanged {
                                agent_name,
                                character_id,
                                tasks: tasks.clone(),
                            }).await;

                            workspace.update_pane_tasks(self.pane_id.clone(), tasks).await;
                        }
                        ClawMessage::SubscriptionEvent { event } => {
                            let state_lock = self.state.lock().await;

                            if let crate::engine::ClawEvent::Custom { name, .. } = &event
                                && name == "request_sleep"
                                && state_lock.status != crate::engine::AgentStatus::Sleep
                            {
                                drop(state_lock);
                                let req = crate::engine::TransitionRequest {
                                    target_state: crate::engine::AgentStatus::Sleep,
                                    source: crate::engine::TriggerSource::Swarm { trace_id: vec![] },
                                };
                                self.handle_transition(req, &mut current_turn, &mut urgent_backlog).await;
                                continue;
                            }

                            if state_lock.status == crate::engine::AgentStatus::Sleep {
                                drop(state_lock);

                                let req = crate::engine::TransitionRequest {
                                    target_state: crate::engine::AgentStatus::Waiting,
                                    source: crate::engine::TriggerSource::System,
                                };
                                self.handle_transition(req, &mut current_turn, &mut urgent_backlog).await;

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
                        ClawMessage::ScrollbackReply { request_id, content } => {
                            let mut state = self.state.lock().await;
                            if let Some(tx) = state.pending_scrollbacks.remove(&request_id) {
                                let _ = tx.send(content);
                            }
                        }
                        ClawMessage::TaskCompletedEvent { task_id, result } => {
                            let mut state_lock = self.state.lock().await;

                            if let Some(tx) = state_lock.pending_delegations.remove(&task_id) {
                                let _ = tx.send(result.clone());
                            }

                            state_lock.awaiting_tasks.retain(|id| id != &task_id);

                            for task in &mut state_lock.pending_tasks {
                                if task.id == task_id {
                                    task.status = TaskStatus::Completed;
                                }
                            }

                            if state_lock.status == crate::engine::AgentStatus::Sleep && state_lock.awaiting_tasks.is_empty() {
                                drop(state_lock);

                                let req = crate::engine::TransitionRequest {
                                    target_state: crate::engine::AgentStatus::Waiting,
                                    source: crate::engine::TriggerSource::System,
                                };
                                self.handle_transition(req, &mut current_turn, &mut urgent_backlog).await;

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
                }
                _ = task_interval.tick() => {
                    let now = chrono::Utc::now();
                    let mut state_lock = self.state.lock().await;
                    let mut completed_indices = Vec::new();
                    let mut completed_payload: Option<String> = None;

                    for (i, task) in state_lock.pending_tasks.iter_mut().enumerate() {
                        if task.due_at <= now && task.status == TaskStatus::Pending {
                            if completed_payload.is_none() && task.task_type == TaskType::Notification {
                                completed_payload = Some(task.payload.clone());
                            }
                            match task.task_type {
                                TaskType::Notification => {
                                    let text = task.payload.clone();
                                    let tx_ui = self.tx_ui.clone();
                                    let db = self.db.clone();
                                    let session_id = self.session_id.clone();
                                    let pane_id = self.pane_id.clone();
                                    tokio::spawn(async move {
                                        let event = ClawEngineEvent::SystemMessage { text };
                                        persist_visual_event(db, session_id, pane_id, &event);
                                        let _ = tx_ui.send(event).await;
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
                        state_lock.pending_tasks.retain(|t| t.status == TaskStatus::Pending);
                        let agent_name = state_lock.agent_name.clone();
                        let character_id = state_lock.character_id.clone();
                        let agent_name_for_event = agent_name.clone();
                        let tasks = state_lock.pending_tasks.clone();
                        drop(state_lock);

                        let _ = self.tx_ui.send(ClawEngineEvent::TaskStatusChanged {
                            agent_name,
                            character_id: character_id.clone(),
                            tasks: tasks.clone(),
                        }).await;

                        let _ = self.tx_ui.send(ClawEngineEvent::TaskCompleted {
                            agent_name: agent_name_for_event,
                            task_id: uuid::Uuid::nil(),
                            character_id,
                            message: completed_payload,
                        }).await;

                        workspace.update_pane_tasks(self.pane_id.clone(), tasks).await;
                    }
                }
            }
        }

        workspace.unregister_pane(self.pane_id).await;
    }
}
