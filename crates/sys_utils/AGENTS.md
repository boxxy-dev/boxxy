# Boxxy-Sys-Utils Agent & Architecture

## Component Responsibilities

Minimal, leaf-level utility crate for system environment detection, isolated dev/ephemeral configuration routing, and global async runtime management. Contains no AI, GTK, D-Bus, or workspace-internal dependencies. Provides the *mechanism* for system checks and ambient network fetches; all *policy* (e.g., whether to fetch location based on user privacy settings) lives in callers.

## Architecture & Modules

Organized into focused domain submodules re-exported from `src/lib.rs`:

- **`env.rs` — Declarative `BOXXY_*` & Provider Env Registry:** `ENV_SPECS` and `PROVIDER_ENV_KEY_MAP` (covering all built-in LLM providers plus `TAVILY_API_KEY`), zero-allocation boolean readers (`parse_bool_str()`, `env_truthy()`, `env_falsy()`, `debug_context()`, `shell_integration_enabled()`), diagnostic readers (`simulate_update_raw()`, `telemetry_endpoint()`), terminal-safe secret masking (`mask_secret()`, revealing prefixes only for `>= 12`-char printable values), data-driven daemon spawn environment construction (`daemon_spawn_env()`), and `BOXXY_`-prefixed fast-path child-PTY environment scrubbing (`scrub_child_pty_env()`, `scrubbed_pty_var_names()`).
- **`config_dir.rs` — Infallible Config Root Resolution:** `get_config_dir()` and `get_stable_config_dir()`. Performs zero filesystem I/O (`Path::is_absolute()` only, never `canonicalize()`) and never caches, consulting `--config-dir` → `$BOXXY_CONFIG_DIR` → ephemeral runtime directory → isolated dev directory → `ProjectDirs` → `$HOME/.config/boxxy-terminal` → UID-scoped `/tmp/boxxy-<uid>/boxxy-terminal` fallback.
- **`dev_mode/` (`mod.rs`, `seed.rs`) — Isolated Dev & Ephemeral Lifecycle:**
  - `dev_mode/mod.rs`: `CliDevOverrides`, `configure_dev_options()`, `sanitize_instance_slug()`, single-lock `instance_id()`, `is_dev_instance()`, `is_ephemeral_mode()`, `should_sync_settings()`, `HashSet<&'static str>`-interned `agent_bus_name()` / `gtk_app_id()`, and `dev_header_tag()`.
  - `dev_mode/seed.rs`: `prepare_dev_environment()` and `seed_config_tree_from()`. Coordinates ephemeral sessions across `boxxy-app` and `boxxy-agent` via `0700` owner-verified directories and atomic `flock` `LOCK_EX` → `LOCK_SH` downgrade on `.ephemeral.lock` (never seeding Stable files in `--ephemeral` mode, even when `BOXXY_CONFIG_DIR` is forwarded to the daemon). Seeds `settings.json`, `state.json`, `user/`, `themes/`, `bookmarks/` (excluding `runs/`), and `boxxyclaw/{BLACKLIST.md,MEMORY.md,characters,skills}` via atomic owner-writable (`0600`) `.tmp` + `rename` copies, dereferencing NixOS `home-manager` `/nix/store` symlinks without inheriting `0444` read-only permissions and rejecting nested destinations.
- **`platform.rs` — Sandbox & Package Channel Detection:** `is_flatpak()` (checks `/.flatpak-info` via `OnceLock<bool>` and `FLATPAK_ID` directly so `GTK_USE_PORTAL=1` or Snap never false-positive as Flatpak), component-aware `classify_package_type()` / `detect_package_type()` (`"flatpak"`, `"distro"`, or `"native"`), and `can_self_update()` (enabled strictly for non-dev `"native"` builds when `disable-self-update` is not set).
- **`runtime.rs` — Global Async Runtime:** Fork-safe lazy `OnceLock<Runtime>` (`runtime()`, `current_worker_index()`) tuned with `max_io_events_per_tick(256)` and `max_io_events_per_busy_tick(64)`.
- **`remote.rs` — Ambient Location & Live Exchange Rates:** `LocationContext`, `fetch_location_context()`, `get_location_context()`, `fetch_exchange_rates()`, and `get_exchange_rate(code)`. Uses a redirect-disabled `reqwest::Client` (`connect_timeout = 2s`, `timeout = 4s`), 16 KiB bounded body reads, a 60-second failure retry cooldown, finite `> 0.0` rate validation, and strict control-character / prompt-delimiter rejection on cleartext `ip-api.com` fields.

## Design Constraints

### Policy vs. Mechanism
`boxxy-sys-utils` provides the *mechanism* for location and exchange-rate fetching only. The *policy* — whether to fetch based on user settings — must remain in the caller. This crate must never import preferences or check settings.

### HTTP for `ip-api.com`
`fetch_location_context()` uses `http://ip-api.com/json/?fields=status,message,city,country,timezone` and must stay HTTP (`ip-api.com`'s free tier does not support HTTPS). Because responses travel over cleartext HTTP and callers interpolate `LocationContext` into LLM prompts, all fields must pass strict length, control-character, delimiter, and IANA timezone validation before caching.

### No Lock Poisoning
Uses `parking_lot::RwLock` (`const fn RwLock::new(...)`) for `CLI_OVERRIDES`, `LOCATION_CACHE`, and `EXCHANGE_RATES_CACHE` to avoid lock poisoning and `OnceLock` indirection on read paths.
