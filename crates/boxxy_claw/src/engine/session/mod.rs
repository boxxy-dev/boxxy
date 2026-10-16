use crate::engine::{ClawEngineEvent, ClawEnvironment, ClawMessage, persist_visual_event};
use boxxy_db::Db;
use rig::message::Message;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

mod approvals;
mod diagnostics;
mod lifecycle;
pub mod run;

pub struct SessionState {
    pub session_id: String,
    pub pane_id: String,
    pub agent_name: String,
    pub character_id: String,
    pub character_display_name: String,
    pub pinned: bool,
    pub web_search_enabled: bool,
    pub total_tokens: u64,
    pub context_tokens: u64,
    pub total_cost_usd: f64,
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
    /// Index in `history` up to which messages have already been flushed to `flush_history`.
    pub flushed_watermark: usize,
    pub pending_lazy_diagnosis: Option<(String, String, String)>,
    pub persistent_agent: Option<crate::engine::agent::ClawAgent>,
    pub current_claw_model: Option<boxxy_model_selection::ModelProvider>,
    pub current_memory_model: Option<boxxy_model_selection::ModelProvider>,
    pub current_search_label: String,
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
    pub context_tokens: u64,
    pub total_cost_usd: f64,
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
            context_tokens: 0,
            total_cost_usd: 0.0,
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
                web_search_enabled: settings.enable_web_search && settings.web_search_on_by_default,
                total_tokens: 0,
                context_tokens: 0,
                total_cost_usd: 0.0,
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
                flushed_watermark: 0,
                pending_lazy_diagnosis: None,
                persistent_agent: None,
                current_claw_model: settings.claw_model.clone(),
                current_memory_model: settings.memory_model.clone(),
                current_search_label: crate::engine::agent_config::resolve_active_search_label(
                    &settings,
                    settings.enable_web_search && settings.web_search_on_by_default,
                )
                .to_string(),
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
        if !current_turn_active && let Some(m) = backlog.pop_front() {
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
}
