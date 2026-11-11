use super::WorkspaceRegistry;

impl WorkspaceRegistry {
    #[must_use]
    pub fn subscribe_tasks(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.tasks_changed_tx.subscribe()
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
        all_tasks.get(pane_id).map_or(0, |list| {
            let count = list
                .iter()
                .filter(|t| t.status == crate::engine::TaskStatus::Pending)
                .count();
            u32::try_from(count).unwrap_or(u32::MAX)
        })
    }

    pub async fn get_all_pending_tasks(&self) -> Vec<(String, crate::engine::ScheduledTask)> {
        let all_tasks = self.tasks.read().await;
        let panes = self.panes.read().await;
        let mut pending = Vec::new();

        for (pane_id, tasks) in all_tasks.iter() {
            let agent_name = panes
                .get(pane_id)
                .map_or_else(|| "Unknown Agent".to_string(), |p| p.name.clone());
            for task in tasks {
                if task.status == crate::engine::TaskStatus::Pending {
                    pending.push((agent_name.clone(), task.clone()));
                }
            }
        }
        drop(panes);
        drop(all_tasks);
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
                .map_or_else(|| "Unknown Agent".to_string(), |p| p.name.clone());
            for task in tasks {
                if task.status == crate::engine::TaskStatus::Pending {
                    pending.push((pane_id.clone(), agent_name.clone(), task.clone()));
                }
            }
        }
        drop(panes);
        drop(all_tasks);
        pending
    }

    pub async fn register_pending_command(
        &self,
        target_pane_id: String,
        source_pane_id: Option<String>,
        reply_tx: tokio::sync::oneshot::Sender<Result<String, String>>,
    ) {
        let displaced = self
            .pending_commands
            .write()
            .await
            .insert(target_pane_id, (source_pane_id, reply_tx));
        if let Some((_, prev_tx)) = displaced {
            let _ = prev_tx.send(Err(
                "Command superseded by a newer command on the target pane.".to_string(),
            ));
        }
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
                (src_opt.as_deref() == Some(source_pane_id)).then(|| target_id.clone())
            })
            .collect();
        let mut fulfilled = false;
        for target_id in matching_targets {
            if let Some((_, reply_tx)) = pending.remove(&target_id) {
                let _ = reply_tx.send(Err(err_msg.clone()));
                fulfilled = true;
            }
        }
        drop(pending);
        fulfilled
    }

    pub async fn has_pending_command(&self, target_pane_id: &str) -> bool {
        self.pending_commands
            .read()
            .await
            .contains_key(target_pane_id)
    }

    pub async fn register_delegated_task(
        &self,
        request_id: uuid::Uuid,
    ) -> tokio::sync::watch::Receiver<Option<String>> {
        let (tx, rx) = tokio::sync::watch::channel(None);
        self.delegated_tasks
            .write()
            .await
            .insert(request_id, (tx, rx.clone()));
        rx
    }

    pub async fn remove_delegated_task(&self, request_id: uuid::Uuid) {
        self.delegated_tasks.write().await.remove(&request_id);
    }

    pub async fn has_delegated_task(&self, request_id: uuid::Uuid) -> bool {
        self.delegated_tasks.read().await.contains_key(&request_id)
    }

    pub async fn complete_delegated_task(&self, request_id: uuid::Uuid, result: String) {
        let tx_opt = {
            let map = self.delegated_tasks.read().await;
            map.get(&request_id).map(|(tx, _)| tx.clone())
        };
        if let Some(tx) = tx_opt {
            let _ = tx.send(Some(result));
        }
    }

    /// Waits up to `timeout` for a delegated task promise to resolve.
    ///
    /// # Errors
    /// Returns `Err` if `request_id` is unknown, the channel closes prematurely, or `timeout` elapses.
    pub async fn wait_for_delegated_task(
        &self,
        request_id: uuid::Uuid,
        timeout: std::time::Duration,
    ) -> Result<String, String> {
        let mut rx = {
            let map = self.delegated_tasks.read().await;
            let Some((_, rx)) = map.get(&request_id) else {
                return Err(format!("Task {request_id} not found."));
            };
            rx.clone()
        };

        let existing = rx.borrow_and_update().clone();
        if let Some(existing) = existing {
            self.delegated_tasks.write().await.remove(&request_id);
            return Ok(existing);
        }

        let res = tokio::time::timeout(timeout, async {
            loop {
                if rx.changed().await.is_err() {
                    return Err("Delegated task channel closed before completion.".to_string());
                }
                let current = rx.borrow_and_update().clone();
                if let Some(val) = current {
                    return Ok(val);
                }
            }
        })
        .await;

        self.delegated_tasks.write().await.remove(&request_id);

        match res {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!("Timed out waiting for task {request_id}.")),
        }
    }
}
