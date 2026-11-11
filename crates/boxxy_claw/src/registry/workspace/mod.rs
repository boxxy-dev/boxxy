mod promises;
mod pubsub;

pub use pubsub::EventFilter;

use std::collections::HashMap;
use std::fmt::Write as _;
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

pub(super) type PendingCommandWaiter = (
    Option<String>,
    tokio::sync::oneshot::Sender<Result<String, String>>,
);

pub(super) type DelegatedTaskChannel = (
    tokio::sync::watch::Sender<Option<String>>,
    tokio::sync::watch::Receiver<Option<String>>,
);

pub struct WorkspaceRegistry {
    pub(super) panes: Arc<RwLock<HashMap<String, PaneState>>>,
    next_alias_index: std::sync::atomic::AtomicU64,
    pub(super) tasks: Arc<RwLock<HashMap<String, Vec<crate::engine::ScheduledTask>>>>,
    pub(super) tasks_changed_tx: tokio::sync::broadcast::Sender<()>,
    global_intent: Arc<RwLock<Option<String>>>,
    pub(super) subscriptions: Arc<RwLock<HashMap<String, Vec<EventFilter>>>>,
    pub(super) locks: Arc<RwLock<HashMap<String, String>>>,
    pub(super) pending_commands: Arc<RwLock<HashMap<String, PendingCommandWaiter>>>,
    pub(super) delegated_tasks: Arc<RwLock<HashMap<uuid::Uuid, DelegatedTaskChannel>>>,
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
    #[must_use]
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
            delegated_tasks: Arc::new(RwLock::new(HashMap::new())),
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

    pub async fn rebind_pane(&self, old_id: &str, new_id: String) {
        if old_id == new_id {
            return;
        }
        {
            let mut panes = self.panes.write().await;
            if let Some(mut state) = panes.remove(old_id) {
                state.id.clone_from(&new_id);
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
            for filters in subs.values_mut() {
                for filter in filters.iter_mut() {
                    match filter {
                        EventFilter::ProcessExited {
                            pane_id: Some(target),
                        }
                        | EventFilter::OutputMatch {
                            pane_id: target, ..
                        } if target == old_id => {
                            target.clone_from(&new_id);
                        }
                        _ => {}
                    }
                }
            }
        }
        {
            let mut locks = self.locks.write().await;
            for holder in locks.values_mut() {
                if holder == old_id {
                    holder.clone_from(&new_id);
                }
            }
        }
        {
            let mut pending = self.pending_commands.write().await;
            for (src_opt, _) in pending.values_mut() {
                if src_opt.as_deref() == Some(old_id) {
                    *src_opt = Some(new_id.clone());
                }
            }
            if let Some(waiter) = pending.remove(old_id) {
                pending.insert(new_id, waiter);
            }
        }
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
        let alias = panes
            .get(&id)
            .map_or_else(|| self.mint_alias(&panes), |existing| existing.alias.clone());
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
        let resolved_alias = {
            let mut panes = self.panes.write().await;
            let alias = panes
                .get(&id)
                .map_or_else(|| self.mint_alias(&panes), |existing| existing.alias.clone());
            let default_name = alias.clone();
            let entry = panes.entry(id.clone()).or_insert_with(|| PaneState {
                id: id.clone(),
                alias,
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
            if let Some(ref snap) = snapshot {
                entry.last_snapshot = Some(snap.clone());
            }
            entry.alias.clone()
        };

        if let Some(snap) = snapshot {
            self.evaluate_output_match_subscriptions(&id, &snap).await;
        }

        resolved_alias
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

    pub async fn add_pane_skill(&self, id: &str, skill: String) {
        let mut panes = self.panes.write().await;
        if let Some(pane) = panes.get_mut(id)
            && !pane
                .active_skills
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&skill))
        {
            pane.active_skills.push(skill);
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

    /// Finds the best peer pane (excluding `exclude_pane_id`) capable of assisting with
    /// `capability`, checking active skills first, then agent display name, then CWD match.
    pub async fn find_peer_for_capability(
        &self,
        capability: &str,
        exclude_pane_id: &str,
    ) -> Option<(
        String,
        String,
        async_channel::Sender<crate::engine::ClawMessage>,
    )> {
        let cap_lower = capability.trim().to_lowercase();
        if cap_lower.is_empty() {
            return None;
        }
        let cap_norm = normalize_agent_key(capability);
        let panes = self.panes.read().await;

        let candidates: Vec<&PaneState> = panes
            .values()
            .filter(|p| p.id != exclude_pane_id && p.tx.is_some())
            .collect();

        if let Some(peer) = candidates.iter().find(|p| {
            p.active_skills.iter().any(|s| {
                let s_low = s.to_lowercase();
                s_low.contains(&cap_lower) || cap_lower.contains(&s_low)
            })
        }) && let Some(tx) = &peer.tx
        {
            return Some((peer.id.clone(), peer.name.clone(), tx.clone()));
        }

        if !cap_norm.is_empty()
            && let Some(peer) = candidates.iter().find(|p| {
                let name_norm = normalize_agent_key(&p.name);
                name_norm.contains(&cap_norm) || p.name.to_lowercase().contains(&cap_lower)
            })
            && let Some(tx) = &peer.tx
        {
            return Some((peer.id.clone(), peer.name.clone(), tx.clone()));
        }

        if let Some(peer) = candidates
            .iter()
            .find(|p| p.cwd.to_lowercase().contains(&cap_lower))
            && let Some(tx) = &peer.tx
        {
            return Some((peer.id.clone(), peer.name.clone(), tx.clone()));
        }

        None
    }

    pub async fn evict_session(&self, session_id: &str) {
        let target_tx = {
            let panes = self.panes.read().await;
            panes.values().find_map(|p| {
                (p.session_id.as_deref() == Some(session_id))
                    .then(|| p.tx.clone())
                    .flatten()
            })
        };

        if let Some(tx) = target_tx {
            let _ = tx.send(crate::engine::ClawMessage::Evict).await;
        }
    }

    pub async fn unregister_pane(&self, id: String) {
        self.panes.write().await.remove(&id);

        let displaced_waiter = self.pending_commands.write().await.remove(&id);
        if let Some((_, reply_tx)) = displaced_waiter {
            let _ = reply_tx.send(Err(format!(
                "Target pane '{id}' closed before command finished."
            )));
        }
        let _ = self
            .drain_pending_commands_for_source(
                &id,
                format!("Source pane '{id}' was unregistered."),
            )
            .await;

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

        if let Some(pane) = panes.values().find(|p| {
            p.id.eq_ignore_ascii_case(trimmed)
                || p.alias.eq_ignore_ascii_case(trimmed)
                || p.name.eq_ignore_ascii_case(trimmed)
        }) {
            return Some(pane.id.clone());
        }

        if !query_norm.is_empty()
            && let Some(pane) = panes.values().find(|p| {
                normalize_agent_key(&p.name) == query_norm
                    || normalize_agent_key(&p.alias) == query_norm
            })
        {
            return Some(pane.id.clone());
        }

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
        drop(panes);

        None
    }

    pub async fn get_global_radar(&self, current_pane_id: String) -> String {
        let mut radar = String::new();

        {
            let panes = self.panes.read().await;
            let mut sorted_peers: Vec<_> =
                panes.values().filter(|p| p.id != current_pane_id).collect();

            if !sorted_peers.is_empty() {
                radar.push_str("\n--- GLOBAL RADAR (Other Workspace Panes & Active Agents) ---\n");
                radar.push_str(
                    "You can target any open pane by its alias (e.g. `pane-2`), ID, or agent name using `terminal_exec(..., target_pane=...)`, `read_pane_buffer(agent_name)`, `send_keystrokes_to_pane(agent_name, keys)`, or `subscribe_to_pane(agent_name, event_type)`. For panes with an attached peer agent, you can also delegate tasks using `delegate_task(agent_name, prompt)`.\n",
                );

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
                    let cmd = peer
                        .last_command
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("idle");
                    let is_agent = peer.tx.is_some() || peer.session_id.is_some();
                    if is_agent {
                        let status = peer
                            .status
                            .as_deref()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or("Waiting");
                        let quality = match peer.context_quality {
                            Some(crate::engine::ContextQuality::Full) => "Full Context",
                            Some(crate::engine::ContextQuality::Degraded) => "Degraded Context",
                            None => "Unknown Context",
                        };

                        let _ = writeln!(
                            radar,
                            "- [Agent: '{}' | Alias: `{}` | ID: `{}`]: in `{}` | Last command `{}` | Status: {} [{}]",
                            peer.name, peer.alias, peer.id, peer.cwd, cmd, status, quality
                        );
                    } else {
                        let status = peer.status.as_deref().unwrap_or("Idle");
                        let _ = writeln!(
                            radar,
                            "- [Plain Terminal: `{}` | ID: `{}`]: in `{}` | Last command `{}` | Status: {}",
                            peer.alias, peer.id, peer.cwd, cmd, status
                        );
                    }
                }
            }
        }

        if let Some(intent) = self.global_intent.read().await.as_deref() {
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

pub(super) fn normalize_lock_resource(resource: &str) -> String {
    let mut trimmed = resource.trim();
    while let Some(rest) = trimmed.strip_prefix("./") {
        trimmed = rest;
    }
    trimmed.trim_end_matches('/').to_string()
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

    #[tokio::test]
    async fn delegated_task_promise_resolves_before_and_during_wait() {
        let reg = Arc::new(WorkspaceRegistry::new());

        let id1 = uuid::Uuid::new_v4();
        reg.register_delegated_task(id1).await;
        assert!(reg.has_delegated_task(id1).await);
        reg.complete_delegated_task(id1, "Result A".to_string())
            .await;
        let out1 = reg
            .wait_for_delegated_task(id1, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(out1, "Result A");
        assert!(!reg.has_delegated_task(id1).await);

        let id2 = uuid::Uuid::new_v4();
        reg.register_delegated_task(id2).await;
        let reg_clone = reg.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            reg_clone
                .complete_delegated_task(id2, "Result B".to_string())
                .await;
        });
        let out2 = reg
            .wait_for_delegated_task(id2, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(out2, "Result B");
        assert!(!reg.has_delegated_task(id2).await);

        let id3 = uuid::Uuid::new_v4();
        reg.register_delegated_task(id3).await;
        let err3 = reg
            .wait_for_delegated_task(id3, std::time::Duration::from_millis(10))
            .await
            .unwrap_err();
        assert!(err3.contains("Timed out"));
        assert!(!reg.has_delegated_task(id3).await);

        let unregistered = uuid::Uuid::new_v4();
        reg.complete_delegated_task(unregistered, "ignored".to_string())
            .await;
        assert!(!reg.has_delegated_task(unregistered).await);
    }

    #[tokio::test]
    async fn output_match_subscription_fires_when_pane_snapshot_updates() {
        let reg = WorkspaceRegistry::new();
        let (tx, rx) = async_channel::unbounded();
        reg.register_pane_tx("watcher".to_string(), tx).await;
        reg.subscribe(
            "watcher".to_string(),
            EventFilter::OutputMatch {
                pane_id: "server".to_string(),
                regex: "ERROR:\\s+database".to_string(),
            },
        )
        .await;

        reg.update_pane_state(
            "server".to_string(),
            None,
            None,
            "/tmp/app".to_string(),
            None,
            Some("Starting service...\nListening on :8080".to_string()),
        )
        .await;
        assert!(rx.try_recv().is_err());

        reg.update_pane_state(
            "server".to_string(),
            None,
            None,
            "/tmp/app".to_string(),
            None,
            Some("Listening on :8080\nERROR: database connection refused".to_string()),
        )
        .await;
        match rx.try_recv() {
            Ok(crate::engine::ClawMessage::SubscriptionEvent {
                event:
                    crate::engine::ClawEvent::OutputMatch {
                        pane_id,
                        matched_text,
                        ..
                    },
            }) => {
                assert_eq!(pane_id, "server");
                assert_eq!(matched_text, "ERROR: database connection refused");
            }
            other => panic!("Expected OutputMatch subscription event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unregistering_target_pane_wakes_pending_command_waiter() {
        let reg = WorkspaceRegistry::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        reg.register_pending_command(
            "companion-pane".to_string(),
            Some("home-pane".to_string()),
            tx,
        )
        .await;

        reg.unregister_pane("companion-pane".to_string()).await;
        let err = rx.await.unwrap().unwrap_err();
        assert!(err.contains("closed before command finished"));
    }
}
