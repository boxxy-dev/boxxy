use crate::claw::CharacterRegistry;
use crate::ipc::ClawSessionManager;
use crate::pty::registry::PtyRegistry;
use boxxy_claw::registry::workspace::WorkspaceRegistry;
use futures_util::StreamExt;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell, watch};
use zbus::Connection;

static ACTIVE_CLIENTS: OnceCell<Mutex<HashSet<String>>> = OnceCell::const_new();

async fn get_active_clients() -> &'static Mutex<HashSet<String>> {
    ACTIVE_CLIENTS
        .get_or_init(|| async { Mutex::new(HashSet::new()) })
        .await
}

pub async fn register_client(bus_name: String) {
    if !bus_name.is_empty() {
        let clients_mutex = get_active_clients().await;
        let mut clients = clients_mutex.lock().await;
        clients.insert(bus_name);
    }
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

            // Check if it was a registered UI client
            let was_client = {
                let clients_mutex = get_active_clients().await;
                let mut clients = clients_mutex.lock().await;
                clients.remove(bus_name)
            };

            if was_client {
                let current = *client_count_tx.borrow();
                let new_count = current.saturating_sub(1);
                let _ = client_count_tx.send(new_count);
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
                if claim.holder_kind == boxxy_claw_protocol::characters::HolderKind::Pane {
                    log::debug!("Cascading swarm cleanup for pane {}", claim.holder_id);
                    workspace.release_all_locks(&claim.holder_id).await;
                    workspace.unregister_pane(claim.holder_id.clone()).await;
                    workspace
                        .update_pane_tasks(claim.holder_id, Vec::new())
                        .await;
                }
            }
            crate::ipc::emit_pending_tasks_changed(&conn_clone, &pty_registry).await;
        }
    });

    Ok(())
}
