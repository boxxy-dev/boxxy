//! Side-by-side isolated development instances (`--dev`, `--instance`, `--ephemeral`)
//! and D-Bus / `GApplication` namespace routing.

mod seed;

pub use crate::config_dir::get_stable_config_dir;
pub use seed::{prepare_dev_environment, seed_config_tree_from};

use crate::env::env_truthy;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

pub const STABLE_GTK_APP_ID: &str = "dev.boxxy.BoxxyTerminal";
pub const DEVEL_GTK_APP_ID: &str = "dev.boxxy.BoxxyTerminal.Devel";
pub const STABLE_AGENT_BUS_NAME: &str = "dev.boxxy.BoxxyAgent";
pub const DEVEL_AGENT_BUS_NAME: &str = "dev.boxxy.BoxxyAgent.Devel";

#[allow(clippy::struct_excessive_bools)]
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

/// Interns custom D-Bus / `GApplication` IDs once per distinct named instance slug.
/// Uses `HashSet<&'static str>` so each leaked string is allocated only once.
static INTERNED_STRINGS: LazyLock<RwLock<HashSet<&'static str>>> =
    LazyLock::new(|| RwLock::new(HashSet::new()));

/// Registers CLI-level development flags (`--dev`, `--instance`, `--ephemeral`,
/// `--sync-settings`, `--config-dir`, `--debug-context`) early in `main()` before `Settings::init()` runs.
pub fn configure_dev_options(overrides: CliDevOverrides) {
    *CLI_OVERRIDES.write() = overrides;
}

#[cfg(test)]
pub(crate) fn snapshot_cli_overrides() -> CliDevOverrides {
    CLI_OVERRIDES.read().clone()
}

pub(crate) fn cli_config_dir() -> Option<PathBuf> {
    CLI_OVERRIDES.read().config_dir.clone()
}

pub(crate) fn cli_debug_context() -> bool {
    CLI_OVERRIDES.read().debug_context
}

fn intern_static(s: String) -> &'static str {
    if let Some(&existing) = INTERNED_STRINGS.read().get(s.as_str()) {
        return existing;
    }
    let mut guard = INTERNED_STRINGS.write();
    if let Some(&existing) = guard.get(s.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// Sanitizes an arbitrary instance name into a safe `[a-z0-9_-]` slug (max 48 chars),
/// stripping leading and trailing `-`/`_` separators without wasting the length budget.
#[must_use]
pub fn sanitize_instance_slug(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len().min(48));
    for ch in raw.trim().chars() {
        if out.len() >= 48 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '_') && !out.is_empty() {
            out.push(ch);
        }
    }
    let trimmed_len = out.trim_end_matches(['-', '_']).len();
    out.truncate(trimmed_len);
    if out.is_empty() { None } else { Some(out) }
}

/// Resolves `(instance_slug, is_ephemeral)` with a single `CLI_OVERRIDES` read lock
/// and zero unnecessary heap clones on the common Stable path.
pub(crate) fn resolve_instance_and_ephemeral() -> (Option<String>, bool) {
    let (cli_slug, cli_dev_any, cli_ephemeral) = {
        let cli = CLI_OVERRIDES.read();
        let slug = cli.instance.as_deref().and_then(sanitize_instance_slug);
        let dev_any = cli.dev || cli.ephemeral || cli.sync_settings;
        (slug, dev_any, cli.ephemeral)
    };

    let ephemeral = cli_ephemeral || env_truthy("BOXXY_EPHEMERAL");

    if let Some(slug) = cli_slug {
        return (Some(slug), ephemeral);
    }

    if let Ok(raw_env) = std::env::var("BOXXY_INSTANCE")
        && let Some(slug) = sanitize_instance_slug(&raw_env)
    {
        return (Some(slug), ephemeral);
    }

    if cli_dev_any || ephemeral || env_truthy("BOXXY_DEV") || env_truthy("BOXXY_SYNC_SETTINGS") {
        return (Some("dev".to_string()), ephemeral);
    }

    (None, false)
}

/// Returns the active isolated instance slug (e.g. `Some("dev")` or `Some("wt-83")`),
/// or `None` when running as the primary Stable installation.
#[must_use]
pub fn instance_id() -> Option<String> {
    resolve_instance_and_ephemeral().0
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

fn format_namespaced_id(base: &'static str, slug: &str) -> &'static str {
    let mut out = String::with_capacity(base.len() + 3 + slug.len());
    out.push_str(base);
    out.push_str(".i_");
    for b in slug.bytes() {
        out.push(if b == b'-' { '_' } else { char::from(b) });
    }
    intern_static(out)
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
        Some(slug) => format_namespaced_id(DEVEL_AGENT_BUS_NAME, slug),
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
        Some(slug) => format_namespaced_id(DEVEL_GTK_APP_ID, slug),
    }
}

/// Returns a compact bracketed tag for the `HeaderBar` in dev mode (e.g. `"dev"`),
/// or `None` for the stable installation.
#[must_use]
pub fn dev_header_tag() -> Option<String> {
    let (slug_opt, ephemeral) = resolve_instance_and_ephemeral();
    let slug = slug_opt?;
    Some(match (slug.as_str(), ephemeral) {
        ("dev", false) => "dev".to_string(),
        ("dev", true) => "dev · ephemeral".to_string(),
        (custom, false) => format!("[dev:{custom}]"),
        (custom, true) => format!("[dev:{custom} · ephemeral]"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get_config_dir;
    use crate::test_support::EnvStateGuard;
    use serial_test::serial;

    #[test]
    fn test_sanitize_instance_slug_edge_cases() {
        assert_eq!(sanitize_instance_slug(""), None);
        assert_eq!(sanitize_instance_slug("   "), None);
        assert_eq!(sanitize_instance_slug("---___!!!"), None);
        assert_eq!(
            sanitize_instance_slug("83-Fix-Branch!").as_deref(),
            Some("83-fix-branch")
        );
        // Leading hyphens must not consume the 48-char slug budget
        let many_leading = format!("{}valid-slug", "-".repeat(60));
        assert_eq!(
            sanitize_instance_slug(&many_leading).as_deref(),
            Some("valid-slug")
        );
        // Long slugs cap cleanly at <= 48 chars and trim trailing separators
        let long_slug = format!("{}---", "a".repeat(60));
        let sanitized = sanitize_instance_slug(&long_slug).expect("valid slug");
        assert_eq!(sanitized.len(), 48);
    }

    #[test]
    #[serial(config_dir)]
    fn test_dev_instance_bus_app_and_config_names() {
        let mut guard = EnvStateGuard::new(None);
        assert_eq!(instance_id(), None);
        assert!(!is_dev_instance());
        assert_eq!(agent_bus_name(), STABLE_AGENT_BUS_NAME);
        assert_eq!(gtk_app_id(), STABLE_GTK_APP_ID);
        assert_eq!(dev_header_tag(), None);

        // Standard BOXXY_DEV=1
        guard.set_extra_var("BOXXY_DEV", "1");
        assert_eq!(instance_id().as_deref(), Some("dev"));
        assert!(is_dev_instance());
        assert_eq!(agent_bus_name(), DEVEL_AGENT_BUS_NAME);
        assert_eq!(gtk_app_id(), DEVEL_GTK_APP_ID);
        assert_eq!(dev_header_tag().as_deref(), Some("dev"));
        assert!(
            get_config_dir()
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .ends_with("boxxy-terminal-dev")
        );

        // Custom named instance (starting with digits / containing hyphens)
        guard.set_extra_var("BOXXY_INSTANCE", "83-Fix-Branch!");
        assert_eq!(instance_id().as_deref(), Some("83-fix-branch"));
        assert_eq!(
            agent_bus_name(),
            "dev.boxxy.BoxxyAgent.Devel.i_83_fix_branch"
        );
        assert_eq!(
            gtk_app_id(),
            "dev.boxxy.BoxxyTerminal.Devel.i_83_fix_branch"
        );
        assert_eq!(dev_header_tag().as_deref(), Some("[dev:83-fix-branch]"));

        // Ephemeral flag
        guard.remove_extra_var("BOXXY_INSTANCE");
        guard.set_extra_var("BOXXY_EPHEMERAL", "1");
        assert!(is_ephemeral_mode());
        assert_eq!(dev_header_tag().as_deref(), Some("dev · ephemeral"));
        assert!(
            get_config_dir()
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .contains("boxxy-terminal-ephemeral-dev")
        );
    }
}
