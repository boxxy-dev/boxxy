use boxxy_db::models::BroadcastArticleUpsert;
use chrono::DateTime;
use parking_lot::RwLock;
use serde::Deserialize;
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

pub const DEFAULT_BROADCAST_ENDPOINT: &str =
    "https://qfwhesnmixgmczkdnvsu.supabase.co/rest/v1/rpc/get_broadcast_feed";
const SUPABASE_PUBLISHABLE_KEY: &str = "sb_publishable_Z-SbIil88PSj3ri24CM7aw_8vzsCFie";
const USER_AGENT: &str = concat!("boxxy-terminal/", env!("CARGO_PKG_VERSION"));

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_FEED_ITEMS: usize = 4;
const MAX_WITHDRAWN_IDS: usize = 200;
const MAX_SLUG_CHARS: usize = 100;
const MAX_TITLE_CHARS: usize = 140;
const MAX_SUMMARY_CHARS: usize = 300;
const MAX_ACTION_LABEL_CHARS: usize = 40;
const MAX_BODY_BYTES: usize = 65_536;

static HTTP_CLIENT: RwLock<Option<reqwest::Client>> = RwLock::new(None);

#[derive(Debug, Error)]
pub enum BroadcastClientError {
    #[error("failed to initialize HTTP client: {0}")]
    HttpClientInit(String),
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("unexpected HTTP status {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("broadcast response exceeded {MAX_RESPONSE_BYTES} byte limit")]
    ResponseTooLarge,
    #[error("invalid JSON feed payload: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("unsupported broadcast schema_version {0}")]
    UnsupportedSchemaVersion(i64),
    #[error("invalid server_time timestamp '{0}'")]
    InvalidServerTime(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedBroadcastFeed {
    pub server_time: String,
    pub items: Vec<BroadcastArticleUpsert>,
    pub withdrawn_ids: Vec<String>,
}

#[derive(Deserialize)]
struct RawFeedPayload {
    schema_version: i64,
    server_time: String,
    #[serde(default)]
    items: Vec<RawFeedItem>,
    #[serde(default)]
    withdrawn_ids: Vec<String>,
}

#[derive(Deserialize)]
struct RawFeedItem {
    id: String,
    slug: String,
    title: String,
    summary: String,
    body_md: String,
    #[serde(default)]
    category: String,
    status: String,
    #[serde(default = "default_notify")]
    notify: bool,
    action_label: Option<String>,
    action_url: Option<String>,
    published_at: String,
    updated_at: Option<String>,
    #[serde(default = "default_revision")]
    revision: i64,
}

const fn default_notify() -> bool {
    true
}

const fn default_revision() -> i64 {
    1
}

fn ensure_http_client() -> Result<reqwest::Client, BroadcastClientError> {
    let cached = HTTP_CLIENT.read().clone();
    if let Some(existing) = cached {
        return Ok(existing);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| BroadcastClientError::HttpClientInit(e.to_string()))?;
    *HTTP_CLIENT.write() = Some(client.clone());
    Ok(client)
}

#[must_use]
pub fn resolve_endpoint() -> String {
    boxxy_sys_utils::env::broadcast_endpoint()
        .unwrap_or_else(|| DEFAULT_BROADCAST_ENDPOINT.to_string())
}

fn canonical_uuid(raw: &str) -> Option<String> {
    Uuid::parse_str(raw.trim()).ok().map(|u| u.to_string())
}

fn is_valid_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > MAX_SLUG_CHARS {
        return false;
    }
    if slug.starts_with('-') || slug.ends_with('-') || slug.contains("--") {
        return false;
    }
    slug.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn sanitize_single_line(raw: &str, max_chars: usize) -> Option<String> {
    let mut cleaned = String::with_capacity(raw.len().min(max_chars));
    for ch in raw.trim().chars() {
        if ch.is_control() {
            if matches!(ch, '\n' | '\r' | '\t') {
                cleaned.push(' ');
            }
            continue;
        }
        if cleaned.chars().count() >= max_chars {
            break;
        }
        cleaned.push(ch);
    }
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_category(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "feature" => "feature",
        "community" => "community",
        "maintenance" => "maintenance",
        "security" => "security",
        _ => "news",
    }
}

fn validate_https_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() <= "https://".len() || trimmed.len() > 2048 {
        return None;
    }
    if !trimmed.starts_with("https://") {
        return None;
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    Some(trimmed.to_string())
}

fn sanitize_item(raw: &RawFeedItem) -> Option<BroadcastArticleUpsert> {
    let id = canonical_uuid(&raw.id)?;
    let slug = raw.slug.trim().to_ascii_lowercase();
    if !is_valid_slug(&slug) {
        log::warn!("Broadcast: dropping item '{id}' with invalid slug '{slug}'");
        return None;
    }

    let status = match raw.status.trim() {
        "published" => "published".to_string(),
        "archived" => "archived".to_string(),
        _ => return None,
    };

    let title = sanitize_single_line(&raw.title, MAX_TITLE_CHARS)?;
    let summary = sanitize_single_line(&raw.summary, MAX_SUMMARY_CHARS)?;

    let body_trimmed = raw.body_md.trim();
    if body_trimmed.is_empty() || body_trimmed.len() > MAX_BODY_BYTES {
        log::warn!("Broadcast: dropping item '{id}' with empty or oversized body_md");
        return None;
    }

    DateTime::parse_from_rfc3339(raw.published_at.trim()).ok()?;
    let published_at = raw.published_at.trim().to_string();
    let updated_at = raw
        .updated_at
        .as_deref()
        .map(str::trim)
        .filter(|s| DateTime::parse_from_rfc3339(s).is_ok())
        .map_or_else(|| published_at.clone(), ToOwned::to_owned);

    let (action_label, action_url) = match (raw.action_label.as_deref(), raw.action_url.as_deref())
    {
        (Some(lbl), Some(url)) => {
            if let (Some(valid_lbl), Some(valid_url)) = (
                sanitize_single_line(lbl, MAX_ACTION_LABEL_CHARS),
                validate_https_url(url),
            ) {
                (Some(valid_lbl), Some(valid_url))
            } else {
                (None, None)
            }
        }
        _ => (None, None),
    };

    Some(BroadcastArticleUpsert {
        id,
        slug,
        title,
        summary,
        body_md: body_trimmed.to_string(),
        category: normalize_category(&raw.category).to_string(),
        status,
        notify: raw.notify,
        action_label,
        action_url,
        published_at,
        updated_at,
        revision: raw.revision.max(1),
    })
}

/// Parses and validates a raw JSON byte slice returned by `public.get_broadcast_feed`.
pub fn parse_and_validate_feed_bytes(
    bytes: &[u8],
) -> Result<ValidatedBroadcastFeed, BroadcastClientError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(BroadcastClientError::ResponseTooLarge);
    }

    let raw: RawFeedPayload = serde_json::from_slice(bytes)?;
    if raw.schema_version != 1 {
        return Err(BroadcastClientError::UnsupportedSchemaVersion(
            raw.schema_version,
        ));
    }

    let server_time = raw.server_time.trim().to_string();
    if DateTime::parse_from_rfc3339(&server_time).is_err() {
        return Err(BroadcastClientError::InvalidServerTime(server_time));
    }

    let items = raw
        .items
        .iter()
        .take(50)
        .filter_map(sanitize_item)
        .take(MAX_FEED_ITEMS)
        .collect();

    let withdrawn_ids = raw
        .withdrawn_ids
        .into_iter()
        .take(MAX_WITHDRAWN_IDS)
        .filter_map(|id| canonical_uuid(&id))
        .collect();

    Ok(ValidatedBroadcastFeed {
        server_time,
        items,
        withdrawn_ids,
    })
}

/// Fetches and validates the latest Boxxy News feed from the configured RPC endpoint.
pub async fn fetch_broadcast_feed(
    endpoint: &str,
) -> Result<ValidatedBroadcastFeed, BroadcastClientError> {
    let client = ensure_http_client()?;
    let mut resp = client
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("apikey", SUPABASE_PUBLISHABLE_KEY)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {SUPABASE_PUBLISHABLE_KEY}"),
        )
        .body(r#"{"p_limit":4}"#)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(BroadcastClientError::HttpStatus(resp.status()));
    }

    if let Some(len) = resp.content_length()
        && len > MAX_RESPONSE_BYTES as u64
    {
        return Err(BroadcastClientError::ResponseTooLarge);
    }

    let mut body = Vec::with_capacity(8 * 1024);
    while let Some(chunk) = resp.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(BroadcastClientError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }

    parse_and_validate_feed_bytes(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_feed_and_strips_unsafe_action_urls_or_malformed_items() {
        let payload = br###"{
            "schema_version": 1,
            "server_time": "2026-11-26T15:00:00Z",
            "items": [
                {
                    "id": "11111111-1111-4111-8111-111111111111",
                    "slug": "valid-announcement",
                    "title": "  Boxxy News Launch  ",
                    "summary": "Short summary\nwith newline normalized",
                    "body_md": "## Welcome\n\nBody text.",
                    "category": "FEATURE",
                    "status": "published",
                    "notify": true,
                    "action_label": "Insecure Link",
                    "action_url": "http://example.com/not-https",
                    "published_at": "2026-11-26T14:00:00Z",
                    "updated_at": "2026-11-26T14:00:00Z",
                    "revision": 2
                },
                {
                    "id": "not-a-uuid",
                    "slug": "bad-item",
                    "title": "Bad",
                    "summary": "Bad",
                    "body_md": "Bad",
                    "category": "news",
                    "status": "published",
                    "published_at": "2026-11-26T14:00:00Z"
                }
            ],
            "withdrawn_ids": [
                "22222222-2222-4222-8222-222222222222",
                "invalid-uuid"
            ]
        }"###;

        let parsed = parse_and_validate_feed_bytes(payload).expect("valid feed");
        assert_eq!(parsed.items.len(), 1);
        let item = &parsed.items[0];
        assert_eq!(item.title, "Boxxy News Launch");
        assert_eq!(item.summary, "Short summary with newline normalized");
        assert_eq!(item.category, "feature");
        // Non-HTTPS action_url must be stripped cleanly
        assert_eq!(item.action_label, None);
        assert_eq!(item.action_url, None);
        assert_eq!(item.revision, 2);
        assert_eq!(
            parsed.withdrawn_ids,
            vec!["22222222-2222-4222-8222-222222222222".to_string()]
        );
    }

    #[test]
    fn rejects_unsupported_schema_version() {
        let payload = br#"{"schema_version": 2, "server_time": "2026-11-26T15:00:00Z"}"#;
        assert!(matches!(
            parse_and_validate_feed_bytes(payload),
            Err(BroadcastClientError::UnsupportedSchemaVersion(2))
        ));
    }

    #[test]
    fn caps_parsed_feed_at_latest_4_items() {
        let mut raw_items = Vec::new();
        for idx in 1..=6 {
            raw_items.push(format!(
                r#"{{
                    "id": "00000000-0000-4000-8000-00000000000{idx}",
                    "slug": "item-{idx}",
                    "title": "Item {idx}",
                    "summary": "Summary {idx}",
                    "body_md": "Body {idx}",
                    "category": "news",
                    "status": "published",
                    "published_at": "2026-11-2{idx}T12:00:00Z"
                }}"#
            ));
        }
        let payload = format!(
            r#"{{"schema_version": 1, "server_time": "2026-11-26T15:00:00Z", "items": [{}]}}"#,
            raw_items.join(",")
        );
        let parsed = parse_and_validate_feed_bytes(payload.as_bytes()).expect("valid feed");
        assert_eq!(parsed.items.len(), 4);
        assert_eq!(parsed.items[0].slug, "item-1");
        assert_eq!(parsed.items[3].slug, "item-4");
    }
}
