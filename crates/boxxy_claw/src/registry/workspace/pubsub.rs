use super::{WorkspaceRegistry, normalize_lock_resource};

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

impl WorkspaceRegistry {
    /// Acquires an exclusive advisory lock on `resource` for `pane_id`.
    ///
    /// # Errors
    /// Returns `Err` with the holding agent's display name if another pane holds the lock.
    pub async fn acquire_lock(&self, pane_id: String, resource: String) -> Result<(), String> {
        let key = normalize_lock_resource(&resource);
        let conflict_holder = {
            let mut locks = self.locks.write().await;
            if let Some(holder) = locks.get(&key) {
                if holder == &pane_id {
                    return Ok(());
                }
                Some(holder.clone())
            } else {
                locks.insert(key, pane_id);
                None
            }
        };

        if let Some(holder) = conflict_holder {
            let holder_name = self
                .panes
                .read()
                .await
                .get(&holder)
                .map_or_else(|| "Unknown Agent".to_string(), |p| p.name.clone());
            return Err(format!("LOCKED by agent '{holder_name}'"));
        }
        Ok(())
    }

    pub async fn release_lock(&self, pane_id: String, resource: String) {
        let key = normalize_lock_resource(&resource);
        let mut locks = self.locks.write().await;
        if let Some(holder) = locks.get(&key)
            && holder == &pane_id
        {
            locks.remove(&key);
        }
    }

    /// Returns any resource still locked by `pane_id`, if at least one lock remains held.
    pub async fn held_lock_for_pane(&self, pane_id: &str) -> Option<String> {
        let locks = self.locks.read().await;
        locks
            .iter()
            .find_map(|(res, holder)| (holder == pane_id).then(|| res.clone()))
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
        let to_notify = {
            let mut subs = self.subscriptions.write().await;
            let panes = self.panes.read().await;
            let mut targets = Vec::new();

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
                    targets.push(tx.clone());
                }
            }
            subs.retain(|_, list| !list.is_empty());
            targets
        };

        for tx in to_notify {
            let _ = tx
                .send(crate::engine::ClawMessage::SubscriptionEvent {
                    event: event.clone(),
                })
                .await;
        }
    }

    /// Evaluates active `EventFilter::OutputMatch` subscriptions targeting `pane_id` against
    /// `snapshot` using size-bounded regex compilation, publishing matching events.
    pub(super) async fn evaluate_output_match_subscriptions(&self, pane_id: &str, snapshot: &str) {
        if snapshot.is_empty() {
            return;
        }
        let candidate_regexes: Vec<String> = {
            let subs = self.subscriptions.read().await;
            let mut patterns = Vec::new();
            for filters in subs.values() {
                for filter in filters {
                    if let EventFilter::OutputMatch {
                        pane_id: target_id,
                        regex,
                    } = filter
                        && target_id == pane_id
                        && !patterns.contains(regex)
                    {
                        patterns.push(regex.clone());
                    }
                }
            }
            patterns
        };

        for pattern in candidate_regexes {
            let Ok(re) = regex::RegexBuilder::new(&pattern)
                .size_limit(1 << 20)
                .build()
            else {
                continue;
            };
            let matched_line = snapshot
                .lines()
                .rev()
                .find(|line| re.is_match(line))
                .map(ToString::to_string)
                .or_else(|| {
                    re.find(snapshot)
                        .map(|m| m.as_str().lines().next().unwrap_or("").to_string())
                });

            if let Some(line) = matched_line {
                self.publish_event(crate::engine::ClawEvent::OutputMatch {
                    pane_id: pane_id.to_string(),
                    regex: pattern,
                    matched_text: line,
                })
                .await;
            }
        }
    }
}
