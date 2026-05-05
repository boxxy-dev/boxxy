use crate::engine::{
    ClawEngineEvent, ClawEnvironment, ClawMessage, PersistentClawRow, TaskStatus, TaskType,
    context::load_session_context, persist_visual_event, turn::spawn_turn,
};
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
    pub character_id: String,
    pub character_display_name: String,
    pub pinned: bool,
    pub web_search_enabled: bool,
    pub total_tokens: u64,
    pub tracked_pids: Arc<tokio::sync::RwLock<std::collections::HashSet<u32>>>,
    pub api_keys: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    pub ollama_url: Arc<tokio::sync::RwLock<String>>,
    pub pending_terminal_reply: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
    /// Set when the agent emits a legacy `InjectCommand` event (from a
    /// parsed text-response command, not the `ProposeTerminalCommand`
    /// tool). Used at `CommandFinished` time to distinguish agent-
    /// originated commands from user-typed ones — the former should
    /// auto-diagnose on failure, the latter should only surface the
    /// passive "Lazy Error Indicator" pill.
    /// Cleared on consumption (next `CommandFinished`) or explicit
    /// reject (`CancelPending`).
    pub pending_inject_command: bool,
    pub pending_file_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_file_delete_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_kill_process_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_get_clipboard_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_set_clipboard_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_scrollbacks:
        std::collections::HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<String>>,
    pub pending_delegations:
        std::collections::HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<String>>,
    pub history: Vec<Message>,
    pub pending_lazy_diagnosis: Option<(String, String, String)>,
    pub persistent_agent: Option<crate::engine::agent::ClawAgent>,
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

impl SessionState {
    /// Reject all waiting tool channels with a user-interruption payload.
    /// Returns `true` if any channel was pending (so the caller knows whether
    /// to skip spawning a new turn).
    pub fn drain_pending_with_interruption(&mut self, message: &str) -> bool {
        let mut fulfilled = false;

        if let Some(reply) = self.pending_terminal_reply.take() {
            let _ = reply.send(Err(format!("[USER_INTERRUPTION]: {message}")));
            fulfilled = true;
        }
        if let Some(reply) = self.pending_file_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }
        if let Some(reply) = self.pending_file_delete_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }
        if let Some(reply) = self.pending_kill_process_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }
        if let Some(reply) = self.pending_get_clipboard_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }
        if let Some(reply) = self.pending_set_clipboard_reply.take() {
            let _ = reply.send(false);
            fulfilled = true;
        }

        if fulfilled {
            self.history
                .push(rig::message::Message::user(message.to_string()));
        }

        fulfilled
    }

    /// Reject all waiting tool channels without recording history (used by CancelPending).
    pub fn drain_pending_cancel(&mut self) {
        self.pending_inject_command = false;
        if let Some(reply) = self.pending_terminal_reply.take() {
            let _ = reply.send(Err("[USER_EXPLICIT_REJECT]".to_string()));
        }
        if let Some(reply) = self.pending_file_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_file_delete_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_kill_process_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_get_clipboard_reply.take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = self.pending_set_clipboard_reply.take() {
            let _ = reply.send(false);
        }
    }
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

        // Overwritten immediately by with_identity() in the daemon before the session is spawned.
        let name = String::new();
        let session_id = uuid::Uuid::new_v4().to_string();

        // We defer loading session context and skills until the first use
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
                pending_get_clipboard_reply: None,
                pending_set_clipboard_reply: None,
                pending_scrollbacks: std::collections::HashMap::new(),
                pending_delegations: std::collections::HashMap::new(),
                history: Vec::new(),
                pending_lazy_diagnosis: None,
                persistent_agent: None,
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

    /// Overrides the session's auto-generated identity with values from the
    /// daemon's persistent agent registry, so an agent's identity survives
    /// across UI restarts. Must be called before `start()`.
    pub fn with_identity(
        mut self,
        character_id: String,
        character_display_name: String,
        session_id: String,
    ) -> Self {
        self.name = character_display_name.clone();
        self.character_id = character_id.clone();
        self.session_id = session_id.clone();
        // Synchronise the inner state. `try_lock` is safe here: the session
        // hasn't been spawned yet, so there's no other owner of the lock.
        if let Ok(mut state) = self.state.try_lock() {
            state.agent_name = character_display_name.clone();
            state.character_id = character_id;
            state.character_display_name = character_display_name;
            state.session_id = session_id;
        }
        self
    }

    pub fn new_headless(
        parent_pane_id: String,
        parent_session_id: uuid::Uuid,
        parent_character_name: &str,
        profile: String,
    ) -> (Self, async_channel::Sender<ClawMessage>) {
        let pane_id = format!("{}-headless-{}", parent_pane_id, uuid::Uuid::new_v4());
        let (mut session, tx, _) = Self::new(pane_id.clone());

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
            "You are a specialized transient background worker. {}\n",
            profile
        );

        (session, tx)
    }

    pub fn start(self, env: Arc<dyn ClawEnvironment>) {
        tokio::spawn(async move {
            self.run(env).await;
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

    async fn clear_ui_history(&self) {
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::RestoreHistory(vec![]))
            .await;
    }

    async fn handle_transition(
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
                log::warn!("{}", e);
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

        // Handle entering sleep
        if req.target_state == crate::engine::AgentStatus::Sleep {
            state_lock.sleep_timestamp = Some(chrono::Utc::now().timestamp_millis());
        }

        state_lock.status = req.target_state.clone();
        let agent_name = self.name.clone();
        let character_id = self.character_id.clone();
        drop(state_lock);

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

    fn is_urgent_msg(msg: &ClawMessage) -> bool {
        if let ClawMessage::Transition(req) = msg {
            if req.source == crate::engine::TriggerSource::User {
                return true;
            }
        }
        if let ClawMessage::SubscriptionEvent {
            event: crate::engine::ClawEvent::Custom { name, .. },
        } = msg
        {
            if name == "request_sleep" {
                return true;
            }
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
                | ClawMessage::GetClipboardReply { .. }
                | ClawMessage::SetClipboardReply { .. }
                | ClawMessage::ScrollbackReply { .. }
                | ClawMessage::TaskCompletedEvent { .. }
                | ClawMessage::CommandFinished { .. }
        )
    }

    async fn get_next_msg(
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
        if !current_turn_active {
            if let Some(m) = backlog.pop_front() {
                return Some(m);
            }
        }

        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if Self::is_urgent_msg(&msg) {
                        return Some(msg);
                    } else {
                        if current_turn_active {
                            backlog.push_back(msg);
                        } else {
                            return Some(msg);
                        }
                    }
                }
                Err(_) => return None,
            }
        }
    }

    pub async fn run(mut self, env: Arc<dyn ClawEnvironment>) {
        // LAZY LOADING: We don't initialize the DB or load skills until we receive a message.
        // This ensures BoxxyClaw doesn't consume memory/CPU if the user doesn't use the AI.

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

                    let needs_initialization = match &msg {
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

                    if !is_initialized && needs_initialization {
                        info!(
                            "Initializing Claw Session for pane {} ({}) upon first request...",
                            self.pane_id, self.name
                        );

                        // If this is an explicit Initialize message, we'll handle the identity
                        // announcement in the match block below to avoid double announcements.
                        if !matches!(&msg, ClawMessage::Initialize) {
                            let (web_search_enabled, char_id) = {
                                let state_lock = self.state.lock().await;
                                (state_lock.web_search_enabled, state_lock.character_id.clone())
                            };
                            let _ = self
                                .tx_ui
                                .send(ClawEngineEvent::Identity {
                                    agent_name: self.name.clone(),
                                    character_id: char_id,
                                    pinned: self.pinned,
                                    web_search_enabled,
                                    total_tokens: self.total_tokens,
                                })
                                .await;
                        }

                        let session_context = load_session_context(&self.character_id);
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
                        ClawMessage::SettingsInvalidated => {
                            info!("[{}] Settings changed. Next turn will trigger a hot-swap.", self.name);

                            let mut state_lock = self.state.lock().await;

                            let model_label = |m: &Option<_>| -> String {
                                m.as_ref()
                                    .map(|p: &boxxy_model_selection::ModelProvider| p.format_label())
                                    .unwrap_or_else(|| "Default".to_string())
                            };

                            let (old_claw, old_memory) = (
                                model_label(&state_lock.persistent_agent.as_ref().and_then(|a| a.config.model.clone())),
                                model_label(&state_lock.current_memory_model)
                            );

                            // Cross-process settings sync: the UI writes
                            // `settings.json` on save, but the daemon's in-memory
                            // cache doesn't observe disk changes. Re-hydrate
                            // from disk so the new model / toggles take effect.
                            boxxy_preferences::Settings::reload();

                            let after = boxxy_preferences::Settings::load();
                            let (new_claw, new_memory) =
                                (model_label(&after.claw_model), model_label(&after.memory_model));

                            if new_claw == old_claw && new_memory == old_memory {
                                // Non-model change (persistence, theme, ...) — no log entry.
                                drop(state_lock);
                                continue;
                            }

                            // Update the session's memory model tracker
                            state_lock.current_memory_model = after.memory_model.clone();

                            let event = ClawEngineEvent::SystemMessage {
                                text: format!("Claw: {new_claw} . Dreams: {new_memory}"),
                            };
                            crate::engine::persist_visual_event(
                                self.db.clone(),
                                self.session_id.clone(),
                                self.pane_id.clone(),
                                &event
                            );
                            let _ = self.tx_ui.send(event).await;

                            drop(state_lock);
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
                                    .await;                            }
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
                                    state_lock.history.push(rig::message::Message::User {
                                        content: rig::OneOrMany::one(rig::message::UserContent::text(format!("[WHILE_YOU_SLEPT]\n{}", summary))),
                                    });
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

                            // Let the router know this background task is done
                            let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
                        }
                        ClawMessage::Transition(req) => {
                            self.handle_transition(req, &mut current_turn, &mut urgent_backlog).await;
                        }
                        ClawMessage::WatchdogTimeout { task_id, state } => {
                            let state_lock = self.state.lock().await;
                            // Only trigger if we are still in the expected state and there are no new tasks
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

                            self.clear_ui_history().await;
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
                                        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();

                                        // Resolve character: UUID first (stable), then
                                        // name/display_name (legacy), then first available
                                        // (deleted character fallback). When the fallback
                                        // fires, write the new UUID back so it only resolves
                                        // once and the session is consistently owned.
                                        let char_info = registry
                                            .iter()
                                            .find(|c| c.config.id == session.character_id)
                                            .or_else(|| {
                                                session.agent_name.as_deref().and_then(|n| {
                                                    registry.iter().find(|c| {
                                                        c.config.name == n
                                                            || c.config.display_name == n
                                                    })
                                                })
                                            })
                                            .or_else(|| registry.first());

                                        let is_fallback = char_info
                                            .map(|c| c.config.id != session.character_id)
                                            .unwrap_or(false);

                                        if let Some(info) = char_info {
                                            self.name = info.config.display_name.clone();
                                            self.character_id = info.config.id.clone();
                                            if is_fallback {
                                                info!(
                                                    "Character '{}' not found; reassigning session {} to '{}'",
                                                    session.character_id, session_id, info.config.display_name
                                                );
                                                let _ = store
                                                    .update_session_character(
                                                        &session_id,
                                                        &info.config.id,
                                                        &info.config.display_name,
                                                    )
                                                    .await;
                                            }
                                        } else if let Some(n) = session.agent_name.as_deref() {
                                            self.name = n.to_string();
                                        }

                                        self.pinned = session.pinned;
                                        self.total_tokens = session.total_tokens as u64;

                                        let mut state_lock = self.state.lock().await;
                                        state_lock.session_id = session_id.clone();
                                        state_lock.agent_name = self.name.clone();
                                        state_lock.character_id = self.character_id.clone();
                                        state_lock.character_display_name = self.name.clone();
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
                                                character_id: self.character_id.clone(),
                                                pinned: self.pinned,
                                                web_search_enabled,
                                                total_tokens: self.total_tokens,
                                            })
                                            .await;

                                        let _ = self.tx_ui.send(ClawEngineEvent::TaskStatusChanged {
                                            agent_name: agent_name_clone,
                                            character_id: self.character_id.clone(),
                                            tasks: tasks_clone.clone(),
                                        }).await;

                                        workspace.update_pane_tasks(self.pane_id.clone(), tasks_clone).await;

                                        let session_type = if self.pinned { "pinned" } else { "normal" };
                                        boxxy_telemetry::track_session_resume(session_type).await;

                                        let settings = boxxy_preferences::Settings::load();
                                        let claw_model = settings.claw_model.as_ref().map(|m| m.format_label()).unwrap_or_else(|| "Default".to_string());
                                        let memory_model = settings.memory_model.as_ref().map(|m| m.format_label()).unwrap_or_else(|| "Default".to_string());

                                        self.send_ui(ClawEngineEvent::SystemMessage {
                                                text: format!("Claw: {} ● Dreams: {}", claw_model, memory_model),
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

                            // 1. Use the character name assigned at session creation via with_identity().
                            let name = self.name.clone();

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

                            self.clear_ui_history().await;

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
                                    character_id: self.character_id.clone(),
                                    pinned: false,
                                    web_search_enabled,
                                    total_tokens: 0,
                                })
                                .await;

                            let settings = boxxy_preferences::Settings::load();
                            let claw_model = settings.claw_model.as_ref().map(|m| m.format_label()).unwrap_or_else(|| "Default".to_string());
                            let memory_model = settings.memory_model.as_ref().map(|m| m.format_label()).unwrap_or_else(|| "Default".to_string());

                            let event = ClawEngineEvent::SystemMessage {
                                text: format!("Claw: {} ● Dreams: {}", claw_model, memory_model),
                            };
                            crate::engine::persist_visual_event(
                                self.db.clone(),
                                self.session_id.clone(),
                                self.pane_id.clone(),
                                &event
                            );

                            // A brief delay ensures the UI has fully processed the `Identity` event
                            // and created the overlay/sidebar structures before we append the SystemMessage.
                            // Without this, the listview virtualization might race against the history clearing.
                            let tx_ui = self.tx_ui.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                let _ = tx_ui.send(event).await;
                            });
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

                                // Cheap exit-code filter: some tools use
                                // non-zero to signal a normal outcome
                                // (grep with no match, diff with
                                // differences). Diagnosing these would
                                // flood the lazy pill. Keep this list
                                // minimal — we can grow it if it proves
                                // noisy.
                                if exit_code == 1 {
                                    let cmd = snapshot.lines().next().unwrap_or("");
                                    let first_word =
                                        cmd.split_whitespace().next().unwrap_or("");
                                    if matches!(first_word, "grep" | "diff" | "test" | "[") {
                                        continue;
                                    }
                                }

                                let prompt = format!(
                                    "The user's command failed with exit code {exit_code}. Please analyze the terminal snapshot and suggest a fix."
                                );

                                // Sleep mode gate — short-circuit before
                                // we decide anything else. An agent in
                                // Sleep shouldn't pop UI or burn tokens.
                                if state_lock.status != crate::engine::AgentStatus::Waiting {
                                    drop(state_lock);
                                    continue;
                                }

                                // Origin split: if this is a user-typed
                                // command, surface the passive Lazy
                                // Error pill and stash the prompt. Zero
                                // tokens until the user clicks and
                                // `RequestLazyDiagnosis` arrives. If
                                // it's an agent-proposed command
                                // (legacy `InjectCommand` path — the
                                // `ProposeTerminalCommand` tool path
                                // already unblocked above), keep the
                                // current proactive-diagnose behavior so
                                // the accept→error→auto-fix loop works.
                                let agent_originated = state_lock.pending_inject_command;
                                state_lock.pending_inject_command = false;

                                if !agent_originated {
                                    state_lock.pending_lazy_diagnosis =
                                        Some((prompt.clone(), snapshot.clone(), cwd.clone()));
                                    drop(state_lock);
                                    let character_id = self.character_id.clone();
                                    let _ = self
                                        .tx_ui
                                        .send(ClawEngineEvent::LazyErrorIndicator {
                                            visible: true,
                                            agent_name: self.name.clone(),
                                            character_id,
                                        })
                                        .await;
                                    continue;
                                }

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
                                            character_id: self.character_id.clone(),
                                            is_thinking: false,
                                        })
                                        .await;                                }
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
                                    .await;                            }
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
                                        .await;                                }
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

                            // Handle external sleep requests
                            if let crate::engine::ClawEvent::Custom { name, .. } = &event {
                                if name == "request_sleep" && state_lock.status != crate::engine::AgentStatus::Sleep {
                                    drop(state_lock);
                                    let req = crate::engine::TransitionRequest {
                                        target_state: crate::engine::AgentStatus::Sleep,
                                        source: crate::engine::TriggerSource::Swarm { trace_id: vec![] },
                                    };
                                    self.handle_transition(req, &mut current_turn, &mut urgent_backlog).await;
                                    continue;
                                }
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

                            // If this was a direct tool delegation (oneshot), unblock it
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
                    // 1. Process Due Tasks
                    let now = chrono::Utc::now();
                    let mut state_lock = self.state.lock().await;
                    let mut completed_indices = Vec::new();

                    let mut completed_payload: Option<String> = None;

                    for (i, task) in state_lock.pending_tasks.iter_mut().enumerate() {
                        if task.due_at <= now && task.status == TaskStatus::Pending {
                            // Grab the first payload we see for the notification
                            if completed_payload.is_none() && task.task_type == TaskType::Notification {
                                completed_payload = Some(task.payload.clone());
                            }
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

                                        // We no longer emit PushGlobalNotification here.
                                        // Instead, we rely on the TaskCompleted event that gets emitted 
                                        // at the end of this block to trigger the UI notification.
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
                            task_id: uuid::Uuid::nil(), // Placeholder for now as we don't have it easily available here
                            character_id,
                            message: completed_payload,
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

#[cfg(test)]
mod pending_drain_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use tokio::sync::RwLock;

    fn make_state() -> SessionState {
        let (tx, _rx) = async_channel::unbounded();
        SessionState {
            session_id: "test".to_string(),
            pane_id: "test".to_string(),
            agent_name: "test".to_string(),
            character_id: "test".to_string(),
            character_display_name: "test".to_string(),
            pinned: false,
            web_search_enabled: false,
            total_tokens: 0,
            tracked_pids: Arc::new(RwLock::new(HashSet::new())),
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            ollama_url: Arc::new(RwLock::new(String::new())),
            pending_terminal_reply: None,
            pending_inject_command: false,
            pending_file_reply: None,
            pending_file_delete_reply: None,
            pending_kill_process_reply: None,
            pending_get_clipboard_reply: None,
            pending_set_clipboard_reply: None,
            pending_scrollbacks: HashMap::new(),
            pending_delegations: HashMap::new(),
            history: Vec::new(),
            pending_lazy_diagnosis: None,
            persistent_agent: None,
            current_memory_model: None,
            last_tools: None,
            pending_tasks: Vec::new(),
            last_snapshot_hash: None,
            status: crate::engine::AgentStatus::Waiting,
            context_quality: crate::engine::ContextQuality::Full,
            parent_id: None,
            is_headless: false,
            sleep_timestamp: None,
            awaiting_tasks: Vec::new(),
            tx_self: tx,
            mcp_handle: None,
        }
    }

    #[tokio::test]
    async fn drain_with_interruption_nothing_pending_returns_false() {
        let mut state = make_state();
        assert!(!state.drain_pending_with_interruption("hi"));
        assert!(state.history.is_empty());
    }

    #[tokio::test]
    async fn drain_with_interruption_terminal_reply_pushes_history() {
        let mut state = make_state();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        state.pending_terminal_reply = Some(tx);
        assert!(state.drain_pending_with_interruption("cancel"));
        // Channel received the interruption error
        let res = rx.try_recv();
        assert!(res.is_ok());
        assert!(res.unwrap().is_err());
        // History has the user message
        assert_eq!(state.history.len(), 1);
    }

    #[tokio::test]
    async fn drain_cancel_clears_inject_command_flag() {
        let mut state = make_state();
        state.pending_inject_command = true;
        let (tx, _rx) = tokio::sync::oneshot::channel::<bool>();
        state.pending_file_reply = Some(tx);
        state.drain_pending_cancel();
        assert!(!state.pending_inject_command);
        assert!(state.pending_file_reply.is_none());
    }
}
