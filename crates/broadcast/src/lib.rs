#![allow(clippy::missing_errors_doc)]

//! GTK-free remote Markdown announcement client, validator, and SQLite sync coordinator
//! for **Boxxy News** (`boxxy-broadcast`).

pub mod client;
pub mod service;

pub use boxxy_db::models::StoredBroadcastArticle;
pub use client::{BroadcastClientError, DEFAULT_BROADCAST_ENDPOINT, ValidatedBroadcastFeed};
pub use service::{
    BroadcastEvent, SyncTrigger, get_cached_article, get_next_pending_article,
    is_auto_broadcast_enabled, last_sync_succeeded, list_cached_articles, mark_article_dismissed,
    mark_article_read, start_background_poller, subscribe_events, sync_if_stale, sync_now,
};
