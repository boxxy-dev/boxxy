use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};

#[derive(Clone, Debug)]
pub struct PaneState {
    pub id: String,
    pub alias: String,
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

type PendingCommandWaiter = (
    Option<String>,
    tokio::sync::oneshot::Sender<Result<String, String>>,
);

pub struct WorkspaceRegistry {
    // Map of pane_id -> PaneState
    panes: Arc<RwLock<HashMap<String, PaneState>>>,
    next_alias_index: std::sync::atomic::AtomicU64,
    // Map of pane_id -> Vec<ScheduledTask>
    tasks: Arc<RwLock<HashMap<String, Vec<crate::engine::ScheduledTask>>>>,
    tasks_changed_tx: tokio::sync::broadcast::Sender<()>,
    // Global shared intent/scratchpad for system-wide orchestration
    global_intent: Arc<RwLock<Option<String>>>,
    // Event subscriptions: subscriber_pane_id -> Vec<EventFilter>
    subscriptions: Arc<RwLock<HashMap<String, Vec<EventFilter>>>>,
    // Resource locks: resource_path -> holder_pane_id
    locks: Arc<RwLock<HashMap<String, String>>>,
    // Pending cross-pane terminal_exec waiters: target_pane_id -> (source_pane_id, reply_tx)
    pending_commands: Arc<RwLock<HashMap<String, PendingCommandWaiter>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventFilter {
    ProcessExited {
        pane_id: Option<String>,
    },
    OutputMatch {
        pane_id: String,
        regex: String,
    },
    Custom {
        source_agent: Option<String>,
        name: String,
    },
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
        let (tasks_changed_tx, _) = tokio::sync::broadcast::channel(32);
        Self {
            panes: Arc::new(RwLock::new(HashMap::new())),
            next_alias_index: std::sync::atomic::AtomicU64::new(1),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            tasks_changed_tx,
            global_intent: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            locks: Arc::new(RwLock::new(HashMap::new())),
            pending_commands: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn mint_alias(&self, existing: &HashMap<String, PaneState>) -> String {
        for idx in 1..10_000u64 {
            let candidate = format!("pane-{idx}");
            if !existing
                .values()
                .any(|p| p.alias.eq_ignore_ascii_case(&candidate))
            {
                let _ = self.next_alias_index.fetch_max(
                    idx + 1,
                    std::sync::atomic::Ordering::Relaxed,
                );
                return candidate;
            }
        }
        let n = self
            .next_alias_index
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("pane-{n}")
    }

    pub fn subscribe_tasks(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.tasks_changed_tx.subscribe()
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

    pub async fn subscribe(&self, subscriber_id: String, filter: EventFilter) -> bool {
        let mut subs = self.subscriptions.write().await;
        let list = subs.entry(subscriber_id).or_default();
        if list.contains(&filter) {
            return false;
        }
        list.push(filter);
        true
    }

    pub async fn unsubscribe_all(&self, subscriber_id: &str) {
        let mut subs = self.subscriptions.write().await;
        subs.remove(subscriber_id);
    }

    pub async fn publish_event(&self, event: crate::engine::ClawEvent) {
        let mut subs = self.subscriptions.write().await;
        let panes = self.panes.read().await;
        let mut to_notify = Vec::new();

        for (subscriber_id, filters) in subs.iter_mut() {
            let mut matched = false;
            filters.retain(|filter| {
                let m = match (&event, filter) {
                    (
                        crate::engine::ClawEvent::ProcessExited { pane_id, .. },
                        EventFilter::ProcessExited {
                            pane_id: filter_pane_id,
                        },
                    ) => filter_pane_id.as_ref().is_none_or(|id| id == pane_id),

                    (
                        crate::engine::ClawEvent::OutputMatch { pane_id, regex, .. },
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
                        filter_source
                            .as_ref()
                            .is_none_or(|source| source == source_agent)
                            && name == filter_name
                    }
                    _ => false,
                };
                if m {
                    matched = true;
                }
                !m
            });

            if matched
                && let Some(pane) = panes.get(subscriber_id)
                && let Some(tx) = &pane.tx
            {
                to_notify.push(tx.clone());
            }
        }
        subs.retain(|_, list| !list.is_empty());
        drop(panes);
        drop(subs);

        for tx in to_notify {
            let _ = tx
                .send(crate::engine::ClawMessage::SubscriptionEvent {
                    event: event.clone(),
                })
                .await;
        }
    }

    pub async fn update_pane_tasks(&self, id: String, tasks: Vec<crate::engine::ScheduledTask>) {
        {
            let mut all_tasks = self.tasks.write().await;
            if tasks.is_empty() {
                all_tasks.remove(&id);
            } else {
                all_tasks.insert(id, tasks);
            }
        }
        let _ = self.tasks_changed_tx.send(());
    }

    pub async fn get_pane_pending_tasks(
        &self,
        pane_id: &str,
    ) -> Vec<crate::engine::ScheduledTask> {
        let all_tasks = self.tasks.read().await;
        all_tasks
            .get(pane_id)
            .map(|list| {
                list.iter()
                    .filter(|t| t.status == crate::engine::TaskStatus::Pending)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn get_pane_pending_task_count(&self, pane_id: &str) -> u32 {
        let all_tasks = self.tasks.read().await;
        all_tasks
            .get(pane_id)
            .map(|list| {
                list.iter()
                    .filter(|t| t.status == crate::engine::TaskStatus::Pending)
                    .count() as u32
            })
            .unwrap_or(0)
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

    pub async fn get_all_pending_tasks_with_pane(
        &self,
    ) -> Vec<(String, String, crate::engine::ScheduledTask)> {
        let all_tasks = self.tasks.read().await;
        let panes = self.panes.read().await;
        let mut pending = Vec::new();

        for (pane_id, tasks) in all_tasks.iter() {
            let agent_name = panes
                .get(pane_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Unknown Agent".to_string());
            for task in tasks {
                if task.status == crate::engine::TaskStatus::Pending {
                    pending.push((pane_id.clone(), agent_name.clone(), task.clone()));
                }
            }
        }
        pending
    }

    pub async fn rebind_pane(&self, old_id: &str, new_id: String) {
        if old_id == new_id {
            return;
        }
        {
            let mut panes = self.panes.write().await;
            if let Some(mut state) = panes.remove(old_id) {
                state.id = new_id.clone();
                panes.insert(new_id.clone(), state);
            }
        }
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task_list) = tasks.remove(old_id) {
                tasks.insert(new_id.clone(), task_list);
            }
        }
        let _ = self.tasks_changed_tx.send(());
        {
            let mut subs = self.subscriptions.write().await;
            if let Some(sub_list) = subs.remove(old_id) {
                subs.insert(new_id.clone(), sub_list);
            }
        }
        {
            let mut locks = self.locks.write().await;
            for holder in locks.values_mut() {
                if holder == old_id {
                    *holder = new_id.clone();
                }
            }
        }
        {
            let mut pending = self.pending_commands.write().await;
            if let Some(waiter) = pending.remove(old_id) {
                pending.insert(new_id, waiter);
            }
        }
    }

    pub async fn register_pending_command(
        &self,
        target_pane_id: String,
        source_pane_id: Option<String>,
        reply_tx: tokio::sync::oneshot::Sender<Result<String, String>>,
    ) {
        let mut pending = self.pending_commands.write().await;
        pending.insert(target_pane_id, (source_pane_id, reply_tx));
    }

    pub async fn complete_pending_command(
        &self,
        target_pane_id: &str,
        exit_code: i32,
        snapshot: String,
    ) -> bool {
        let waiter = self.pending_commands.write().await.remove(target_pane_id);
        if let Some((_source_pane, reply_tx)) = waiter {
            if exit_code == 0 {
                let _ = reply_tx.send(Ok(snapshot));
            } else {
                let _ = reply_tx.send(Err(format!(
                    "Command failed with exit code {exit_code}:\n{snapshot}"
                )));
            }
            true
        } else {
            false
        }
    }

    pub async fn drain_pending_commands_for_source(
        &self,
        source_pane_id: &str,
        err_msg: String,
    ) -> bool {
        let mut pending = self.pending_commands.write().await;
        let matching_targets: Vec<String> = pending
            .iter()
            .filter_map(|(target_id, (src_opt, _))| {
                if src_opt.as_deref() == Some(source_pane_id) {
                    Some(target_id.clone())
                } else {
                    None
                }
            })
            .collect();
        let mut fulfilled = false;
        for target_id in matching_targets {
            if let Some((_, reply_tx)) = pending.remove(&target_id) {
                let _ = reply_tx.send(Err(err_msg.clone()));
                fulfilled = true;
            }
        }
        fulfilled
    }

    pub async fn has_pending_command(&self, target_pane_id: &str) -> bool {
        self.pending_commands
            .read()
            .await
            .contains_key(target_pane_id)
    }

    pub async fn get_pane_status(&self, id: &str) -> Option<String> {
        let panes = self.panes.read().await;
        panes.get(id).and_then(|p| p.status.clone())
    }

    pub async fn register_pane_tx(
        &self,
        id: String,
        tx: async_channel::Sender<crate::engine::ClawMessage>,
    ) {
        let mut panes = self.panes.write().await;
        let alias = if let Some(existing) = panes.get(&id) {
            existing.alias.clone()
        } else {
            self.mint_alias(&panes)
        };
        let entry = panes.entry(id.clone()).or_insert_with(|| PaneState {
            id,
            alias,
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

    pub async fn get_pane_alias(&self, id: &str) -> Option<String> {
        let panes = self.panes.read().await;
        panes.get(id).map(|p| p.alias.clone())
    }

    pub async fn get_pane_tx_by_name(
        &self,
        name: &str,
    ) -> Option<async_channel::Sender<crate::engine::ClawMessage>> {
        let id = self.resolve_pane_id_by_name(name).await?;
        self.get_pane_tx_by_id(&id).await
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
    ) -> String {
        let mut panes = self.panes.write().await;
        let alias = if let Some(existing) = panes.get(&id) {
            existing.alias.clone()
        } else {
            self.mint_alias(&panes)
        };
        let default_name = alias.clone();
        let entry = panes.entry(id.clone()).or_insert_with(|| PaneState {
            id,
            alias: alias.clone(),
            session_id: session_id.clone(),
            name: name.clone().unwrap_or(default_name),
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

        if let Some(n) = name
            && !n.is_empty()
        {
            entry.name = n;
        }

        if !cwd.is_empty() {
            entry.cwd = cwd;
        }
        if last_command.is_some() {
            entry.last_command = last_command;
        }
        if snapshot.is_some() {
            entry.last_snapshot = snapshot;
        }
        entry.alias.clone()
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

    pub async fn set_pane_quality(
        &self,
        id: String,
        quality: Option<crate::engine::ContextQuality>,
    ) {
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
        let trimmed = name.trim().trim_start_matches('@').trim();
        if trimmed.is_empty() {
            return None;
        }
        let query_norm = normalize_agent_key(trimmed);
        let panes = self.panes.read().await;

        // 1. Exact ID, alias, or case-insensitive name match
        if let Some(pane) = panes.values().find(|p| {
            p.id.eq_ignore_ascii_case(trimmed)
                || p.alias.eq_ignore_ascii_case(trimmed)
                || p.name.eq_ignore_ascii_case(trimmed)
        }) {
            return Some(pane.id.clone());
        }

        // 2. Normalized name or alias match (e.g. "red-pony" == "Red Pony" == "red_pony", "pane-2" == "pane 2")
        if !query_norm.is_empty()
            && let Some(pane) = panes.values().find(|p| {
                normalize_agent_key(&p.name) == query_norm
                    || normalize_agent_key(&p.alias) == query_norm
            })
        {
            return Some(pane.id.clone());
        }

        // 3. Fallback to CLAIMS_CACHE + CHARACTER_CACHE (petname, character display name, character ID)
        let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
        let catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();

        for claim in claims.iter() {
            if !panes.contains_key(&claim.holder_id) {
                continue;
            }
            if claim.holder_id.eq_ignore_ascii_case(trimmed)
                || claim.character_id.eq_ignore_ascii_case(trimmed)
                || normalize_agent_key(&claim.petname) == query_norm
            {
                return Some(claim.holder_id.clone());
            }
            if let Some(char_info) = catalog.iter().find(|c| c.config.id == claim.character_id)
                && (normalize_agent_key(&char_info.config.display_name) == query_norm
                    || normalize_agent_key(&char_info.config.name) == query_norm)
            {
                return Some(claim.holder_id.clone());
            }
        }

        None
    }

    pub async fn get_global_radar(&self, current_pane_id: String) -> String {
        let panes = self.panes.read().await;
        let mut radar = String::new();

        let peers: Vec<_> = panes.values().filter(|p| p.id != current_pane_id).collect();

        if !peers.is_empty() {
            radar.push_str("\n--- GLOBAL RADAR (Other Workspace Panes & Active Agents) ---\n");
            radar.push_str(
                "You can target any open pane by its alias (e.g. `pane-2`), ID, or agent name using `terminal_exec(..., target_pane=...)`, `read_pane_buffer(agent_name)`, `send_keystrokes_to_pane(agent_name, keys)`, or `subscribe_to_pane(agent_name, event_type)`. For panes with an attached peer agent, you can also delegate tasks using `delegate_task(agent_name, prompt)`.\n",
            );

            let mut sorted_peers = peers.clone();
            // Simple load balancing signal sort:
            // 1. Waiting + Full Context (Best)
            // 2. Sleep + Full Context
            // 3. Waiting + Degraded
            // 4. Others (Working, Locking, Faulted, Plain Terminals)
            sorted_peers.sort_by_key(|p| {
                let is_waiting = p.status.as_deref() == Some("Waiting");
                let is_sleep = p.status.as_deref() == Some("Sleep");
                let is_full =
                    matches!(p.context_quality, Some(crate::engine::ContextQuality::Full));

                if is_waiting && is_full {
                    1
                } else if is_sleep && is_full {
                    2
                } else if is_waiting && !is_full {
                    3
                } else if is_sleep && !is_full {
                    4
                } else {
                    5
                }
            });

            for peer in sorted_peers {
                let cmd = peer.last_command.as_deref().unwrap_or("idle");
                let is_agent = peer.tx.is_some() || peer.session_id.is_some();
                if is_agent {
                    let status = peer.status.as_deref().unwrap_or("Unknown");
                    let quality = match peer.context_quality {
                        Some(crate::engine::ContextQuality::Full) => "Full Context",
                        Some(crate::engine::ContextQuality::Degraded) => "Degraded Context",
                        None => "Unknown Context",
                    };

                    radar.push_str(&format!(
                        "- [Agent: '{}' | Alias: `{}` | ID: `{}`]: in `{}` | Last command `{}` | Status: {} [{}]\n",
                        peer.name, peer.alias, peer.id, peer.cwd, cmd, status, quality
                    ));
                } else {
                    let status = peer.status.as_deref().unwrap_or("Idle");
                    radar.push_str(&format!(
                        "- [Plain Terminal: `{}` | ID: `{}`]: in `{}` | Last command `{}` | Status: {}\n",
                        peer.alias, peer.id, peer.cwd, cmd, status
                    ));
                }
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
                alias: p.alias.clone(),
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

fn normalize_agent_key(s: &str) -> String {
    s.trim()
        .trim_start_matches('@')
        .to_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plain_pane_registration_alias_resolution_and_radar_formatting() {
        let reg = WorkspaceRegistry::new();

        let alias1 = reg
            .update_pane_state(
                "uuid-home".to_string(),
                Some("sess-1".to_string()),
                Some("niko".to_string()),
                "/tmp/project".to_string(),
                None,
                None,
            )
            .await;
        assert_eq!(alias1, "pane-1");

        let alias2 = reg
            .update_pane_state(
                "uuid-companion".to_string(),
                None,
                None,
                "/tmp/linux".to_string(),
                Some("make defconfig".to_string()),
                Some("configuration written to .config".to_string()),
            )
            .await;
        assert_eq!(alias2, "pane-2");

        // Resolve by alias, normalized alias ("pane 2"), or exact UUID
        assert_eq!(
            reg.resolve_pane_id_by_name("pane-2").await.as_deref(),
            Some("uuid-companion")
        );
        assert_eq!(
            reg.resolve_pane_id_by_name("pane 2").await.as_deref(),
            Some("uuid-companion")
        );
        assert_eq!(
            reg.resolve_pane_id_by_name("uuid-companion")
                .await
                .as_deref(),
            Some("uuid-companion")
        );

        // Radar from home pane must show companion pane as a Plain Terminal with alias & CWD
        let radar = reg.get_global_radar("uuid-home".to_string()).await;
        assert!(radar.contains("[Plain Terminal: `pane-2` | ID: `uuid-companion`]"));
        assert!(radar.contains("/tmp/linux"));
        assert!(radar.contains("make defconfig"));
    }

    #[tokio::test]
    async fn cross_pane_pending_command_completes_and_drains_cleanly() {
        let reg = WorkspaceRegistry::new();

        let (tx1, rx1) = tokio::sync::oneshot::channel();
        reg.register_pending_command(
            "uuid-pane-2".to_string(),
            Some("uuid-pane-1".to_string()),
            tx1,
        )
        .await;

        let completed = reg
            .complete_pending_command(
                "uuid-pane-2",
                0,
                "Kernel: arch/x86/boot/bzImage is ready".to_string(),
            )
            .await;
        assert!(completed);
        assert_eq!(
            rx1.await.unwrap().unwrap(),
            "Kernel: arch/x86/boot/bzImage is ready"
        );

        // Test rejection/interruption drain by source pane
        let (tx2, rx2) = tokio::sync::oneshot::channel();
        reg.register_pending_command(
            "uuid-pane-2".to_string(),
            Some("uuid-pane-1".to_string()),
            tx2,
        )
        .await;

        let drained = reg
            .drain_pending_commands_for_source("uuid-pane-1", "[USER_EXPLICIT_REJECT]".to_string())
            .await;
        assert!(drained);
        assert_eq!(rx2.await.unwrap().unwrap_err(), "[USER_EXPLICIT_REJECT]");
    }
}
