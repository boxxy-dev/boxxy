use crate::Attachment;
use serde::{Deserialize, Serialize};

const MAX_HISTORY_ITEMS: i64 = 100;
const MAX_RICH_ATTACHMENTS: usize = 20;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct HistoryItem {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

pub struct MsgHistory {
    items: Vec<HistoryItem>,
    draft: HistoryItem,
    idx: Option<usize>,
    loaded: bool,
}

impl Default for MsgHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl MsgHistory {
    #[must_use] 
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            draft: HistoryItem::default(),
            idx: None,
            loaded: false,
        }
    }

    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;

        let items_rc = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let items_clone = items_rc.clone();

        let () = boxxy_ai_core::utils::runtime().block_on(async move {
            if let Ok(db) = boxxy_db::Db::new().await {
                let store = boxxy_db::store::Store::new(db.pool());
                if let Ok(records) = store.get_recent_msgbar_history(MAX_HISTORY_ITEMS).await {
                    let mut items = Vec::new();
                    for record in records {
                        let attachments: Vec<Attachment> =
                            serde_json::from_str(&record.attachments).unwrap_or_default();
                        items.push(HistoryItem {
                            text: record.text,
                            attachments,
                        });
                    }
                    *items_clone.lock().unwrap() = items;
                }
            }
        });

        if let Ok(items) = items_rc.lock() {
            self.items = items.clone();
        }
    }

    pub fn push(&mut self, text: String, attachments: Vec<Attachment>) {
        self.ensure_loaded();

        if !text.is_empty() || !attachments.is_empty() {
            self.items.push(HistoryItem {
                text: text.clone(),
                attachments: attachments.clone(),
            });

            // In-memory pruning
            if self.items.len() > MAX_HISTORY_ITEMS as usize {
                let overflow = self.items.len() - MAX_HISTORY_ITEMS as usize;
                self.items.drain(0..overflow);
            }

            if self.items.len() > MAX_RICH_ATTACHMENTS {
                let cutoff_idx = self.items.len() - MAX_RICH_ATTACHMENTS;
                for i in 0..cutoff_idx {
                    for att in &mut self.items[i].attachments {
                        if att.is_image && att.content.len() > 1024 {
                            att.content = "[Image data pruned for performance]".to_string();
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
        // Strip heavy attachments for the DB record if this makes us go over our rich memory threshold globally
        // (For simplicity here, we just save the attachments as is, since SQLite can handle megabytes trivially.
        // We just pruned them in RAM).

        boxxy_ai_core::utils::runtime().spawn(async move {
            if let Ok(db) = boxxy_db::Db::new().await {
                let store = boxxy_db::store::Store::new(db.pool());
                if let Ok(json) = serde_json::to_string(&attachments)
                    && store.insert_msgbar_history(&text, &json).await.is_ok() {
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
        self.ensure_loaded();

        if self.items.is_empty() {
            return None;
        }

        if self.idx.is_none() {
            // Save the current draft before going into history
            self.draft = HistoryItem {
                text: current_text,
                attachments: current_atts,
            };
            self.idx = Some(self.items.len().saturating_sub(1));
        } else {
            let current_idx = self.idx.unwrap();
            if current_idx > 0 {
                self.idx = Some(current_idx - 1);
            }
        }

        self.idx.map(|i| self.items[i].clone())
    }

    pub fn navigate_down(&mut self) -> Option<HistoryItem> {
        self.ensure_loaded();

        if let Some(current_idx) = self.idx {
            if current_idx + 1 < self.items.len() {
                self.idx = Some(current_idx + 1);
                Some(self.items[current_idx + 1].clone())
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
