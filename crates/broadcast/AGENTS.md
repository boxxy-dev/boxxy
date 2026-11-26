# Broadcast Crate (`boxxy-broadcast`)

## Responsibility
Provides the GTK-free remote Markdown announcement client, payload validator, and background SQLite synchronization service for **Boxxy News**.

## Module Layout
- **`src/lib.rs`**: Public API surface re-exporting `StoredBroadcastArticle`, `BroadcastEvent`, `SyncTrigger`, `start_background_poller`, `sync_now`, `sync_if_stale`, `list_cached_articles`, `get_cached_article`, `get_next_pending_article`, `mark_article_read`, `mark_article_dismissed`, `is_auto_broadcast_enabled`, and `last_sync_succeeded`.
- **`src/client.rs`**: Bounded `reqwest` client (`connect_timeout = 5s`, `timeout = 8s`, `redirect::Policy::none()`, streamed response cap of `512 KiB`) calling the PostgREST RPC `public.get_broadcast_feed` (`p_limit = 4`, `DEFAULT_BROADCAST_ENDPOINT`, overridable via `BOXXY_BROADCAST_ENDPOINT`). Validates `schema_version == 1`, UUIDs, slugs, lengths, `body_md <= 64 KiB`, RFC3339 timestamps, and `https://` action URLs.
- **`src/service.rs`**: Process-wide `SYNC_LOCK`-serialized sync coordinator, `BROADCAST_EVENT_BUS` notifier, 60-second dialog-open staleness cooldown (`sync_if_stale`), and `Once`-guarded background poller (`12s` post-launch settle delay + `60m + 0..5m` jitter).

## Key Invariants
1. **Zero Tracking:** Fetches `public.get_broadcast_feed` using only `SUPABASE_PUBLISHABLE_KEY` without transmitting `install_id`, `session_id`, or read/dismiss receipts. All user dispositions stay local in `boxxy-db`.
2. **Silent First-Boot Baseline:** Delegates reconciliation to `Store::reconcile_broadcast_feed` so existing published posts populate local history on first install with zero toast notifications.
3. **Opt-Out & `--ephemeral` Isolation:** `is_auto_broadcast_enabled()` honors `Settings.enable_broadcasts` (working out of the box in both Stable and `cargo dev-app` `--dev`) and suppresses background polling only in `--ephemeral` clean-room runs unless `BOXXY_BROADCAST_ENDPOINT` is explicitly set.
4. **Single-Latest-Toast Guarantee:** When multiple new announcements arrive while the user was away, only the single newest post stays `'pending'` for a toast notification; older unseen posts step down to `'dismissed'` so they retain their unread dot in `Boxxy News` without queueing follow-up toasts.
