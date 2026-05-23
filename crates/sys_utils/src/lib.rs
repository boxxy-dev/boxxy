use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static LOCATION_CACHE: OnceLock<RwLock<Option<LocationContext>>> = OnceLock::new();

#[derive(serde::Deserialize, Clone, Debug)]
pub struct LocationContext {
    pub city: String,
    pub country: String,
    pub timezone: String,
}

/// Returns the active config root for Boxxy.
///
/// Resolution order:
///   1. `$BOXXY_CONFIG_DIR` if set and non-empty (used by the daemon when
///      launched from the Flatpak UI via `flatpak-spawn --host`).
///   2. `ProjectDirs::from("org", "boxxy", "boxxy-terminal").config_dir()`.
///   3. Last-resort fallback: `$HOME/.config/boxxy-terminal`.
///
/// Infallible — callers want a `PathBuf`, not an `Option`/`Result`.
/// Does **not** create the directory; that's the caller's job (most call
/// sites already do `fs::create_dir_all` before writing).
pub fn get_config_dir() -> PathBuf {
    if let Ok(val) = std::env::var("BOXXY_CONFIG_DIR") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.is_absolute() {
                return path;
            } else {
                log::warn!(
                    "BOXXY_CONFIG_DIR is set but is not an absolute path: '{}'. Ignoring and falling back.",
                    trimmed
                );
            }
        }
    }

    if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        dirs.config_dir().to_path_buf()
    } else {
        let home = home::home_dir().expect("Could not determine home directory");
        home.join(".config").join("boxxy-terminal")
    }
}

/// Returns a reference to the global multi-threaded Tokio runtime.
/// This runtime is used for background tasks (I/O, CPU-heavy work)
/// to keep them off the GTK UI thread.
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

/// Returns true if the application is running inside a Flatpak sandbox.
pub fn is_flatpak() -> bool {
    ashpd::is_sandboxed()
}

/// Returns true if the internal self-updater is allowed to run.
/// This is disabled in Flatpak or if the `disable-self-update` feature is enabled.
pub fn can_self_update() -> bool {
    #[cfg(any(feature = "disable-self-update", not(feature = "self-update")))]
    {
        false
    }
    #[cfg(all(feature = "self-update", not(feature = "disable-self-update")))]
    {
        !is_flatpak()
    }
}

/// Fetches the current location context in the background.
pub async fn fetch_location_context() {
    let cache = LOCATION_CACHE.get_or_init(|| RwLock::new(None));

    // Don't re-fetch if we already have it
    if cache.read().is_some() {
        return;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();

    // Use http://ip-api.com/json/ (Free, no key required, returns city/country/timezone)
    // Note: Free tier does not support HTTPS.
    let res = match client.get("http://ip-api.com/json/").send().await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Failed to fetch location context: {}", e);
            return;
        }
    };

    if let Ok(loc) = res.json::<LocationContext>().await {
        *cache.write() = Some(loc);
    }
}

/// Returns the current location context from cache.
pub fn get_location_context() -> Option<LocationContext> {
    LOCATION_CACHE.get()?.read().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn test_can_self_update_logic() {
        let flatpak = is_flatpak();
        let can_update = can_self_update();

        if cfg!(feature = "disable-self-update") {
            assert!(
                !can_update,
                "Self-update must be disabled when feature is enabled"
            );
        } else if flatpak {
            assert!(!can_update, "Self-update must be disabled when in flatpak");
        } else {
            assert!(
                can_update,
                "Self-update should be enabled for native builds without the feature flag"
            );
        }
    }

    #[test]
    #[serial]
    fn test_get_config_dir_default() {
        unsafe { std::env::remove_var("BOXXY_CONFIG_DIR"); }
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial]
    fn test_get_config_dir_absolute() {
        unsafe { std::env::set_var("BOXXY_CONFIG_DIR", "/tmp/boxxy-test-absolute"); }
        let path = get_config_dir();
        assert_eq!(path, PathBuf::from("/tmp/boxxy-test-absolute"));
    }

    #[test]
    #[serial]
    fn test_get_config_dir_trailing_slash() {
        unsafe { std::env::set_var("BOXXY_CONFIG_DIR", "/tmp/boxxy-test-absolute/"); }
        let path = get_config_dir();
        // PathBuf handles trailing slashes or simplifies them:
        assert_eq!(path.join("file"), PathBuf::from("/tmp/boxxy-test-absolute/file"));
    }

    #[test]
    #[serial]
    fn test_get_config_dir_unset_empty() {
        unsafe { std::env::set_var("BOXXY_CONFIG_DIR", ""); }
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial]
    fn test_get_config_dir_whitespace_only() {
        unsafe { std::env::set_var("BOXXY_CONFIG_DIR", "   "); }
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial]
    fn test_get_config_dir_relative_ignored() {
        unsafe { std::env::set_var("BOXXY_CONFIG_DIR", "relative/path/to/config"); }
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
        assert_ne!(path, PathBuf::from("relative/path/to/config"));
    }

    #[test]
    #[serial]
    fn test_get_config_dir_non_existent() {
        unsafe { std::env::set_var("BOXXY_CONFIG_DIR", "/tmp/does-not-exist-yet-random-name-1234"); }
        let path = get_config_dir();
        // No I/O performed, should succeed infallible
        assert_eq!(path, PathBuf::from("/tmp/does-not-exist-yet-random-name-1234"));
    }
}
