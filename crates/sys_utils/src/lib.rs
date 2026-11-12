pub mod dev_mode;
pub mod env;

pub use dev_mode::{
    CliDevOverrides, DEVEL_AGENT_BUS_NAME, DEVEL_GTK_APP_ID, STABLE_AGENT_BUS_NAME,
    STABLE_GTK_APP_ID, agent_bus_name, configure_dev_options, dev_header_tag,
    get_stable_config_dir, gtk_app_id, instance_id, is_dev_instance, is_ephemeral_mode,
    prepare_dev_environment, sanitize_instance_slug, seed_config_tree_from, should_sync_settings,
};

use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static HTTP_CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
static LOCATION_CACHE: RwLock<Option<LocationContext>> = RwLock::new(None);
static EXCHANGE_RATES_CACHE: RwLock<Option<HashMap<String, f64>>> = RwLock::new(None);
static FETCH_LOCATION_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static FETCH_RATES_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

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
///   1. `$BOXXY_CONFIG_DIR` if set, non-empty, and absolute.
///   2. Ephemeral dev directory (`$XDG_RUNTIME_DIR/boxxy-terminal-ephemeral-<slug>`) when `--ephemeral` / `BOXXY_EPHEMERAL=1` is set.
///   3. Isolated dev directory (`~/.config/boxxy-terminal-<slug>`) when `--dev` / `BOXXY_DEV=1` / `BOXXY_INSTANCE=<slug>` is set.
///   4. `ProjectDirs::from("org", "boxxy", "boxxy-terminal").config_dir()`.
///   5. `$HOME/.config/boxxy-terminal`.
///   6. Last-resort fallback: `std::env::temp_dir().join("boxxy-terminal")`.
///
/// Infallible — callers want a `PathBuf`, not an `Option`/`Result`.
/// Does **not** create the directory; that's the caller's job (most call
/// sites already do `fs::create_dir_all` before writing).
pub fn get_config_dir() -> PathBuf {
    if let Some(cli_dir) = dev_mode::cli_config_dir() {
        if cli_dir.is_absolute() {
            return cli_dir;
        }
        log::warn!(
            "--config-dir is set but is not an absolute path: '{}'. Ignoring and falling back.",
            cli_dir.display()
        );
    }

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

    if is_ephemeral_mode() {
        let slug = instance_id().unwrap_or_else(|| "dev".to_string());
        return dev_mode::resolve_ephemeral_config_dir(&slug);
    }

    if let Some(slug) = instance_id() {
        return dev_mode::resolve_dev_config_dir(&slug);
    }

    get_stable_config_dir()
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
/// This is disabled in Flatpak, in isolated dev instances, or if the `disable-self-update` feature is enabled.
#[must_use]
pub fn can_self_update() -> bool {
    #[cfg(any(feature = "disable-self-update", not(feature = "self-update")))]
    {
        false
    }
    #[cfg(all(feature = "self-update", not(feature = "disable-self-update")))]
    {
        !is_flatpak() && !is_dev_instance()
    }
}

fn shared_http_client() -> Option<&'static reqwest::Client> {
    HTTP_CLIENT
        .get_or_init(|| {
            match reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
            {
                Ok(client) => Some(client),
                Err(e) => {
                    log::warn!("Failed to build HTTP client: {e}");
                    None
                }
            }
        })
        .as_ref()
}

/// Fetches the current location context in the background.
pub async fn fetch_location_context() {
    // Fast-path check before acquiring the async fetch lock
    if LOCATION_CACHE.read().is_some() {
        return;
    }

    let _fetch_guard = FETCH_LOCATION_LOCK.lock().await;
    if LOCATION_CACHE.read().is_some() {
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
        *LOCATION_CACHE.write() = Some(loc);
    }
}

/// Returns the current location context from cache.
#[must_use]
pub fn get_location_context() -> Option<LocationContext> {
    LOCATION_CACHE.read().clone()
}

/// Fetches live USD exchange rates from the ECB/Frankfurter API (`api.frankfurter.dev`)
/// in the background and caches them in memory for the lifetime of the process.
///
/// This is a mechanism-only helper: callers decide the policy (e.g. only invoking
/// this when the user's selected currency is not USD).
pub async fn fetch_exchange_rates() {
    // Fast-path check before acquiring the async fetch lock
    if EXCHANGE_RATES_CACHE.read().is_some() {
        return;
    }

    let _fetch_guard = FETCH_RATES_LOCK.lock().await;
    if EXCHANGE_RATES_CACHE.read().is_some() {
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
        *EXCHANGE_RATES_CACHE.write() = Some(payload.rates);
    }
}

/// Returns the cached live exchange rate from `1.0 USD` for the given ISO currency code
/// (e.g. `"EUR"`, `"CNY"`, `"INR"`, `"BRL"`), or `None` if not yet fetched / offline.
#[must_use]
pub fn get_exchange_rate(code: &str) -> Option<f64> {
    EXCHANGE_RATES_CACHE
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

    /// RAII guard that mutates `BOXXY_*` config/dev environment variables and CLI
    /// overrides for the duration of a test and restores prior state on drop.
    /// Also asserts that the caller holds the `config_dir` serial lock.
    struct ConfigDirEnvGuard {
        previous_env: Vec<(&'static str, Option<OsString>)>,
        previous_cli: CliDevOverrides,
    }

    impl ConfigDirEnvGuard {
        fn set(value: Option<&str>) -> Self {
            assert!(
                is_locked_serially(Some("config_dir")),
                "Tests mutating BOXXY_CONFIG_DIR must be annotated with #[serial(config_dir)]"
            );
            let previous_env = env::ENV_SPECS
                .iter()
                .filter(|s| s.name.starts_with("BOXXY_"))
                .map(|s| (s.name, std::env::var_os(s.name)))
                .collect::<Vec<_>>();
            for (k, _) in &previous_env {
                unsafe { std::env::remove_var(k) };
            }
            if let Some(v) = value {
                unsafe { std::env::set_var("BOXXY_CONFIG_DIR", v) };
            }
            let previous_cli = CliDevOverrides::default();
            configure_dev_options(CliDevOverrides::default());
            Self {
                previous_env,
                previous_cli,
            }
        }
    }

    impl Drop for ConfigDirEnvGuard {
        fn drop(&mut self) {
            for (k, prev) in &self.previous_env {
                match prev {
                    Some(val) => unsafe { std::env::set_var(k, val) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
            configure_dev_options(self.previous_cli.clone());
        }
    }

    #[test]
    #[serial(config_dir)]
    fn test_can_self_update_logic() {
        let _guard = ConfigDirEnvGuard::set(None);
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

        // Dev instances must always disable self-update
        configure_dev_options(CliDevOverrides {
            dev: true,
            ..Default::default()
        });
        assert!(
            !can_self_update(),
            "Self-update must be disabled in dev mode"
        );
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
        let prev = EXCHANGE_RATES_CACHE.read().clone();

        let mut rates = HashMap::new();
        rates.insert("EUR".to_string(), 0.92);
        rates.insert("ZERO".to_string(), 0.0);
        rates.insert("NEG".to_string(), -1.5);
        rates.insert("NAN".to_string(), f64::NAN);
        rates.insert("INF".to_string(), f64::INFINITY);
        *EXCHANGE_RATES_CACHE.write() = Some(rates);

        assert_eq!(get_exchange_rate("EUR"), Some(0.92));
        assert_eq!(get_exchange_rate("GBP"), None);
        assert_eq!(get_exchange_rate("ZERO"), None);
        assert_eq!(get_exchange_rate("NEG"), None);
        assert_eq!(get_exchange_rate("NAN"), None);
        assert_eq!(get_exchange_rate("INF"), None);

        *EXCHANGE_RATES_CACHE.write() = prev;
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

    #[test]
    #[serial(config_dir)]
    fn test_dev_instance_bus_app_and_config_names() {
        let _guard = ConfigDirEnvGuard::set(None);
        assert_eq!(instance_id(), None);
        assert!(!is_dev_instance());
        assert_eq!(agent_bus_name(), STABLE_AGENT_BUS_NAME);
        assert_eq!(gtk_app_id(), STABLE_GTK_APP_ID);
        assert_eq!(dev_header_tag(), None);

        // Standard BOXXY_DEV=1
        unsafe { std::env::set_var("BOXXY_DEV", "1") };
        assert_eq!(instance_id().as_deref(), Some("dev"));
        assert!(is_dev_instance());
        assert_eq!(agent_bus_name(), DEVEL_AGENT_BUS_NAME);
        assert_eq!(gtk_app_id(), DEVEL_GTK_APP_ID);
        assert_eq!(dev_header_tag().as_deref(), Some("dev"));
        assert!(
            get_config_dir()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("boxxy-terminal-dev")
        );

        // Custom named instance (e.g. starting with digits / containing hyphens)
        unsafe { std::env::set_var("BOXXY_INSTANCE", "83-Fix-Branch!") };
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
        unsafe { std::env::remove_var("BOXXY_INSTANCE") };
        unsafe { std::env::set_var("BOXXY_EPHEMERAL", "1") };
        assert!(is_ephemeral_mode());
        assert_eq!(dev_header_tag().as_deref(), Some("dev · ephemeral"));
        assert!(
            get_config_dir()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("boxxy-terminal-ephemeral-dev")
        );
    }

    #[test]
    fn test_seed_config_tree_excludes_sqlite_db_and_runs() {
        let src_tmp = tempfile::tempdir().expect("src tempdir");
        let dst_tmp = tempfile::tempdir().expect("dst tempdir");
        let src = src_tmp.path();
        let dst = dst_tmp.path().join("seeded-dev");

        std::fs::write(src.join("settings.json"), r#"{"theme":"default"}"#).unwrap();
        std::fs::write(src.join("last_seen_commit"), "abc1234").unwrap();
        std::fs::create_dir_all(src.join("themes")).unwrap();
        std::fs::write(src.join("themes/custom.toml"), "name = 'Synthetic'").unwrap();
        std::fs::create_dir_all(src.join("bookmarks/runs")).unwrap();
        std::fs::write(src.join("bookmarks/build.sh"), "echo ok").unwrap();
        std::fs::write(src.join("bookmarks/runs/tmp.sh"), "echo transient").unwrap();
        std::fs::create_dir_all(src.join("boxxyclaw/characters/demo")).unwrap();
        std::fs::write(
            src.join("boxxyclaw/characters/demo/CHARACTER.toml"),
            "display_name = 'Demo'",
        )
        .unwrap();
        std::fs::write(src.join("boxxyclaw/boxxy.db"), b"sqlite-binary-stub").unwrap();
        std::fs::write(src.join("boxxyclaw/boxxy.db-wal"), b"wal-stub").unwrap();

        seed_config_tree_from(src, &dst).expect("seed should succeed");

        assert!(dst.join("settings.json").is_file());
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
    fn test_ephemeral_lock_wipes_only_when_unheld() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let eph = tmp.path().join("ephemeral");
        std::fs::create_dir_all(&eph).unwrap();
        std::fs::write(eph.join("stale.json"), "{}").unwrap();

        // First process acquires exclusive lock cleanly, wiping `stale.json`, then holds shared lock
        let guard_one = dev_mode::prepare_ephemeral_dir_in(&eph).expect("first lock");
        assert!(!eph.join("stale.json").exists());

        // Simulate active runtime DB created by the first process (e.g. boxxy-agent)
        std::fs::write(eph.join("live_session.db"), "active").unwrap();

        // Second sibling process (e.g. boxxy-app) joins while `guard_one` is still alive:
        // must NOT wipe `live_session.db`!
        let guard_two = dev_mode::prepare_ephemeral_dir_in(&eph).expect("second lock");
        assert!(eph.join("live_session.db").is_file());

        // Once both processes exit (drop locks), the next ephemeral launch wipes cleanly again
        drop(guard_one);
        drop(guard_two);
        let _guard_three = dev_mode::prepare_ephemeral_dir_in(&eph).expect("third lock");
        assert!(!eph.join("live_session.db").exists());
    }

    #[test]
    #[serial(config_dir)]
    fn test_env_registry_daemon_propagation_and_pty_scrubbing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("custom-cfg");
        let _guard = ConfigDirEnvGuard::set(None);

        configure_dev_options(CliDevOverrides {
            dev: true,
            ephemeral: true,
            debug_context: true,
            config_dir: Some(cfg.clone()),
            ..Default::default()
        });
        assert_eq!(get_config_dir(), cfg);
        assert!(env::debug_context());

        unsafe {
            std::env::set_var(
                "BOXXY_TELEMETRY_ENDPOINT",
                "https://collector.example.com/v1/metrics",
            );
        }

        let spawned = env::daemon_spawn_env(&cfg);
        assert!(spawned.contains(&("BOXXY_DEBUG_CONTEXT", "1".to_string())));
        assert!(spawned.contains(&("BOXXY_INSTANCE", "dev".to_string())));
        assert!(spawned.contains(&("BOXXY_EPHEMERAL", "1".to_string())));
        assert!(spawned.contains(&(
            "BOXXY_TELEMETRY_ENDPOINT",
            "https://collector.example.com/v1/metrics".to_string()
        )));
        assert!(spawned.contains(&("BOXXY_CONFIG_DIR", cfg.to_string_lossy().into_owned())));

        let mut pty_env = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("BOXXY_DEV".to_string(), "1".to_string()),
            ("BOXXY_DEBUG_CONTEXT".to_string(), "1".to_string()),
            ("BOXXY_ORIG_ZDOTDIR".to_string(), "/tmp/zdot".to_string()),
            ("BOXXY_SHELL_INTEGRATION".to_string(), "false".to_string()),
        ];
        env::scrub_child_pty_env(&mut pty_env);
        assert_eq!(
            pty_env,
            vec![
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("BOXXY_SHELL_INTEGRATION".to_string(), "0".to_string()),
            ]
        );
    }
}
