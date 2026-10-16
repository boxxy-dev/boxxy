use log::{error, info, warn};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

type DirSnapshot = BTreeMap<PathBuf, (u64, SystemTime)>;

fn get_dir_snapshot(dir: &Path) -> DirSnapshot {
    let mut snapshot = BTreeMap::new();
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if let Some(ext) = path.extension()
            && (ext == "toml" || ext == "png" || ext == "json")
            && let Ok(metadata) = std::fs::metadata(path)
        {
            let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let rel = path.strip_prefix(dir).unwrap_or(path).to_path_buf();
            snapshot.insert(rel, (metadata.len(), mtime));
        }
    }
    snapshot
}

fn has_any_toml(snapshot: &DirSnapshot) -> bool {
    snapshot
        .keys()
        .any(|p| p.extension().is_some_and(|ext| ext == "toml"))
}

pub async fn spawn_character_watcher(characters_dir: PathBuf, tx: tokio::sync::mpsc::Sender<()>) {
    tokio::task::spawn_blocking(move || {
        let (inner_tx, inner_rx) = std::sync::mpsc::channel();

        let mut debouncer = match new_debouncer(Duration::from_millis(500), inner_tx) {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to create character debouncer: {}", e);
                return;
            }
        };

        if !characters_dir.exists()
            && let Err(e) = std::fs::create_dir_all(&characters_dir)
        {
            error!(
                "Failed to create characters directory {:?}: {}",
                characters_dir, e
            );
        }

        if let Err(e) = debouncer
            .watcher()
            .watch(&characters_dir, RecursiveMode::Recursive)
        {
            warn!(
                "Failed to watch characters directory {:?}: {}",
                characters_dir, e
            );
            return;
        }

        info!(
            "Watching character directory for changes: {:?}",
            characters_dir
        );

        let mut last_snapshot = get_dir_snapshot(&characters_dir);

        for res in inner_rx {
            match res {
                Ok(_events) => {
                    if !characters_dir.exists() {
                        let _ = std::fs::create_dir_all(&characters_dir);
                        let _ = debouncer
                            .watcher()
                            .watch(&characters_dir, RecursiveMode::Recursive);
                    }

                    let current_snapshot = get_dir_snapshot(&characters_dir);
                    if current_snapshot != last_snapshot || !has_any_toml(&current_snapshot) {
                        last_snapshot = current_snapshot;
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
