//! Infallible configuration directory resolution for Stable, Dev, and Ephemeral modes.
//!
//! Strictly performs zero filesystem I/O (`Path::is_absolute` only, never `canonicalize`)
//! and never caches, allowing callers and tests to switch `BOXXY_CONFIG_DIR` or CLI
//! overrides dynamically.

use std::path::PathBuf;

/// Returns a per-UID fallback root inside `std::env::temp_dir()` so multi-user
/// headless systems without `$HOME` or `$XDG_RUNTIME_DIR` never collide in `/tmp`.
pub(crate) fn user_scoped_temp_root() -> PathBuf {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` is an async-signal-safe, infallible POSIX syscall taking no pointers.
        let uid = unsafe { libc::geteuid() };
        std::env::temp_dir().join(format!("boxxy-{uid}"))
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join("boxxy")
    }
}

/// Resolves the canonical Stable config directory (ignoring `BOXXY_CONFIG_DIR` and dev flags).
#[must_use]
pub fn get_stable_config_dir() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        return dirs.config_dir().to_path_buf();
    }
    if let Some(home) = std::env::home_dir().filter(|p| !p.as_os_str().is_empty()) {
        return home.join(".config").join("boxxy-terminal");
    }
    user_scoped_temp_root().join("boxxy-terminal")
}

pub(crate) fn resolve_ephemeral_config_dir(slug: &str) -> PathBuf {
    let runtime_root = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(user_scoped_temp_root);
    runtime_root.join(format!("boxxy-terminal-ephemeral-{slug}"))
}

pub(crate) fn resolve_dev_config_dir(slug: &str) -> PathBuf {
    let dir_name = format!("boxxy-terminal-{slug}");
    let stable = get_stable_config_dir();
    stable.parent().map_or_else(
        || user_scoped_temp_root().join(&dir_name),
        |p| p.join(&dir_name),
    )
}

/// Validates `--config-dir` and `$BOXXY_CONFIG_DIR`, returning `Some(path)` only
/// when non-empty and absolute.
pub(crate) fn explicit_config_dir_override(warn_on_relative: bool) -> Option<PathBuf> {
    if let Some(cli_dir) = crate::dev_mode::cli_config_dir()
        && !cli_dir.as_os_str().is_empty()
    {
        if cli_dir.is_absolute() {
            return Some(cli_dir);
        }
        if warn_on_relative {
            log::warn!(
                "--config-dir is set but is not an absolute path: '{}'. Ignoring and falling back.",
                cli_dir.display()
            );
        }
    }

    if let Ok(val) = std::env::var("BOXXY_CONFIG_DIR") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.is_absolute() {
                return Some(path);
            }
            if warn_on_relative {
                log::warn!(
                    "BOXXY_CONFIG_DIR is set but is not an absolute path: '{trimmed}'. Ignoring and falling back."
                );
            }
        }
    }

    None
}

/// Returns the active config root for Boxxy.
///
/// Resolution order:
///   1. `--config-dir <PATH>` if set, non-empty, and absolute.
///   2. `$BOXXY_CONFIG_DIR` if set, non-empty, and absolute.
///   3. Ephemeral dev directory (`$XDG_RUNTIME_DIR/boxxy-terminal-ephemeral-<slug>`) when `--ephemeral` / `BOXXY_EPHEMERAL=1` is set.
///   4. Isolated dev directory (`~/.config/boxxy-terminal-<slug>`) when `--dev` / `BOXXY_DEV=1` / `BOXXY_INSTANCE=<slug>` is set.
///   5. `ProjectDirs::from("org", "boxxy", "boxxy-terminal").config_dir()`.
///   6. `$HOME/.config/boxxy-terminal`.
///   7. Last-resort fallback: `/tmp/boxxy-<uid>/boxxy-terminal`.
///
/// Infallible — callers receive a `PathBuf` directly without I/O or caching.
/// Creating the directory is the caller's responsibility.
#[must_use]
pub fn get_config_dir() -> PathBuf {
    if let Some(override_dir) = explicit_config_dir_override(true) {
        return override_dir;
    }

    let (slug, ephemeral) = crate::dev_mode::resolve_instance_and_ephemeral();
    if ephemeral {
        let effective_slug = slug.as_deref().unwrap_or("dev");
        return resolve_ephemeral_config_dir(effective_slug);
    }

    if let Some(slug) = slug {
        return resolve_dev_config_dir(&slug);
    }

    get_stable_config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EnvStateGuard;
    use crate::{CliDevOverrides, configure_dev_options};
    use serial_test::serial;

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_default() {
        let _guard = EnvStateGuard::new(None);
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_absolute() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let expected = tmp.path().join("boxxy-test-absolute");
        let _guard = EnvStateGuard::new(Some(&expected.to_string_lossy()));
        let path = get_config_dir();
        assert_eq!(path, expected);
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_cli_takes_precedence_over_env() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env_dir = tmp.path().join("from-env");
        let cli_dir = tmp.path().join("from-cli");
        let _guard = EnvStateGuard::new(Some(&env_dir.to_string_lossy()));

        configure_dev_options(CliDevOverrides {
            config_dir: Some(cli_dir.clone()),
            ..Default::default()
        });
        assert_eq!(get_config_dir(), cli_dir);

        // Relative CLI path falls back cleanly to valid absolute env override
        configure_dev_options(CliDevOverrides {
            config_dir: Some(PathBuf::from("relative/cli/path")),
            ..Default::default()
        });
        assert_eq!(get_config_dir(), env_dir);
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_trimmed_whitespace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let expected = tmp.path().join("boxxy-test-trimmed");
        let padded = format!("  {}  \n", expected.to_string_lossy());
        let _guard = EnvStateGuard::new(Some(&padded));
        let path = get_config_dir();
        assert_eq!(path, expected);
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_trailing_slash() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("boxxy-test-slash");
        let with_slash = format!("{}/", base.to_string_lossy());
        let _guard = EnvStateGuard::new(Some(&with_slash));
        let path = get_config_dir();
        assert_eq!(path.join("file"), base.join("file"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_unset_empty() {
        let _guard = EnvStateGuard::new(Some(""));
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_whitespace_only() {
        let _guard = EnvStateGuard::new(Some("   "));
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_relative_ignored() {
        let _guard = EnvStateGuard::new(Some("relative/path/to/config"));
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
        assert_ne!(path, PathBuf::from("relative/path/to/config"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_non_existent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let non_existent = tmp.path().join("does-not-exist-yet").join("nested");
        let _guard = EnvStateGuard::new(Some(&non_existent.to_string_lossy()));
        let path = get_config_dir();
        // No I/O performed: succeeds infallibly without creating the directory
        assert_eq!(path, non_existent);
        assert!(!non_existent.exists());
    }
}
