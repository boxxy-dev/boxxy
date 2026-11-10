use crate::claw::CharacterRegistry;
use crate::ipc::ClawSessionManager;
use crate::ipc::session_manager::cascade_workspace_cleanup;
use crate::pty::registry::PtyRegistry;
use boxxy_claw::registry::workspace::WorkspaceRegistry;
use futures_util::StreamExt;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, watch};
use zbus::Connection;

static ACTIVE_CLIENTS: OnceLock<parking_lot::Mutex<HashSet<String>>> = OnceLock::new();

fn active_clients() -> &'static parking_lot::Mutex<HashSet<String>> {
    ACTIVE_CLIENTS.get_or_init(|| parking_lot::Mutex::new(HashSet::new()))
}

/// Atomically registers `bus_name` in `ACTIVE_CLIENTS` and synchronizes `client_count_tx`
/// under the same lock so duplicate `claim_startup_token` calls or concurrent client
/// launches/exits can never drift `client_count` away from `ACTIVE_CLIENTS.len()`.
///
/// Returns `(was_empty_before_connect, new_client_count)`.
pub fn register_client_and_sync_count(
    bus_name: String,
    client_count_tx: &watch::Sender<usize>,
) -> (bool, usize) {
    let mut clients = active_clients().lock();
    let was_empty = clients.is_empty();
    if !bus_name.is_empty() {
        clients.insert(bus_name);
    }
    let new_count = clients.len();
    client_count_tx.send_if_modified(|curr| {
        if *curr != new_count {
            *curr = new_count;
            true
        } else {
            false
        }
    });
    (was_empty, new_count)
}

/// Atomically removes `bus_name` from `ACTIVE_CLIENTS` and updates `client_count_tx`
/// under the same lock. Returns `Some(new_client_count)` if `bus_name` was registered.
pub fn unregister_client_and_sync_count(
    bus_name: &str,
    client_count_tx: &watch::Sender<usize>,
) -> Option<usize> {
    let mut clients = active_clients().lock();
    if !clients.remove(bus_name) {
        return None;
    }
    let new_count = clients.len();
    client_count_tx.send_if_modified(|curr| {
        if *curr != new_count {
            *curr = new_count;
            true
        } else {
            false
        }
    });
    Some(new_count)
}

pub async fn spawn_owner_tracker(
    conn: &Connection,
    registry: Arc<CharacterRegistry>,
    pty_registry: PtyRegistry,
    session_manager: Arc<Mutex<ClawSessionManager>>,
    workspace: Arc<WorkspaceRegistry>,
    client_count_tx: watch::Sender<usize>,
) -> anyhow::Result<()> {
    let proxy = zbus::fdo::DBusProxy::new(conn).await?;
    // Filter `arg2=''` (`new_owner == ""`) at the D-Bus broker level via MatchRule so the
    // daemon is only woken when a bus name is released/disconnected, not on every new connect.
    let mut stream = proxy.receive_name_owner_changed_with_args(&[(2, "")]).await?;
    let conn_clone = conn.clone();

    tokio::spawn(async move {
        while let Some(sig) = stream.next().await {
            let Ok(args) = sig.args() else { continue };

            // Only unique names being released matter to us. Unique names start with ':'.
            if !args.name.as_str().starts_with(':') {
                continue;
            }
            // If new_owner is empty, the name was released (client disconnected/crashed).
            if args.new_owner.is_some() {
                continue;
            }

            let bus_name = args.name.as_str();

            if let Some(new_count) =
                unregister_client_and_sync_count(bus_name, &client_count_tx)
            {
                log::info!(
                    "Client {} disconnected. Total clients: {}",
                    bus_name,
                    new_count
                );
                // Flush telemetry immediately when a user quits the app so sessions
                // shorter than the 30-minute periodic interval are not lost.
                tokio::spawn(async {
                    boxxy_telemetry::flush_journal().await;
                });
            }

            // Automatically detach (or SIGTERM if persistence is off) any PTY sessions
            // still attached to this disconnected client. This guarantees crash-resilient
            // PTY persistence even if the UI exited before async Drop futures ran.
            let pty_outcomes = pty_registry.release_owner(bus_name).await;
            let mut detached_pane_ids = HashSet::new();
            for (pid, outcome) in pty_outcomes {
                log::info!(
                    "Client {} disconnect released PTY pid={} -> {:?}",
                    bus_name,
                    pid,
                    outcome
                );
                if outcome == crate::pty::registry::DetachOutcome::Detached
                    && let Some(orig_id) = pty_registry.get_original_pane_id(pid).await
                {
                    detached_pane_ids.insert(orig_id);
                }
            }

            // Fast path: skip lock acquisition if we don't own anything.
            if !registry.has_owner(bus_name).await {
                if !detached_pane_ids.is_empty() {
                    crate::ipc::emit_pending_tasks_changed(&conn_clone, &pty_registry).await;
                }
                continue;
            }

            // Preserve claims whose panes were detached into the background or have
            // pending AI reminders/tasks so their ClawSession actors keep running.
            let snapshot = registry.snapshot().await;
            for claim in snapshot.claims.iter().filter(|c| c.owner_bus_name == bus_name) {
                let has_tasks = workspace.get_pane_pending_task_count(&claim.holder_id).await > 0;
                if detached_pane_ids.contains(&claim.holder_id) || has_tasks {
                    log::info!(
                        "Preserving detached ClawSession for pane {} on client {} disconnect",
                        claim.holder_id,
                        bus_name
                    );
                    registry.detach_holder(&claim.holder_id).await;
                }
            }

            log::info!("Client {} releasing remaining owned claims", bus_name);
            let released_claims = registry.release_owner(bus_name).await;

            // Cascade swarm and session actor cleanup for each released pane claim.
            {
                let mut mgr = session_manager.lock().await;
                for claim in &released_claims {
                    if claim.holder_kind == boxxy_claw_protocol::characters::HolderKind::Pane {
                        mgr.shutdown_pane(&claim.holder_id);
                    }
                }
            }
            for claim in released_claims {
                log::debug!("Cascading swarm cleanup for pane {}", claim.holder_id);
                cascade_workspace_cleanup(&workspace, &claim).await;
            }
            crate::ipc::emit_pending_tasks_changed(&conn_clone, &pty_registry).await;
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_client_registration_is_idempotent_and_recovers_ghost_mode() {
        // Use synthetic test-only bus names that cannot collide with real `:1.*` D-Bus names
        let (tx, rx) = watch::channel(99usize);
        let name_a = format!(":test.{}.a", uuid::Uuid::new_v4());
        let name_b = format!(":test.{}.b", uuid::Uuid::new_v4());

        let (_was_empty, c1) = register_client_and_sync_count(name_a.clone(), &tx);
        // Calling register a second time with the same bus_name must NOT double-increment
        let (_was_empty2, c2) = register_client_and_sync_count(name_a.clone(), &tx);
        assert_eq!(c1, c2);
        assert_eq!(*rx.borrow(), c2);

        let (_, c3) = register_client_and_sync_count(name_b.clone(), &tx);
        assert_eq!(c3, c1 + 1);

        assert_eq!(unregister_client_and_sync_count(&name_a, &tx), Some(c1));
        // Second unregister of same disconnected bus_name is a no-op
        assert_eq!(unregister_client_and_sync_count(&name_a, &tx), None);
        assert_eq!(
            unregister_client_and_sync_count(&name_b, &tx),
            Some(c1.saturating_sub(1))
        );
    }
}
