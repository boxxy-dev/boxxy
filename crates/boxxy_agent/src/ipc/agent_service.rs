//! Server-side D-Bus implementation of `dev.boxxy.BoxxyTerminal.Agent`.

use crate::claw::notifier;
use crate::daemon::DaemonCore;
use crate::ipc::broadcasters::{
    build_pending_tasks_snapshot, emit_mcp_status_changed, emit_pending_tasks_changed,
};
use crate::ipc::session_manager::{ClawSessionManager, HostedSession, teardown_holder};
use anyhow::Result;
use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use std::sync::Arc;
use tokio::sync::{Mutex, watch};
use zbus::{Connection, object_server::SignalEmitter};

pub struct AgentInterface {
    pub core: Arc<DaemonCore>,
    pub client_count_tx: watch::Sender<usize>,
    pub(crate) session_manager: Arc<Mutex<ClawSessionManager>>,
}

#[zbus::interface(name = "dev.boxxy.BoxxyTerminal.Agent")]
impl AgentInterface {
    async fn get_version(&self) -> String {
        let is_deleted_on_disk = std::fs::read_link("/proc/self/exe")
            .ok()
            .and_then(|p| p.to_str().map(|s| s.ends_with(" (deleted)")))
            .unwrap_or(false);
        if is_deleted_on_disk {
            format!("{}-stale", crate::daemon::singleton::AGENT_VERSION)
        } else {
            crate::daemon::singleton::AGENT_VERSION.to_string()
        }
    }

    async fn get_config_dir(&self) -> String {
        boxxy_sys_utils::get_config_dir()
            .to_string_lossy()
            .into_owned()
    }

    async fn update_credentials(
        &self,
        api_keys: std::collections::HashMap<String, String>,
        ollama_url: String,
    ) {
        {
            let keys_guard = self.core.state.api_keys.read().await;
            let url_guard = self.core.state.ollama_url.read().await;
            if *keys_guard == api_keys && *url_guard == ollama_url {
                return;
            }
        }

        log::info!("Updating credentials for {} providers", api_keys.len());

        let mut keys_guard = self.core.state.api_keys.write().await;
        *keys_guard = api_keys;
        let mut url_guard = self.core.state.ollama_url.write().await;
        *url_guard = ollama_url;
    }

    async fn notify_settings_invalidated(&self, #[zbus(connection)] conn: &zbus::Connection) {
        log::debug!("Settings invalidated signal received from UI. Reloading cache.");
        boxxy_preferences::Settings::reload();
        boxxy_claw::registry::skills::reload_if_initialized().await;
        let settings = boxxy_preferences::Settings::load();
        if !settings.enable_telemetry {
            tokio::spawn(async {
                boxxy_telemetry::disable_and_purge().await;
            });
        }
        let persist = settings.pty_persistence;
        self.core
            .state
            .pty_registry
            .sync_default_persistence(persist)
            .await;

        let mcp_servers = settings.mcp_servers.clone();
        let conn_clone = conn.clone();
        tokio::spawn(async move {
            let statuses = boxxy_claw::sync_and_probe_mcp_servers(mcp_servers).await;
            boxxy_claw_protocol::characters::update_mcp_status_cache(statuses.clone());
            emit_mcp_status_changed(&conn_clone, &statuses).await;
        });
    }

    #[zbus(signal)]
    async fn mcp_status_changed(
        emitter: &SignalEmitter<'_>,
        statuses_json: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn claims_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        snapshot_json: String,
    ) -> zbus::Result<()>;

    async fn claim_startup_token(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> String {
        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let (was_empty, new_count) = crate::ipc::client_tracker::register_client_and_sync_count(
            owner_bus_name,
            &self.client_count_tx,
        );

        let telemetry_session_id = if was_empty {
            boxxy_telemetry::rotate_session_id()
        } else {
            boxxy_telemetry::session_id()
        };
        log::info!("Client connected. Total clients: {}", new_count);

        let db_was_reset =
            boxxy_db::DATABASE_WAS_RESET.swap(false, std::sync::atomic::Ordering::SeqCst);
        let snapshot = self.core.registry.snapshot().await;

        let token = boxxy_claw_protocol::characters::StartupToken {
            daemon_version: crate::daemon::singleton::AGENT_VERSION.to_string(),
            db_was_reset,
            initial_revision: snapshot.revision,
            telemetry_session_id,
        };
        serde_json::to_string(&token).unwrap_or_default()
    }

    async fn get_registry_snapshot(&self) -> String {
        let snapshot = self.core.registry.snapshot().await;
        serde_json::to_string(&snapshot).unwrap_or_default()
    }

    async fn try_claim_character(
        &self,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
        holder_id: String,
        holder_kind: u8,
        character_id: String,
    ) -> String {
        let kind = match boxxy_claw_protocol::characters::HolderKind::try_from(holder_kind) {
            Ok(k) => k,
            Err(_) => {
                return serde_json::to_string(&Result::<
                    boxxy_claw_protocol::characters::ClaimedSession,
                    boxxy_claw_protocol::characters::ClaimError,
                >::Err(
                    boxxy_claw_protocol::characters::ClaimError::UnknownCharacter { character_id },
                ))
                .unwrap_or_default();
            }
        };

        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let result = self
            .do_claim(emitter, conn, holder_id, kind, character_id, owner_bus_name)
            .await;
        serde_json::to_string(&result).unwrap_or_default()
    }

    async fn release_holder(&self, #[zbus(connection)] conn: &zbus::Connection, holder_id: &str) {
        if let Some(claim) =
            teardown_holder(&self.session_manager, &self.core.registry, holder_id).await
        {
            log::info!(
                "Released holder {} (kind {:?})",
                holder_id,
                claim.holder_kind
            );
        }
        emit_pending_tasks_changed(conn, &self.core.state.pty_registry).await;
    }

    async fn detach_holder(&self, #[zbus(connection)] conn: &zbus::Connection, holder_id: &str) {
        if self.core.registry.is_incognito_holder(holder_id).await {
            self.release_holder(conn, holder_id).await;
            return;
        }
        if let Some(claim) = self.core.registry.detach_holder(holder_id).await {
            log::info!(
                "Detached holder {} into background (session_id={})",
                holder_id,
                claim.session_id
            );
            emit_pending_tasks_changed(conn, &self.core.state.pty_registry).await;
        }
    }

    async fn reattach_holder(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
        old_holder_id: &str,
        new_holder_id: String,
    ) -> String {
        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let res = self
            .do_reattach_holder(conn, old_holder_id, new_holder_id, owner_bus_name)
            .await;
        serde_json::to_string(&res).unwrap_or_default()
    }

    async fn reattach_holder_by_pid(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
        pid: u32,
        new_holder_id: String,
    ) -> String {
        let owner_bus_name = header
            .sender()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let old_holder_opt = self.core.state.pty_registry.get_original_pane_id(pid).await;
        let res = if let Some(old_holder_id) = old_holder_opt {
            self.core
                .state
                .pty_registry
                .set_original_pane_id(pid, new_holder_id.clone())
                .await;
            self.do_reattach_holder(conn, &old_holder_id, new_holder_id, owner_bus_name)
                .await
        } else {
            None
        };
        serde_json::to_string(&res).unwrap_or_default()
    }

    async fn get_pending_tasks(&self) -> String {
        let tasks = build_pending_tasks_snapshot(&self.core.state.pty_registry).await;
        serde_json::to_string(&tasks).unwrap_or_else(|_| "[]".to_string())
    }

    async fn cancel_pending_task(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        pane_id: &str,
        task_id: &str,
    ) {
        let Ok(parsed_id) = uuid::Uuid::parse_str(task_id) else {
            return;
        };
        let tx_opt = {
            let manager = self.session_manager.lock().await;
            manager
                .sessions
                .values()
                .find(|s| s.pane_id == pane_id)
                .map(|s| s.tx.clone())
        };
        if let Some(tx) = tx_opt {
            let _ = tx
                .send(ClawMessage::CancelTask { task_id: parsed_id })
                .await;
        } else {
            let workspace = boxxy_claw::registry::workspace::global_workspace().await;
            let raw = workspace.get_all_pending_tasks_with_pane().await;
            let remaining: Vec<boxxy_claw_protocol::ScheduledTask> = raw
                .into_iter()
                .filter(|(p, _, t)| p == pane_id && t.id != parsed_id)
                .map(|(_, _, t)| t)
                .collect();
            workspace
                .update_pane_tasks(pane_id.to_string(), remaining)
                .await;
            emit_pending_tasks_changed(conn, &self.core.state.pty_registry).await;
        }
    }

    async fn update_workspace_pane(
        &self,
        pane_id: String,
        cwd: String,
        last_command: String,
        snapshot: String,
        status: String,
    ) -> String {
        let workspace = boxxy_claw::registry::workspace::global_workspace().await;
        let cmd_opt = (!last_command.is_empty()).then_some(last_command);
        let snap_opt = (!snapshot.is_empty()).then_some(snapshot);
        let alias = workspace
            .update_pane_state(pane_id.clone(), None, None, cwd, cmd_opt, snap_opt)
            .await;
        if !status.is_empty() {
            let status_opt = (status != "Idle").then_some(status);
            workspace.set_pane_status(pane_id, status_opt).await;
        }
        alias
    }

    async fn unregister_workspace_pane(&self, pane_id: String) {
        let workspace = boxxy_claw::registry::workspace::global_workspace().await;
        workspace.unregister_pane(pane_id).await;
    }

    async fn notify_pane_command_finished(
        &self,
        pane_id: String,
        exit_code: i32,
        snapshot: String,
        cwd: String,
    ) {
        let workspace = boxxy_claw::registry::workspace::global_workspace().await;
        let last_cmd = snapshot.lines().next().unwrap_or("").to_string();
        let cmd_opt = (!last_cmd.is_empty()).then_some(last_cmd);
        let snap_opt = (!snapshot.is_empty()).then_some(snapshot.clone());
        workspace
            .update_pane_state(pane_id.clone(), None, None, cwd, cmd_opt, snap_opt)
            .await;

        let _ = workspace
            .complete_pending_command(&pane_id, exit_code, snapshot)
            .await;

        workspace
            .publish_event(boxxy_claw_protocol::ClawEvent::ProcessExited {
                pane_id,
                exit_code,
            })
            .await;
    }

    async fn resolve_peer(&self, query_json: &str) -> String {
        let peer_opt = if let Ok(query) =
            serde_json::from_str::<boxxy_claw_protocol::characters::PeerQuery>(query_json)
        {
            self.core.registry.resolve_peer(&query).await
        } else {
            None
        };
        serde_json::to_string(&peer_opt).unwrap_or_default()
    }

    async fn request_reload(&self) {
        log::info!("Self-reload requested. Flushing sessions, telemetry, and executing new binary...");
        self.session_manager.lock().await.shutdown_all();
        boxxy_telemetry::flush_journal().await;

        let exe_raw = std::env::current_exe()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // If the binary on disk was atomically swapped via rename(), Linux's
        // `/proc/self/exe` symlink appends `" (deleted)"`. Strip that suffix so
        // `execvp` launches the newly installed binary at the canonical path.
        let exe_clean = exe_raw
            .strip_suffix(" (deleted)")
            .unwrap_or(&exe_raw)
            .to_string();

        // Filter out `--background` and `--sync-settings` so the reloaded process
        // replaces the current daemon PID in-place without daemonizing a second time
        // or overwriting runtime settings changes.
        let args: Vec<String> = std::env::args()
            .filter(|a| a != "--background" && a != "--sync-settings")
            .collect();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Ok(c_exe) = std::ffi::CString::new(exe_clean) {
                let c_args: Vec<std::ffi::CString> = args
                    .iter()
                    .filter_map(|s| std::ffi::CString::new(s.as_str()).ok())
                    .collect();
                let _ = nix::unistd::execvp(&c_exe, &c_args);
            }
        });
    }

    async fn request_stop(&self) {
        log::info!("Stop requested. Terminating sessions, flushing telemetry, and exiting...");
        self.session_manager.lock().await.shutdown_all();
        self.core.state.pty_registry.terminate_all().await;
        boxxy_telemetry::flush_journal().await;
        std::process::exit(0);
    }

    async fn post_claw_message(&self, session_id: &str, message_json: &str) {
        if let Ok(message) = serde_json::from_str::<ClawMessage>(message_json) {
            let pane_info = {
                let manager = self.session_manager.lock().await;
                manager
                    .sessions
                    .get(session_id)
                    .map(|s| (s.pane_id.clone(), s.tx.clone()))
            };

            if let Some((pane_id, tx)) = pane_info {
                if let ClawMessage::ResumeSession {
                    session_id: ref resume_sid,
                    character_id: ref override_cid,
                } = message
                {
                    let store = boxxy_db::store::Store::new(self.core.db.pool());
                    if let Ok(Some(db_session)) = store.get_session(resume_sid).await {
                        let (target_cid, target_disp, target_agent) =
                            if let Some(cid) = override_cid.as_deref().filter(|s| !s.is_empty()) {
                                (cid, "", None)
                            } else {
                                (
                                    db_session.character_id.as_str(),
                                    db_session.character_display_name.as_str(),
                                    db_session.agent_name.as_deref(),
                                )
                            };
                        if let Some((_, display_name)) = self
                            .core
                            .registry
                            .resolve_and_update_holder_character(
                                &pane_id,
                                target_cid,
                                target_disp,
                                target_agent,
                                Some(resume_sid),
                            )
                            .await
                        {
                            let mut manager = self.session_manager.lock().await;
                            if let Some(hosted) = manager.sessions.get_mut(session_id) {
                                *hosted.agent_name.write() = display_name;
                                *hosted.active_session_id.write() = resume_sid.clone();
                                let aliased = hosted.clone();
                                manager.sessions.insert(resume_sid.clone(), aliased);
                            }
                        }
                    }
                }
                let _ = tx.send(message).await;
            }
        }
    }

    async fn list_claw_sessions(&self) -> Vec<(String, String, String)> {
        let mut manager = self.session_manager.lock().await;
        manager.prune_dead();
        let mut seen_panes = std::collections::HashSet::new();
        let mut out = Vec::new();
        for s in manager.sessions.values() {
            let effective_sid = s.active_session_id.read().clone();
            if seen_panes.insert(s.pane_id.clone()) {
                out.push((
                    effective_sid,
                    s.pane_id.clone(),
                    s.agent_name.read().clone(),
                ));
            }
        }
        out
    }

    async fn request_focus_pane(
        &self,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
        pane_id: &str,
    ) {
        log::info!("Focus requested for pane {}", pane_id);
        // If `pane_id` belongs to a detached background session, restore it
        // instead of emitting `FocusPane` into a pane with no attached UI viewer.
        if let Some((pid, true)) = self
            .core
            .state
            .pty_registry
            .find_pid_by_pane_id(pane_id)
            .await
        {
            log::info!(
                "Pane {} is detached in background (pid={}); restoring detached session",
                pane_id,
                pid
            );
            self.request_restore_detached(emitter, pid).await;
            return;
        }

        let manager = self.session_manager.lock().await;
        // Find any session tied to this pane. Usually there is only one.
        for session in manager.sessions.values() {
            if session.pane_id == pane_id {
                let effective_sid = session.active_session_id.read().clone();
                let event = ClawEngineEvent::FocusPane;
                if let Ok(event_json) = serde_json::to_string(&event) {
                    let _ = Self::claw_event(&emitter, effective_sid, event_json).await;
                }
                break;
            }
        }
    }

    #[zbus(signal)]
    async fn claw_event(
        emitter: &SignalEmitter<'_>,
        session_id: String,
        event_json: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn detached_sessions_changed(
        emitter: &SignalEmitter<'_>,
        sessions_json: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn pending_tasks_changed(
        emitter: &SignalEmitter<'_>,
        tasks_json: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn restore_detached_requested(
        emitter: &SignalEmitter<'_>,
        pid: u32,
        cwd: String,
    ) -> zbus::Result<()>;

    async fn get_detached_sessions(&self) -> String {
        let list =
            crate::pty::query_detached_sessions_detailed(&self.core.state.pty_registry).await;
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }

    async fn request_restore_detached(
        &self,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
        pid: u32,
    ) {
        let list =
            crate::pty::query_detached_sessions_detailed(&self.core.state.pty_registry).await;
        if list.is_empty() {
            return;
        }

        if self.core.ghost.is_ghost() {
            let mut pending = self.core.state.pending_restores.write().await;
            if pid == 0 {
                // 0 sentinel signals "restore all grouped by split layout"
                if !pending.contains(&0) {
                    pending.push(0);
                }
            } else {
                for t in list.into_iter().filter(|s| s.pid == pid) {
                    if !pending.contains(&t.pid) {
                        pending.push(t.pid);
                    }
                }
            }
        } else if pid == 0 {
            let _ = Self::restore_detached_requested(&emitter, 0, String::new()).await;
        } else {
            for t in list.into_iter().filter(|s| s.pid == pid) {
                let _ = Self::restore_detached_requested(&emitter, t.pid, t.cwd).await;
            }
        }
    }

    async fn take_pending_restores(&self) -> Vec<(u32, String)> {
        let queued: Vec<u32> = {
            let mut pending = self.core.state.pending_restores.write().await;
            std::mem::take(&mut *pending)
        };
        if queued.is_empty() {
            return Vec::new();
        }
        if queued.contains(&0) {
            return vec![(0, String::new())];
        }
        let list =
            crate::pty::query_detached_sessions_detailed(&self.core.state.pty_registry).await;
        list.into_iter()
            .filter(|s| queued.contains(&s.pid))
            .map(|s| (s.pid, s.cwd))
            .collect()
    }

    async fn terminate_detached_session(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        pid: u32,
    ) {
        let list =
            crate::pty::query_detached_sessions_detailed(&self.core.state.pty_registry).await;
        let targets: Vec<u32> = if pid == 0 {
            list.into_iter().map(|s| s.pid).collect()
        } else {
            list.into_iter()
                .filter(|s| s.pid == pid)
                .map(|s| s.pid)
                .collect()
        };

        for target_pid in targets {
            if let Some(orig_pane_id) = self
                .core
                .state
                .pty_registry
                .get_original_pane_id(target_pid)
                .await
            {
                let _ = teardown_holder(&self.session_manager, &self.core.registry, &orig_pane_id)
                    .await;
            }
            self.core
                .state
                .pty_registry
                .set_persistence(target_pid, false)
                .await;
            let _ = crate::pty::registry::kill_process_group_safe(target_pid, libc::SIGTERM);
        }
        emit_pending_tasks_changed(conn, &self.core.state.pty_registry).await;
    }
}

/// Converts select `ClawEngineEvent`s into host desktop notifications over
/// `org.freedesktop.Notifications` so reminders and long tasks always appear
/// reliably on all Linux desktops (COSMIC, KDE, Sway, GNOME) whether Boxxy
/// Terminal is open, unfocused, or running in Ghost Mode.
async fn maybe_send_desktop_notification(
    conn: &Connection,
    agent_name: &str,
    event: &ClawEngineEvent,
) {
    let raw_first = agent_name.split_whitespace().next().unwrap_or(agent_name);
    let first_name = crate::pty::prettify_character_name(raw_first);
    match event {
        ClawEngineEvent::PushGlobalNotification { title, message } => {
            notifier::send(conn, title, message).await;
        }
        ClawEngineEvent::TaskCompleted { message, .. } => {
            let title = format!("{first_name} sends a reminder");
            let body = message
                .clone()
                .unwrap_or_else(|| format!("{first_name} has finished a timer."));
            notifier::send(conn, &title, &body).await;
        }
        ClawEngineEvent::LongTaskCompleted { message, .. } => {
            let title = format!("{first_name} completed a task");
            notifier::send(conn, &title, message).await;
        }
        _ => {}
    }
}

impl AgentInterface {
    async fn do_claim(
        &self,
        emitter: zbus::object_server::SignalEmitter<'_>,
        conn: &zbus::Connection,
        holder_id: String,
        holder_kind: boxxy_claw_protocol::characters::HolderKind,
        character_id: String,
        owner_bus_name: String,
    ) -> Result<
        boxxy_claw_protocol::characters::ClaimedSession,
        boxxy_claw_protocol::characters::ClaimError,
    > {
        let claim_res = self
            .core
            .registry
            .try_claim(
                holder_id.clone(),
                holder_kind,
                character_id.clone(),
                owner_bus_name.clone(),
            )
            .await;

        match claim_res {
            Ok(claimed_session) => {
                let session_id = claimed_session.session_id.clone();
                let is_incognito =
                    boxxy_claw_protocol::characters::is_incognito_character(&character_id);
                let character_display_name = if is_incognito {
                    boxxy_claw_protocol::characters::INCOGNITO_DISPLAY_NAME.to_string()
                } else {
                    self.core
                        .registry
                        .character_display_name(&character_id)
                        .await
                        .unwrap_or_else(|| "Unknown".to_string())
                };

                let (session, tx, rx_ui) = boxxy_claw::engine::ClawSession::new(holder_id.clone());
                let session = session.with_identity(
                    character_id.clone(),
                    character_display_name.clone(),
                    session_id.clone(),
                );

                // Register with workspace before emitting signal; for ephemeral incognito
                // sessions, purge any previous normal-character bindings on `holder_id`
                // so swarm tools never message a superseded session.
                if holder_kind == boxxy_claw_protocol::characters::HolderKind::Pane {
                    let workspace = boxxy_claw::registry::workspace::global_workspace().await;
                    if is_incognito {
                        workspace.release_all_locks(&holder_id).await;
                        workspace.unregister_pane(holder_id.clone()).await;
                        workspace
                            .update_pane_tasks(holder_id.clone(), Vec::new())
                            .await;
                    } else {
                        workspace
                            .register_pane_tx(holder_id.clone(), tx.clone())
                            .await;
                    }
                }

                {
                    // Initialize session with core state
                    let mut state_lock = session.state.lock().await;
                    state_lock.tracked_pids = self.core.state.tracked_pids.clone();
                    state_lock.api_keys = self.core.state.api_keys.clone();
                    state_lock.ollama_url = self.core.state.ollama_url.clone();
                    state_lock.tx_self = tx.clone();
                }

                let active_session_id = Arc::new(parking_lot::RwLock::new(session_id.clone()));
                let shared_agent_name = Arc::new(parking_lot::RwLock::new(character_display_name));
                let env = {
                    let mut manager = self.session_manager.lock().await;
                    // Shut down any superseded ClawSession actor previously bound to `holder_id`
                    // (e.g. character switch or `/incognito` transition) so its history flushes
                    // and its forwarder task exits cleanly instead of leaking.
                    manager.shutdown_pane(&holder_id);
                    manager.sessions.insert(
                        session_id.clone(),
                        HostedSession {
                            pane_id: holder_id.clone(),
                            agent_name: shared_agent_name.clone(),
                            active_session_id: active_session_id.clone(),
                            tx,
                        },
                    );
                    manager.env.clone()
                };
                session.start(env);

                let emitter = emitter.to_owned();
                let conn_clone = conn.clone();
                let pty_reg_clone = self.core.state.pty_registry.clone();
                let agent_name_for_events = shared_agent_name;
                tokio::spawn(async move {
                    let mut batch = Vec::with_capacity(16);
                    while let Ok(first_event) = rx_ui.recv().await {
                        batch.clear();
                        batch.push(first_event);
                        while batch.len() < 64 {
                            match rx_ui.try_recv() {
                                Ok(ev) => batch.push(ev),
                                Err(_) => break,
                            }
                        }

                        let mut notify_tasks = false;
                        let mut signal_msgs = Vec::with_capacity(batch.len());
                        let effective_sid = active_session_id.read().clone();
                        let current_agent_name = agent_name_for_events.read().clone();

                        for event in batch.drain(..) {
                            maybe_send_desktop_notification(
                                &conn_clone,
                                &current_agent_name,
                                &event,
                            )
                            .await;

                            if matches!(
                                event,
                                ClawEngineEvent::TaskStatusChanged { .. }
                                    | ClawEngineEvent::TaskCompleted { .. }
                            ) {
                                notify_tasks = true;
                            }

                            if let Ok(event_json) = serde_json::to_string(&event) {
                                let mut builder = zbus::Message::signal(
                                    emitter.path(),
                                    "dev.boxxy.BoxxyTerminal.Agent",
                                    "ClawEvent",
                                );
                                if let Some(sender) = conn_clone.unique_name() {
                                    builder = builder.sender(sender);
                                }
                                if let Some(dest) = emitter.destination() {
                                    builder = builder.destination(dest);
                                }
                                if let Ok(msg) = builder.build(&(&effective_sid, &event_json)) {
                                    signal_msgs.push(msg);
                                }
                            }
                        }

                        if notify_tasks {
                            emit_pending_tasks_changed(&conn_clone, &pty_reg_clone).await;
                        }

                        if !signal_msgs.is_empty() {
                            // Batch-send queued ClawEvent signals in a single vectored socket write (zbus 6.2+)
                            let _ = conn_clone.send_all(&signal_msgs).await;
                        }
                    }
                });

                Ok(claimed_session)
            }
            Err(e) => Err(e),
        }
    }

    async fn do_reattach_holder(
        &self,
        conn: &zbus::Connection,
        old_holder_id: &str,
        new_holder_id: String,
        owner_bus_name: String,
    ) -> Option<boxxy_claw_protocol::characters::ClaimedSession> {
        let mut resolved_old = old_holder_id.to_string();
        let mut claimed = self
            .core
            .registry
            .reattach_holder(&resolved_old, new_holder_id.clone(), owner_bus_name.clone())
            .await;

        // Fallback: if `old_holder_id` did not match directly (e.g. only a single
        // unowned detached claim exists in `CharacterRegistry`), rebind that detached
        // claim so the backgrounded agent session is never stranded.
        if claimed.is_none() {
            let unowned = self.core.registry.unowned_holder_ids().await;
            if unowned.len() == 1 {
                resolved_old = unowned[0].clone();
                claimed = self
                    .core
                    .registry
                    .reattach_holder(&resolved_old, new_holder_id.clone(), owner_bus_name)
                    .await;
            }
        }

        let claimed = claimed?;
        log::info!(
            "Reattached holder {} -> {} (session_id={})",
            resolved_old,
            new_holder_id,
            claimed.session_id
        );

        {
            let mut manager = self.session_manager.lock().await;
            if resolved_old != new_holder_id {
                manager.shutdown_pane(&new_holder_id);
            }
            manager.rebind_pane(&resolved_old, &new_holder_id);
        }

        emit_pending_tasks_changed(conn, &self.core.state.pty_registry).await;
        Some(claimed)
    }
}
