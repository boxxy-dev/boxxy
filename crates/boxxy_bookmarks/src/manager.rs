use crate::Bookmark;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock, RwLock};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BookmarkEvent {
    Added(Bookmark),
    Updated(Bookmark),
    Removed(Uuid),
    Reordered(Vec<Uuid>),
    Reloaded,
}

pub static BOOKMARKS_EVENT_BUS: LazyLock<broadcast::Sender<BookmarkEvent>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(32);
    tx
});

static BOOKMARKS_CACHE: OnceLock<RwLock<BookmarksData>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BookmarksData {
    pub bookmarks: Vec<Bookmark>,
    pub order: Vec<Uuid>,
}

pub struct BookmarksManager;

impl BookmarksManager {
    fn get_base_dir() -> Option<PathBuf> {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir().join("bookmarks");
            if !config_dir.exists() {
                let _ = fs::create_dir_all(&config_dir);
            }
            return Some(config_dir);
        }
        None
    }

    fn get_index_path() -> Option<PathBuf> {
        Self::get_base_dir().map(|d| d.join("bookmarks.json"))
    }

    fn get_script_path(filename: &str) -> Option<PathBuf> {
        Self::get_base_dir().map(|d| d.join(filename))
    }

    pub fn get_runs_dir() -> Option<PathBuf> {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let runs_dir = dirs
                .config_dir()
                .join("cache")
                .join("bookmarks")
                .join("runs");
            if !runs_dir.exists() {
                let _ = fs::create_dir_all(&runs_dir);
            }
            return Some(runs_dir);
        }
        None
    }

    pub fn clean_runs_dir() {
        if let Some(runs_dir) = Self::get_runs_dir() {
            let _ = fs::remove_dir_all(&runs_dir);
            let _ = fs::create_dir_all(&runs_dir);
        }
    }

    pub fn init() {
        // Clean up runs dir on startup
        Self::clean_runs_dir();

        let _ = BOOKMARKS_CACHE.get_or_init(|| {
            let mut data = BookmarksData::default();
            if let Some(path) = Self::get_index_path()
                && let Ok(content) = fs::read_to_string(path)
                && let Ok(d) = serde_json::from_str::<BookmarksData>(&content) {
                    data = d;
                }

            // Sync order with bookmarks if needed
            if data.order.len() != data.bookmarks.len() {
                data.order = data.bookmarks.iter().map(|b| b.id).collect();
            }

            // Migration & Lazy Load Preparation
            let mut migrated = false;
            for bm in &mut data.bookmarks {
                // If script is still in JSON, migrate to file
                if !bm.script.is_empty() {
                    if bm.filename.is_empty() {
                        bm.filename = Self::generate_unique_filename(&bm.name, &bm.script, None);
                    }
                    if let Some(path) = Self::get_script_path(&bm.filename) {
                        let _ = fs::write(path, &bm.script);
                        bm.script = String::new(); // Clear from index
                        migrated = true;
                    }
                }
            }

            if migrated {
                let index = data.clone();
                if let Some(path) = Self::get_index_path()
                    && let Ok(content) = serde_json::to_string_pretty(&index)
                {
                    let _ = fs::write(path, content);
                }
            }

            RwLock::new(data)
        });
    }

    fn get_data() -> BookmarksData {
        if let Some(cache) = BOOKMARKS_CACHE.get() {
            return cache.read().unwrap().clone();
        }
        Self::init();
        Self::get_data()
    }

    fn save_data(data: &BookmarksData) {
        if let Some(cache) = BOOKMARKS_CACHE.get() {
            *cache.write().unwrap() = data.clone();
        }

        if let Some(path) = Self::get_index_path()
            && let Ok(content) = serde_json::to_string_pretty(data)
        {
            let _ = fs::write(path, content);
        }
    }

    pub fn list() -> Vec<Bookmark> {
        let data = Self::get_data();
        let mut sorted = Vec::with_capacity(data.bookmarks.len());

        for id in &data.order {
            if let Some(bm) = data.bookmarks.iter().find(|b| &b.id == id) {
                sorted.push(bm.clone());
            }
        }

        // Add any bookmarks missing from order (shouldn't happen but for safety)
        for bm in data.bookmarks {
            if !data.order.contains(&bm.id) {
                sorted.push(bm);
            }
        }

        sorted
    }

    pub fn add(name: String, script: String) -> Bookmark {
        let mut data = Self::get_data();
        let filename = Self::generate_unique_filename(&name, &script, None);

        // Save script to file
        if let Some(path) = Self::get_script_path(&filename) {
            let _ = fs::write(path, &script);
        }

        let mut bookmark = Bookmark::new(name, String::new(), filename);
        data.bookmarks.push(bookmark.clone());
        data.order.push(bookmark.id);

        Self::save_data(&data);

        // Put script back for the event so UI can use it immediately if needed
        bookmark.script = script;
        let _ = BOOKMARKS_EVENT_BUS.send(BookmarkEvent::Added(bookmark.clone()));
        bookmark
    }

    pub fn update(id: Uuid, name: String, script: String) -> Option<Bookmark> {
        let mut data = Self::get_data();
        if let Some(bm) = data.bookmarks.iter_mut().find(|b| b.id == id) {
            let name_changed = bm.name != name;
            let old_filename = bm.filename.clone();

            if name_changed {
                let script_to_use = Self::get_script(&old_filename).unwrap_or_default();
                let new_filename = Self::generate_unique_filename(&name, &script_to_use, Some(id));
                if new_filename != old_filename {
                    if let (Some(old_path), Some(new_path)) = (
                        Self::get_script_path(&old_filename),
                        Self::get_script_path(&new_filename),
                    ) {
                        let _ = fs::rename(old_path, new_path);
                    }
                    bm.filename = new_filename;
                }
            }

            bm.name = name;

            // Save script to file
            if let Some(path) = Self::get_script_path(&bm.filename) {
                let _ = fs::write(path, &script);
            }

            let mut updated = bm.clone();
            Self::save_data(&data);

            updated.script = script; // Send with script in event
            let _ = BOOKMARKS_EVENT_BUS.send(BookmarkEvent::Updated(updated.clone()));
            return Some(updated);
        }
        None
    }

    pub fn delete(id: Uuid) {
        let mut data = Self::get_data();
        if let Some(idx) = data.bookmarks.iter().position(|b| b.id == id) {
            let bm = &data.bookmarks[idx];
            if let Some(path) = Self::get_script_path(&bm.filename) {
                let _ = fs::remove_file(path);
            }
            data.bookmarks.remove(idx);
            data.order.retain(|u| u != &id);
            Self::save_data(&data);
            let _ = BOOKMARKS_EVENT_BUS.send(BookmarkEvent::Removed(id));
        }
    }

    pub fn reorder(new_order: Vec<Uuid>) {
        let mut data = Self::get_data();
        data.order = new_order.clone();
        Self::save_data(&data);
        let _ = BOOKMARKS_EVENT_BUS.send(BookmarkEvent::Reordered(new_order));
    }

    pub fn get_by_id(id: Uuid) -> Option<Bookmark> {
        let data = Self::get_data();
        if let Some(mut bm) = data.bookmarks.into_iter().find(|b| b.id == id) {
            bm.script = Self::get_script(&bm.filename).unwrap_or_default();
            return Some(bm);
        }
        None
    }

    pub fn get_script(filename: &str) -> Option<String> {
        if let Some(path) = Self::get_script_path(filename) {
            return fs::read_to_string(path).ok();
        }
        None
    }

    fn generate_unique_filename(name: &str, script: &str, current_id: Option<Uuid>) -> String {
        let name = name.trim();
        let (base, ext) = if let Some(pos) = name.rfind('.') {
            let (b, e) = name.split_at(pos);
            if e.len() > 1 && (e == ".sh" || e == ".py" || e == ".js" || e == ".rb") {
                (b, e)
            } else {
                let default_ext = if script.starts_with("```python")
                    || script.contains("#!/usr/bin/env python")
                    || script.contains("#!/usr/bin/python")
                {
                    ".py"
                } else {
                    ".sh"
                };
                (name, default_ext)
            }
        } else {
            let default_ext = if script.starts_with("```python")
                || script.contains("#!/usr/bin/env python")
                || script.contains("#!/usr/bin/python")
            {
                ".py"
            } else {
                ".sh"
            };
            (name, default_ext)
        };

        let sanitized = base
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();

        let mut filename = format!("{}{}", sanitized, ext);
        let mut count = 1;

        let data = if let Some(cache) = BOOKMARKS_CACHE.get() {
            cache.read().unwrap().clone()
        } else {
            BookmarksData::default()
        };

        while data
            .bookmarks
            .iter()
            .any(|b| b.filename == filename && Some(b.id) != current_id)
        {
            count += 1;
            filename = format!("{}_{}{}", sanitized, count, ext);
        }

        filename
    }
}
