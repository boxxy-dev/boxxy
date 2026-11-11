//! Background D-Bus signal broadcasters (`ClaimsChanged`, `DetachedSessionsChanged`,
//! `PendingTasksChanged`, and `McpStatusChanged`).

use crate::daemon::DaemonCore;
use crate::ipc::session_manager::{ClawSessionManager, teardown_holder};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use zbus::Connection;

static LAST_PENDING_TASKS_EMPTY: AtomicBool = AtomicBool::new(false);

pub(crate) fn spawn_signal_broadcasters(
    conn: &Connection,
    core: &Arc<DaemonCore>,
    session_manager: &Arc<Mutex<ClawSessionManager>>,
) {
    // 1. Broadcast character registry changes (`ClaimsChanged`) to all D-Bus clients.
    let mut rx_registry = core.registry.subscribe();
    let reg_for_signals = core.registry.clone();
    let conn_for_signals = conn.clone();
    tokio::spawn(async move {
        loop {
            let snapshot = match rx_registry.recv().await {
                Ok(snapshot) => snapshot,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    log::warn!(
                        "ClaimsChanged broadcaster lagged by {skipped} updates; emitting latest snapshot"
                    );
                    reg_for_signals.snapshot().await
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if let Ok(snapshot_json) = serde_json::to_string(&snapshot) {
                let _ = conn_for_signals
                    .emit_signal(
                        Option::<zbus::names::BusName>::None,
                        "/dev/boxxy/Agent",
                        "dev.boxxy.BoxxyTerminal.Agent",
                        "ClaimsChanged",
                        &(snapshot_json,),
                    )
                    .await;
            }
        }
    });

    // 2. Broadcast detached PTY session changes (`DetachedSessionsChanged`) to all D-Bus
    // clients and garbage-collect unowned detached CharacterClaims whose backing PTY
    // has exited with no remaining scheduled tasks.
    let mut rx_pty = core.state.pty_registry.subscribe();
    let pty_reg_for_signals = core.state.pty_registry.clone();
    let reg_for_pty_gc = core.registry.clone();
    let mgr_for_pty_gc = session_manager.clone();
    let conn_for_pty_signals = conn.clone();
    tokio::spawn(async move {
        let mut last_detached_json: Option<String> = None;
        loop {
            match rx_pty.recv().await {
                Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
            // Coalesce burst notifications that queued up while querying /proc or emitting
            while matches!(
                rx_pty.try_recv(),
                Ok(()) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
            ) {}

            // Reap idle prompt-only detached shells before building the snapshot.
            crate::pty::reap_idle_detached_shells(&pty_reg_for_signals).await;

            // Garbage-collect any detached holder whose PTY is no longer running
            // and that has no pending tasks.
            let unowned_holders = reg_for_pty_gc.unowned_holder_ids().await;
            if !unowned_holders.is_empty() {
                let workspace = boxxy_claw::registry::workspace::global_workspace().await;
                for holder_id in unowned_holders {
                    let pty_alive = pty_reg_for_signals
                        .find_pid_by_pane_id(&holder_id)
                        .await
                        .is_some();
                    let has_tasks = workspace.get_pane_pending_task_count(&holder_id).await > 0;
                    if !pty_alive && !has_tasks {
                        log::info!(
                            "Reaping orphaned detached holder {} (PTY exited and no pending tasks)",
                            holder_id
                        );
                        let _ = teardown_holder(&mgr_for_pty_gc, &reg_for_pty_gc, &holder_id).await;
                    }
                }
            }

            let list = crate::pty::query_detached_sessions_detailed(&pty_reg_for_signals).await;
            if let Ok(json) = serde_json::to_string(&list)
                && last_detached_json.as_deref() != Some(json.as_str())
            {
                last_detached_json = Some(json.clone());
                let _ = conn_for_pty_signals
                    .emit_signal(
                        Option::<zbus::names::BusName>::None,
                        "/dev/boxxy/Agent",
                        "dev.boxxy.BoxxyTerminal.Agent",
                        "DetachedSessionsChanged",
                        &(json,),
                    )
                    .await;
            }
        }
    });

    // 3. Broadcast pending scheduled task changes (`PendingTasksChanged`) whenever
    // `WorkspaceRegistry` tasks are updated or rebound.
    let pty_reg_for_tasks = core.state.pty_registry.clone();
    let conn_for_tasks = conn.clone();
    tokio::spawn(async move {
        let workspace_for_tasks = boxxy_claw::registry::workspace::global_workspace().await;
        let mut rx_tasks = workspace_for_tasks.subscribe_tasks();
        loop {
            match rx_tasks.recv().await {
                Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
            while matches!(
                rx_tasks.try_recv(),
                Ok(()) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
            ) {}
            emit_pending_tasks_changed(&conn_for_tasks, &pty_reg_for_tasks).await;
        }
    });
}

pub(crate) async fn build_pending_tasks_snapshot(
    pty_registry: &crate::pty::registry::PtyRegistry,
) -> Vec<boxxy_claw_protocol::PendingTaskInfo> {
    let workspace = boxxy_claw::registry::workspace::global_workspace().await;
    let raw_tasks = workspace.get_all_pending_tasks_with_pane().await;
    if raw_tasks.is_empty() {
        return Vec::new();
    }

    let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
    let catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
    let now = chrono::Utc::now();

    let mut out = Vec::with_capacity(raw_tasks.len());
    for (pane_id, fallback_agent_name, task) in raw_tasks {
        let claim_opt = claims.iter().find(|c| c.holder_id == pane_id);
        let (character_id, session_id, claim_detached) = if let Some(claim) = claim_opt {
            (
                claim.character_id.clone(),
                claim.session_id.clone(),
                claim.owner_bus_name.is_empty(),
            )
        } else {
            (String::new(), String::new(), false)
        };

        let char_info = catalog.iter().find(|c| c.config.id == character_id);
        let raw_character_name = char_info
            .map(|c| c.config.display_name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback_agent_name);
        let character_name = crate::pty::prettify_character_name(&raw_character_name);
        let character_slug = char_info
            .map(|c| c.config.name.clone())
            .unwrap_or_default();

        let (pid, pty_detached) = match pty_registry.find_pid_by_pane_id(&pane_id).await {
            Some((p, det)) => (Some(p), det),
            None => (None, false),
        };

        let due_at_unix = task.due_at.timestamp();
        let remaining_secs = (task.due_at - now).num_seconds().max(0);

        let display_payload = if let Some(ref iv) = task.interval {
            format!(
                "{} ({} {}/{})",
                task.payload, iv.round_label, iv.current_round, iv.total_rounds
            )
        } else {
            task.payload
        };

        out.push(boxxy_claw_protocol::PendingTaskInfo {
            id: task.id,
            pane_id,
            session_id,
            character_id,
            character_name,
            character_slug,
            task_type: task.task_type,
            payload: display_payload,
            due_at_unix,
            remaining_secs,
            is_detached: pty_detached || claim_detached,
            pid,
        });
    }
    out.sort_by_key(|t| t.due_at_unix);
    out
}

pub(crate) async fn emit_pending_tasks_changed(
    conn: &Connection,
    pty_registry: &crate::pty::registry::PtyRegistry,
) {
    let tasks = build_pending_tasks_snapshot(pty_registry).await;
    let is_empty = tasks.is_empty();
    let was_empty = LAST_PENDING_TASKS_EMPTY.swap(is_empty, Ordering::AcqRel);
    // Avoid spamming redundant back-to-back empty `PendingTasksChanged("[]")` D-Bus
    // signals when panes without scheduled tasks close or detach.
    if !(is_empty && was_empty)
        && let Ok(json) = serde_json::to_string(&tasks)
    {
        let _ = conn
            .emit_signal(
                Option::<zbus::names::BusName>::None,
                "/dev/boxxy/Agent",
                "dev.boxxy.BoxxyTerminal.Agent",
                "PendingTasksChanged",
                &(json,),
            )
            .await;
    }
    pty_registry.notify_changed();
}

pub(crate) async fn emit_mcp_status_changed(conn: &Connection, statuses: &[(String, bool)]) {
    if let Ok(json) = serde_json::to_string(statuses) {
        let _ = conn
            .emit_signal(
                Option::<zbus::names::BusName>::None,
                "/dev/boxxy/Agent",
                "dev.boxxy.BoxxyTerminal.Agent",
                "McpStatusChanged",
                &(json,),
            )
            .await;
    }
}
