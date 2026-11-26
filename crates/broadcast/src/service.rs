use crate::client::{fetch_broadcast_feed, resolve_endpoint};
use anyhow::Result;
use boxxy_db::Db;
use boxxy_db::models::StoredBroadcastArticle;
use parking_lot::{Mutex, RwLock};
use rand::RngExt;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

const STARTUP_SETTLE_DELAY: Duration = Duration::from_secs(12);
const POLL_BASE_INTERVAL_SECS: u64 = 3600;
const POLL_JITTER_MAX_SECS: u64 = 300;
const DIALOG_STALE_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncTrigger {
    Background,
    DialogOpen,
    ManualRefresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadcastEvent {
    Updated { sync_succeeded: bool },
}

static BROADCAST_EVENT_BUS: LazyLock<broadcast::Sender<BroadcastEvent>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(16);
    tx
});

static SYNC_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(|| AsyncMutex::new(()));
static LAST_SYNC_INSTANT: Mutex<Option<Instant>> = Mutex::new(None);
static LAST_SYNC_ONLINE: AtomicBool = AtomicBool::new(true);
static POLLER_STARTED: AtomicBool = AtomicBool::new(false);

/// Caches the `Db` pool paired with its resolved `db_path` so tests that swap
/// `BOXXY_CONFIG_DIR` transparently open a fresh database at the new path.
static DB_HANDLE: RwLock<Option<(PathBuf, Db)>> = RwLock::new(None);

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

async fn ensure_db() -> Result<Db> {
    let cached = DB_HANDLE.read().clone();
    let expected_path = boxxy_sys_utils::get_config_dir().join("boxxyclaw/boxxy.db");
    if let Some((cached_path, db)) = cached
        && cached_path == expected_path
    {
        return Ok(db);
    }

    let db = Db::open_at(&expected_path).await?;
    *DB_HANDLE.write() = Some((expected_path, db.clone()));
    Ok(db)
}

fn is_within_dialog_cooldown() -> bool {
    LAST_SYNC_INSTANT
        .lock()
        .is_some_and(|prev| prev.elapsed() < DIALOG_STALE_COOLDOWN)
}

/// Subscribes to live broadcast feed/state notifications (`BroadcastEvent::Updated`).
#[must_use]
pub fn subscribe_events() -> broadcast::Receiver<BroadcastEvent> {
    BROADCAST_EVENT_BUS.subscribe()
}

/// Returns whether the most recent network sync succeeded (`false` indicates offline fallback).
#[must_use]
pub fn last_sync_succeeded() -> bool {
    LAST_SYNC_ONLINE.load(Ordering::Relaxed)
}

/// Returns `true` if automatic background polling and persistent toast notifications are active.
///
/// Works out of the box in both Stable and `--dev` (`cargo dev-app`) using
/// `DEFAULT_BROADCAST_ENDPOINT` (since the feed RPC is strictly read-only), and is suppressed
/// only in `--ephemeral` clean-room runs unless `BOXXY_BROADCAST_ENDPOINT` is explicitly set.
#[must_use]
pub fn is_auto_broadcast_enabled() -> bool {
    if boxxy_sys_utils::is_ephemeral_mode() && boxxy_sys_utils::env::broadcast_endpoint().is_none()
    {
        return false;
    }
    boxxy_preferences::Settings::load().enable_broadcasts
}

/// Lists all locally cached Boxxy News articles ordered newest-first (`0ms` offline-capable).
pub async fn list_cached_articles() -> Result<Vec<StoredBroadcastArticle>> {
    let db = ensure_db().await?;
    db.store().list_broadcast_articles().await
}

/// Fetches a single cached Boxxy News article by UUID.
pub async fn get_cached_article(id: &str) -> Result<Option<StoredBroadcastArticle>> {
    let db = ensure_db().await?;
    db.store().get_broadcast_article(id).await
}

/// Returns the newest cached article whose receipt is currently `'pending'`, if any.
pub async fn get_next_pending_article() -> Result<Option<StoredBroadcastArticle>> {
    let db = ensure_db().await?;
    db.store().get_next_pending_broadcast().await
}

/// Marks an article `'read'` in local SQLite and notifies open UI listeners.
pub async fn mark_article_read(id: &str) -> Result<()> {
    let db = ensure_db().await?;
    db.store().mark_broadcast_read(id, now_epoch_ms()).await?;
    let _ = BROADCAST_EVENT_BUS.send(BroadcastEvent::Updated {
        sync_succeeded: last_sync_succeeded(),
    });
    Ok(())
}

/// Marks a `'pending'` toast `'dismissed'` when closed via `×` so it never auto-toasts again.
pub async fn mark_article_dismissed(id: &str) -> Result<()> {
    let db = ensure_db().await?;
    db.store()
        .mark_broadcast_dismissed(id, now_epoch_ms())
        .await?;
    let _ = BROADCAST_EVENT_BUS.send(BroadcastEvent::Updated {
        sync_succeeded: last_sync_succeeded(),
    });
    Ok(())
}

/// Synchronizes the remote feed immediately (serialized across concurrent callers).
pub async fn sync_now(trigger: SyncTrigger) -> Result<Vec<StoredBroadcastArticle>> {
    let _guard = SYNC_LOCK.lock().await;

    if trigger == SyncTrigger::DialogOpen && is_within_dialog_cooldown() {
        return list_cached_articles().await;
    }

    let endpoint = resolve_endpoint();
    match fetch_broadcast_feed(&endpoint).await {
        Ok(feed) => {
            let db = ensure_db().await?;
            let store = db.store();
            store
                .reconcile_broadcast_feed(
                    &feed.server_time,
                    &feed.items,
                    &feed.withdrawn_ids,
                    now_epoch_ms(),
                )
                .await?;
            *LAST_SYNC_INSTANT.lock() = Some(Instant::now());
            LAST_SYNC_ONLINE.store(true, Ordering::Relaxed);
            let _ = BROADCAST_EVENT_BUS.send(BroadcastEvent::Updated {
                sync_succeeded: true,
            });
            store.list_broadcast_articles().await
        }
        Err(err) => {
            log::debug!("Boxxy News sync failed ({trigger:?}): {err}");
            LAST_SYNC_ONLINE.store(false, Ordering::Relaxed);
            let _ = BROADCAST_EVENT_BUS.send(BroadcastEvent::Updated {
                sync_succeeded: false,
            });
            Err(anyhow::Error::new(err))
        }
    }
}

/// Refreshes the remote feed in the background only if the last successful sync was `>= 60s` ago.
pub async fn sync_if_stale() -> Result<Vec<StoredBroadcastArticle>> {
    if is_within_dialog_cooldown() {
        return list_cached_articles().await;
    }
    sync_now(SyncTrigger::DialogOpen).await
}

fn next_poll_sleep_duration() -> Duration {
    let jitter_secs = rand::rng().random_range(0_u64..POLL_JITTER_MAX_SECS);
    Duration::from_secs(POLL_BASE_INTERVAL_SECS + jitter_secs)
}

/// Starts the single process-wide background broadcast poller on the global Tokio runtime.
///
/// Calling this multiple times (e.g. when opening additional windows) is a safe no-op.
pub fn start_background_poller() {
    if POLLER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    boxxy_sys_utils::runtime().spawn(async move {
        tokio::time::sleep(STARTUP_SETTLE_DELAY).await;

        loop {
            if is_auto_broadcast_enabled() {
                // Surface any unresolved `'pending'` toast from a prior session immediately,
                // even before network I/O completes or when completely offline.
                if matches!(get_next_pending_article().await, Ok(Some(_))) {
                    let _ = BROADCAST_EVENT_BUS.send(BroadcastEvent::Updated {
                        sync_succeeded: last_sync_succeeded(),
                    });
                }

                let _ = sync_now(SyncTrigger::Background).await;
            }

            tokio::time::sleep(next_poll_sleep_duration()).await;
        }
    });
}

#[cfg(test)]
pub(crate) fn reset_sync_cooldown_for_test() {
    *LAST_SYNC_INSTANT.lock() = None;
    LAST_SYNC_ONLINE.store(true, Ordering::Relaxed);
    *DB_HANDLE.write() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_mock_rpc_server(responses: Vec<(u16, String)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let addr = listener.local_addr().expect("local_addr");
        let shared = Arc::new(Mutex::new(responses.into_iter()));

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let next = shared.lock().next();
                let Some((status, body)) = next else {
                    break;
                };
                tokio::spawn(async move {
                    let mut req_buf = [0_u8; 4096];
                    let _ = stream.read(&mut req_buf).await;
                    let status_text = if status == 200 { "OK" } else { "Bad Gateway" };
                    let reply = format!(
                        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });

        format!("http://{addr}/rest/v1/rpc/get_broadcast_feed")
    }

    #[tokio::test]
    #[serial]
    async fn end_to_end_mock_sync_baseline_pending_and_cooldown() {
        let tmp = tempfile::tempdir().expect("tempdir");
        reset_sync_cooldown_for_test();

        boxxy_sys_utils::configure_dev_options(boxxy_sys_utils::CliDevOverrides {
            dev: true,
            ephemeral: true,
            config_dir: Some(tmp.path().to_path_buf()),
            ..Default::default()
        });

        let baseline_json = r#"{
            "schema_version": 1,
            "server_time": "2026-11-26T10:00:00Z",
            "items": [{
                "id": "11111111-1111-4111-8111-111111111111",
                "slug": "welcome-post",
                "title": "Welcome to Boxxy News",
                "summary": "Initial baseline post",
                "body_md": "Hello from Boxxy!",
                "category": "news",
                "status": "published",
                "notify": true,
                "published_at": "2026-11-25T10:00:00Z",
                "revision": 1
            }],
            "withdrawn_ids": []
        }"#
        .to_string();

        let new_live_json = r#"{
            "schema_version": 1,
            "server_time": "2026-11-26T12:00:00Z",
            "items": [
                {
                    "id": "22222222-2222-4222-8222-222222222222",
                    "slug": "fresh-release",
                    "title": "Fresh Feature Shipped",
                    "summary": "New release summary",
                    "body_md": "Details inside.",
                    "category": "feature",
                    "status": "published",
                    "notify": true,
                    "action_label": "Docs",
                    "action_url": "https://example.com/docs",
                    "published_at": "2026-11-26T11:30:00Z",
                    "revision": 1
                },
                {
                    "id": "11111111-1111-4111-8111-111111111111",
                    "slug": "welcome-post",
                    "title": "Welcome to Boxxy News",
                    "summary": "Initial baseline post",
                    "body_md": "Hello from Boxxy!",
                    "category": "news",
                    "status": "published",
                    "notify": true,
                    "published_at": "2026-11-25T10:00:00Z",
                    "revision": 1
                }
            ],
            "withdrawn_ids": []
        }"#
        .to_string();

        let endpoint = spawn_mock_rpc_server(vec![(200, baseline_json), (200, new_live_json)]).await;
        // SAFETY: serialized via `#[serial]` within unit test process.
        unsafe {
            std::env::set_var("BOXXY_BROADCAST_ENDPOINT", &endpoint);
        }

        // `--ephemeral` with explicit `BOXXY_BROADCAST_ENDPOINT` enables auto broadcast testing
        assert!(is_auto_broadcast_enabled());

        // 1. First sync establishes silent baseline -> 1 cached article, 0 pending toasts
        let initial = sync_now(SyncTrigger::ManualRefresh).await.unwrap();
        assert_eq!(initial.len(), 1);
        assert_eq!(get_next_pending_article().await.unwrap(), None);

        // 2. `sync_if_stale` within 60s skips network and returns cached list immediately
        let cached_fast = sync_if_stale().await.unwrap();
        assert_eq!(cached_fast.len(), 1);

        // 3. `ManualRefresh` bypasses cooldown and receives the new post -> marked `'pending'`
        let refreshed = sync_now(SyncTrigger::ManualRefresh).await.unwrap();
        assert_eq!(refreshed.len(), 2);
        let pending = get_next_pending_article()
            .await
            .unwrap()
            .expect("new post should be pending");
        assert_eq!(pending.slug, "fresh-release");

        // 4. Mark read clears pending
        mark_article_read(&pending.id).await.unwrap();
        assert_eq!(get_next_pending_article().await.unwrap(), None);

        // Clean up env var -> in `--ephemeral` mode without endpoint override, auto broadcast is disabled,
        // whereas regular `--dev` mode enables auto broadcast out of the box.
        unsafe {
            std::env::remove_var("BOXXY_BROADCAST_ENDPOINT");
        }
        assert!(!is_auto_broadcast_enabled());

        boxxy_sys_utils::configure_dev_options(boxxy_sys_utils::CliDevOverrides {
            dev: true,
            ephemeral: false,
            config_dir: Some(tmp.path().to_path_buf()),
            ..Default::default()
        });
        assert!(is_auto_broadcast_enabled());
    }
}
