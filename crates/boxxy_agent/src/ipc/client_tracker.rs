use crate::claw::CharacterRegistry;
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
    workspace: Arc<WorkspaceRegistry>,
    client_count_tx: watch::Sender<usize>,
) -> anyhow::Result<()> {
    let proxy = zbus::fdo::DBusProxy::new(conn).await?;
    let mut stream = proxy.receive_name_owner_changed().await?;

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

            // Check if it was a registered UI client
            let was_client = {
                let clients_mutex = get_active_clients().await;
                let mut clients = clients_mutex.lock().await;
                clients.remove(args.name.as_str())
            };

            if was_client {
                let current = *client_count_tx.borrow();
                let new_count = current.saturating_sub(1);
                let _ = client_count_tx.send(new_count);
                log::info!(
                    "Client {} disconnected. Total clients: {}",
                    args.name,
                    new_count
                );
            }

            // Fast path: skip lock acquisition if we don't own anything.
            if !registry.has_owner(args.name.as_str()).await {
                continue;
            }

            log::info!("Client {} releasing all owned claims", args.name);
            let released_claims = registry.release_owner(args.name.as_str()).await;

            // Cascade swarm cleanup for each released pane claim.
            for claim in released_claims {
                if claim.holder_kind == boxxy_claw_protocol::characters::HolderKind::Pane {
                    log::debug!("Cascading swarm cleanup for pane {}", claim.holder_id);
                    workspace.release_all_locks(&claim.holder_id).await;
                    workspace.unregister_pane(claim.holder_id).await;
                }
            }
        }
    });

    Ok(())
}
