use crate::models::{BroadcastArticleUpsert, StoredBroadcastArticle};
use crate::store::Store;
use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use sqlx::QueryBuilder;

/// Maximum number of full Markdown bodies retained in `broadcast_articles`.
///
/// The desktop app surfaces only the latest 4 announcements; users browse the full archive on
/// `https://boxxy.dev/news/`. Lightweight `<80 B` user decisions in `broadcast_receipts` are
/// intentionally kept indefinitely without a foreign key so pruned older posts never re-notify.
pub const MAX_CACHED_BROADCAST_ARTICLES: i64 = 4;

/// Maximum age (relative to server time) for a newly observed post to trigger a desktop toast.
///
/// Prevents a user reopening Boxxy after months offline from getting toasted about stale news
/// while still populating the post inside the `Boxxy News` archive list.
pub const NEW_NOTIFICATION_MAX_AGE_DAYS: i64 = 14;

const META_INITIAL_BASELINE_AT: &str = "initial_baseline_at";
const META_LAST_SYNC_MS: &str = "last_sync_ms";

fn parse_rfc3339_utc(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("Invalid RFC3339 timestamp: {raw}"))
}

/// Formats a UTC timestamp in fixed-width millisecond ISO-8601 (`YYYY-MM-DDTHH:MM:SS.sssZ`)
/// so SQLite `TEXT` `ORDER BY published_at DESC` is strictly chronological regardless of the
/// input's timezone offset or fractional-second precision.
fn canonical_utc_rfc3339(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

const UPSERT_ARTICLE_SQL: &str = r"
INSERT INTO broadcast_articles (
    id,
    slug,
    title,
    summary,
    body_md,
    category,
    status,
    notify,
    action_label,
    action_url,
    published_at,
    updated_at,
    revision,
    cached_at_ms
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    slug = excluded.slug,
    title = excluded.title,
    summary = excluded.summary,
    body_md = excluded.body_md,
    category = excluded.category,
    status = excluded.status,
    notify = excluded.notify,
    action_label = excluded.action_label,
    action_url = excluded.action_url,
    published_at = excluded.published_at,
    updated_at = excluded.updated_at,
    revision = excluded.revision,
    cached_at_ms = excluded.cached_at_ms
";

impl Store<'_> {
    /// Atomically reconciles a remote `get_broadcast_feed` snapshot into local SQLite storage:
    /// - Establishes `'initial_baseline_at'` on first-ever sync and marks existing posts `'baseline'`
    ///   (zero toasts on fresh installs).
    /// - Upserts cached articles and assigns `'pending'` vs `'baseline'` only for unseen UUIDs,
    ///   never overwriting `'read'` or `'dismissed'` receipts when an author edits a post.
    /// - Evicts any cached article no longer present in the server's live feed (covering Withdraw,
    ///   hard Delete Forever, reverting to Draft, or direct SQL deletion) and cancels pending toasts.
    /// - Prunes `broadcast_articles` to the newest [`MAX_CACHED_BROADCAST_ARTICLES`] rows.
    #[allow(clippy::too_many_lines)]
    pub async fn reconcile_broadcast_feed(
        &self,
        server_time_rfc3339: &str,
        items: &[BroadcastArticleUpsert],
        withdrawn_ids: &[String],
        now_ms: i64,
    ) -> Result<()> {
        let server_dt = parse_rfc3339_utc(server_time_rfc3339)?;
        let server_canonical = canonical_utc_rfc3339(server_dt);
        let max_toast_age = TimeDelta::days(NEW_NOTIFICATION_MAX_AGE_DAYS);

        let mut tx = self.pool.begin().await?;

        let existing_baseline_raw: Option<String> = sqlx::query_scalar(
            "SELECT value FROM broadcast_meta WHERE key = ? LIMIT 1",
        )
        .bind(META_INITIAL_BASELINE_AT)
        .fetch_optional(&mut *tx)
        .await?;

        let baseline_dt = if let Some(raw) = existing_baseline_raw {
            Some(parse_rfc3339_utc(&raw).unwrap_or(server_dt))
        } else {
            sqlx::query(
                "INSERT INTO broadcast_meta (key, value) VALUES (?, ?) \
                 ON CONFLICT(key) DO NOTHING",
            )
            .bind(META_INITIAL_BASELINE_AT)
            .bind(&server_canonical)
            .execute(&mut *tx)
            .await?;
            None
        };

        let mut live_ids: Vec<&str> = Vec::with_capacity(items.len());

        for item in items {
            if !matches!(item.status.as_str(), "published" | "archived") {
                continue;
            }
            let Ok(pub_dt) = parse_rfc3339_utc(&item.published_at) else {
                log::warn!(
                    "Broadcast: skipping article '{}' with invalid published_at '{}'",
                    item.id,
                    item.published_at
                );
                continue;
            };
            let upd_dt = parse_rfc3339_utc(&item.updated_at).unwrap_or(pub_dt);
            let pub_canonical = canonical_utc_rfc3339(pub_dt);
            let upd_canonical = canonical_utc_rfc3339(upd_dt);

            sqlx::query(UPSERT_ARTICLE_SQL)
                .bind(&item.id)
                .bind(&item.slug)
                .bind(&item.title)
                .bind(&item.summary)
                .bind(&item.body_md)
                .bind(&item.category)
                .bind(&item.status)
                .bind(i64::from(item.notify))
                .bind(&item.action_label)
                .bind(&item.action_url)
                .bind(&pub_canonical)
                .bind(&upd_canonical)
                .bind(item.revision.max(1))
                .bind(now_ms)
                .execute(&mut *tx)
                .await?;

            live_ids.push(item.id.as_str());

            let initial_disposition = baseline_dt.map_or("baseline", |base_dt| {
                let is_recent = server_dt.signed_duration_since(pub_dt) <= max_toast_age;
                if item.status == "published" && item.notify && pub_dt > base_dt && is_recent {
                    "pending"
                } else {
                    "baseline"
                }
            });

            // `DO NOTHING` guarantees content edits (`revision > 1`) never reset `'read'` or `'dismissed'`.
            sqlx::query(
                "INSERT INTO broadcast_receipts (id, disposition, updated_at_ms) \
                 VALUES (?, ?, ?) \
                 ON CONFLICT(id) DO NOTHING",
            )
            .bind(&item.id)
            .bind(initial_disposition)
            .bind(now_ms)
            .execute(&mut *tx)
            .await?;
        }

        // Authoritative snapshot eviction: any cached article absent from `live_ids` was withdrawn,
        // hard-deleted, reverted to draft, or superseded on the server — remove it from local cache
        // and cancel any `'pending'` toast while keeping `'read'`/`'dismissed'`/`'baseline'` receipts.
        if live_ids.is_empty() {
            sqlx::query(
                "UPDATE broadcast_receipts \
                 SET disposition = 'dismissed', updated_at_ms = ? \
                 WHERE disposition = 'pending' \
                   AND id IN (SELECT id FROM broadcast_articles)",
            )
            .bind(now_ms)
            .execute(&mut *tx)
            .await?;

            sqlx::query("DELETE FROM broadcast_articles")
                .execute(&mut *tx)
                .await?;
        } else {
            let mut cancel_qb = QueryBuilder::new(
                "UPDATE broadcast_receipts SET disposition = 'dismissed', updated_at_ms = ",
            );
            cancel_qb.push_bind(now_ms);
            cancel_qb.push(
                " WHERE disposition = 'pending' AND id IN (SELECT id FROM broadcast_articles WHERE id NOT IN (",
            );
            let mut sep = cancel_qb.separated(", ");
            for id in &live_ids {
                sep.push_bind(*id);
            }
            sep.push_unseparated("))");
            cancel_qb.build().execute(&mut *tx).await?;

            let mut del_qb = QueryBuilder::new("DELETE FROM broadcast_articles WHERE id NOT IN (");
            let mut del_sep = del_qb.separated(", ");
            for id in &live_ids {
                del_sep.push_bind(*id);
            }
            del_sep.push_unseparated(")");
            del_qb.build().execute(&mut *tx).await?;
        }

        for withdrawn_id in withdrawn_ids {
            let trimmed = withdrawn_id.trim();
            if trimmed.is_empty() {
                continue;
            }
            sqlx::query("DELETE FROM broadcast_articles WHERE id = ?")
                .bind(trimmed)
                .execute(&mut *tx)
                .await?;

            sqlx::query(
                "INSERT INTO broadcast_receipts (id, disposition, updated_at_ms) \
                 VALUES (?, 'dismissed', ?) \
                 ON CONFLICT(id) DO UPDATE SET \
                     disposition = CASE \
                         WHEN broadcast_receipts.disposition = 'read' THEN 'read' \
                         ELSE 'dismissed' \
                     END, \
                     updated_at_ms = excluded.updated_at_ms",
            )
            .bind(trimmed)
            .bind(now_ms)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT INTO broadcast_meta (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(META_LAST_SYNC_MS)
        .bind(now_ms.to_string())
        .execute(&mut *tx)
        .await?;

        // Retain only the newest cached Markdown bodies while leaving `broadcast_receipts` intact.
        sqlx::query(
            "DELETE FROM broadcast_articles WHERE id NOT IN ( \
                SELECT id FROM broadcast_articles \
                ORDER BY published_at DESC, id DESC \
                LIMIT ? \
            )",
        )
        .bind(MAX_CACHED_BROADCAST_ARTICLES)
        .execute(&mut *tx)
        .await?;

        // Single-latest-toast invariant: at most ONE article (strictly the newest pending
        // published post) may remain `'pending'`. Any older unseen arrivals step down to
        // `'dismissed'` so they retain their unread dot inside `Boxxy News` without queueing
        // back-to-back toast notifications after the user reads or dismisses the latest toast.
        sqlx::query(
            "UPDATE broadcast_receipts \
             SET disposition = 'dismissed', updated_at_ms = ? \
             WHERE disposition = 'pending' \
               AND id IS NOT ( \
                   SELECT a.id \
                   FROM broadcast_articles a \
                   INNER JOIN broadcast_receipts r ON r.id = a.id \
                   WHERE r.disposition = 'pending' AND a.status = 'published' \
                   ORDER BY a.published_at DESC, a.id DESC \
                   LIMIT 1 \
               )",
        )
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Lists the newest locally cached Boxxy News articles (up to [`MAX_CACHED_BROADCAST_ARTICLES`])
    /// ordered newest-first alongside their user disposition.
    pub async fn list_broadcast_articles(&self) -> Result<Vec<StoredBroadcastArticle>> {
        let rows = sqlx::query_as::<_, StoredBroadcastArticle>(
            r"
            SELECT
                a.id,
                a.slug,
                a.title,
                a.summary,
                a.body_md,
                a.category,
                a.status,
                (a.notify != 0) AS notify,
                a.action_label,
                a.action_url,
                a.published_at,
                a.updated_at,
                a.revision,
                a.cached_at_ms,
                COALESCE(r.disposition, 'baseline') AS disposition
            FROM broadcast_articles a
            LEFT JOIN broadcast_receipts r ON r.id = a.id
            ORDER BY a.published_at DESC, a.id DESC
            LIMIT ?
            ",
        )
        .bind(MAX_CACHED_BROADCAST_ARTICLES)
        .fetch_all(self.pool)
        .await?;

        Ok(rows)
    }

    /// Fetches a single cached Boxxy News article by UUID.
    pub async fn get_broadcast_article(&self, id: &str) -> Result<Option<StoredBroadcastArticle>> {
        let row = sqlx::query_as::<_, StoredBroadcastArticle>(
            r"
            SELECT
                a.id,
                a.slug,
                a.title,
                a.summary,
                a.body_md,
                a.category,
                a.status,
                (a.notify != 0) AS notify,
                a.action_label,
                a.action_url,
                a.published_at,
                a.updated_at,
                a.revision,
                a.cached_at_ms,
                COALESCE(r.disposition, 'baseline') AS disposition
            FROM broadcast_articles a
            LEFT JOIN broadcast_receipts r ON r.id = a.id
            WHERE a.id = ?
            LIMIT 1
            ",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;

        Ok(row)
    }

    /// Returns the newest cached article whose receipt is currently `'pending'` (eligible for a toast).
    pub async fn get_next_pending_broadcast(&self) -> Result<Option<StoredBroadcastArticle>> {
        let row = sqlx::query_as::<_, StoredBroadcastArticle>(
            r"
            SELECT
                a.id,
                a.slug,
                a.title,
                a.summary,
                a.body_md,
                a.category,
                a.status,
                (a.notify != 0) AS notify,
                a.action_label,
                a.action_url,
                a.published_at,
                a.updated_at,
                a.revision,
                a.cached_at_ms,
                r.disposition
            FROM broadcast_articles a
            INNER JOIN broadcast_receipts r ON r.id = a.id
            WHERE r.disposition = 'pending' AND a.status = 'published'
            ORDER BY a.published_at DESC, a.id DESC
            LIMIT 1
            ",
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(row)
    }

    /// Marks a broadcast UUID as `'read'` so it never toasts again and clears its unread dot.
    pub async fn mark_broadcast_read(&self, id: &str, now_ms: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO broadcast_receipts (id, disposition, updated_at_ms) \
             VALUES (?, 'read', ?) \
             ON CONFLICT(id) DO UPDATE SET \
                 disposition = 'read', \
                 updated_at_ms = excluded.updated_at_ms",
        )
        .bind(id)
        .bind(now_ms)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Resolves a `'pending'` toast when the user clicks `×` on the toast notification,
    /// keeping the unread dot in `Boxxy News` while never downgrading an already `'read'` post.
    pub async fn mark_broadcast_dismissed(&self, id: &str, now_ms: i64) -> Result<()> {
        sqlx::query(
            "UPDATE broadcast_receipts \
             SET disposition = 'dismissed', updated_at_ms = ? \
             WHERE id = ? AND disposition = 'pending'",
        )
        .bind(now_ms)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Returns the epoch millisecond timestamp of the last successful feed sync, if any.
    pub async fn get_broadcast_last_sync_ms(&self) -> Result<Option<i64>> {
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT value FROM broadcast_meta WHERE key = ? LIMIT 1",
        )
        .bind(META_LAST_SYNC_MS)
        .fetch_optional(self.pool)
        .await?;

        Ok(raw.and_then(|v| v.trim().parse::<i64>().ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;

    fn sample_article(
        id: &str,
        slug: &str,
        published_at: &str,
        status: &str,
        notify: bool,
    ) -> BroadcastArticleUpsert {
        BroadcastArticleUpsert {
            id: id.to_string(),
            slug: slug.to_string(),
            title: format!("Title for {slug}"),
            summary: format!("Summary for {slug}"),
            body_md: format!("## Heading\n\nBody content for {slug}."),
            category: "news".to_string(),
            status: status.to_string(),
            notify,
            action_label: Some("Learn More".to_string()),
            action_url: Some("https://example.com/docs".to_string()),
            published_at: published_at.to_string(),
            updated_at: published_at.to_string(),
            revision: 1,
        }
    }

    #[tokio::test]
    async fn fresh_install_baseline_populates_history_with_zero_pending_toasts() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        let items = vec![
            sample_article(
                "00000000-0000-4000-8000-000000000002",
                "second-post",
                "2026-11-20T12:00:00Z",
                "published",
                true,
            ),
            sample_article(
                "00000000-0000-4000-8000-000000000001",
                "first-post",
                "2026-11-10T09:00:00Z",
                "published",
                true,
            ),
        ];

        store
            .reconcile_broadcast_feed("2026-11-26T10:00:00Z", &items, &[], 1_764_151_200_000)
            .await
            .unwrap();

        assert_eq!(store.get_next_pending_broadcast().await.unwrap(), None);
        assert_eq!(
            store.get_broadcast_last_sync_ms().await.unwrap(),
            Some(1_764_151_200_000)
        );

        let listed = store.list_broadcast_articles().await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].slug, "second-post");
        assert_eq!(listed[0].disposition, "baseline");
        assert!(!listed[0].is_unread());
        assert_eq!(listed[1].slug, "first-post");
        assert!(!listed[1].is_unread());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn post_baseline_lifecycle_edits_dismissal_read_and_withdrawal() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        // 1. Initial baseline sync with empty feed
        store
            .reconcile_broadcast_feed("2026-11-26T10:00:00Z", &[], &[], 1_000)
            .await
            .unwrap();

        // 2. Subsequent sync delivers:
        //    - Alpha: published after baseline with notify=true -> 'pending'
        //    - Silent: published after baseline with notify=false -> 'baseline'
        //    - Archived: archived -> 'baseline'
        //    - Stale: older than 14 days relative to server_time -> 'baseline'
        let alpha_id = "00000000-0000-4000-8000-000000000010";
        let silent_id = "00000000-0000-4000-8000-000000000011";
        let archived_id = "00000000-0000-4000-8000-000000000012";
        let stale_id = "00000000-0000-4000-8000-000000000013";

        let batch = [
            sample_article(
                alpha_id,
                "post-a",
                "2026-11-26T11:00:00Z",
                "published",
                true,
            ),
            sample_article(
                silent_id,
                "post-b",
                "2026-11-26T11:05:00Z",
                "published",
                false,
            ),
            sample_article(
                archived_id,
                "post-c",
                "2026-11-26T11:10:00Z",
                "archived",
                true,
            ),
            sample_article(
                stale_id,
                "post-d-stale",
                "2026-11-27T00:00:00Z",
                "published",
                true,
            ),
        ];

        store
            .reconcile_broadcast_feed("2026-11-26T12:00:00Z", &batch[..3], &[], 2_000)
            .await
            .unwrap();

        let pending = store
            .get_next_pending_broadcast()
            .await
            .unwrap()
            .expect("alpha should be pending");
        assert_eq!(pending.id, alpha_id);
        assert!(pending.is_unread());

        let silent_row = store.get_broadcast_article(silent_id).await.unwrap().unwrap();
        assert_eq!(silent_row.disposition, "baseline");
        assert!(!silent_row.is_unread());

        let archived_row = store
            .get_broadcast_article(archived_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(archived_row.disposition, "baseline");

        // 3. User clicks `×` on toast -> transitions 'pending' -> 'dismissed' (still unread in list, never re-toasts)
        store.mark_broadcast_dismissed(alpha_id, 3_000).await.unwrap();
        assert_eq!(store.get_next_pending_broadcast().await.unwrap(), None);
        let dismissed_alpha = store.get_broadcast_article(alpha_id).await.unwrap().unwrap();
        assert_eq!(dismissed_alpha.disposition, "dismissed");
        assert!(dismissed_alpha.is_unread());

        // 4. Author edits Alpha (`revision = 2`) -> cached text updates, disposition stays 'dismissed' (no re-toast!)
        let mut edited_alpha = batch[0].clone();
        edited_alpha.title = "Updated Title".to_string();
        edited_alpha.body_md = "Updated Markdown".to_string();
        edited_alpha.revision = 2;
        store
            .reconcile_broadcast_feed("2026-11-26T12:30:00Z", &[edited_alpha], &[], 4_000)
            .await
            .unwrap();

        assert_eq!(store.get_next_pending_broadcast().await.unwrap(), None);
        let updated_alpha = store.get_broadcast_article(alpha_id).await.unwrap().unwrap();
        assert_eq!(updated_alpha.title, "Updated Title");
        assert_eq!(updated_alpha.revision, 2);
        assert_eq!(updated_alpha.disposition, "dismissed");

        // 5. User opens Alpha in dialog -> transitions 'dismissed' -> 'read', and a late `mark_broadcast_dismissed`
        //    never downgrades 'read' back to 'dismissed'.
        store.mark_broadcast_read(alpha_id, 5_000).await.unwrap();
        store.mark_broadcast_dismissed(alpha_id, 5_500).await.unwrap();
        let read_alpha = store.get_broadcast_article(alpha_id).await.unwrap().unwrap();
        assert_eq!(read_alpha.disposition, "read");
        assert!(!read_alpha.is_unread());

        // 6. Stale post (20 days old when client finally reconnects) is cached as 'baseline' without toasting
        store
            .reconcile_broadcast_feed("2026-12-20T00:00:00Z", &[batch[3].clone()], &[], 6_000)
            .await
            .unwrap();
        assert_eq!(store.get_next_pending_broadcast().await.unwrap(), None);
        let stale_row = store.get_broadcast_article(stale_id).await.unwrap().unwrap();
        assert_eq!(stale_row.disposition, "baseline");

        // 7. New pending post arrives and is then withdrawn -> removed from cache & pending cancelled
        let withdrawn_id = "00000000-0000-4000-8000-000000000014";
        let transient_post = sample_article(
            withdrawn_id,
            "post-e",
            "2026-12-20T01:00:00Z",
            "published",
            true,
        );
        store
            .reconcile_broadcast_feed("2026-12-20T02:00:00Z", &[transient_post], &[], 7_000)
            .await
            .unwrap();
        assert_eq!(
            store
                .get_next_pending_broadcast()
                .await
                .unwrap()
                .unwrap()
                .id,
            withdrawn_id
        );

        store
            .reconcile_broadcast_feed(
                "2026-12-20T03:00:00Z",
                &[],
                &[withdrawn_id.to_string()],
                8_000,
            )
            .await
            .unwrap();
        assert_eq!(store.get_next_pending_broadcast().await.unwrap(), None);
        assert_eq!(store.get_broadcast_article(withdrawn_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn cache_prunes_articles_to_latest_4_while_retaining_all_receipts() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        let mut items = Vec::with_capacity(12);
        for idx in 0..12_u32 {
            let day = idx + 1;
            items.push(sample_article(
                &format!("00000000-0000-4000-8000-{idx:012}"),
                &format!("post-{idx:02}"),
                &format!("2026-11-{day:02}T12:00:00Z"),
                "published",
                true,
            ));
        }

        store
            .reconcile_broadcast_feed("2026-11-30T00:00:00Z", &items, &[], 9_000)
            .await
            .unwrap();

        let cached = store.list_broadcast_articles().await.unwrap();
        assert_eq!(cached.len(), 4);
        assert_eq!(
            i64::try_from(cached.len()).unwrap(),
            MAX_CACHED_BROADCAST_ARTICLES
        );
        assert_eq!(cached[0].slug, "post-11");
        assert_eq!(cached[3].slug, "post-08");

        let receipt_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM broadcast_receipts")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(receipt_count, 12);
    }

    #[tokio::test]
    async fn hard_deleted_or_missing_server_articles_are_evicted_without_withdrawn_ids() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        // Establish baseline first
        store
            .reconcile_broadcast_feed("2026-11-26T10:00:00Z", &[], &[], 1_000)
            .await
            .unwrap();

        let keep_id = "00000000-0000-4000-8000-000000000021";
        let hard_deleted_id = "00000000-0000-4000-8000-000000000022";
        let keep_post = sample_article(
            keep_id,
            "keep-post",
            "2026-11-26T11:00:00Z",
            "published",
            true,
        );
        let deleted_post = sample_article(
            hard_deleted_id,
            "deleted-post",
            "2026-11-26T12:00:00Z",
            "published",
            true,
        );

        // Both posts arrive live -> newest (`deleted_post`) is pending
        store
            .reconcile_broadcast_feed(
                "2026-11-26T12:05:00Z",
                &[deleted_post, keep_post.clone()],
                &[],
                2_000,
            )
            .await
            .unwrap();
        assert_eq!(store.list_broadcast_articles().await.unwrap().len(), 2);
        assert_eq!(
            store.get_next_pending_broadcast().await.unwrap().unwrap().id,
            hard_deleted_id
        );

        // Admin hard-deletes `deleted_post` from Postgres (`withdrawn_ids` is empty!)
        store
            .reconcile_broadcast_feed("2026-11-26T12:10:00Z", &[keep_post], &[], 3_000)
            .await
            .unwrap();

        let remaining = store.list_broadcast_articles().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, keep_id);
        assert_eq!(store.get_broadcast_article(hard_deleted_id).await.unwrap(), None);

        // Older `keep_post` keeps its unread dot (`'dismissed'`) without queueing a second toast
        assert!(remaining[0].is_unread());
        assert_eq!(store.get_next_pending_broadcast().await.unwrap(), None);

        // Admin deletes the last post too -> empty feed clears local cache completely
        store
            .reconcile_broadcast_feed("2026-11-26T12:15:00Z", &[], &[], 4_000)
            .await
            .unwrap();
        assert_eq!(store.list_broadcast_articles().await.unwrap(), Vec::new());
        assert_eq!(store.get_next_pending_broadcast().await.unwrap(), None);
    }

    #[tokio::test]
    async fn multiple_new_articles_only_toast_single_latest_without_followup_spam() {
        let db = Db::new_in_memory().await.unwrap();
        let store = db.store();

        store
            .reconcile_broadcast_feed("2026-11-26T10:00:00Z", &[], &[], 1_000)
            .await
            .unwrap();

        let id_1 = "00000000-0000-4000-8000-000000000031";
        let id_2 = "00000000-0000-4000-8000-000000000032";
        let id_3 = "00000000-0000-4000-8000-000000000033";

        let batch = [
            sample_article(id_3, "post-3-newest", "2026-11-26T13:00:00Z", "published", true),
            sample_article(id_2, "post-2-middle", "2026-11-26T12:00:00Z", "published", true),
            sample_article(id_1, "post-1-older", "2026-11-26T11:00:00Z", "published", true),
        ];

        store
            .reconcile_broadcast_feed("2026-11-26T13:05:00Z", &batch, &[], 2_000)
            .await
            .unwrap();

        // Only the newest post is `'pending'` for a toast
        let pending = store
            .get_next_pending_broadcast()
            .await
            .unwrap()
            .expect("newest post should be pending");
        assert_eq!(pending.id, id_3);

        // Once the user dismisses (or reads) that single toast, NO second toast pops up for older posts
        store.mark_broadcast_dismissed(id_3, 3_000).await.unwrap();
        assert_eq!(store.get_next_pending_broadcast().await.unwrap(), None);

        // Yet all 3 unseen posts still show their unread dot inside the Boxxy News dialog
        let listed = store.list_broadcast_articles().await.unwrap();
        assert_eq!(listed.len(), 3);
        assert!(listed.iter().all(StoredBroadcastArticle::is_unread));
    }
}
