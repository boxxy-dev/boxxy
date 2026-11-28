use crate::engine::{ClawEngineEvent, ClawEnvironment, ClawMessage, persist_visual_event};
use boxxy_db::Db;
use rig::message::Message;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

mod approvals;
mod diagnostics;
mod lifecycle;
mod resumption;
pub mod roundtable;
pub mod run;

/// Live in-flight turn snapshot shared with concurrent `/btw` side queries without
/// acquiring the async `SessionState` mutex on every streamed chunk.
#[derive(Debug, Clone, Default)]
pub struct InFlightSnapshot {
    /// Exact committed `Arc<Message>` transcript passed to the latest model call of an
    /// active turn (`[...prepared_history, step_prompt]`), preserving all provider
    /// reasoning/tool signatures and warm KV prompt-cache prefixes.
    pub messages: Option<Vec<Arc<Message>>>,
    /// Index of the current turn's initial user prompt inside `messages` (`prepared_history.len()`),
    /// allowing completed mid-turn `[Assistant(ToolCall), User(ToolResult)]` pairs at
    /// `messages[turn_start_idx + 1..]` to be salvaged if the turn is aborted or rejected.
    pub turn_start_idx: usize,
    /// Human-readable label of the currently executing step (e.g. `"Running cargo test…"`).
    pub active_status: Option<String>,
    /// Cumulative tokens billed across already-completed model steps of the active turn.
    pub settled_tokens: u64,
    /// Context window occupancy reported by the latest completed model step of the active turn.
    pub settled_context_tokens: u64,
    /// Cumulative USD cost billed across already-completed model steps of the active turn.
    pub settled_cost_usd: f64,
}

impl InFlightSnapshot {
    pub fn clear(&mut self) {
        self.messages = None;
        self.turn_start_idx = 0;
        self.active_status = None;
        self.settled_tokens = 0;
        self.settled_context_tokens = 0;
        self.settled_cost_usd = 0.0;
    }
}

pub struct SessionState {
    pub session_id: String,
    pub pane_id: String,
    pub agent_name: String,
    pub character_id: String,
    pub character_display_name: String,
    pub incognito: bool,
    pub current_dir: String,
    pub pinned: bool,
    pub web_search_enabled: bool,
    pub vibe_enabled: bool,
    pub total_tokens: u64,
    pub context_tokens: u64,
    pub total_cost_usd: f64,
    pub tracked_pids: Arc<tokio::sync::RwLock<std::collections::HashSet<u32>>>,
    pub api_keys: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    pub ollama_url: Arc<tokio::sync::RwLock<String>>,
    pub pending_terminal_reply: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
    /// True once the current `terminal_exec` proposal has been accepted (or Vibe auto-approved)
    /// and is actively executing in the terminal, so follow-up prompts enqueue instead of steering.
    pub terminal_command_accepted: bool,
    pub pending_inject_command: bool,
    /// Unaccepted `InjectCommand` (` ```bash ` block) awaiting user click on "Accept & Run" or "Reject",
    /// recorded in `history` if explicitly rejected via `CancelPending` or superseded by a new user prompt.
    pub pending_unaccepted_inject_cmd: Option<String>,
    pub pending_file_reply:
        Option<tokio::sync::oneshot::Sender<boxxy_core_toolbox::ApprovalOutcome>>,
    pub pending_file_delete_reply:
        Option<tokio::sync::oneshot::Sender<boxxy_core_toolbox::ApprovalOutcome>>,
    pub pending_kill_process_reply:
        Option<tokio::sync::oneshot::Sender<boxxy_core_toolbox::ApprovalOutcome>>,
    pub pending_background_reply:
        Option<tokio::sync::oneshot::Sender<boxxy_core_toolbox::ApprovalOutcome>>,
    pub pending_get_clipboard_reply:
        Option<tokio::sync::oneshot::Sender<boxxy_core_toolbox::ApprovalOutcome>>,
    pub pending_set_clipboard_reply:
        Option<tokio::sync::oneshot::Sender<boxxy_core_toolbox::ApprovalOutcome>>,
    /// Summary of the currently pending proposal (e.g. file path + content preview) so
    /// zero-token rejection still records what was proposed and rejected in `history`.
    pub pending_proposal_summary: Option<String>,
    pub pending_scrollbacks:
        std::collections::HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<String>>,
    pub pending_spawns:
        std::collections::HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<(String, String)>>,
    pub pending_delegations:
        std::collections::HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<String>>,
    pub history: Vec<Message>,
    /// Index in `history` up to which messages have already been flushed to `flush_history`.
    pub flushed_watermark: usize,
    /// Prevents concurrent turns from spawning duplicate background session-title LLM calls.
    pub title_requested: bool,
    pub pending_lazy_diagnosis: Option<(String, String, String)>,
    pub persistent_agent: Option<crate::engine::agent::ClawAgent>,
    pub current_claw_model: Option<boxxy_model_selection::ModelProvider>,
    pub current_context_window: Option<u64>,
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
    /// Shell commands `(command_line, exit_code)` observed while in `AgentStatus::Sleep`,
    /// drained on Hard Wake (`summarize_wake_delta`) so `[WHILE_YOU_SLEPT]` reflects real delta.
    pub sleep_commands: Vec<(String, i32)>,
    pub awaiting_tasks: Vec<uuid::Uuid>,
    pub tx_self: async_channel::Sender<ClawMessage>,
    pub mcp_handle: Option<std::sync::Arc<boxxy_mcp::manager::McpClientManager>>,
    pub active_speaker: Option<roundtable::SpeakerIdentity>,
    pub roundtable: roundtable::RoundtableState,
    pub next_roundtable_turn: Option<(roundtable::SpeakerIdentity, String)>,
    /// Tracks the latest known FNV-1a content hash of files read or written in this session
    /// so `file_write` can refuse to clobber manual user edits made externally on disk.
    pub read_file_hashes: std::collections::HashMap<String, u64>,
    /// Pre-edit file snapshots `(path, previous_content)` captured during the current turn.
    pub current_turn_snapshots: Vec<(String, Option<String>)>,
    /// Pre-edit file snapshots from the most recently completed turn that modified files,
    /// used when `current_turn_snapshots` is empty at the time the user invokes `/undo`.
    pub last_turn_snapshots: Vec<(String, Option<String>)>,
    /// Chronological reasoning, tool, skill, and diff items recorded during the active turn.
    pub current_turn_trace: Vec<boxxy_claw_protocol::TurnTraceItem>,
    /// `(caption, path)` pairs produced by `generate_image` during the active turn,
    /// used as a safety net so generated images always render in the overlay even if omitted in prose.
    pub generated_images_this_turn: Vec<(String, String)>,
    /// Base64-encoded user images attached or `@file`-referenced in the active turn,
    /// allowing `generate_image` to edit pasted/attached images without a disk path.
    pub current_turn_user_images: Vec<String>,
    /// Lock-light snapshot of the currently running turn's committed messages and active step
    /// so `/btw` side queries observe live mid-turn tool results and warm KV cache prefixes.
    pub in_flight_snapshot: Arc<std::sync::Mutex<InFlightSnapshot>>,
    /// Set to `true` when the latest completed turn ended with an error or a blocking command
    /// proposal so queued follow-up prompts pause instead of auto-executing over broken/unapproved state.
    pub last_turn_paused_queue: bool,
}

impl SessionState {
    /// Returns `(agent_name, character_id)` of the character currently speaking in this turn
    /// (falling back to the pane's canonical Host character display name).
    #[must_use]
    pub fn effective_speaker(&self) -> (String, String) {
        self.active_speaker.as_ref().map_or_else(
            || {
                if !self.character_id.is_empty() && !self.incognito {
                    let cached = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
                    let host = roundtable::resolve_host_speaker(
                        &self.character_id,
                        &self.agent_name,
                        &cached,
                    );
                    (host.display_name, self.character_id.clone())
                } else {
                    (self.agent_name.clone(), self.character_id.clone())
                }
            },
            |spk| (spk.display_name.clone(), spk.character_id.clone()),
        )
    }

    /// Atomically takes and clears `in_flight_snapshot`, salvaging both any already-billed
    /// per-step token/cost counters and any settled mid-turn `(Assistant(ToolCall), User(ToolResult))`
    /// pairs into `self` so aborted, rejected, or handed-off turns never lose completed work or spend.
    ///
    /// Returns `(salvaged_message_count, salvaged_any_usage)`.
    pub fn salvage_in_flight_tool_steps(&mut self) -> (usize, bool) {
        let (messages_opt, turn_start_idx, settled_tokens, settled_ctx, settled_cost) = {
            let mut snap = self
                .in_flight_snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let msgs = snap.messages.take();
            let idx = snap.turn_start_idx;
            let tok = snap.settled_tokens;
            let ctx = snap.settled_context_tokens;
            let cost = snap.settled_cost_usd;
            snap.clear();
            drop(snap);
            (msgs, idx, tok, ctx, cost)
        };

        let salvaged_usage = settled_tokens > 0 || settled_ctx > 0 || settled_cost > 0.0;
        if salvaged_usage {
            self.total_tokens = self.total_tokens.saturating_add(settled_tokens);
            if settled_ctx > 0 {
                self.context_tokens = settled_ctx;
            }
            if settled_cost.is_finite() && settled_cost > 0.0 {
                self.total_cost_usd += settled_cost;
            }
        }

        let Some(messages) = messages_opt else {
            return (0, salvaged_usage);
        };
        let Some(tail) = messages.get(turn_start_idx.saturating_add(1)..) else {
            return (0, salvaged_usage);
        };
        if tail.len() < 2 {
            return (0, salvaged_usage);
        }

        // Only salvage onto an uncommitted turn whose trailing message in `self.history`
        // is still the initial user prompt pushed by `spawn_turn` (not a tool result or assistant reply).
        let ends_at_uncommitted_prompt = matches!(
            self.history.last(),
            Some(Message::User { content }) if content.iter().all(|c| c.as_tool_result().is_none())
        );
        if !ends_at_uncommitted_prompt {
            return (0, salvaged_usage);
        }

        let (spk_name, spk_cid) = self.effective_speaker();
        let mut salvaged = 0usize;
        let mut pair_idx = 0usize;
        while pair_idx + 1 < tail.len() {
            let asst_candidate = (*tail[pair_idx]).clone();
            let user_candidate = (*tail[pair_idx + 1]).clone();
            match (asst_candidate, user_candidate) {
                (mut asst_msg @ Message::Assistant(_), user_msg @ Message::User { .. }) => {
                    let has_tool_call = matches!(
                        &asst_msg,
                        Message::Assistant(turn) if turn.content.iter().any(|c| c.as_tool_call().is_some())
                    );
                    let has_tool_result = matches!(
                        &user_msg,
                        Message::User { content } if content.iter().any(|c| c.as_tool_result().is_some())
                    );
                    if !has_tool_call || !has_tool_result {
                        break;
                    }
                    if let Message::Assistant(ref mut turn) = asst_msg {
                        for item in &mut turn.content {
                            if let rig::message::AssistantContent::Text(t) = item
                                && !t.text.trim().is_empty()
                                && crate::engine::turn::history::parse_speaker_tag(&t.text)
                                    .is_none()
                            {
                                t.text = crate::engine::turn::history::tag_assistant_text(
                                    &spk_cid, &spk_name, &t.text,
                                );
                            }
                        }
                    }
                    self.history.push(asst_msg);
                    self.history.push(user_msg);
                    salvaged += 2;
                    pair_idx += 2;
                }
                _ => break,
            }
        }
        (salvaged, salvaged_usage)
    }

    /// Resets transient per-session context, snapshot/file hashes, `/undo` buffers,
    /// lazy-error stashes, and roundtable state across session boundaries (`Initialize`,
    /// `ResumeSession`, `Deactivate`, `Evict`).
    pub fn reset_ephemeral_session_context(&mut self) {
        self.last_snapshot_hash = None;
        self.read_file_hashes.clear();
        self.current_turn_snapshots.clear();
        self.last_turn_snapshots.clear();
        self.current_turn_trace.clear();
        self.generated_images_this_turn.clear();
        self.current_turn_user_images.clear();
        self.pending_lazy_diagnosis = None;
        self.pending_proposal_summary = None;
        self.terminal_command_accepted = false;
        self.pending_inject_command = false;
        self.pending_unaccepted_inject_cmd = None;
        self.active_speaker = None;
        self.next_roundtable_turn = None;
        self.roundtable.clear();
        self.roundtable.drama_mode = false;
        self.sleep_timestamp = None;
        self.sleep_commands.clear();
        self.awaiting_tasks.clear();
        self.last_turn_paused_queue = false;
        self.in_flight_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Returns true if an in-turn oneshot approval channel is actively waiting on user consent.
    #[must_use]
    pub const fn has_in_turn_approval(&self) -> bool {
        (self.pending_terminal_reply.is_some() && !self.terminal_command_accepted)
            || self.pending_file_reply.is_some()
            || self.pending_file_delete_reply.is_some()
            || self.pending_kill_process_reply.is_some()
            || self.pending_background_reply.is_some()
            || self.pending_get_clipboard_reply.is_some()
            || self.pending_set_clipboard_reply.is_some()
    }

    /// Returns true if any tool approval channel or command injection is currently waiting for user action.
    #[must_use]
    pub const fn has_pending_approval(&self) -> bool {
        self.pending_inject_command || self.has_in_turn_approval()
    }
}

pub struct ClawSession {
    pub pane_id: String,
    pub session_id: String,
    pub name: String,
    pub character_id: String,
    pub incognito: bool,
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
    #[must_use]
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
            incognito: false,
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
                pane_id,
                agent_name: name,
                character_id: String::new(),
                character_display_name: String::new(),
                incognito: false,
                current_dir: String::new(),
                pinned: false,
                web_search_enabled: settings.enable_web_search && settings.web_search_on_by_default,
                vibe_enabled: false,
                total_tokens: 0,
                context_tokens: 0,
                total_cost_usd: 0.0,
                tracked_pids: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
                api_keys: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
                ollama_url: Arc::new(tokio::sync::RwLock::new(String::new())),
                pending_terminal_reply: None,
                terminal_command_accepted: false,
                pending_inject_command: false,
                pending_unaccepted_inject_cmd: None,
                pending_file_reply: None,
                pending_file_delete_reply: None,
                pending_kill_process_reply: None,
                pending_background_reply: None,
                pending_get_clipboard_reply: None,
                pending_set_clipboard_reply: None,
                pending_proposal_summary: None,
                pending_scrollbacks: std::collections::HashMap::new(),
                pending_spawns: std::collections::HashMap::new(),
                pending_delegations: std::collections::HashMap::new(),
                history: Vec::new(),
                flushed_watermark: 0,
                title_requested: false,
                pending_lazy_diagnosis: None,
                persistent_agent: None,
                current_claw_model: settings.claw_model.clone(),
                current_context_window: boxxy_ai_core::model_context_window_with_endpoints(
                    &settings.claw_model,
                    &settings.openai_compatible_endpoints,
                ),
                current_memory_model: settings.effective_memory_model(),
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
                sleep_commands: Vec::new(),
                awaiting_tasks: Vec::new(),
                tx_self: tx.clone(),
                mcp_handle: None,
                active_speaker: None,
                roundtable: roundtable::RoundtableState::default(),
                next_roundtable_turn: None,
                read_file_hashes: std::collections::HashMap::new(),
                current_turn_snapshots: Vec::new(),
                last_turn_snapshots: Vec::new(),
                current_turn_trace: Vec::new(),
                generated_images_this_turn: Vec::new(),
                current_turn_user_images: Vec::new(),
                in_flight_snapshot: Arc::new(std::sync::Mutex::new(InFlightSnapshot::default())),
                last_turn_paused_queue: false,
            })),
        };

        (session, tx, rx_ui)
    }

    #[must_use]
    pub fn with_identity(
        mut self,
        character_id: String,
        character_display_name: String,
        session_id: String,
    ) -> Self {
        let is_incognito = boxxy_claw_protocol::characters::is_incognito_character(&character_id);
        self.name = character_display_name.clone();
        self.character_id = character_id.clone();
        self.incognito = is_incognito;
        self.session_id = session_id.clone();
        if let Ok(mut state) = self.state.try_lock() {
            let settings = boxxy_preferences::Settings::load();
            let resolved_claw = if is_incognito {
                settings.claw_model.clone()
            } else {
                settings.resolve_character_model(&character_id)
            };
            state.agent_name = character_display_name.clone();
            state.character_id = character_id;
            state.character_display_name = character_display_name;
            state.incognito = is_incognito;
            state.session_id = session_id;
            if is_incognito {
                state.web_search_enabled = false;
                state.vibe_enabled = false;
                state.pinned = false;
            }
            state.current_search_label =
                crate::engine::agent_config::resolve_active_search_label_for_model(
                    &settings,
                    &resolved_claw,
                    state.web_search_enabled,
                )
                .to_string();
            state.current_context_window = boxxy_ai_core::model_context_window_with_endpoints(
                &resolved_claw,
                &settings.openai_compatible_endpoints,
            );
            state.current_claw_model = resolved_claw;
            state.current_memory_model = settings.effective_memory_model();
        }
        self
    }

    pub fn start(self, env: Arc<dyn ClawEnvironment>) {
        tokio::spawn(async move {
            self.run(env).await;
        });
    }

    pub(super) async fn send_ui(&self, event: ClawEngineEvent) {
        if !self.incognito {
            persist_visual_event(
                self.db.clone(),
                self.session_id.clone(),
                self.pane_id.clone(),
                &event,
            );
        }
        let _ = self.tx_ui.send(event).await;
    }

    pub(super) async fn clear_ui_history(&self) {
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::RestoreHistory(vec![]))
            .await;
    }

    /// Emits `ClawEngineEvent::UsageUpdated` with the session's authoritative token, context,
    /// and USD cost counters so the Claw sidebar stays in sync across out-of-band mutations.
    pub(super) async fn emit_usage_updated(&self) {
        let (is_headless, total_tokens, context_tokens, context_window, total_cost_usd) = {
            let state_lock = self.state.lock().await;
            let settings = boxxy_preferences::Settings::load();
            let resolved_model = if state_lock.incognito {
                settings.claw_model.clone()
            } else if !state_lock.character_id.is_empty() {
                settings.resolve_character_model(&state_lock.character_id)
            } else {
                state_lock.current_claw_model.clone()
            };
            let win = boxxy_ai_core::model_context_window_with_endpoints(
                &resolved_model,
                &settings.openai_compatible_endpoints,
            );
            (
                state_lock.is_headless,
                state_lock.total_tokens,
                state_lock.context_tokens,
                win,
                state_lock.total_cost_usd,
            )
        };
        if !is_headless {
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::UsageUpdated {
                    total_tokens,
                    context_tokens,
                    context_window,
                    total_cost_usd,
                })
                .await;
        }
    }

    /// Updates `sessions.history_json` and session token/cost metrics in SQLite when an
    /// already-persisted session mutates `SessionState` locally (such as mid-turn step salvage,
    /// `/undo`, proposal rejection, or ghost-dismissal), ensuring cross-pane `/resume` restores
    /// exact LLM history and usage without creating phantom rows.
    pub(super) async fn persist_history_if_present(&self) {
        if self.incognito {
            return;
        }
        let payload_opt = {
            let state_lock = self.state.lock().await;
            if state_lock.is_headless || state_lock.history.is_empty() {
                None
            } else {
                serde_json::to_string(&state_lock.history).ok().map(|json| {
                    (
                        json,
                        i64::try_from(state_lock.total_tokens).unwrap_or(i64::MAX),
                        i64::try_from(state_lock.context_tokens).unwrap_or(i64::MAX),
                        state_lock.total_cost_usd,
                    )
                })
            }
        };
        let Some((history_json, total_tokens, context_tokens, total_cost_usd)) = payload_opt else {
            return;
        };
        let db_opt = self.db.lock().await.clone();
        if let Some(db) = db_opt {
            let store = boxxy_db::store::Store::new(db.pool());
            let _ = store
                .update_session_history_and_usage(
                    &self.session_id,
                    &history_json,
                    total_tokens,
                    context_tokens,
                    total_cost_usd,
                )
                .await;
        }
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
            ClawMessage::Initialize
                | ClawMessage::SettingsInvalidated
                | ClawMessage::Reload
                | ClawMessage::ToggleVibe(_)
                | ClawMessage::ToggleWebSearch(_)
                | ClawMessage::TogglePin(_)
                | ClawMessage::CancelPending
                | ClawMessage::Deactivate
                | ClawMessage::Evict
                | ClawMessage::SleepToggle(_)
                | ClawMessage::Abort
                | ClawMessage::TurnFinished
                | ClawMessage::WakeSummaryComplete { .. }
                | ClawMessage::WatchdogTimeout { .. }
                | ClawMessage::ClawQuery { .. }
                | ClawMessage::UserMessage { .. }
                | ClawMessage::FileWriteReply { .. }
                | ClawMessage::FileDeleteReply { .. }
                | ClawMessage::KillProcessReply { .. }
                | ClawMessage::BackgroundCommandReply { .. }
                | ClawMessage::TerminalCommandAccepted { .. }
                | ClawMessage::CompleteRunningTerminalCommand { .. }
                | ClawMessage::GetClipboardReply { .. }
                | ClawMessage::SetClipboardReply { .. }
                | ClawMessage::ScrollbackReply { .. }
                | ClawMessage::SpawnAgentReply { .. }
                | ClawMessage::TaskCompletedEvent { .. }
                | ClawMessage::ForegroundProcessChanged { .. }
                | ClawMessage::CommandFinished { .. }
                | ClawMessage::RebindPane { .. }
                | ClawMessage::CancelTask { .. }
                | ClawMessage::TogglePauseTask { .. }
                | ClawMessage::CancelAllTasks
                | ClawMessage::BtwQuery { .. }
                | ClawMessage::CancelBtw { .. }
                | ClawMessage::RemoveQueuedPrompt { .. }
                | ClawMessage::ClearQueuedPrompts
                | ClawMessage::SendQueuedPrompt { .. }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_task_messages_bypass_active_turn_backlog() {
        let (tx, rx) = async_channel::unbounded();
        let mut urgent_backlog = VecDeque::new();
        let mut backlog = VecDeque::new();

        // Queue a scheduled trigger and soft-clear while a turn is active — both must wait in
        // `backlog` without aborting the active turn, whereas `CancelTask` bypasses immediately.
        tx.send(ClawMessage::ScheduledTaskTriggered {
            prompt: "[SCHEDULED COMMAND TRIGGERED] echo hi".to_string(),
        })
        .await
        .unwrap();
        let id = uuid::Uuid::new_v4();
        tx.send(ClawMessage::CancelTask { task_id: id })
            .await
            .unwrap();

        let next = ClawSession::get_next_msg(&rx, &mut urgent_backlog, &mut backlog, true)
            .await
            .expect("CancelTask should be yielded immediately during an active turn");
        assert!(matches!(next, ClawMessage::CancelTask { task_id } if task_id == id));
        assert_eq!(backlog.len(), 1);
        assert!(matches!(
            backlog.front(),
            Some(ClawMessage::ScheduledTaskTriggered { .. })
        ));

        tx.send(ClawMessage::CancelAllTasks).await.unwrap();
        let clear_all = ClawSession::get_next_msg(&rx, &mut urgent_backlog, &mut backlog, true)
            .await
            .expect("CancelAllTasks should also bypass the active turn backlog");
        assert!(matches!(clear_all, ClawMessage::CancelAllTasks));

        // `/btw` queries and cancellations must also dispatch immediately while a turn is active
        let req_id = uuid::Uuid::new_v4();
        tx.send(ClawMessage::BtwQuery {
            request_id: req_id,
            question: "What changed in src/module.rs?".to_string(),
            prior_exchange: None,
            snapshot: String::new(),
            cwd: "/workspace".to_string(),
        })
        .await
        .unwrap();
        let btw = ClawSession::get_next_msg(&rx, &mut urgent_backlog, &mut backlog, true)
            .await
            .expect("BtwQuery must bypass backlog during an active turn");
        assert!(matches!(btw, ClawMessage::BtwQuery { request_id, .. } if request_id == req_id));

        tx.send(ClawMessage::CancelBtw {
            request_id: Some(req_id),
        })
        .await
        .unwrap();
        let cancel_btw = ClawSession::get_next_msg(&rx, &mut urgent_backlog, &mut backlog, true)
            .await
            .expect("CancelBtw must bypass backlog during an active turn");
        assert!(matches!(
            cancel_btw,
            ClawMessage::CancelBtw {
                request_id: Some(id)
            } if id == req_id
        ));

        // `WakeSummaryComplete` and `WatchdogTimeout` MUST also bypass `backlog` while
        // `current_turn_active` is `true` so Hard Wake (`summarize_wake_delta`) never deadlocks!
        tx.send(ClawMessage::WakeSummaryComplete {
            result: Ok(String::new()),
        })
        .await
        .unwrap();
        let wake_done = ClawSession::get_next_msg(&rx, &mut urgent_backlog, &mut backlog, true)
            .await
            .expect("WakeSummaryComplete must bypass backlog while Dreamer task is active");
        assert!(matches!(wake_done, ClawMessage::WakeSummaryComplete { .. }));

        // `SettingsInvalidated` must stay in FIFO order with `ClawQuery` inside `urgent_backlog`
        // so a settings/model/character hot-swap is never overtaken by an immediately following query!
        tx.send(ClawMessage::SettingsInvalidated).await.unwrap();
        tx.send(ClawMessage::ClawQuery {
            prompt_id: None,
            query: "test after hot-swap".to_string(),
            snapshot: String::new(),
            cwd: "/workspace".to_string(),
            image_attachments: Vec::new(),
            image_attachment_meta: Vec::new(),
        })
        .await
        .unwrap();
        let first_urgent = ClawSession::get_next_msg(&rx, &mut urgent_backlog, &mut backlog, false)
            .await
            .expect("SettingsInvalidated must be yielded before subsequent ClawQuery");
        assert!(matches!(first_urgent, ClawMessage::SettingsInvalidated));
        let second_urgent =
            ClawSession::get_next_msg(&rx, &mut urgent_backlog, &mut backlog, false)
                .await
                .expect("ClawQuery follows SettingsInvalidated");
        assert!(matches!(second_urgent, ClawMessage::ClawQuery { .. }));
    }
}
