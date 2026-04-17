use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};

#[derive(Clone, Debug)]
pub struct PaneState {
    pub id: String,
    pub session_id: Option<String>,
    pub name: String,
    pub cwd: String,
    pub last_command: Option<String>,
    pub last_snapshot: Option<String>,
    pub status: Option<String>,
    pub context_quality: Option<crate::engine::ContextQuality>,
    pub tx: Option<async_channel::Sender<crate::engine::ClawMessage>>,
    pub active_skills: Vec<String>,
}

pub struct WorkspaceRegistry {
    // Map of pane_id -> PaneState
    panes: Arc<RwLock<HashMap<String, PaneState>>>,
    // Map of pane_id -> Vec<ScheduledTask>
    tasks: Arc<RwLock<HashMap<String, Vec<crate::engine::ScheduledTask>>>>,
    // Global shared intent/scratchpad for system-wide orchestration
    global_intent: Arc<RwLock<Option<String>>>,
    // Event subscriptions: subscriber_pane_id -> Vec<EventFilter>
    subscriptions: Arc<RwLock<HashMap<String, Vec<EventFilter>>>>,
    // Resource locks: resource_path -> holder_pane_id
    locks: Arc<RwLock<HashMap<String, String>>>,
}

#[derive(Debug, Clone)]
pub enum EventFilter {
    ProcessExited { pane_id: Option<String> },
    OutputMatch { pane_id: String, regex: String },
    Custom { source_agent: Option<String>, name: String },
}

static WORKSPACE: OnceCell<Arc<WorkspaceRegistry>> = OnceCell::const_new();

pub async fn global_workspace() -> Arc<WorkspaceRegistry> {
    WORKSPACE
        .get_or_init(|| async { Arc::new(WorkspaceRegistry::new()) })
        .await
        .clone()
}

impl Default for WorkspaceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self {
            panes: Arc::new(RwLock::new(HashMap::new())),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            global_intent: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn acquire_lock(&self, pane_id: String, resource: String) -> Result<(), String> {
        let mut locks = self.locks.write().await;
        if let Some(holder) = locks.get(&resource) {
            if holder == &pane_id {
                return Ok(());
            }
            let panes = self.panes.read().await;
            let holder_name = panes
                .get(holder)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Unknown Agent".to_string());
            return Err(format!("LOCKED by agent '{}'", holder_name));
        }
        locks.insert(resource, pane_id);
        Ok(())
    }

    pub async fn release_lock(&self, pane_id: String, resource: String) {
        let mut locks = self.locks.write().await;
        if let Some(holder) = locks.get(&resource) {
            if holder == &pane_id {
                locks.remove(&resource);
            }
        }
    }

    pub async fn release_all_locks(&self, pane_id: &str) {
        let mut locks = self.locks.write().await;
        locks.retain(|_, holder| holder != pane_id);
    }

    pub async fn subscribe(&self, subscriber_id: String, filter: EventFilter) {
        let mut subs = self.subscriptions.write().await;
        subs.entry(subscriber_id).or_default().push(filter);
    }

    pub async fn unsubscribe_all(&self, subscriber_id: &str) {
        let mut subs = self.subscriptions.write().await;
        subs.remove(subscriber_id);
    }

    pub async fn publish_event(&self, event: crate::engine::ClawEvent) {
        let subs = self.subscriptions.read().await;
        let panes = self.panes.read().await;

        for (subscriber_id, filters) in subs.iter() {
            let mut matched = false;
            for filter in filters {
                matched = match (&event, filter) {
                    (
                        crate::engine::ClawEvent::ProcessExited { pane_id, .. },
                        EventFilter::ProcessExited {
                            pane_id: filter_pane_id,
                        },
                    ) => filter_pane_id.is_none() || filter_pane_id.as_ref() == Some(pane_id),

                    (
                        crate::engine::ClawEvent::OutputMatch {
                            pane_id, regex, ..
                        },
                        EventFilter::OutputMatch {
                            pane_id: filter_pane_id,
                            regex: filter_regex,
                        },
                    ) => pane_id == filter_pane_id && regex == filter_regex,

                    (
                        crate::engine::ClawEvent::Custom {
                            source_agent, name, ..
                        },
                        EventFilter::Custom {
                            source_agent: filter_source,
                            name: filter_name,
                        },
                    ) => {
                        (filter_source.is_none() || filter_source.as_ref() == Some(source_agent))
                            && name == filter_name
                    }
                    _ => false,
                };
                if matched {
                    break;
                }
            }

            if matched {
                if let Some(pane) = panes.get(subscriber_id) {
                    if let Some(tx) = &pane.tx {
                        let _ = tx
                            .send(crate::engine::ClawMessage::SubscriptionEvent {
                                event: event.clone(),
                            })
                            .await;
                    }
                }
            }
        }
    }

    pub async fn update_pane_tasks(&self, id: String, tasks: Vec<crate::engine::ScheduledTask>) {
        let mut all_tasks = self.tasks.write().await;
        if tasks.is_empty() {
            all_tasks.remove(&id);
        } else {
            all_tasks.insert(id, tasks);
        }
    }

    pub async fn get_all_pending_tasks(&self) -> Vec<(String, crate::engine::ScheduledTask)> {
        let all_tasks = self.tasks.read().await;
        let mut pending = Vec::new();
        let panes = self.panes.read().await;

        for (pane_id, tasks) in all_tasks.iter() {
            let agent_name = panes
                .get(pane_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Unknown Agent".to_string());
            for task in tasks {
                if task.status == crate::engine::TaskStatus::Pending {
                    pending.push((agent_name.clone(), task.clone()));
                }
            }
        }
        pending
    }

    pub async fn register_pane_tx(
        &self,
        id: String,
        tx: async_channel::Sender<crate::engine::ClawMessage>,
    ) {
        let mut panes = self.panes.write().await;
        let entry = panes.entry(id.clone()).or_insert_with(|| PaneState {
            id,
            session_id: None,
            name: "Unknown Agent".to_string(),
            cwd: "/".to_string(),
            last_command: None,
            last_snapshot: None,
            status: None,
            context_quality: None,
            tx: None,
            active_skills: Vec::new(),
        });
        entry.tx = Some(tx);
    }

    pub async fn get_pane_tx_by_name(
        &self,
        name: &str,
    ) -> Option<async_channel::Sender<crate::engine::ClawMessage>> {
        let panes = self.panes.read().await;
        let name_lower = name.to_lowercase();
        panes
            .values()
            .find(|p| p.name.to_lowercase() == name_lower)
            .and_then(|p| p.tx.clone())
    }

    pub async fn get_pane_tx_by_id(
        &self,
        id: &str,
    ) -> Option<async_channel::Sender<crate::engine::ClawMessage>> {
        let panes = self.panes.read().await;
        panes.get(id).and_then(|p| p.tx.clone())
    }

    pub async fn update_pane_state(
        &self,
        id: String,
        session_id: Option<String>,
        name: Option<String>,
        cwd: String,
        last_command: Option<String>,
        snapshot: Option<String>,
    ) {
        let mut panes = self.panes.write().await;
        let entry = panes.entry(id.clone()).or_insert_with(|| PaneState {
            id,
            session_id: session_id.clone(),
            name: name.clone().unwrap_or_else(|| "Unknown Agent".to_string()),
            cwd: cwd.clone(),
            last_command: None,
            last_snapshot: None,
            status: None,
            context_quality: None,
            tx: None,
            active_skills: Vec::new(),
        });

        if let Some(s) = session_id {
            entry.session_id = Some(s);
        }

        if let Some(n) = name {
            entry.name = n;
        }

        entry.cwd = cwd;
        if last_command.is_some() {
            entry.last_command = last_command;
        }
        if snapshot.is_some() {
            entry.last_snapshot = snapshot;
        }
    }

    pub async fn update_pane_session(&self, id: String, session_id: String) {
        let mut panes = self.panes.write().await;
        if let Some(pane) = panes.get_mut(&id) {
            pane.session_id = Some(session_id);
        }
    }

    pub async fn update_pane_skills(&self, id: String, skills: Vec<String>) {
        let mut panes = self.panes.write().await;
        if let Some(pane) = panes.get_mut(&id) {
            pane.active_skills = skills;
        }
    }

    pub async fn find_agent_by_skill(&self, skill: &str) -> Vec<String> {
        let panes = self.panes.read().await;
        let skill_lower = skill.to_lowercase();
        panes
            .values()
            .filter(|p| {
                p.active_skills
                    .iter()
                    .any(|s| s.to_lowercase().contains(&skill_lower))
            })
            .map(|p| p.name.clone())
            .collect()
    }

    pub async fn evict_session(&self, session_id: &str) {
        let panes = self.panes.read().await;
        let target_tx = panes.values().find_map(|p| {
            if p.session_id.as_deref() == Some(session_id) {
                p.tx.clone()
            } else {
                None
            }
        });
        drop(panes);

        if let Some(tx) = target_tx {
            let _ = tx.send(crate::engine::ClawMessage::Evict).await;
        }
    }

    pub async fn unregister_pane(&self, id: String) {
        let mut panes = self.panes.write().await;
        panes.remove(&id);
        drop(panes);
        
        self.release_all_locks(&id).await;
        self.unsubscribe_all(&id).await;
    }

    pub async fn set_pane_status(&self, id: String, status: Option<String>) {
        let mut panes = self.panes.write().await;
        if let Some(pane) = panes.get_mut(&id) {
            pane.status = status;
        }
    }

    pub async fn set_pane_quality(&self, id: String, quality: Option<crate::engine::ContextQuality>) {
        let mut panes = self.panes.write().await;
        if let Some(pane) = panes.get_mut(&id) {
            pane.context_quality = quality;
        }
    }

    pub async fn get_pane_snapshot(&self, id: String) -> Option<String> {
        let panes = self.panes.read().await;
        panes.get(&id).and_then(|p| p.last_snapshot.clone())
    }

    pub async fn get_pane_cwd(&self, id: String) -> Option<String> {
        let panes = self.panes.read().await;
        panes.get(&id).map(|p| p.cwd.clone())
    }

    pub async fn resolve_pane_id_by_name(&self, name: &str) -> Option<String> {
        let panes = self.panes.read().await;
        let name_lower = name.to_lowercase();
        panes
            .values()
            .find(|p| p.name.to_lowercase() == name_lower)
            .map(|p| p.id.clone())
    }

    pub async fn get_global_radar(&self, current_pane_id: String) -> String {
        let panes = self.panes.read().await;
        let mut radar = String::new();

        let peers: Vec<_> = panes.values().filter(|p| p.id != current_pane_id).collect();

        if !peers.is_empty() {
            radar.push_str("\n--- GLOBAL RADAR (Other Active Agents) ---\n");
            radar.push_str(
                "You can read these panes using `read_pane_buffer(agent_name)` or delegate tasks using `delegate_task(agent_name, prompt)`.\n",
            );
            
            let mut sorted_peers = peers.clone();
            // Simple load balancing signal sort:
            // 1. Waiting + Full Context (Best)
            // 2. Sleep + Full Context
            // 3. Waiting + Degraded
            // 4. Others (Working, Locking, Faulted)
            sorted_peers.sort_by_key(|p| {
                let is_waiting = p.status.as_deref() == Some("Waiting");
                let is_sleep = p.status.as_deref() == Some("Sleep");
                let is_full = matches!(p.context_quality, Some(crate::engine::ContextQuality::Full));
                
                if is_waiting && is_full { 1 }
                else if is_sleep && is_full { 2 }
                else if is_waiting && !is_full { 3 }
                else if is_sleep && !is_full { 4 }
                else { 5 }
            });

            for peer in sorted_peers {
                let cmd = peer.last_command.as_deref().unwrap_or("idle");
                let status = peer
                    .status
                    .as_deref()
                    .unwrap_or("Unknown");
                let quality = match peer.context_quality {
                    Some(crate::engine::ContextQuality::Full) => "Full Context",
                    Some(crate::engine::ContextQuality::Degraded) => "Degraded Context",
                    None => "Unknown Context",
                };
                
                radar.push_str(&format!(
                    "- Agent '{}' (ID: {}): in {} | Last command `{}` | Status: {} [{}]\n",
                    peer.name, peer.id, peer.cwd, cmd, status, quality
                ));
            }
        }

        // Add global shared intent/scratchpad
        let global_intent = self.global_intent.read().await;
        if let Some(intent) = &*global_intent {
            radar.push_str("\n--- GLOBAL WORKSPACE INTENT ---\n");
            radar.push_str(intent);
            radar.push('\n');
        }

        radar
    }

    pub async fn get_all_agents(&self) -> Vec<crate::engine::tools::workspace::AgentInfo> {
        let panes = self.panes.read().await;
        panes
            .values()
            .map(|p| crate::engine::tools::workspace::AgentInfo {
                name: p.name.clone(),
                id: p.id.clone(),
                cwd: p.cwd.clone(),
                last_command: p.last_command.clone().unwrap_or_else(|| "idle".to_string()),
                status: p.status.clone().unwrap_or_else(|| "active".to_string()),
            })
            .collect()
    }

    pub async fn set_global_intent(&self, intent: String) {
        let mut global_intent = self.global_intent.write().await;
        *global_intent = Some(intent);
    }
}
