use log::{debug, error, info, warn};
use notify_debouncer_mini::{
    Config, DebounceEventResult, DebouncedEventBatch, DebouncedEventKind, Debouncer,
    new_debouncer_opt,
    notify::{
        self, ErrorKind, EventKindMask, PathOp, RecommendedWatcher, RecursiveMode, WatchPathConfig,
        Watcher,
    },
};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

const DEFAULT_DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(500);

/// Per-watch `notify` v9 `user_data` tags used to distinguish failures on the primary
/// recursive `characters/` watch from the optional non-recursive parent watch.
const WATCH_CHARACTERS_DIR: u64 = 1;
const WATCH_CHARACTERS_PARENT: u64 = 2;

type DirSnapshot = BTreeMap<PathBuf, (u64, SystemTime)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchAction {
    Ignore,
    CheckSnapshot { force_rewatch: bool },
}

const fn is_watched_character_extension(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("toml")
        || ext.eq_ignore_ascii_case("png")
        || ext.eq_ignore_ascii_case("webp")
        || ext.eq_ignore_ascii_case("jpg")
        || ext.eq_ignore_ascii_case("jpeg")
        || ext.eq_ignore_ascii_case("json")
}

fn is_hidden_or_temp_name(name: &OsStr) -> bool {
    let Some(s) = name.to_str() else {
        return true;
    };
    s.is_empty()
        || s.starts_with('.')
        || s.ends_with('~')
        || Path::new(s)
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("tmp")
                    || ext.eq_ignore_ascii_case("swp")
                    || ext.eq_ignore_ascii_case("swo")
                    || ext.eq_ignore_ascii_case("bak")
            })
}

fn is_relevant_character_path(characters_dir: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(characters_dir) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return true;
    }

    let mut depth = 0usize;
    let mut last_name: Option<&OsStr> = None;
    for comp in rel.components() {
        let Component::Normal(name) = comp else {
            return false;
        };
        if is_hidden_or_temp_name(name) {
            return false;
        }
        depth += 1;
        last_name = Some(name);
    }

    // Direct children of `characters/` may be character package folders (even with dots in their
    // directory name, e.g. `niko.v2` after `rm -rf`) or `characters.json`; skip `README.md`.
    if depth == 1 {
        if let Some(name) = last_name.and_then(OsStr::to_str)
            && name.eq_ignore_ascii_case("readme.md")
        {
            return false;
        }
        if !path.is_file() {
            return true;
        }
    }

    rel.extension()
        .and_then(OsStr::to_str)
        .is_none_or(is_watched_character_extension)
}

fn classify_debounced_batch(characters_dir: &Path, batch: &DebouncedEventBatch) -> BatchAction {
    match batch {
        // Immediate backend rescan notification (e.g. kernel queue overflow): bypass per-path
        // filters, verify watch coverage, and rescan the full directory snapshot.
        DebouncedEventBatch::Rescan => BatchAction::CheckSnapshot {
            force_rewatch: true,
        },
        DebouncedEventBatch::Events(events) => {
            let mut has_settled_relevant = false;
            let mut root_touched = false;

            for event in events {
                if !is_relevant_character_path(characters_dir, &event.path) {
                    continue;
                }
                if event.path == characters_dir {
                    root_touched = true;
                }
                // `AnyContinuous` indicates a file is still actively being written; the mini
                // debouncer guarantees a final `Any` event once writes settle for the timeout.
                if event.kind != DebouncedEventKind::AnyContinuous {
                    has_settled_relevant = true;
                }
            }

            if has_settled_relevant {
                BatchAction::CheckSnapshot {
                    force_rewatch: root_touched,
                }
            } else {
                BatchAction::Ignore
            }
        }
        _ => BatchAction::Ignore,
    }
}

fn handle_watcher_error(err: &notify::Error) -> BatchAction {
    match &err.kind {
        ErrorKind::PathNotFound => {
            debug!(
                "Character watcher path missing ({:?}); scheduling watch recovery.",
                err.paths
            );
            BatchAction::CheckSnapshot {
                force_rewatch: true,
            }
        }
        ErrorKind::MaxFilesWatch => {
            warn!(
                "Character watcher reached OS file watch limit ({err}). \
                 Consider increasing 'fs.inotify.max_user_watches'."
            );
            BatchAction::Ignore
        }
        ErrorKind::WatchStartFailed => {
            warn!("Character watcher failed to start or restart event stream: {err}");
            BatchAction::CheckSnapshot {
                force_rewatch: true,
            }
        }
        ErrorKind::Io(io_err) if io_err.raw_os_error() == Some(libc::EMFILE) => {
            warn!(
                "Character watcher exhausted file descriptors ({err}). \
                 Check 'fs.inotify.max_user_instances' or open file limits."
            );
            BatchAction::Ignore
        }
        _ => {
            error!("Character watcher error: {err}");
            BatchAction::Ignore
        }
    }
}

fn get_dir_snapshot(dir: &Path) -> DirSnapshot {
    let mut snapshot = BTreeMap::new();
    // Follow symlinks to match `notify::Config::default()` (`follow_symlinks: true`) and
    // `character_loader`'s symlinked character package support; `WalkDir` detects cycles safely.
    let walker = WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden_or_temp_name(e.file_name()));

    for entry in walker.filter_map(Result::ok) {
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(OsStr::to_str)
            && is_watched_character_extension(ext)
            && let Ok(metadata) = entry.metadata()
            && metadata.is_file()
        {
            let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let rel = path.strip_prefix(dir).unwrap_or(path).to_path_buf();
            snapshot.insert(rel, (metadata.len(), mtime));
        }
    }
    snapshot
}

fn has_any_toml(snapshot: &DirSnapshot) -> bool {
    snapshot.keys().any(|p| {
        p.extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
    })
}

fn ensure_characters_watch(
    watcher: &mut dyn Watcher,
    characters_dir: &Path,
    force_rewatch: bool,
) {
    let dir_created = if characters_dir.exists() {
        false
    } else {
        if let Err(e) = std::fs::create_dir_all(characters_dir) {
            warn!(
                "Failed to recreate characters directory {}: {e}",
                characters_dir.display()
            );
            return;
        }
        true
    };

    let is_watched = watcher
        .watched_paths()
        .is_ok_and(|paths| paths.iter().any(|wp| wp.path == characters_dir));

    if (force_rewatch || dir_created || !is_watched)
        && let Err(e) = watcher.watch_with(
            characters_dir,
            WatchPathConfig::new(RecursiveMode::Recursive).with_user_data(WATCH_CHARACTERS_DIR),
        )
    {
        warn!(
            "Failed to re-watch characters directory {}: {e}",
            characters_dir.display()
        );
    }
}

fn init_character_debouncer(
    characters_dir: &Path,
    timeout: Duration,
    inner_tx: tokio::sync::mpsc::UnboundedSender<DebounceEventResult>,
) -> Option<(Debouncer<RecommendedWatcher>, DirSnapshot)> {
    if !characters_dir.exists()
        && let Err(e) = std::fs::create_dir_all(characters_dir)
    {
        error!(
            "Failed to create characters directory {}: {e}",
            characters_dir.display()
        );
    }

    let notify_config = notify::Config::default().with_event_kinds(EventKindMask::CORE);
    let config = Config::default()
        .with_timeout(timeout)
        .with_notify_config(notify_config);

    let mut debouncer = match new_debouncer_opt::<_, RecommendedWatcher>(config, inner_tx) {
        Ok(d) => d,
        Err(e) => {
            error!("Failed to create character debouncer: {e}");
            return None;
        }
    };

    // Register `characters_dir` first so it stays active even if watching its parent fails.
    // Watching the parent non-recursively lets us detect atomic directory replacements
    // (`rm -rf characters && mv new_chars characters`) within a single debounce window.
    let mut ops = vec![PathOp::Watch(
        characters_dir.to_path_buf(),
        WatchPathConfig::new(RecursiveMode::Recursive).with_user_data(WATCH_CHARACTERS_DIR),
    )];
    if let Some(parent) = characters_dir.parent()
        && !parent.as_os_str().is_empty()
        && parent.exists()
    {
        ops.push(PathOp::Watch(
            parent.to_path_buf(),
            WatchPathConfig::new(RecursiveMode::NonRecursive)
                .with_user_data(WATCH_CHARACTERS_PARENT),
        ));
    }

    if let Err(err) = debouncer.watcher().update_paths(ops) {
        if err.source.user_data() == Some(WATCH_CHARACTERS_PARENT) {
            debug!(
                "Optional non-recursive parent watch for {} unavailable: {}",
                characters_dir.display(),
                err.source
            );
        } else {
            warn!(
                "Failed to watch characters directory {}: {}",
                characters_dir.display(),
                err.source
            );
            return None;
        }
    }

    info!(
        "Watching character directory for changes: {}",
        characters_dir.display()
    );

    let initial_snapshot = get_dir_snapshot(characters_dir);
    Some((debouncer, initial_snapshot))
}

pub fn spawn_character_watcher(characters_dir: PathBuf, tx: tokio::sync::mpsc::Sender<()>) {
    spawn_character_watcher_with_timeout(characters_dir, DEFAULT_DEBOUNCE_TIMEOUT, tx);
}

fn spawn_character_watcher_with_timeout(
    characters_dir: PathBuf,
    timeout: Duration,
    tx: tokio::sync::mpsc::Sender<()>,
) {
    tokio::spawn(async move {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let dir_for_init = characters_dir.clone();

        let Ok(Some((debouncer, mut last_snapshot))) =
            tokio::task::spawn_blocking(move || {
                init_character_debouncer(&dir_for_init, timeout, inner_tx)
            })
            .await
        else {
            return;
        };

        let debouncer = Arc::new(parking_lot::Mutex::new(debouncer));
        let mut notified_missing_toml = false;

        loop {
            let res = tokio::select! {
                () = tx.closed() => break,
                maybe_res = inner_rx.recv() => match maybe_res {
                    Some(res) => res,
                    None => break,
                },
            };

            let mut action = match &res {
                Ok(batch) => classify_debounced_batch(&characters_dir, batch),
                Err(err) => handle_watcher_error(err),
            };

            // Coalesce any additional batches already queued in the channel.
            while let Ok(queued) = inner_rx.try_recv() {
                let next = match &queued {
                    Ok(batch) => classify_debounced_batch(&characters_dir, batch),
                    Err(err) => handle_watcher_error(err),
                };
                action = match (action, next) {
                    (
                        BatchAction::CheckSnapshot { force_rewatch: a },
                        BatchAction::CheckSnapshot { force_rewatch: b },
                    ) => BatchAction::CheckSnapshot {
                        force_rewatch: a || b,
                    },
                    (BatchAction::CheckSnapshot { force_rewatch }, BatchAction::Ignore)
                    | (BatchAction::Ignore, BatchAction::CheckSnapshot { force_rewatch }) => {
                        BatchAction::CheckSnapshot { force_rewatch }
                    }
                    (BatchAction::Ignore, BatchAction::Ignore) => BatchAction::Ignore,
                };
            }

            let BatchAction::CheckSnapshot { force_rewatch } = action else {
                continue;
            };

            let debouncer_for_step = Arc::clone(&debouncer);
            let dir_for_step = characters_dir.clone();
            let Ok(current_snapshot) = tokio::task::spawn_blocking(move || {
                {
                    let mut guard = debouncer_for_step.lock();
                    ensure_characters_watch(guard.watcher(), &dir_for_step, force_rewatch);
                }
                get_dir_snapshot(&dir_for_step)
            })
            .await
            else {
                continue;
            };

            let missing_toml = !has_any_toml(&current_snapshot);
            if current_snapshot != last_snapshot || (missing_toml && !notified_missing_toml) {
                last_snapshot = current_snapshot;
                notified_missing_toml = missing_toml;
                if tx.send(()).await.is_err() {
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_debouncer_mini::DebouncedEvent;

    #[test]
    fn path_filter_accepts_character_assets_and_rejects_noise() {
        let root = Path::new("/tmp/synthetic_cfg/boxxyclaw/characters");

        // Relevant character package files & directories
        assert!(is_relevant_character_path(root, root));
        assert!(is_relevant_character_path(
            root,
            &root.join("alpha-pack/CHARACTER.toml")
        ));
        assert!(is_relevant_character_path(
            root,
            &root.join("alpha-pack/AVATAR.PNG")
        ));
        assert!(is_relevant_character_path(
            root,
            &root.join("alpha-pack/portrait.webp")
        ));
        assert!(is_relevant_character_path(
            root,
            &root.join("characters.json")
        ));
        assert!(is_relevant_character_path(root, &root.join("alpha-pack")));
        assert!(is_relevant_character_path(root, &root.join("alpha.v2")));

        // Sibling files in `boxxyclaw/`, hidden/atomic temp files, swap files, and README
        assert!(!is_relevant_character_path(
            root,
            Path::new("/tmp/synthetic_cfg/boxxyclaw/MEMORY.md")
        ));
        assert!(!is_relevant_character_path(
            root,
            &root.join("alpha-pack/.CHARACTER.toml.1234.0.tmp")
        ));
        assert!(!is_relevant_character_path(
            root,
            &root.join("alpha-pack/.CHARACTER.toml.swp")
        ));
        assert!(!is_relevant_character_path(
            root,
            &root.join("alpha-pack/CHARACTER.toml~")
        ));
        assert!(!is_relevant_character_path(
            root,
            &root.join("alpha-pack/notes.bak")
        ));
        assert!(!is_relevant_character_path(root, &root.join("README.md")));
    }

    #[test]
    fn batch_classifier_handles_rescan_and_skips_in_progress_writes() {
        let root = Path::new("/tmp/synthetic_cfg/boxxyclaw/characters");

        // Rescan always forces watch verification and snapshot diffing.
        assert_eq!(
            classify_debounced_batch(root, &DebouncedEventBatch::Rescan),
            BatchAction::CheckSnapshot {
                force_rewatch: true
            }
        );

        // Ongoing continuous writes wait for the guaranteed settled `Any` event.
        let streaming = DebouncedEventBatch::Events(vec![DebouncedEvent::new(
            root.join("alpha/portrait.webp"),
            DebouncedEventKind::AnyContinuous,
        )]);
        assert_eq!(classify_debounced_batch(root, &streaming), BatchAction::Ignore);

        // Once settled (`Any`), snapshot check fires without forcing re-watch.
        let settled = DebouncedEventBatch::Events(vec![DebouncedEvent::new(
            root.join("alpha/portrait.webp"),
            DebouncedEventKind::Any,
        )]);
        assert_eq!(
            classify_debounced_batch(root, &settled),
            BatchAction::CheckSnapshot {
                force_rewatch: false
            }
        );

        // Root directory replacement sets `force_rewatch: true`.
        let root_event = DebouncedEventBatch::Events(vec![DebouncedEvent::new(
            root.to_path_buf(),
            DebouncedEventKind::Any,
        )]);
        assert_eq!(
            classify_debounced_batch(root, &root_event),
            BatchAction::CheckSnapshot {
                force_rewatch: true
            }
        );
    }

    #[tokio::test]
    async fn watcher_detects_edits_ignores_reads_and_survives_dir_replacement() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let boxxyclaw_dir = tmp.path().join("boxxyclaw");
        let characters_dir = boxxyclaw_dir.join("characters");
        let pack_dir = characters_dir.join("alpha");
        std::fs::create_dir_all(&pack_dir).expect("create initial pack");
        let char_toml = pack_dir.join("CHARACTER.toml");
        std::fs::write(&char_toml, "id = \"1\"\nname = \"alpha\"\n").expect("write initial toml");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_character_watcher_with_timeout(
            characters_dir.clone(),
            Duration::from_millis(50),
            tx,
        );

        // Give the background `spawn_blocking` init task a brief moment to register watches.
        tokio::time::sleep(Duration::from_millis(60)).await;

        // 1. Reading files and writing sibling files or swap files must NOT trigger reloads.
        let _ = std::fs::read(&char_toml).expect("read toml");
        std::fs::write(boxxyclaw_dir.join("MEMORY.md"), "# Memory\n").expect("write sibling");
        std::fs::write(pack_dir.join(".CHARACTER.toml.swp"), "swap").expect("write swap");
        assert!(
            tokio::time::timeout(Duration::from_millis(150), rx.recv())
                .await
                .is_err(),
            "reads, sibling writes, and swap files must not trigger character reload"
        );

        // 2. Modifying CHARACTER.toml triggers a notification.
        std::fs::write(&char_toml, "id = \"1\"\nname = \"alpha\"\ncolor = \"#112233\"\n")
            .expect("update toml");
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("timed out waiting for character edit notification")
            .expect("watcher channel closed unexpectedly");

        // 3. Replacing the entire `characters/` directory within a single burst (`rm -rf` + `rename`)
        //    triggers a reload AND keeps watching the replacement directory for subsequent edits.
        let staged_dir = tmp.path().join("staged_characters");
        let beta_dir = staged_dir.join("beta");
        std::fs::create_dir_all(&beta_dir).expect("create staged dir");
        std::fs::write(
            beta_dir.join("CHARACTER.toml"),
            "id = \"2\"\nname = \"beta\"\n",
        )
        .expect("write staged toml");

        std::fs::remove_dir_all(&characters_dir).expect("remove old characters_dir");
        std::fs::rename(&staged_dir, &characters_dir).expect("rename replacement characters_dir");

        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("timed out waiting for directory replacement notification")
            .expect("watcher channel closed unexpectedly");
        while rx.try_recv().is_ok() {}

        // 4. Subsequent edits inside the replaced `characters/` tree still fire reliably.
        std::fs::write(
            characters_dir.join("beta/CHARACTER.toml"),
            "id = \"2\"\nname = \"beta\"\ncolor = \"#445566\"\n",
        )
        .expect("edit file inside replaced characters_dir");

        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("timed out waiting for edit after directory replacement")
            .expect("watcher channel closed unexpectedly");
    }
}
