use crate::Attachment;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const MAX_HISTORY_ITEMS: i64 = 100;
const MAX_RICH_ATTACHMENTS: usize = 20;

lazy_static! {
    static ref GLOBAL_HISTORY: Arc<Mutex<Vec<HistoryItem>>> = Arc::new(Mutex::new(Vec::new()));
    static ref HISTORY_LOADED: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct HistoryItem {
    pub text: String,
    pub attachments: Vec<Attachment>,
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
        Self::ensure_loaded();
        Self {
            draft: HistoryItem::default(),
            idx: None,
        }
    }

    fn ensure_loaded() {
        let mut loaded = HISTORY_LOADED.lock().unwrap();
        if *loaded {
            return;
        }
        *loaded = true;

        let items_rc = GLOBAL_HISTORY.clone();

        let () = boxxy_sys_utils::runtime().block_on(async move {
            if let Ok(db) = boxxy_db::Db::new().await {
                let store = boxxy_db::store::Store::new(db.pool());
                if let Ok(records) = store.get_recent_msgbar_history(MAX_HISTORY_ITEMS).await {
                    let mut items = Vec::new();
                    for record in records {
                        let attachments: Vec<Attachment> =
                            serde_json::from_str(&record.attachments_json).unwrap_or_default();
                        items.push(HistoryItem {
                            text: record.text,
                            attachments,
                        });
                    }
                    *items_rc.lock().unwrap() = items;
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
                let mut items = GLOBAL_HISTORY.lock().unwrap();
                items.push(item);

                // In-memory pruning
                if items.len() > MAX_HISTORY_ITEMS as usize {
                    let overflow = items.len() - MAX_HISTORY_ITEMS as usize;
                    items.drain(0..overflow);
                }

                if items.len() > MAX_RICH_ATTACHMENTS {
                    let cutoff_idx = items.len() - MAX_RICH_ATTACHMENTS;
                    for i in 0..cutoff_idx {
                        for att in &mut items[i].attachments {
                            if att.is_image && att.content.len() > 1024 {
                                att.content = "[Image data pruned for performance]".to_string();
                            }
                        }
                    }
                }
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
        let items = GLOBAL_HISTORY.lock().unwrap();
        if items.is_empty() {
            return None;
        }

        if self.idx.is_none() {
            // Save the current draft before going into history
            self.draft = HistoryItem {
                text: current_text,
                attachments: current_atts,
            };
            self.idx = Some(items.len().saturating_sub(1));
        } else {
            let current_idx = self.idx.unwrap();
            if current_idx > 0 {
                self.idx = Some(current_idx - 1);
            }
        }

        self.idx.map(|i| items[i].clone())
    }

    pub fn navigate_down(&mut self) -> Option<HistoryItem> {
        let items = GLOBAL_HISTORY.lock().unwrap();
        if let Some(current_idx) = self.idx {
            if current_idx + 1 < items.len() {
                self.idx = Some(current_idx + 1);
                Some(items[current_idx + 1].clone())
            } else {
                // Reached the bottom, return the draft and reset index
                self.idx = None;
                Some(self.draft.clone())
            }
        } else {
            None
        }
    }
}
