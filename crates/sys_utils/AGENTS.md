# Boxxy-Sys-Utils Agent & Architecture

## Component Responsibilities

Minimal, leaf-level utility crate for system environment detection and global async runtime management. No AI dependencies. No GTK. No workspace-internal dependencies. Provides the *mechanism* for system checks; all *policy* (e.g., whether to fetch location based on user settings) lives in callers.

## Core Utilities

- **Declarative `BOXXY_*` & Provider Env Registry (`env.rs`):** `ENV_SPECS` and `PROVIDER_ENV_KEY_MAP`, plus typed readers (`debug_context()`, `shell_integration_enabled()`, `simulate_update_raw()`, `telemetry_endpoint()`, `mask_secret()`), data-driven daemon spawn environment construction (`daemon_spawn_env()`), and child-PTY environment scrubbing (`scrub_child_pty_env()`, `scrubbed_pty_var_names()`).
- **Config root & dev instance resolution:** `get_config_dir()`, `get_stable_config_dir()`, `instance_id()`, `is_dev_instance()`, `is_ephemeral_mode()`, `agent_bus_name()`, `gtk_app_id()`, `prepare_dev_environment()`, `seed_config_tree_from()` (infallible resolver consulting `--config-dir` / `$BOXXY_CONFIG_DIR` if absolute, then ephemeral/dev directories when `--dev`/`--ephemeral`/`--instance`/`BOXXY_DEV`/`BOXXY_INSTANCE` is active, then `ProjectDirs`, then `$HOME/.config/boxxy-terminal`)
- **Environment detection:** `is_flatpak()`, `can_self_update()` (disabled in Flatpak, isolated dev instances, or `disable-self-update`)
- **Global async runtime:** `runtime()`, `current_worker_index()` (tuned with `max_io_events_per_tick(256)` and `max_io_events_per_busy_tick(64)`)
- **Location context:** `LocationContext`, `fetch_location_context()`, `get_location_context()`
- **Live currency exchange rates:** `fetch_exchange_rates()`, `get_exchange_rate(code)` (fetches ECB rates from `https://api.frankfurter.dev/v1/latest?base=USD&symbols=EUR,CNY,INR,BRL` and caches in `EXCHANGE_RATES_CACHE`)

## Design Constraints

### Policy vs. Mechanism
`boxxy-sys-utils` provides the *mechanism* for location fetching only. The *policy* — whether to fetch based on user privacy settings — must remain in the caller. This crate must never import preferences or check settings; callers decide if and when to call `fetch_location_context()`.

### HTTP for ip-api.com
The implementation uses `http://ip-api.com/json/` and must stay HTTP. ip-api.com's free tier does not support HTTPS — it requires a paid plan. Do not attempt to switch the scheme; it will fail silently or with a TLS error on their end.

### No Lock Poisoning
Uses `parking_lot::RwLock` for `LOCATION_CACHE` and `EXCHANGE_RATES_CACHE` (initialized via `const fn RwLock::new(None)`) to avoid lock poisoning, eliminate `OnceLock` indirection, and provide better performance than `std::sync::RwLock`.
