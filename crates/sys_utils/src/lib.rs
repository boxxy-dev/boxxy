use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static HTTP_CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
static LOCATION_CACHE: OnceLock<RwLock<Option<LocationContext>>> = OnceLock::new();
static EXCHANGE_RATES_CACHE: OnceLock<RwLock<Option<HashMap<String, f64>>>> = OnceLock::new();

#[derive(serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LocationContext {
    pub city: String,
    pub country: String,
    pub timezone: String,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct FrankfurterResponse {
    rates: HashMap<String, f64>,
}

/// Returns the active config root for Boxxy.
///
/// Resolution order:
///   1. `$BOXXY_CONFIG_DIR` if set and non-empty (used by the daemon when
///      launched from the Flatpak UI via `flatpak-spawn --host`).
///   2. `ProjectDirs::from("org", "boxxy", "boxxy-terminal").config_dir()`.
///   3. `$HOME/.config/boxxy-terminal`.
///   4. Last-resort fallback: `std::env::temp_dir().join("boxxy-terminal")`.
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
            }
            log::warn!(
                "BOXXY_CONFIG_DIR is set but is not an absolute path: '{trimmed}'. Ignoring and falling back."
            );
        }
    }

    if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        dirs.config_dir().to_path_buf()
    } else if let Some(home) = home::home_dir() {
        home.join(".config").join("boxxy-terminal")
    } else {
        std::env::temp_dir().join("boxxy-terminal")
    }
}

/// Returns a reference to the global multi-threaded Tokio runtime.
/// This runtime is used for background tasks (I/O, CPU-heavy work)
/// to keep them off the GTK UI thread.
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        // Bound I/O events per tick to 256 (matching the per-worker local queue
        // capacity) and cap busy ticks at 64 (Tokio 1.54+) so bursts of PTY/D-Bus
        // readiness do not overflow a busy worker's local queue into the global
        // injection queue.
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_io_events_per_tick(256)
            .max_io_events_per_busy_tick(64)
            .thread_name("boxxy-worker")
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

/// Returns the 0-based index of the current Tokio worker thread if called
/// from within a runtime worker thread (stabilized in Tokio 1.54).
#[must_use]
pub fn current_worker_index() -> Option<usize> {
    tokio::runtime::worker_index()
}

/// Returns true if the application is running inside a Flatpak sandbox.
pub fn is_flatpak() -> bool {
    ashpd::is_sandboxed()
}

/// Returns true if the internal self-updater is allowed to run.
/// This is disabled in Flatpak or if the `disable-self-update` feature is enabled.
#[must_use]
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

fn shared_http_client() -> Option<&'static reqwest::Client> {
    HTTP_CLIENT
        .get_or_init(|| match reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            Ok(client) => Some(client),
            Err(e) => {
                log::warn!("Failed to build HTTP client: {e}");
                None
            }
        })
        .as_ref()
}

/// Fetches the current location context in the background.
pub async fn fetch_location_context() {
    let cache = LOCATION_CACHE.get_or_init(|| RwLock::new(None));

    // Don't re-fetch if we already have it
    if cache.read().is_some() {
        return;
    }

    let Some(client) = shared_http_client() else {
        return;
    };

    // Use http://ip-api.com/json/ (Free, no key required, returns city/country/timezone)
    // Note: Free tier does not support HTTPS.
    let res = match client.get("http://ip-api.com/json/").send().await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Failed to fetch location context: {e}");
            return;
        }
    };

    if let Ok(loc) = res.json::<LocationContext>().await {
        *cache.write() = Some(loc);
    }
}

/// Returns the current location context from cache.
#[must_use]
pub fn get_location_context() -> Option<LocationContext> {
    LOCATION_CACHE.get()?.read().clone()
}

/// Fetches live USD exchange rates from the ECB/Frankfurter API (`api.frankfurter.dev`)
/// in the background and caches them in memory for the lifetime of the process.
///
/// This is a mechanism-only helper: callers decide the policy (e.g. only invoking
/// this when the user's selected currency is not USD).
pub async fn fetch_exchange_rates() {
    let cache = EXCHANGE_RATES_CACHE.get_or_init(|| RwLock::new(None));

    // Don't re-fetch if already populated during this session.
    if cache.read().is_some() {
        return;
    }

    let Some(client) = shared_http_client() else {
        return;
    };

    let url = "https://api.frankfurter.dev/v1/latest?base=USD&symbols=EUR,CNY,INR,BRL";
    let res = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Failed to fetch live exchange rates: {e}");
            return;
        }
    };

    if let Ok(payload) = res.json::<FrankfurterResponse>().await
        && !payload.rates.is_empty()
    {
        *cache.write() = Some(payload.rates);
    }
}

/// Returns the cached live exchange rate from `1.0 USD` for the given ISO currency code
/// (e.g. `"EUR"`, `"CNY"`, `"INR"`, `"BRL"`), or `None` if not yet fetched / offline.
#[must_use]
pub fn get_exchange_rate(code: &str) -> Option<f64> {
    EXCHANGE_RATES_CACHE
        .get()?
        .read()
        .as_ref()?
        .get(code)
        .copied()
        .filter(|&r| r.is_finite() && r > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::{is_locked_serially, serial};
    use std::ffi::OsString;

    /// RAII guard that mutates `BOXXY_CONFIG_DIR` for the duration of a test
    /// and restores its prior state on drop (even on panic).
    /// Also asserts that the caller holds the `config_dir` serial lock.
    struct ConfigDirEnvGuard {
        previous: Option<OsString>,
    }

    impl ConfigDirEnvGuard {
        fn set(value: Option<&str>) -> Self {
            assert!(
                is_locked_serially(Some("config_dir")),
                "Tests mutating BOXXY_CONFIG_DIR must be annotated with #[serial(config_dir)]"
            );
            let previous = std::env::var_os("BOXXY_CONFIG_DIR");
            match value {
                Some(v) => unsafe { std::env::set_var("BOXXY_CONFIG_DIR", v) },
                None => unsafe { std::env::remove_var("BOXXY_CONFIG_DIR") },
            }
            Self { previous }
        }
    }

    impl Drop for ConfigDirEnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(prev) => unsafe { std::env::set_var("BOXXY_CONFIG_DIR", prev) },
                None => unsafe { std::env::remove_var("BOXXY_CONFIG_DIR") },
            }
        }
    }

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
    #[serial(config_dir)]
    fn test_get_config_dir_default() {
        let _guard = ConfigDirEnvGuard::set(None);
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_absolute() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let expected = tmp.path().join("boxxy-test-absolute");
        let _guard = ConfigDirEnvGuard::set(Some(&expected.to_string_lossy()));
        let path = get_config_dir();
        assert_eq!(path, expected);
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_trimmed_whitespace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let expected = tmp.path().join("boxxy-test-trimmed");
        let padded = format!("  {}  \n", expected.to_string_lossy());
        let _guard = ConfigDirEnvGuard::set(Some(&padded));
        let path = get_config_dir();
        assert_eq!(path, expected);
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_trailing_slash() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("boxxy-test-slash");
        let with_slash = format!("{}/", base.to_string_lossy());
        let _guard = ConfigDirEnvGuard::set(Some(&with_slash));
        let path = get_config_dir();
        assert_eq!(path.join("file"), base.join("file"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_unset_empty() {
        let _guard = ConfigDirEnvGuard::set(Some(""));
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_whitespace_only() {
        let _guard = ConfigDirEnvGuard::set(Some("   "));
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_relative_ignored() {
        let _guard = ConfigDirEnvGuard::set(Some("relative/path/to/config"));
        let path = get_config_dir();
        assert!(path.to_string_lossy().contains("boxxy-terminal"));
        assert_ne!(path, PathBuf::from("relative/path/to/config"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_get_config_dir_non_existent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let non_existent = tmp.path().join("does-not-exist-yet").join("nested");
        let _guard = ConfigDirEnvGuard::set(Some(&non_existent.to_string_lossy()));
        let path = get_config_dir();
        // No I/O performed, should succeed infallibly without creating the directory
        assert_eq!(path, non_existent);
        assert!(!non_existent.exists());
    }

    #[test]
    #[serial(exchange_rates)]
    fn test_get_exchange_rate_filtering() {
        let cache = EXCHANGE_RATES_CACHE.get_or_init(|| RwLock::new(None));
        let prev = cache.read().clone();

        let mut rates = HashMap::new();
        rates.insert("EUR".to_string(), 0.92);
        rates.insert("ZERO".to_string(), 0.0);
        rates.insert("NEG".to_string(), -1.5);
        rates.insert("NAN".to_string(), f64::NAN);
        rates.insert("INF".to_string(), f64::INFINITY);
        *cache.write() = Some(rates);

        assert_eq!(get_exchange_rate("EUR"), Some(0.92));
        assert_eq!(get_exchange_rate("GBP"), None);
        assert_eq!(get_exchange_rate("ZERO"), None);
        assert_eq!(get_exchange_rate("NEG"), None);
        assert_eq!(get_exchange_rate("NAN"), None);
        assert_eq!(get_exchange_rate("INF"), None);

        *cache.write() = prev;
    }

    #[test]
    fn test_runtime_and_worker_index() {
        assert_eq!(
            current_worker_index(),
            None,
            "current_worker_index must be None outside a Tokio worker thread"
        );
        let idx = runtime().block_on(async {
            runtime()
                .spawn(async { current_worker_index() })
                .await
                .expect("spawned task")
        });
        assert!(
            idx.is_some(),
            "current_worker_index must be Some(_) inside a multi-thread runtime worker"
        );
    }
}
