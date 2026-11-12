use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

pub const STABLE_GTK_APP_ID: &str = "dev.boxxy.BoxxyTerminal";
pub const DEVEL_GTK_APP_ID: &str = "dev.boxxy.BoxxyTerminal.Devel";
pub const STABLE_AGENT_BUS_NAME: &str = "dev.boxxy.BoxxyAgent";
pub const DEVEL_AGENT_BUS_NAME: &str = "dev.boxxy.BoxxyAgent.Devel";

const EPHEMERAL_LOCK_FILE: &str = ".ephemeral.lock";

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CliDevOverrides {
    pub dev: bool,
    pub instance: Option<String>,
    pub ephemeral: bool,
    pub sync_settings: bool,
    pub config_dir: Option<PathBuf>,
    pub debug_context: bool,
}

static CLI_OVERRIDES: RwLock<CliDevOverrides> = RwLock::new(CliDevOverrides {
    dev: false,
    instance: None,
    ephemeral: false,
    sync_settings: false,
    config_dir: None,
    debug_context: false,
});

static INTERNED_STRINGS: LazyLock<RwLock<HashMap<String, &'static str>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static EPHEMERAL_SESSION_LOCK: OnceLock<File> = OnceLock::new();

/// Registers CLI-level development flags (`--dev`, `--instance`, `--ephemeral`,
/// `--sync-settings`, `--config-dir`, `--debug-context`) early in `main()` before `Settings::init()` runs.
pub fn configure_dev_options(overrides: CliDevOverrides) {
    *CLI_OVERRIDES.write() = overrides;
}

pub(crate) fn cli_config_dir() -> Option<PathBuf> {
    CLI_OVERRIDES.read().config_dir.clone()
}

pub(crate) fn cli_debug_context() -> bool {
    CLI_OVERRIDES.read().debug_context
}

fn intern_static(s: String) -> &'static str {
    if let Some(&existing) = INTERNED_STRINGS.read().get(&s) {
        return existing;
    }
    let mut guard = INTERNED_STRINGS.write();
    if let Some(&existing) = guard.get(&s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.clone().into_boxed_str());
    guard.insert(s, leaked);
    leaked
}

use crate::env::env_truthy;

/// Sanitizes an arbitrary instance name into a safe `[a-z0-9_-]` slug (max 48 chars).
#[must_use]
pub fn sanitize_instance_slug(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len().min(48));
    for ch in raw.trim().chars() {
        if out.len() >= 48 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push(ch);
        }
    }
    let trimmed = out.trim_matches(['-', '_']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Returns the active isolated instance slug (e.g. `Some("dev")` or `Some("wt-83")`),
/// or `None` when running as the primary Stable installation.
#[must_use]
pub fn instance_id() -> Option<String> {
    let cli = CLI_OVERRIDES.read().clone();

    if let Some(ref raw) = cli.instance
        && let Some(slug) = sanitize_instance_slug(raw)
    {
        return Some(slug);
    }

    if let Ok(raw_env) = std::env::var("BOXXY_INSTANCE")
        && let Some(slug) = sanitize_instance_slug(&raw_env)
    {
        return Some(slug);
    }

    if cli.dev
        || cli.ephemeral
        || cli.sync_settings
        || env_truthy("BOXXY_DEV")
        || env_truthy("BOXXY_EPHEMERAL")
        || env_truthy("BOXXY_SYNC_SETTINGS")
    {
        return Some("dev".to_string());
    }

    None
}

/// Returns `true` when running as an isolated development / secondary instance.
#[must_use]
pub fn is_dev_instance() -> bool {
    instance_id().is_some()
}

/// Returns `true` when ephemeral clean-defaults mode is active (`--ephemeral` or `BOXXY_EPHEMERAL=1`).
#[must_use]
pub fn is_ephemeral_mode() -> bool {
    CLI_OVERRIDES.read().ephemeral || env_truthy("BOXXY_EPHEMERAL")
}

/// Returns `true` when settings should be refreshed from the stable installation (`--sync-settings` or `BOXXY_SYNC_SETTINGS=1`).
#[must_use]
pub fn should_sync_settings() -> bool {
    CLI_OVERRIDES.read().sync_settings || env_truthy("BOXXY_SYNC_SETTINGS")
}

/// Returns the session D-Bus well-known service name for `boxxy-agent`.
///
/// - Stable: `"dev.boxxy.BoxxyAgent"`
/// - `--dev` / `BOXXY_DEV=1`: `"dev.boxxy.BoxxyAgent.Devel"`
/// - Named instance (`BOXXY_INSTANCE=foo-1`): `"dev.boxxy.BoxxyAgent.Devel.i_foo_1"`
#[must_use]
pub fn agent_bus_name() -> &'static str {
    match instance_id().as_deref() {
        None => STABLE_AGENT_BUS_NAME,
        Some("dev") => DEVEL_AGENT_BUS_NAME,
        Some(slug) => {
            let dbus_elem = slug.replace('-', "_");
            intern_static(format!("{DEVEL_AGENT_BUS_NAME}.i_{dbus_elem}"))
        }
    }
}

/// Returns the GTK `GApplication` ID for `boxxy-app`.
///
/// - Stable: `"dev.boxxy.BoxxyTerminal"`
/// - `--dev` / `BOXXY_DEV=1`: `"dev.boxxy.BoxxyTerminal.Devel"`
/// - Named instance (`BOXXY_INSTANCE=foo-1`): `"dev.boxxy.BoxxyTerminal.Devel.i_foo_1"`
#[must_use]
pub fn gtk_app_id() -> &'static str {
    match instance_id().as_deref() {
        None => STABLE_GTK_APP_ID,
        Some("dev") => DEVEL_GTK_APP_ID,
        Some(slug) => {
            let gio_elem = slug.replace('-', "_");
            intern_static(format!("{DEVEL_GTK_APP_ID}.i_{gio_elem}"))
        }
    }
}

/// Returns a compact bracketed tag for the HeaderBar in dev mode (e.g. `"dev"`),
/// or `None` for the stable installation.
#[must_use]
pub fn dev_header_tag() -> Option<String> {
    let slug = instance_id()?;
    let ephemeral = is_ephemeral_mode();
    Some(match (slug.as_str(), ephemeral) {
        ("dev", false) => "dev".to_string(),
        ("dev", true) => "dev · ephemeral".to_string(),
        (custom, false) => format!("[dev:{custom}]"),
        (custom, true) => format!("[dev:{custom} · ephemeral]"),
    })
}

/// Resolves the canonical Stable config directory (ignoring `BOXXY_CONFIG_DIR` and dev flags).
#[must_use]
pub fn get_stable_config_dir() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        dirs.config_dir().to_path_buf()
    } else if let Some(home) = home::home_dir() {
        home.join(".config").join("boxxy-terminal")
    } else {
        std::env::temp_dir().join("boxxy-terminal")
    }
}

pub(crate) fn resolve_ephemeral_config_dir(slug: &str) -> PathBuf {
    let runtime_root = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(std::env::temp_dir);
    runtime_root.join(format!("boxxy-terminal-ephemeral-{slug}"))
}

pub(crate) fn resolve_dev_config_dir(slug: &str) -> PathBuf {
    let stable = get_stable_config_dir();
    if let Some(parent) = stable.parent() {
        parent.join(format!("boxxy-terminal-{slug}"))
    } else {
        std::env::temp_dir().join(format!("boxxy-terminal-{slug}"))
    }
}

/// Prepares the isolated development or ephemeral configuration directory before
/// `Settings::init()` runs. No-op when running as the Stable installation.
pub fn prepare_dev_environment() {
    if !is_dev_instance() {
        return;
    }

    let target_dir = crate::get_config_dir();
    let has_explicit_config_override = cli_config_dir()
        .as_ref()
        .is_some_and(|p| p.is_absolute())
        || std::env::var("BOXXY_CONFIG_DIR")
            .ok()
            .is_some_and(|v| Path::new(v.trim()).is_absolute());

    if is_ephemeral_mode() && !has_explicit_config_override {
        if let Some(lock_file) = prepare_ephemeral_dir_in(&target_dir) {
            let _ = EPHEMERAL_SESSION_LOCK.set(lock_file);
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
                    "Synced dev settings into {:?} from {:?}",
                    target_dir,
                    stable_dir
                );
            }
            Err(e) => {
                log::warn!("Failed to sync dev settings into {:?}: {e}", target_dir);
            }
        }
    }
}

/// Wipes `ephemeral_dir` if and only if no sibling ephemeral process (`boxxy-agent` or
/// `boxxy-app`) currently holds a shared lock on `.ephemeral.lock`, then returns a
/// shared-locked `File` handle to keep alive for the process lifetime.
pub(crate) fn prepare_ephemeral_dir_in(ephemeral_dir: &Path) -> Option<File> {
    fs::create_dir_all(ephemeral_dir).ok()?;
    let lock_path = ephemeral_dir.join(EPHEMERAL_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .ok()?;

    if file.try_lock().is_ok() {
        // Exclusive lock acquired: no sibling process is using this ephemeral directory yet.
        // Wipe any leftover artifacts from a previous session so defaults are 100% clean.
        if let Ok(entries) = fs::read_dir(ephemeral_dir) {
            for entry in entries.flatten() {
                if entry.file_name() == EPHEMERAL_LOCK_FILE {
                    continue;
                }
                let path = entry.path();
                if path.is_dir() {
                    let _ = fs::remove_dir_all(&path);
                } else {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        let _ = file.unlock();
        log::info!(
            "Initialized clean ephemeral config directory at {:?}",
            ephemeral_dir
        );
    }

    let _ = file.try_lock_shared();
    Some(file)
}

/// Copies user settings, themes, profile avatar, bookmarks, characters, and skills from
/// `src_config` into `dst_config`, intentionally omitting SQLite databases (`boxxy.db*`),
/// transient caches, and update markers so dev schema changes never affect Stable.
pub fn seed_config_tree_from(src_config: &Path, dst_config: &Path) -> std::io::Result<()> {
    if !src_config.is_dir() || src_config == dst_config {
        return Ok(());
    }
    fs::create_dir_all(dst_config)?;

    for single_file in ["settings.json", "app_state.json"] {
        let src = src_config.join(single_file);
        if src.is_file() {
            fs::copy(&src, dst_config.join(single_file))?;
        }
    }

    for dir_name in ["user", "themes"] {
        let src_sub = src_config.join(dir_name);
        if src_sub.is_dir() {
            copy_dir_filtered(&src_sub, &dst_config.join(dir_name), &[])?;
        }
    }

    let src_bookmarks = src_config.join("bookmarks");
    if src_bookmarks.is_dir() {
        copy_dir_filtered(&src_bookmarks, &dst_config.join("bookmarks"), &["runs"])?;
    }

    let src_claw = src_config.join("boxxyclaw");
    if src_claw.is_dir() {
        let dst_claw = dst_config.join("boxxyclaw");
        fs::create_dir_all(&dst_claw)?;

        for claw_file in ["BLACKLIST.md", "MEMORY.md"] {
            let src = src_claw.join(claw_file);
            if src.is_file() {
                fs::copy(&src, dst_claw.join(claw_file))?;
            }
        }

        for claw_dir in ["characters", "skills"] {
            let src_sub = src_claw.join(claw_dir);
            if src_sub.is_dir() {
                copy_dir_filtered(&src_sub, &dst_claw.join(claw_dir), &[])?;
            }
        }
    }

    Ok(())
}

fn copy_dir_filtered(src: &Path, dst: &Path, skip_names: &[&str]) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if skip_names.iter().any(|&skip| skip == name_str) {
            continue;
        }
        let file_type = entry.file_type()?;
        let dst_path = dst.join(&name);
        if file_type.is_dir() {
            copy_dir_filtered(&entry.path(), &dst_path, skip_names)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}
