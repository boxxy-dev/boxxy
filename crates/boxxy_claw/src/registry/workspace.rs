use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, OnceCell};

#[derive(Clone, Debug)]
pub struct PaneState {
    pub id: String,
    pub cwd: String,
    pub last_command: Option<String>,
    pub last_snapshot: Option<String>,
    pub status: Option<String>,
}

pub struct WorkspaceRegistry {
    // Map of pane_id -> PaneState
    panes: Arc<RwLock<HashMap<String, PaneState>>>,
    // Map of project_path -> workspace_intent
    intents: Arc<RwLock<HashMap<PathBuf, String>>>,
}

static WORKSPACE: OnceCell<Arc<WorkspaceRegistry>> = OnceCell::const_new();

pub async fn global_workspace() -> Arc<WorkspaceRegistry> {
    WORKSPACE.get_or_init(|| async {
        Arc::new(WorkspaceRegistry::new())
    }).await.clone()
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self {
            panes: Arc::new(RwLock::new(HashMap::new())),
            intents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn update_pane_state(&self, id: String, cwd: String, last_command: Option<String>, snapshot: Option<String>) {
        let mut panes = self.panes.write().await;
        let entry = panes.entry(id.clone()).or_insert_with(|| PaneState {
            id,
            cwd: cwd.clone(),
            last_command: None,
            last_snapshot: None,
            status: None,
        });
        entry.cwd = cwd;
        if last_command.is_some() {
            entry.last_command = last_command;
        }
        if snapshot.is_some() {
            entry.last_snapshot = snapshot;
        }
    }

    pub async fn unregister_pane(&self, id: String) {
        let mut panes = self.panes.write().await;
        panes.remove(&id);
    }

    pub async fn set_pane_status(&self, id: String, status: Option<String>) {
        let mut panes = self.panes.write().await;
        if let Some(pane) = panes.get_mut(&id) {
            pane.status = status;
        }
    }

    pub async fn get_pane_snapshot(&self, id: String) -> Option<String> {
        let panes = self.panes.read().await;
        panes.get(&id).and_then(|p| p.last_snapshot.clone())
    }

    pub async fn get_radar_for_project(&self, project_path: &str, current_pane_id: String) -> String {
        let panes = self.panes.read().await;
        let mut radar = String::new();
        
        let peers: Vec<_> = panes.values()
            .filter(|p| p.id != current_pane_id && p.cwd == project_path)
            .collect();

        if !peers.is_empty() {
            radar.push_str("\n--- WORKSPACE RADAR (Peer Panes in this project) ---\n");
            radar.push_str("You can read these panes using `read_pane_buffer(id)` if needed.\n");
            for peer in peers {
                let cmd = peer.last_command.as_deref().unwrap_or("idle");
                let status = peer.status.as_deref().map(|s| format!(" | Status: {}", s)).unwrap_or_default();
                radar.push_str(&format!("- Pane {}: Last command `{}`{}\n", peer.id, cmd, status));
            }
        }

        // Add shared intents/scratchpad
        let intents = self.intents.read().await;
        let path = PathBuf::from(project_path);
        if let Some(intent) = intents.get(&path) {
            radar.push_str("\n--- SHARED WORKSPACE INTENT ---\n");
            radar.push_str(intent);
            radar.push('\n');
        }

        radar
    }

    pub async fn set_project_intent(&self, project_path: &str, intent: String) {
        let mut intents = self.intents.write().await;
        intents.insert(PathBuf::from(project_path), intent);
    }
}
