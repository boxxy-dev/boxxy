use log::{error, info, warn};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

fn get_max_mtime(dir: &Path) -> SystemTime {
    let mut max_mtime = SystemTime::UNIX_EPOCH;
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "toml" || ext == "png" || ext == "json" {
                if let Ok(metadata) = std::fs::metadata(path) {
                    if let Ok(mtime) = metadata.modified() {
                        if mtime > max_mtime {
                            max_mtime = mtime;
                        }
                    }
                }
            }
        }
    }
    max_mtime
}

pub async fn spawn_character_watcher(
    characters_dir: PathBuf,
    tx: tokio::sync::mpsc::Sender<()>,
) {
    tokio::task::spawn_blocking(move || {
        let (inner_tx, inner_rx) = std::sync::mpsc::channel();

        let mut debouncer = match new_debouncer(Duration::from_millis(500), inner_tx) {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to create character debouncer: {}", e);
                return;
            }
        };

        if !characters_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&characters_dir) {
                error!("Failed to create characters directory {:?}: {}", characters_dir, e);
            }
        }

        if let Err(e) = debouncer
            .watcher()
            .watch(&characters_dir, RecursiveMode::Recursive)
        {
            warn!("Failed to watch characters directory {:?}: {}", characters_dir, e);
            return;
        }

        info!("Watching character directory for changes: {:?}", characters_dir);

        let mut last_mtime = get_max_mtime(&characters_dir);

        for res in inner_rx {
            match res {
                Ok(_events) => {
                    let current_mtime = get_max_mtime(&characters_dir);
                    if current_mtime > last_mtime {
                        last_mtime = current_mtime;
                        if tx.blocking_send(()).is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    error!("Character watcher error: {:?}", e);
                }
            }
        }

        drop(debouncer);
    });
}
