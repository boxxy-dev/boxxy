use crate::{Bookmark, ScriptLanguage};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock};
use tokio::sync::broadcast;
pub use tokio::sync::broadcast::error::RecvError as BroadcastRecvError;
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
    /// In-memory cache of filename -> script contents to avoid synchronous disk
    /// reads on the GTK main thread during ListView scrolling and sidebar binds.
    #[serde(skip)]
    pub scripts: HashMap<String, String>,
}

pub struct BookmarksManager;

impl BookmarksManager {
    fn get_base_dir() -> Option<PathBuf> {
        let config_dir = boxxy_sys_utils::get_config_dir().join("bookmarks");
        if !config_dir.exists() {
            let _ = fs::create_dir_all(&config_dir);
        }
        Some(config_dir)
    }

    fn get_index_path() -> Option<PathBuf> {
        Self::get_base_dir().map(|d| d.join("bookmarks.json"))
    }

    fn get_script_path(filename: &str) -> Option<PathBuf> {
        Self::get_base_dir().map(|d| d.join(filename))
    }

    #[must_use]
    pub fn get_runs_dir() -> Option<PathBuf> {
        let config_dir = boxxy_sys_utils::get_config_dir();
        let runs_dir = config_dir.join("cache").join("bookmarks").join("runs");
        if !runs_dir.exists() {
            let _ = fs::create_dir_all(&runs_dir);
        }
        Some(runs_dir)
    }

    pub fn clean_runs_dir() {
        if let Some(runs_dir) = Self::get_runs_dir() {
            let _ = fs::remove_dir_all(&runs_dir);
            let _ = fs::create_dir_all(&runs_dir);
        }
    }

    fn load_from_disk() -> BookmarksData {
        let mut data = BookmarksData::default();
        if let Some(path) = Self::get_index_path()
            && let Ok(content) = fs::read_to_string(path)
            && let Ok(d) = serde_json::from_str::<BookmarksData>(&content)
        {
            data = d;
        }

        // Sync order with bookmarks if needed
        if data.order.len() != data.bookmarks.len() {
            data.order = data.bookmarks.iter().map(|b| b.id).collect();
        }

        // Migration & In-Memory Cache Hydration
        let mut migrated = false;
        let existing_bookmarks = data.bookmarks.clone();
        for bm in &mut data.bookmarks {
            if !bm.script.is_empty() {
                if bm.filename.is_empty() {
                    bm.filename = Self::generate_unique_filename_in(
                        &existing_bookmarks,
                        &bm.name,
                        &bm.script,
                        Some(bm.id),
                    );
                }
                if let Some(path) = Self::get_script_path(&bm.filename) {
                    let _ = fs::write(path, &bm.script);
                    data.scripts.insert(bm.filename.clone(), bm.script.clone());
                    bm.script.clear();
                    migrated = true;
                }
            } else if !bm.filename.is_empty()
                && let Some(path) = Self::get_script_path(&bm.filename)
                && let Ok(script_content) = fs::read_to_string(path)
            {
                data.scripts.insert(bm.filename.clone(), script_content);
            }
        }

        if migrated
            && let Some(path) = Self::get_index_path()
            && let Ok(content) = serde_json::to_string_pretty(&data)
        {
            let _ = fs::write(path, content);
        }

        data
    }

    pub fn init() {
        Self::clean_runs_dir();
        let _ = BOOKMARKS_CACHE.get_or_init(|| RwLock::new(Self::load_from_disk()));
    }

    fn cache() -> &'static RwLock<BookmarksData> {
        BOOKMARKS_CACHE.get_or_init(|| RwLock::new(Self::load_from_disk()))
    }

    fn get_data() -> BookmarksData {
        Self::cache().read().clone()
    }

    fn save_data(data: &BookmarksData) {
        *Self::cache().write() = data.clone();

        if let Some(path) = Self::get_index_path()
            && let Ok(content) = serde_json::to_string_pretty(data)
        {
            let _ = fs::write(path, content);
        }
    }

    #[must_use]
    pub fn list() -> Vec<Bookmark> {
        let data = Self::get_data();
        let mut sorted = Vec::with_capacity(data.bookmarks.len());

        for id in &data.order {
            if let Some(bm) = data.bookmarks.iter().find(|b| &b.id == id) {
                sorted.push(bm.clone());
            }
        }

        for bm in data.bookmarks {
            if !data.order.contains(&bm.id) {
                sorted.push(bm);
            }
        }

        sorted
    }

    #[must_use]
    pub fn count() -> usize {
        Self::cache().read().bookmarks.len()
    }

    pub fn add(name: String, script: String) -> Bookmark {
        let mut data = Self::get_data();
        let filename = Self::generate_unique_filename_in(&data.bookmarks, &name, &script, None);

        if let Some(path) = Self::get_script_path(&filename) {
            let _ = fs::write(path, &script);
        }

        data.scripts.insert(filename.clone(), script.clone());

        let mut bookmark = Bookmark::new(name, String::new(), filename);
        data.bookmarks.push(bookmark.clone());
        data.order.push(bookmark.id);

        Self::save_data(&data);

        bookmark.script = script;
        let _ = BOOKMARKS_EVENT_BUS.send(BookmarkEvent::Added(bookmark.clone()));
        bookmark
    }

    pub fn update(id: Uuid, name: String, script: String) -> Option<Bookmark> {
        let mut data = Self::get_data();
        let existing_bookmarks = data.bookmarks.clone();

        if let Some(bm) = data.bookmarks.iter_mut().find(|b| b.id == id) {
            let name_changed = bm.name != name;
            let old_filename = bm.filename.clone();

            if name_changed {
                let new_filename = Self::generate_unique_filename_in(
                    &existing_bookmarks,
                    &name,
                    &script,
                    Some(id),
                );
                if new_filename != old_filename {
                    if let (Some(old_path), Some(new_path)) = (
                        Self::get_script_path(&old_filename),
                        Self::get_script_path(&new_filename),
                    ) {
                        let _ = fs::rename(old_path, new_path);
                    }
                    data.scripts.remove(&old_filename);
                    bm.filename = new_filename;
                }
            }

            bm.name = name;

            if let Some(path) = Self::get_script_path(&bm.filename) {
                let _ = fs::write(path, &script);
            }
            data.scripts.insert(bm.filename.clone(), script.clone());

            let mut updated = bm.clone();
            Self::save_data(&data);

            updated.script = script;
            let _ = BOOKMARKS_EVENT_BUS.send(BookmarkEvent::Updated(updated.clone()));
            return Some(updated);
        }
        None
    }

    pub fn delete(id: Uuid) {
        let mut data = Self::get_data();
        if let Some(idx) = data.bookmarks.iter().position(|b| b.id == id) {
            let filename = data.bookmarks[idx].filename.clone();
            if let Some(path) = Self::get_script_path(&filename) {
                let _ = fs::remove_file(path);
            }
            data.scripts.remove(&filename);
            data.bookmarks.remove(idx);
            data.order.retain(|u| u != &id);
            Self::save_data(&data);
            let _ = BOOKMARKS_EVENT_BUS.send(BookmarkEvent::Removed(id));
        }
    }

    pub fn reorder(new_order: Vec<Uuid>) {
        let mut data = Self::get_data();
        data.order.clone_from(&new_order);
        Self::save_data(&data);
        let _ = BOOKMARKS_EVENT_BUS.send(BookmarkEvent::Reordered(new_order));
    }

    #[must_use]
    pub fn get_by_id(id: Uuid) -> Option<Bookmark> {
        let data = Self::get_data();
        if let Some(mut bm) = data.bookmarks.into_iter().find(|b| b.id == id) {
            bm.script = Self::get_script(&bm.filename).unwrap_or_default();
            return Some(bm);
        }
        None
    }

    #[must_use]
    pub fn get_script(filename: &str) -> Option<String> {
        if filename.is_empty() {
            return None;
        }
        if let Some(cached) = Self::cache().read().scripts.get(filename).cloned() {
            return Some(cached);
        }
        if let Some(path) = Self::get_script_path(filename)
            && let Ok(content) = fs::read_to_string(path)
        {
            Self::cache()
                .write()
                .scripts
                .insert(filename.to_string(), content.clone());
            return Some(content);
        }
        None
    }

    fn generate_unique_filename_in(
        existing: &[Bookmark],
        name: &str,
        script: &str,
        current_id: Option<Uuid>,
    ) -> String {
        // Normalize common comma-for-dot typos at the end of a script name (e.g. `test,py` -> `test.py`)
        let normalized_name = {
            let trimmed = name.trim();
            if let Some(prefix) = trimmed.strip_suffix(",py") {
                format!("{prefix}.py")
            } else if let Some(prefix) = trimmed.strip_suffix(",sh") {
                format!("{prefix}.sh")
            } else {
                trimmed.to_string()
            }
        };

        let default_ext = ScriptLanguage::detect(&normalized_name, script).default_extension();

        let (base, ext) = if let Some(pos) = normalized_name.rfind('.') {
            let (b, e) = normalized_name.split_at(pos);
            if e.len() > 1 && matches!(e, ".sh" | ".py" | ".js" | ".rb") {
                (b, e)
            } else {
                (normalized_name.as_str(), default_ext)
            }
        } else {
            (normalized_name.as_str(), default_ext)
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

        let sanitized = if sanitized.is_empty() {
            "bookmark".to_string()
        } else {
            sanitized
        };

        let mut filename = format!("{sanitized}{ext}");
        let mut count = 1;

        while existing
            .iter()
            .any(|b| b.filename == filename && Some(b.id) != current_id)
        {
            count += 1;
            filename = format!("{sanitized}_{count}{ext}");
        }

        filename
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_clean_unique_filenames() {
        let existing = vec![Bookmark::new(
            "deploy".to_string(),
            String::new(),
            "deploy.sh".to_string(),
        )];

        // Duplicate name gets incremented suffix
        let next = BookmarksManager::generate_unique_filename_in(
            &existing,
            "deploy",
            "#!/bin/bash\necho hi",
            None,
        );
        assert_eq!(next, "deploy_2.sh");

        // Comma typo `test,py` normalizes to `test.py`
        let py_name =
            BookmarksManager::generate_unique_filename_in(&existing, "test,py", "print(1)", None);
        assert_eq!(py_name, "test.py");
    }
}
