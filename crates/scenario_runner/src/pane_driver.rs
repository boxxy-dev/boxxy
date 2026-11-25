//! Non-dropping headless `ClawSession` pane driver and companion-terminal simulator.

use anyhow::{Result, anyhow};
use boxxy_claw::engine::ClawSession;
use boxxy_claw::registry::workspace::global_workspace;
use boxxy_claw_protocol::{
    AgentStatus, ClawEngineEvent, ClawEnvironment, ClawMessage, TurnTraceItem,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// State of a plain companion terminal pane spawned via `spawn_agent`.
#[derive(Debug, Clone)]
pub struct CompanionPane {
    pub pane_id: String,
    pub alias: String,
    pub cwd: PathBuf,
    pub buffer: String,
    pub keystrokes: Vec<String>,
    pub closed: bool,
}

/// Thread-safe registry of companion panes created during a test workspace run.
#[derive(Clone, Default)]
pub struct CompanionTable {
    inner: Arc<Mutex<HashMap<String, CompanionPane>>>,
}

impl CompanionTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn snapshot_all(&self) -> Vec<CompanionPane> {
        self.inner.lock().values().cloned().collect()
    }

    #[must_use]
    pub fn find_by_id_or_alias(&self, key: &str) -> Option<CompanionPane> {
        let guard = self.inner.lock();
        guard
            .get(key)
            .cloned()
            .or_else(|| guard.values().find(|p| p.alias.eq_ignore_ascii_case(key)).cloned())
    }

    fn insert(&self, pane: CompanionPane) {
        self.inner.lock().insert(pane.pane_id.clone(), pane);
    }

    fn append_output(&self, pane_id: &str, text: &str) -> Option<String> {
        let mut guard = self.inner.lock();
        let entry = guard.get_mut(pane_id)?;
        entry.buffer.push_str(text);
        let updated = entry.buffer.clone();
        drop(guard);
        Some(updated)
    }

    fn record_keystrokes(&self, target_id_or_alias: &str, keys: String) {
        let mut guard = self.inner.lock();
        if let Some(entry) = guard.get_mut(target_id_or_alias) {
            entry.keystrokes.push(keys);
            return;
        }
        if let Some(entry) = guard
            .values_mut()
            .find(|p| p.alias.eq_ignore_ascii_case(target_id_or_alias))
        {
            entry.keystrokes.push(keys);
        }
    }

    fn mark_closed(&self, target_id_or_alias: &str) -> Option<String> {
        let mut guard = self.inner.lock();
        if let Some(entry) = guard.get_mut(target_id_or_alias) {
            entry.closed = true;
            let id = entry.pane_id.clone();
            drop(guard);
            return Some(id);
        }
        if let Some(entry) = guard
            .values_mut()
            .find(|p| p.alias.eq_ignore_ascii_case(target_id_or_alias))
        {
            entry.closed = true;
            let id = entry.pane_id.clone();
            drop(guard);
            return Some(id);
        }
        None
    }

    pub async fn cleanup_all(&self) {
        let ids: Vec<String> = self.inner.lock().keys().cloned().collect();
        let ws = global_workspace().await;
        for id in ids {
            ws.unregister_pane(id).await;
        }
        self.inner.lock().clear();
    }
}

/// Headless controller for an isolated `ClawSession` actor.
///
/// Retains every emitted `ClawEngineEvent` in memory without dropping interleaved events,
/// maintaining a monotonic read cursor for sequential step assertions.
pub struct PaneDriver {
    pane_id: String,
    session_id: String,
    character_id: String,
    character_name: String,
    cwd: PathBuf,
    terminal_buffer: Arc<Mutex<String>>,
    tx: async_channel::Sender<ClawMessage>,
    events: Arc<Mutex<Vec<ClawEngineEvent>>>,
    event_notify: Arc<Notify>,
    cursor: usize,
    auto_run_companion_commands: Arc<AtomicBool>,
    actor_task: JoinHandle<()>,
    collector_task: JoinHandle<()>,
}

impl PaneDriver {
    pub(crate) async fn spawn(
        pane_id: String,
        session_id: String,
        character_id: String,
        character_name: String,
        cwd: PathBuf,
        env: Arc<dyn ClawEnvironment>,
        companions: CompanionTable,
    ) -> Self {
        let (session, tx, rx_ui) = ClawSession::new(pane_id.clone());
        let session = session.with_identity(
            character_id.clone(),
            character_name.clone(),
            session_id.clone(),
        );

        let workspace = global_workspace().await;
        workspace.register_pane_tx(pane_id.clone(), tx.clone()).await;
        workspace
            .update_pane_state(
                pane_id.clone(),
                Some(session_id.clone()),
                Some(character_name.clone()),
                cwd.to_string_lossy().into_owned(),
                None,
                None,
            )
            .await;

        let events = Arc::new(Mutex::new(Vec::with_capacity(64)));
        let event_notify = Arc::new(Notify::new());
        let terminal_buffer = Arc::new(Mutex::new(String::new()));
        let auto_run_companion_commands = Arc::new(AtomicBool::new(false));

        let events_bg = Arc::clone(&events);
        let notify_bg = Arc::clone(&event_notify);
        let tx_bg = tx.clone();
        let cwd_bg = cwd.clone();
        let buf_bg = Arc::clone(&terminal_buffer);
        let auto_companion_bg = Arc::clone(&auto_run_companion_commands);

        let env_collector = Arc::clone(&env);
        let collector_task = tokio::spawn(async move {
            while let Ok(event) = rx_ui.recv().await {
                handle_protocol_side_effects(
                    &event,
                    &tx_bg,
                    &cwd_bg,
                    &buf_bg,
                    &companions,
                    &env_collector,
                    auto_companion_bg.load(Ordering::Acquire),
                )
                .await;
                events_bg.lock().push(event);
                notify_bg.notify_waiters();
            }
        });

        let env_actor = env;
        let actor_task = tokio::spawn(async move {
            session.run(env_actor).await;
        });

        // Send `Initialize` so the session transitions out of `Off` into `Waiting` cleanly.
        let _ = tx.send(ClawMessage::Initialize).await;

        Self {
            pane_id,
            session_id,
            character_id,
            character_name,
            cwd,
            terminal_buffer,
            tx,
            events,
            event_notify,
            cursor: 0,
            auto_run_companion_commands,
            actor_task,
            collector_task,
        }
    }

    #[must_use]
    pub fn pane_id(&self) -> &str {
        &self.pane_id
    }

    /// Updates the local `pane_id` mirror after sending `ClawMessage::RebindPane` so `shutdown()`
    /// unregisters the rebound pane cleanly.
    pub fn set_local_pane_id(&mut self, new_pane_id: impl Into<String>) {
        self.pane_id = new_pane_id.into();
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn character_id(&self) -> &str {
        &self.character_id
    }

    #[must_use]
    pub fn character_name(&self) -> &str {
        &self.character_name
    }

    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Enables automatic execution of `ProposeTerminalCommand` proposals targeting a spawned
    /// companion pane (`pane-2`, etc.) using `ClawEnvironment::exec_shell`.
    pub fn set_auto_run_companion_commands(&self, enabled: bool) {
        self.auto_run_companion_commands
            .store(enabled, Ordering::Release);
    }

    /// Updates this pane's simulated terminal buffer and publishes it to `WorkspaceRegistry`.
    pub async fn set_terminal_snapshot(&self, snapshot: impl Into<String>) {
        let snap = snapshot.into();
        (*self.terminal_buffer.lock()).clone_from(&snap);
        let ws = global_workspace().await;
        ws.update_pane_state(
            self.pane_id.clone(),
            Some(self.session_id.clone()),
            Some(self.character_name.clone()),
            self.cwd.to_string_lossy().into_owned(),
            None,
            Some(snap),
        )
        .await;
    }

    /// Sends any raw [`ClawMessage`] to the underlying `ClawSession`.
    ///
    /// # Errors
    /// Returns an error if the session channel is closed.
    pub async fn send(&self, msg: ClawMessage) -> Result<()> {
        self.tx
            .send(msg)
            .await
            .map_err(|e| anyhow!("failed to send ClawMessage to {}: {e}", self.pane_id))
    }

    /// Submits a user overlay query (`ClawMessage::ClawQuery`) using the pane's current snapshot and CWD.
    ///
    /// # Errors
    /// Returns an error if the session channel is closed.
    pub async fn send_query(&self, query: impl Into<String>) -> Result<()> {
        let snapshot = self.terminal_buffer.lock().clone();
        self.send(ClawMessage::ClawQuery {
            prompt_id: Some(Uuid::new_v4()),
            query: query.into(),
            snapshot,
            cwd: self.cwd.to_string_lossy().into_owned(),
            image_attachments: Vec::new(),
            image_attachment_meta: Vec::new(),
        })
        .await
    }

    /// Submits a `ClawMessage::ClawQuery` carrying inline base64 images and metadata anchors.
    ///
    /// # Errors
    /// Returns an error if the session channel is closed.
    pub async fn send_query_with_images(
        &self,
        query: impl Into<String>,
        image_attachments: Vec<String>,
        image_attachment_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
    ) -> Result<()> {
        let snapshot = self.terminal_buffer.lock().clone();
        self.send(ClawMessage::ClawQuery {
            prompt_id: Some(Uuid::new_v4()),
            query: query.into(),
            snapshot,
            cwd: self.cwd.to_string_lossy().into_owned(),
            image_attachments,
            image_attachment_meta,
        })
        .await
    }

    /// Submits a `ClawMessage::ClawQuery` with an explicit `prompt_id` (useful for queue recall/cancel tests).
    ///
    /// # Errors
    /// Returns an error if the session channel is closed.
    pub async fn send_query_with_id(&self, id: Uuid, query: impl Into<String>) -> Result<()> {
        let snapshot = self.terminal_buffer.lock().clone();
        self.send(ClawMessage::ClawQuery {
            prompt_id: Some(id),
            query: query.into(),
            snapshot,
            cwd: self.cwd.to_string_lossy().into_owned(),
            image_attachments: Vec::new(),
            image_attachment_meta: Vec::new(),
        })
        .await
    }

    /// Sends an ephemeral `/btw` side question and returns its correlation `request_id`.
    ///
    /// # Errors
    /// Returns an error if the session channel is closed.
    pub async fn send_btw(&self, question: impl Into<String>) -> Result<Uuid> {
        let request_id = Uuid::new_v4();
        let snapshot = self.terminal_buffer.lock().clone();
        self.send(ClawMessage::BtwQuery {
            request_id,
            question: question.into(),
            prior_exchange: None,
            snapshot,
            cwd: self.cwd.to_string_lossy().into_owned(),
        })
        .await?;
        Ok(request_id)
    }

    /// Approves or rejects a pending `ProposeFileWrite` proposal.
    ///
    /// # Errors
    /// Returns an error if the session channel is closed.
    pub async fn reply_file_write(&self, approved: bool) -> Result<()> {
        if approved {
            self.send(ClawMessage::FileWriteReply { approved: true })
                .await
        } else {
            self.send(ClawMessage::FileWriteReply { approved: false })
                .await
        }
    }

    /// Notifies the session that a shell command finished with `exit_code` and `snapshot`.
    ///
    /// # Errors
    /// Returns an error if the session channel is closed.
    pub async fn notify_command_finished(
        &self,
        exit_code: i32,
        snapshot: impl Into<String>,
    ) -> Result<()> {
        let snap = snapshot.into();
        (*self.terminal_buffer.lock()).clone_from(&snap);
        self.send(ClawMessage::CommandFinished {
            exit_code,
            snapshot: snap,
            cwd: self.cwd.to_string_lossy().into_owned(),
        })
        .await
    }

    /// Returns a clone of all events recorded since this pane was spawned.
    #[must_use]
    pub fn all_events(&self) -> Vec<ClawEngineEvent> {
        self.events.lock().clone()
    }

    /// Returns all events recorded strictly after `index`.
    #[must_use]
    pub fn events_since(&self, index: usize) -> Vec<ClawEngineEvent> {
        let guard = self.events.lock();
        guard.get(index..).map_or_else(Vec::new, <[ClawEngineEvent]>::to_vec)
    }

    /// Advances the read cursor to the current tail of the event stream.
    pub fn advance_cursor_to_end(&mut self) -> usize {
        let len = self.events.lock().len();
        self.cursor = len;
        len
    }

    /// Waits without polling for the next event at or after the current read cursor
    /// matching `predicate`, advancing the cursor past the matched event.
    ///
    /// # Errors
    /// Returns a timeout error including recently observed events if `timeout` elapses.
    pub async fn wait_for_next<T, F>(&mut self, timeout: Duration, mut predicate: F) -> Result<T>
    where
        F: FnMut(&ClawEngineEvent) -> Option<T>,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.event_notify.notified();
            {
                let guard = self.events.lock();
                for (offset, ev) in guard[self.cursor..].iter().enumerate() {
                    if let Some(matched) = predicate(ev) {
                        self.cursor += offset + 1;
                        return Ok(matched);
                    }
                }
                self.cursor = guard.len();
            }
            tokio::time::timeout_at(deadline, notified)
                .await
                .map_err(|_| {
                    let tail: Vec<String> = self
                        .events
                        .lock()
                        .iter()
                        .rev()
                        .take(8)
                        .map(|e| format!("{e:?}"))
                        .collect();
                    anyhow!(
                        "Timed out after {timeout:?} waiting on pane '{}'. Recent events (newest first):\n{}",
                        self.pane_id,
                        tail.join("\n")
                    )
                })?;
        }
    }

    /// Waits for initial `ClawEngineEvent::Identity` registration after `Initialize`.
    ///
    /// # Errors
    /// Returns an error if `timeout` expires before `Identity` arrives.
    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        self.wait_for_next(timeout, |ev| match ev {
            ClawEngineEvent::Identity { .. } => Some(()),
            _ => None,
        })
        .await
    }

    /// Waits for `SessionStateChanged` with `status == expected`.
    ///
    /// # Errors
    /// Returns an error if `timeout` expires before reaching `expected`.
    pub async fn wait_for_status(&mut self, expected: &AgentStatus, timeout: Duration) -> Result<()> {
        self.wait_for_next(timeout, |ev| match ev {
            ClawEngineEvent::SessionStateChanged { status, .. } if status == expected => Some(()),
            _ => None,
        })
        .await
    }

    /// Waits for the next `DiagnosisComplete` event and returns `(diagnosis_markdown, trace)`.
    ///
    /// # Errors
    /// Returns an error if `timeout` expires before `DiagnosisComplete` arrives.
    pub async fn wait_for_diagnosis(
        &mut self,
        timeout: Duration,
    ) -> Result<(String, Vec<TurnTraceItem>)> {
        self.wait_for_next(timeout, |ev| match ev {
            ClawEngineEvent::DiagnosisComplete {
                diagnosis, trace, ..
            } => Some((diagnosis.clone(), trace.clone())),
            _ => None,
        })
        .await
    }

    /// Cleanly stops the session actor and collector tasks and removes the pane from `WorkspaceRegistry`.
    ///
    /// Aborts `actor_task` before teardown so end-of-test cleanup never spawns detached
    /// `Deactivate` memory-flush requests against a closing `MockLlmServer` (tests verifying
    /// `Deactivate` lifecycle flushes send `ClawMessage::Deactivate` explicitly before `shutdown()`).
    pub async fn shutdown(&mut self) {
        self.actor_task.abort();
        self.collector_task.abort();
        let ws = global_workspace().await;
        ws.release_all_locks(&self.pane_id).await;
        ws.unregister_pane(self.pane_id.clone()).await;
    }
}

impl Drop for PaneDriver {
    fn drop(&mut self) {
        self.actor_task.abort();
        self.collector_task.abort();
    }
}

async fn handle_protocol_side_effects(
    event: &ClawEngineEvent,
    tx_session: &async_channel::Sender<ClawMessage>,
    default_cwd: &Path,
    home_buffer: &Arc<Mutex<String>>,
    companions: &CompanionTable,
    env: &Arc<dyn ClawEnvironment>,
    auto_run_companion_commands: bool,
) {
    match event {
        ClawEngineEvent::RequestSpawnAgent {
            request_id: Some(req_id),
            ..
        } => {
            let new_pane_id = format!("companion-{}", Uuid::new_v4());
            let cwd_str = default_cwd.to_string_lossy().into_owned();
            let ws = global_workspace().await;
            ws.update_pane_state(
                new_pane_id.clone(),
                None,
                None,
                cwd_str,
                None,
                Some(String::new()),
            )
            .await;
            let alias = ws
                .get_pane_alias(&new_pane_id)
                .await
                .unwrap_or_else(|| new_pane_id.clone());
            companions.insert(CompanionPane {
                pane_id: new_pane_id.clone(),
                alias: alias.clone(),
                cwd: default_cwd.to_path_buf(),
                buffer: String::new(),
                keystrokes: Vec::new(),
                closed: false,
            });
            let _ = tx_session
                .send(ClawMessage::SpawnAgentReply {
                    request_id: *req_id,
                    pane_id: new_pane_id,
                    alias,
                })
                .await;
        }
        ClawEngineEvent::RequestScrollback {
            request_id,
            target_pane_id,
            ..
        } => {
            let content = target_pane_id
                .as_deref()
                .and_then(|tid| companions.find_by_id_or_alias(tid).map(|p| p.buffer))
                .unwrap_or_else(|| home_buffer.lock().clone());
            let _ = tx_session
                .send(ClawMessage::ScrollbackReply {
                    request_id: *request_id,
                    content,
                })
                .await;
        }
        ClawEngineEvent::InjectKeystrokes {
            target_agent_name,
            target_pane_id,
            keys,
        } => {
            let key = target_pane_id.as_deref().unwrap_or(target_agent_name);
            companions.record_keystrokes(key, keys.clone());
        }
        ClawEngineEvent::RequestCloseAgent {
            target_agent_name,
            target_pane_id,
        } => {
            let key = target_pane_id.as_deref().unwrap_or(target_agent_name);
            if let Some(closed_id) = companions.mark_closed(key) {
                global_workspace().await.unregister_pane(closed_id).await;
            }
        }
        ClawEngineEvent::ProposeTerminalCommand {
            command,
            target_pane_id: Some(target_id),
            ..
        } if auto_run_companion_commands => {
            execute_companion_terminal_command(target_id, command, default_cwd, companions, env)
                .await;
        }
        _ => {}
    }
}

async fn execute_companion_terminal_command(
    target_id: &str,
    command: &str,
    default_cwd: &Path,
    companions: &CompanionTable,
    env: &Arc<dyn ClawEnvironment>,
) {
    let full_cmd = format!(
        "cd -- '{}' && {}",
        default_cwd.to_string_lossy().replace('\'', "'\\''"),
        command
    );
    let (exit_code, stdout, stderr) = env
        .exec_shell(full_cmd)
        .await
        .unwrap_or_else(|e| (1, String::new(), e.to_string()));
    let combined = if stderr.is_empty() {
        format!("$ {command}\n{stdout}")
    } else {
        format!("$ {command}\n{stdout}{stderr}")
    };
    let full_snap = companions
        .append_output(target_id, &combined)
        .unwrap_or(combined);
    let ws = global_workspace().await;
    ws.update_pane_state(
        target_id.to_string(),
        None,
        None,
        default_cwd.to_string_lossy().into_owned(),
        Some(command.to_string()),
        Some(full_snap.clone()),
    )
    .await;
    ws.complete_pending_command(target_id, exit_code, full_snap)
        .await;
}
