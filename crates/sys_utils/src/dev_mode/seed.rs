//! Isolated dev-directory seeding and `flock`-coordinated ephemeral session lifecycle.

use crate::config_dir::{get_stable_config_dir, resolve_ephemeral_config_dir};
use crate::dev_mode::{resolve_instance_and_ephemeral, should_sync_settings};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

const EPHEMERAL_LOCK_FILE: &str = ".ephemeral.lock";
const MAX_SEED_DEPTH: usize = 16;

static EPHEMERAL_SESSION_LOCK: OnceLock<File> = OnceLock::new();
static SEED_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn create_private_dir_all(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

fn open_lock_file(lock_path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(lock_path)
}

#[cfg(unix)]
fn is_safe_owned_directory(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return false;
    }
    // SAFETY: `geteuid` is an infallible POSIX syscall with no pointer arguments.
    let euid = unsafe { libc::geteuid() };
    meta.uid() == euid
}

#[cfg(not(unix))]
fn is_safe_owned_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir())
}

/// Prepares the isolated development or ephemeral configuration directory before
/// `Settings::init()` runs. No-op when running as the Stable installation.
pub fn prepare_dev_environment() {
    let (Some(slug), ephemeral) = resolve_instance_and_ephemeral() else {
        return;
    };

    let target_dir = crate::get_config_dir();

    if ephemeral {
        let canonical_ephemeral_dir = resolve_ephemeral_config_dir(&slug);
        if target_dir == canonical_ephemeral_dir {
            // Coordinate via `.ephemeral.lock` even when `BOXXY_CONFIG_DIR` was
            // explicitly forwarded to `boxxy-agent` by `daemon_spawn_env`: the first
            // process wipes stale artifacts under `LOCK_EX` and downgrades to `LOCK_SH`;
            // sibling processes join under `LOCK_SH` without wiping or seeding from Stable.
            if EPHEMERAL_SESSION_LOCK.get().is_none()
                && let Some(lock_file) = prepare_ephemeral_dir_in(&target_dir)
            {
                let _ = EPHEMERAL_SESSION_LOCK.set(lock_file);
            }
        } else if let Err(e) = create_private_dir_all(&target_dir) {
            // Custom `--config-dir` alongside `--ephemeral`: create if missing,
            // never wipe user-specified paths, and never seed Stable files.
            log::warn!(
                "Failed to create custom ephemeral config directory '{}': {e}",
                target_dir.display()
            );
        }
        return;
    }

    let stable_dir = get_stable_config_dir();
    if target_dir == stable_dir || !stable_dir.is_dir() {
        return;
    }

    let needs_initial_seed = !target_dir.join("settings.json").exists();
    if needs_initial_seed || should_sync_settings() {
        match seed_config_tree_from(&stable_dir, &target_dir) {
            Ok(()) => {
                log::info!(
                    "Synced dev settings into '{}' from '{}'",
                    target_dir.display(),
                    stable_dir.display()
                );
            }
            Err(e) => {
                log::warn!(
                    "Failed to sync dev settings into '{}': {e}",
                    target_dir.display()
                );
            }
        }
    }
}

/// Wipes `ephemeral_dir` if and only if no sibling ephemeral process (`boxxy-agent` or
/// `boxxy-app`) currently holds a shared lock on `.ephemeral.lock`, then returns a
/// shared-locked `File` handle to keep alive for the process lifetime.
pub(crate) fn prepare_ephemeral_dir_in(ephemeral_dir: &Path) -> Option<File> {
    create_private_dir_all(ephemeral_dir).ok()?;
    if !is_safe_owned_directory(ephemeral_dir) {
        log::warn!(
            "Refusing to use ephemeral directory '{}' (not an owner-controlled real directory)",
            ephemeral_dir.display()
        );
        return None;
    }

    let lock_path = ephemeral_dir.join(EPHEMERAL_LOCK_FILE);
    let file = open_lock_file(&lock_path).ok()?;

    match file.try_lock() {
        Ok(()) => {
            // Exclusive lock acquired: no sibling process holds `LOCK_SH` or `LOCK_EX`.
            // Wipe leftover artifacts from prior runs without following symlinks.
            if let Ok(entries) = fs::read_dir(ephemeral_dir) {
                for entry in entries.flatten() {
                    if entry.file_name() == EPHEMERAL_LOCK_FILE {
                        continue;
                    }
                    let path = entry.path();
                    if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                        let _ = fs::remove_dir_all(&path);
                    } else {
                        let _ = fs::remove_file(&path);
                    }
                }
            }

            #[cfg(unix)]
            {
                // POSIX/Linux `flock(fd, LOCK_SH)` atomically downgrades `LOCK_EX` -> `LOCK_SH`
                // in-place with zero gap where a concurrent sibling could acquire `LOCK_EX`.
                file.lock_shared().ok()?;
            }
            #[cfg(not(unix))]
            {
                let _ = file.unlock();
                file.lock_shared().ok()?;
            }

            log::info!(
                "Initialized clean ephemeral config directory at '{}'",
                ephemeral_dir.display()
            );
        }
        Err(fs::TryLockError::WouldBlock) => {
            // A sibling process holds `LOCK_SH` (active session) or `LOCK_EX` (currently wiping).
            // Blocking `lock_shared()` waits if a wipe is in progress and succeeds immediately
            // once shared, guaranteeing the returned `File` is always actively locked.
            file.lock_shared().ok()?;
        }
        Err(fs::TryLockError::Error(e)) => {
            log::warn!(
                "Failed to lock ephemeral directory '{}': {e}",
                ephemeral_dir.display()
            );
            return None;
        }
    }

    Some(file)
}

/// Resolves `path` to a canonical prefix by canonicalizing its deepest existing
/// ancestor and appending the remaining lexical components.
fn canonicalize_or_resolve(path: &Path) -> io::Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path);
    }
    let mut existing = path;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
                tail.push(name);
                existing = parent;
            }
            _ => return Ok(normalize_lexical(path)),
        }
    }
    let mut base = fs::canonicalize(existing)?;
    for comp in tail.into_iter().rev() {
        base.push(comp);
    }
    Ok(base)
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Copies `src` to `dst` via an atomic same-directory temporary file with owner
/// read+write permissions (`0600` on Unix).
///
/// This prevents concurrent readers (`Settings::init`, `CharacterWatcher`) from
/// observing truncated 0-byte files and avoids inheriting `0444` read-only permissions
/// when `src` is a NixOS `home-manager` symlink into `/nix/store`.
fn atomic_copy_writable_file(src: &Path, dst: &Path) -> io::Result<()> {
    let parent = dst.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination file has no parent directory",
        )
    })?;
    let pid = std::process::id();
    let seq = SEED_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = dst.file_name().and_then(|n| n.to_str()).unwrap_or("seed");
    let tmp_path = parent.join(format!(".{file_name}.{pid}.{seq}.tmp"));

    let mut src_file = File::open(src)?;
    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut tmp_file = opts.open(&tmp_path)?;
    let copy_res = io::copy(&mut src_file, &mut tmp_file).and_then(|_| tmp_file.sync_data());
    drop(tmp_file);

    if let Err(err) = copy_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, dst) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

/// Copies non-DB user configuration files from `src_config` into `dst_config`.
///
/// Includes `settings.json`, `state.json`, `user/`, `themes/`, `bookmarks/` (excluding
/// `runs/`), and `boxxyclaw/{BLACKLIST.md,MEMORY.md,characters,skills}`, while omitting
/// SQLite databases (`boxxy.db*`), transient caches, and update markers so dev schema
/// changes never affect Stable.
///
/// # Errors
///
/// Returns an `io::Error` if `dst_config` is nested inside `src_config` or if directory
/// creation / atomic file copying fails.
pub fn seed_config_tree_from(src_config: &Path, dst_config: &Path) -> io::Result<()> {
    if !src_config.is_dir() || src_config == dst_config {
        return Ok(());
    }

    let src_canon = fs::canonicalize(src_config)?;
    let dst_resolved = canonicalize_or_resolve(dst_config)?;
    if dst_resolved == src_canon {
        return Ok(());
    }
    if dst_resolved.starts_with(&src_canon) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination config directory cannot be nested inside source config directory",
        ));
    }

    fs::create_dir_all(dst_config)?;

    for single_file in ["settings.json", "state.json"] {
        let src = src_config.join(single_file);
        if src.is_file() {
            atomic_copy_writable_file(&src, &dst_config.join(single_file))?;
        }
    }

    for dir_name in ["user", "themes"] {
        let src_sub = src_config.join(dir_name);
        if src_sub.is_dir() {
            copy_dir_filtered(&src_sub, &dst_config.join(dir_name), &[], 0)?;
        }
    }

    let src_bookmarks = src_config.join("bookmarks");
    if src_bookmarks.is_dir() {
        copy_dir_filtered(&src_bookmarks, &dst_config.join("bookmarks"), &["runs"], 0)?;
    }

    let src_claw = src_config.join("boxxyclaw");
    if src_claw.is_dir() {
        let dst_claw = dst_config.join("boxxyclaw");
        fs::create_dir_all(&dst_claw)?;

        for claw_file in ["BLACKLIST.md", "MEMORY.md"] {
            let src = src_claw.join(claw_file);
            if src.is_file() {
                atomic_copy_writable_file(&src, &dst_claw.join(claw_file))?;
            }
        }

        for claw_dir in ["characters", "skills"] {
            let src_sub = src_claw.join(claw_dir);
            if src_sub.is_dir() {
                copy_dir_filtered(&src_sub, &dst_claw.join(claw_dir), &[], 0)?;
            }
        }
    }

    Ok(())
}

fn copy_dir_filtered(src: &Path, dst: &Path, skip_names: &[&str], depth: usize) -> io::Result<()> {
    if depth > MAX_SEED_DEPTH {
        return Ok(());
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(name_str) = name.to_str()
            && skip_names.contains(&name_str)
        {
            continue;
        }
        let file_type = entry.file_type()?;
        let entry_path = entry.path();
        let dst_path = dst.join(&name);

        if file_type.is_dir() {
            copy_dir_filtered(&entry_path, &dst_path, skip_names, depth + 1)?;
        } else if file_type.is_file() {
            atomic_copy_writable_file(&entry_path, &dst_path)?;
        } else if file_type.is_symlink() {
            // Support declarative dotfile managers (e.g. NixOS `home-manager`) whose
            // entries inside `~/.config/boxxy-terminal/` are symlinks into `/nix/store`.
            // Broken/dangling symlinks are skipped rather than aborting startup.
            if let Ok(target_meta) = fs::metadata(&entry_path) {
                if target_meta.is_dir() {
                    copy_dir_filtered(&entry_path, &dst_path, skip_names, depth + 1)?;
                } else if target_meta.is_file() {
                    atomic_copy_writable_file(&entry_path, &dst_path)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EnvStateGuard;
    use crate::{CliDevOverrides, configure_dev_options, get_config_dir};
    use serial_test::serial;

    #[test]
    fn test_seed_config_tree_excludes_sqlite_db_and_runs_and_copies_state_json() {
        let src_tmp = tempfile::tempdir().expect("src tempdir");
        let dst_tmp = tempfile::tempdir().expect("dst tempdir");
        let src = src_tmp.path();
        let dst = dst_tmp.path().join("seeded-dev");

        fs::write(src.join("settings.json"), r#"{"theme":"default"}"#).unwrap();
        fs::write(src.join("state.json"), r#"{"window_width":1280}"#).unwrap();
        fs::write(src.join("last_seen_commit"), "abc1234").unwrap();
        fs::create_dir_all(src.join("themes")).unwrap();
        fs::write(src.join("themes/custom.toml"), "name = 'Synthetic'").unwrap();
        fs::create_dir_all(src.join("bookmarks/runs")).unwrap();
        fs::write(src.join("bookmarks/build.sh"), "echo ok").unwrap();
        fs::write(src.join("bookmarks/runs/tmp.sh"), "echo transient").unwrap();
        fs::create_dir_all(src.join("boxxyclaw/characters/demo")).unwrap();
        fs::write(
            src.join("boxxyclaw/characters/demo/CHARACTER.toml"),
            "display_name = 'Demo'",
        )
        .unwrap();
        fs::write(src.join("boxxyclaw/boxxy.db"), b"sqlite-binary-stub").unwrap();
        fs::write(src.join("boxxyclaw/boxxy.db-wal"), b"wal-stub").unwrap();

        seed_config_tree_from(src, &dst).expect("seed should succeed");

        assert!(dst.join("settings.json").is_file());
        assert!(dst.join("state.json").is_file());
        assert!(dst.join("themes/custom.toml").is_file());
        assert!(dst.join("bookmarks/build.sh").is_file());
        assert!(
            dst.join("boxxyclaw/characters/demo/CHARACTER.toml")
                .is_file()
        );
        // Transient runs, commit markers, and SQLite DB files must never be copied
        assert!(!dst.join("bookmarks/runs").exists());
        assert!(!dst.join("last_seen_commit").exists());
        assert!(!dst.join("boxxyclaw/boxxy.db").exists());
        assert!(!dst.join("boxxyclaw/boxxy.db-wal").exists());
    }

    #[test]
    fn test_seed_config_tree_rejects_nested_destination() {
        let src_tmp = tempfile::tempdir().expect("src tempdir");
        let src = src_tmp.path();
        fs::write(src.join("settings.json"), "{}").unwrap();

        let nested_dst = src.join("themes").join("nested-dev");
        let err = seed_config_tree_from(src, &nested_dst)
            .expect_err("nested destination inside source must fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn test_seed_config_tree_handles_readonly_store_symlinks_and_reseeds() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let store_tmp = tempfile::tempdir().expect("store tempdir");
        let src_tmp = tempfile::tempdir().expect("src tempdir");
        let dst_tmp = tempfile::tempdir().expect("dst tempdir");

        let store_settings = store_tmp.path().join("ro-settings.json");
        let store_theme = store_tmp.path().join("ro-theme.toml");
        fs::write(&store_settings, r#"{"font_size":14}"#).unwrap();
        fs::write(&store_theme, "name = 'StoreTheme'").unwrap();
        fs::set_permissions(&store_settings, fs::Permissions::from_mode(0o444)).unwrap();
        fs::set_permissions(&store_theme, fs::Permissions::from_mode(0o444)).unwrap();

        let src = src_tmp.path();
        fs::create_dir_all(src.join("themes")).unwrap();
        symlink(&store_settings, src.join("settings.json")).unwrap();
        symlink(&store_theme, src.join("themes/linked.toml")).unwrap();
        symlink(
            store_tmp.path().join("missing-target.toml"),
            src.join("themes/dangling.toml"),
        )
        .unwrap();

        let dst = dst_tmp.path().join("dev-out");
        seed_config_tree_from(src, &dst).expect("first seed from read-only symlinks");
        assert!(dst.join("settings.json").is_file());
        assert!(dst.join("themes/linked.toml").is_file());
        assert!(!dst.join("themes/dangling.toml").exists());

        // Second `--sync-settings` run must succeed without `EACCES` on previously seeded files
        seed_config_tree_from(src, &dst).expect("second re-seed over existing files");
    }

    #[test]
    fn test_ephemeral_lock_wipes_only_when_unheld_and_holds_shared_lock() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let eph = tmp.path().join("ephemeral");
        fs::create_dir_all(&eph).unwrap();
        fs::write(eph.join("stale.json"), "{}").unwrap();

        // First process acquires exclusive lock cleanly, wipes `stale.json`, and downgrades to shared
        let guard_one = prepare_ephemeral_dir_in(&eph).expect("first lock");
        assert!(!eph.join("stale.json").exists());

        // Verify `guard_one` genuinely holds a shared lock (exclusive `try_lock` on a new FD must block)
        let probe = open_lock_file(&eph.join(EPHEMERAL_LOCK_FILE)).expect("probe open");
        assert!(
            matches!(probe.try_lock(), Err(fs::TryLockError::WouldBlock)),
            "guard_one must hold an active shared lock preventing exclusive re-entry"
        );
        drop(probe);

        // Simulate active runtime DB created by the first process (e.g. boxxy-agent)
        fs::write(eph.join("live_session.db"), "active").unwrap();

        // Second sibling process (e.g. boxxy-app) joins while `guard_one` is alive:
        // must NOT wipe `live_session.db`!
        let guard_two = prepare_ephemeral_dir_in(&eph).expect("second lock");
        assert!(eph.join("live_session.db").is_file());

        // Even if `guard_one` exits first, `guard_two` still holds `LOCK_SH`
        drop(guard_one);
        let guard_while_two_alive = prepare_ephemeral_dir_in(&eph).expect("join while two alive");
        assert!(eph.join("live_session.db").is_file());

        // Once all holders exit, the next ephemeral launch wipes cleanly again
        drop(guard_two);
        drop(guard_while_two_alive);
        let _guard_three = prepare_ephemeral_dir_in(&eph).expect("third lock");
        assert!(!eph.join("live_session.db").exists());
    }

    #[test]
    #[serial(config_dir)]
    fn test_prepare_dev_environment_ephemeral_never_seeds_from_stable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let fake_home = tmp.path().join("home");
        let stable_cfg = fake_home.join(".config").join("boxxy-terminal");
        fs::create_dir_all(&stable_cfg).unwrap();
        fs::write(stable_cfg.join("settings.json"), r#"{"theme":"stable"}"#).unwrap();

        let custom_eph = tmp.path().join("custom-ephemeral");
        let mut guard = EnvStateGuard::new(None);
        guard.set_extra_var("HOME", &fake_home.to_string_lossy());
        guard.set_extra_var(
            "XDG_CONFIG_HOME",
            &fake_home.join(".config").to_string_lossy(),
        );

        // Simulate `daemon_spawn_env` forwarding both `BOXXY_EPHEMERAL=1` and `BOXXY_CONFIG_DIR`
        configure_dev_options(CliDevOverrides {
            dev: true,
            ephemeral: true,
            config_dir: Some(custom_eph.clone()),
            ..Default::default()
        });

        prepare_dev_environment();
        assert_eq!(get_config_dir(), custom_eph);
        assert!(
            !custom_eph.join("settings.json").exists(),
            "Ephemeral mode must never copy Stable settings.json even when config_dir is set"
        );
    }
}
