//! `ClawSessionManager` actor table and unified pane/holder lifecycle teardown.

use crate::claw::CharacterRegistry;
use boxxy_claw::registry::workspace::WorkspaceRegistry;
use boxxy_claw_protocol::ClawMessage;
use boxxy_claw_protocol::characters::{CharacterClaim, HolderKind};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Bookkeeping for one hosted `ClawSession`. Keeps the pane ID and agent
/// name so the UI can reattach by querying `list_claw_sessions()`.
#[derive(Clone)]
pub(crate) struct HostedSession {
    pub(crate) pane_id: String,
    pub(crate) agent_name: String,
    pub(crate) active_session_id: Arc<parking_lot::RwLock<String>>,
    pub(crate) tx: async_channel::Sender<ClawMessage>,
}

pub struct ClawSessionManager {
    pub(crate) sessions: HashMap<String, HostedSession>,
    pub(crate) env: Arc<crate::claw::ClawSubsystem>,
}

impl ClawSessionManager {
    pub(crate) fn new(env: Arc<crate::claw::ClawSubsystem>) -> Self {
        Self {
            sessions: HashMap::new(),
            env,
        }
    }

    /// Drops any sessions whose sender is closed — the actor has exited on
    /// its own, or the channel was dropped. Keeps the map from growing
    /// unboundedly over the daemon's lifetime.
    pub(crate) fn prune_dead(&mut self) {
        self.sessions.retain(|_, s| !s.tx.is_closed());
    }

    /// Shuts down all hosted `ClawSession` actors belonging to `pane_id`.
    /// Sends `ClawMessage::Deactivate` (aborting any in-flight turn, draining
    /// pending approvals, and flushing unflushed history to SQLite) and closes
    /// the channel so the actor loop and D-Bus forwarder task exit cleanly.
    pub fn shutdown_pane(&mut self, pane_id: &str) {
        self.prune_dead();
        let to_close: Vec<HostedSession> = self
            .sessions
            .extract_if(|_, s| s.pane_id == pane_id)
            .map(|(_, s)| s)
            .collect();
        for session in to_close {
            if !session.tx.is_closed() {
                let _ = session.tx.try_send(ClawMessage::Deactivate);
                session.tx.close();
            }
        }
    }

    /// Shuts down all active `ClawSession` actors when the daemon stops.
    pub fn shutdown_all(&mut self) {
        self.prune_dead();
        for (_, session) in self.sessions.drain() {
            if !session.tx.is_closed() {
                let _ = session.tx.try_send(ClawMessage::Deactivate);
                session.tx.close();
            }
        }
    }

    pub fn rebind_pane(&mut self, old_pane_id: &str, new_pane_id: &str) {
        self.prune_dead();
        let mut notified = false;
        for s in self.sessions.values_mut() {
            if s.pane_id == old_pane_id {
                s.pane_id = new_pane_id.to_string();
                if !notified {
                    notified = true;
                    let _ = s.tx.try_send(ClawMessage::RebindPane {
                        new_pane_id: new_pane_id.to_string(),
                    });
                }
            }
        }
    }
}

/// Cascades swarm lock release, pane unregistration, and scheduled task cleanup
/// in `WorkspaceRegistry` when a `HolderKind::Pane` claim is released.
pub(crate) async fn cascade_workspace_cleanup(
    workspace: &WorkspaceRegistry,
    claim: &CharacterClaim,
) {
    if claim.holder_kind == HolderKind::Pane {
        workspace.release_all_locks(&claim.holder_id).await;
        workspace.unregister_pane(claim.holder_id.clone()).await;
        workspace
            .update_pane_tasks(claim.holder_id.clone(), Vec::new())
            .await;
    }
}

/// Unified teardown coordinator shared by `AgentInterface::release_holder`,
/// `terminate_detached_session`, and the detached-PTY garbage collector:
/// shuts down any hosted `ClawSession` actors for `holder_id`, releases its
/// `CharacterClaim`, and cascades `WorkspaceRegistry` cleanup.
pub(crate) async fn teardown_holder(
    session_manager: &Mutex<ClawSessionManager>,
    registry: &CharacterRegistry,
    holder_id: &str,
) -> Option<CharacterClaim> {
    session_manager.lock().await.shutdown_pane(holder_id);
    let claim = registry.release_holder(holder_id).await?;
    let workspace = boxxy_claw::registry::workspace::global_workspace().await;
    cascade_workspace_cleanup(&workspace, &claim).await;
    Some(claim)
}
