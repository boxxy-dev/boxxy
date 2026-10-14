use crate::Attachment;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock, Mutex};

const MAX_HISTORY_ITEMS: i64 = 100;
const MAX_RICH_ATTACHMENTS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryLoadStatus {
    Uninitialized,
    Loading,
    Ready,
    Failed,
}

pub struct HistoryState {
    pub status: HistoryLoadStatus,
    pub items: Vec<HistoryItem>,
    pub pending_writes: Vec<HistoryItem>,
}

static GLOBAL_HISTORY: LazyLock<Arc<Mutex<HistoryState>>> = LazyLock::new(|| {
    Arc::new(Mutex::new(HistoryState {
        status: HistoryLoadStatus::Uninitialized,
        items: Vec::new(),
        pending_writes: Vec::new(),
    }))
});

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

fn prune_in_memory(items: &mut Vec<HistoryItem>) {
    if items.len() > MAX_HISTORY_ITEMS as usize {
        let overflow = items.len() - MAX_HISTORY_ITEMS as usize;
        items.drain(0..overflow);
    }

    if items.len() > MAX_RICH_ATTACHMENTS {
        let cutoff_idx = items.len() - MAX_RICH_ATTACHMENTS;
        for item in items.iter_mut().take(cutoff_idx) {
            for att in &mut item.attachments {
                if att.is_image && att.content.len() > 1024 {
                    att.content = "[Image data pruned for performance]".to_string();
                }
            }
        }
    }
}

/// Merges historical records loaded asynchronously from SQLite with any items
/// pushed by the user while the database load was in flight.
pub(crate) fn merge_loaded_history(state: &mut HistoryState, mut db_items: Vec<HistoryItem>) {
    for pending in state.pending_writes.drain(..) {
        if db_items.last() != Some(&pending) {
            db_items.push(pending);
        }
    }
    prune_in_memory(&mut db_items);
    state.items = db_items;
    state.status = HistoryLoadStatus::Ready;
}

pub struct MsgHistory {
    draft: HistoryItem,
    idx: Option<usize>,
}

impl Default for MsgHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl MsgHistory {
    #[must_use]
    pub fn new() -> Self {
        Self::ensure_loaded_async();
        Self {
            draft: HistoryItem::default(),
            idx: None,
        }
    }

    fn ensure_loaded_async() {
        {
            let mut state = GLOBAL_HISTORY.lock().unwrap();
            if matches!(
                state.status,
                HistoryLoadStatus::Loading | HistoryLoadStatus::Ready
            ) {
                return;
            }
            state.status = HistoryLoadStatus::Loading;
        }

        let history_rc = GLOBAL_HISTORY.clone();
        boxxy_sys_utils::runtime().spawn(async move {
            match boxxy_db::Db::new().await {
                Ok(db) => {
                    let store = boxxy_db::store::Store::new(db.pool());
                    match store.get_recent_msgbar_history(MAX_HISTORY_ITEMS).await {
                        Ok(records) => {
                            let mut db_items = Vec::with_capacity(records.len());
                            for record in records {
                                let attachments: Vec<Attachment> =
                                    serde_json::from_str(&record.attachments_json)
                                        .unwrap_or_default();
                                db_items.push(HistoryItem {
                                    text: record.text,
                                    attachments,
                                });
                            }
                            let mut state = history_rc.lock().unwrap();
                            merge_loaded_history(&mut state, db_items);
                        }
                        Err(e) => {
                            log::warn!("Failed to load msgbar history from DB: {}", e);
                            history_rc.lock().unwrap().status = HistoryLoadStatus::Failed;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to open DB for msgbar history: {}", e);
                    history_rc.lock().unwrap().status = HistoryLoadStatus::Failed;
                }
            }
        });
    }

    pub fn push(&mut self, text: String, attachments: Vec<Attachment>) {
        if !text.is_empty() || !attachments.is_empty() {
            let item = HistoryItem {
                text: text.clone(),
                attachments: attachments.clone(),
            };

            {
                let mut state = GLOBAL_HISTORY.lock().unwrap();
                if state.status != HistoryLoadStatus::Ready {
                    state.pending_writes.push(item.clone());
                }
                state.items.push(item);
                prune_in_memory(&mut state.items);
            }

            self.save_to_db(text, attachments);
        }

        self.idx = None;
        self.draft = HistoryItem::default();
    }

    fn save_to_db(&self, text: String, attachments: Vec<Attachment>) {
        boxxy_sys_utils::runtime().spawn(async move {
            if let Ok(db) = boxxy_db::Db::new().await {
                let store = boxxy_db::store::Store::new(db.pool());
                if let Ok(json) = serde_json::to_string(&attachments)
                    && store.insert_msgbar_history(&text, &json).await.is_ok()
                {
                    let _ = store.prune_msgbar_history(150, 100).await;
                }
            }
        });
    }

    pub fn reset(&mut self) {
        self.idx = None;
        self.draft = HistoryItem::default();
    }

    pub fn navigate_up(
        &mut self,
        current_text: String,
        current_atts: Vec<Attachment>,
    ) -> Option<HistoryItem> {
        Self::ensure_loaded_async();

        let state = GLOBAL_HISTORY.lock().unwrap();
        if state.items.is_empty() {
            return None;
        }

        if self.idx.is_none() {
            self.draft = HistoryItem {
                text: current_text,
                attachments: current_atts,
            };
            self.idx = Some(state.items.len().saturating_sub(1));
        } else {
            let current_idx = self.idx.unwrap();
            if current_idx > 0 {
                self.idx = Some(current_idx - 1);
            }
        }

        self.idx.map(|i| state.items[i].clone())
    }

    pub fn navigate_down(&mut self) -> Option<HistoryItem> {
        let state = GLOBAL_HISTORY.lock().unwrap();
        if let Some(current_idx) = self.idx {
            if current_idx + 1 < state.items.len() {
                self.idx = Some(current_idx + 1);
                Some(state.items[current_idx + 1].clone())
            } else {
                self.idx = None;
                Some(self.draft.clone())
            }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_loaded_history_preserves_pending_writes_without_duplicates() {
        let mut state = HistoryState {
            status: HistoryLoadStatus::Loading,
            items: vec![HistoryItem {
                text: "typed-while-loading".to_string(),
                attachments: vec![],
            }],
            pending_writes: vec![
                HistoryItem {
                    text: "already-persisted".to_string(),
                    attachments: vec![],
                },
                HistoryItem {
                    text: "typed-while-loading".to_string(),
                    attachments: vec![],
                },
            ],
        };

        let db_items = vec![
            HistoryItem {
                text: "old-item".to_string(),
                attachments: vec![],
            },
            HistoryItem {
                text: "already-persisted".to_string(),
                attachments: vec![],
            },
        ];

        merge_loaded_history(&mut state, db_items);

        assert_eq!(state.status, HistoryLoadStatus::Ready);
        assert!(state.pending_writes.is_empty());
        assert_eq!(
            state
                .items
                .iter()
                .map(|i| i.text.as_str())
                .collect::<Vec<_>>(),
            vec!["old-item", "already-persisted", "typed-while-loading"]
        );
    }
}
