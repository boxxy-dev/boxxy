# Boxxy-Sys-Utils Agent & Architecture

## Component Responsibilities

Minimal, leaf-level utility crate for system environment detection and global async runtime management. No AI dependencies. No GTK. No workspace-internal dependencies. Provides the *mechanism* for system checks; all *policy* (e.g., whether to fetch location based on user settings) lives in callers.

## Core Utilities

- **Config root resolution:** `get_config_dir()` (infallible resolver consulting `$BOXXY_CONFIG_DIR` if absolute, then `ProjectDirs`, then `$HOME/.config/boxxy-terminal`)
- **Environment detection:** `is_flatpak()`, `can_self_update()`
- **Global async runtime:** `runtime()`, `current_worker_index()` (tuned with `max_io_events_per_tick(256)` and `max_io_events_per_busy_tick(64)`)
- **Location context:** `LocationContext`, `fetch_location_context()`, `get_location_context()`
- **Live currency exchange rates:** `fetch_exchange_rates()`, `get_exchange_rate(code)` (fetches ECB rates from `https://api.frankfurter.dev/v1/latest?base=USD&symbols=EUR,CNY,INR,BRL` and caches in `EXCHANGE_RATES_CACHE`)

## Design Constraints

### Policy vs. Mechanism
`boxxy-sys-utils` provides the *mechanism* for location fetching only. The *policy* — whether to fetch based on user privacy settings — must remain in the caller. This crate must never import preferences or check settings; callers decide if and when to call `fetch_location_context()`.

### HTTP for ip-api.com
The implementation uses `http://ip-api.com/json/` and must stay HTTP. ip-api.com's free tier does not support HTTPS — it requires a paid plan. Do not attempt to switch the scheme; it will fail silently or with a TLS error on their end.

### No Lock Poisoning
Uses `parking_lot::RwLock` for `LOCATION_CACHE` to avoid lock poisoning and provide better performance than `std::sync::RwLock`.
