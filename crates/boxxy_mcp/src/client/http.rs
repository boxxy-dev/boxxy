use std::collections::HashMap;
use std::error::Error as StdError;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::RoleClient;
use rmcp::model::ProtocolVersion;
use rmcp::service::{
    ClientCacheConfig, ClientInitializeError, ClientLifecycleMode, RunningService, ServiceError,
    serve_client_with_lifecycle,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, HttpStatusError, InsufficientScopeError, StreamableHttpClientTransport,
    StreamableHttpClientTransportConfig,
};

use super::handler::BoxxyClientHandler;

/// Maximum number of same-origin 307/308 redirects followed per request.
const MAX_REDIRECTS: usize = 10;

/// Redirect policy matching `rmcp 3.8.0`'s default Streamable HTTP client:
/// follows only same-origin 307/308 redirects (up to 10 hops) so JSON-RPC POST
/// bodies are preserved and custom headers / session IDs are never leaked to
/// another origin or rewritten into bodyless GET requests (301/302/303).
pub(crate) fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let preserves_body = matches!(
            attempt.status(),
            reqwest::StatusCode::TEMPORARY_REDIRECT | reqwest::StatusCode::PERMANENT_REDIRECT
        );
        let same_origin = attempt
            .previous()
            .first()
            .is_some_and(|original| original.origin() == attempt.url().origin());
        if !preserves_body || !same_origin {
            attempt.stop()
        } else if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error(format!("exceeded {MAX_REDIRECTS} redirects"))
        } else {
            attempt.follow()
        }
    })
}

/// Walks an error source chain to find `rmcp 3.8.0`'s body-free [`HttpStatusError`]
/// (emitted for HTTP 429 and 5xx responses with optional `Retry-After` metadata).
pub fn find_http_status_error<'a>(
    mut source: Option<&'a (dyn StdError + 'static)>,
) -> Option<&'a HttpStatusError> {
    while let Some(err) = source {
        if let Some(http_status) = err.downcast_ref::<HttpStatusError>() {
            return Some(http_status);
        }
        source = err.source();
    }
    None
}

/// Walks an error source chain to detect HTTP 401/403 authorization challenges.
pub fn find_auth_error_detail<'a>(
    mut source: Option<&'a (dyn StdError + 'static)>,
) -> Option<String> {
    while let Some(err) = source {
        if let Some(auth_req) = err.downcast_ref::<AuthRequiredError>() {
            return Some(format!(
                "HTTP 401 Unauthorized (WWW-Authenticate: {})",
                auth_req.www_authenticate_header
            ));
        }
        if let Some(scope_err) = err.downcast_ref::<InsufficientScopeError>() {
            return Some(format!(
                "HTTP 403 Insufficient Scope (WWW-Authenticate: {})",
                scope_err.www_authenticate_header
            ));
        }
        source = err.source();
    }
    None
}

/// Formats a [`ClientInitializeError`] with full transport, `HttpStatusError` (`Retry-After`),
/// and 401/403 `WWW-Authenticate` challenge details, unwrapping `LegacyFallbackFailed`.
pub fn format_client_init_error(err: &ClientInitializeError) -> String {
    if let Some(challenge) = err.auth_challenge() {
        return format!("Authorization required by MCP server (WWW-Authenticate: {challenge})");
    }
    if err.is_authorization_required() {
        return "Authorization required by MCP server (HTTP 401/403)".to_string();
    }
    match err {
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
            format_client_init_error(fallback)
        }
        ClientInitializeError::TransportError { error, context } => {
            if let Some(status_err) = find_http_status_error(Some(error.error.as_ref())) {
                if let Some(retry_after) = &status_err.retry_after {
                    return format!(
                        "HTTP {} during {context} (Retry-After: {retry_after})",
                        status_err.status
                    );
                }
                return format!("HTTP {} during {context}", status_err.status);
            }
            if let Some(auth_detail) = find_auth_error_detail(Some(error.error.as_ref())) {
                return format!("{auth_detail} during {context}");
            }
            err.to_string()
        }
        _ => err.to_string(),
    }
}

/// Formats a [`ServiceError`] from an active MCP session, enriching transport errors
/// with HTTP 429/5xx `Retry-After` or 401/403 challenge metadata when present.
pub fn format_service_error(err: &ServiceError) -> String {
    if let ServiceError::TransportSend(dyn_err) = err {
        if let Some(status_err) = find_http_status_error(Some(dyn_err.error.as_ref())) {
            if let Some(retry_after) = &status_err.retry_after {
                return format!(
                    "HTTP {} (Retry-After: {retry_after})",
                    status_err.status
                );
            }
            return format!("HTTP {}", status_err.status);
        }
        if let Some(auth_detail) = find_auth_error_detail(Some(dyn_err.error.as_ref())) {
            return auth_detail;
        }
    }
    err.to_string()
}

/// Marker error indicating a permanent configuration issue (invalid URL, unsupported
/// scheme, invalid HTTP header, or missing Stdio binary) that cannot succeed on retry.
#[derive(Debug)]
pub struct PermanentConfigError(pub String);

impl std::fmt::Display for PermanentConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl StdError for PermanentConfigError {}

pub async fn build_streamable_http_client(
    handler: BoxxyClientHandler,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_ms: u64,
) -> Result<RunningService<RoleClient, BoxxyClientHandler>> {
    let trimmed_url = url.trim();
    let parsed_url = reqwest::Url::parse(trimmed_url).map_err(|e| {
        anyhow::Error::new(PermanentConfigError(format!(
            "Invalid HTTP MCP server URL '{trimmed_url}': {e}"
        )))
    })?;
    if !matches!(parsed_url.scheme(), "http" | "https") {
        return Err(anyhow::Error::new(PermanentConfigError(format!(
            "Unsupported URL scheme '{}' for HTTP MCP server '{trimmed_url}' (expected http:// or https://)",
            parsed_url.scheme()
        ))));
    }

    let mut parsed_headers = HashMap::new();
    let mut auth_header = None;

    for (k, v) in headers {
        if k.trim().is_empty() {
            continue;
        }

        // rmcp's StreamableHttpClientTransportConfig has a dedicated `auth_header` method
        // which expects the raw bearer token (without the "Bearer " prefix).
        if k.eq_ignore_ascii_case("authorization") {
            let trimmed = v.trim();
            if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("bearer ") {
                auth_header = Some(trimmed[7..].trim().to_string());
            } else {
                auth_header = Some(trimmed.to_string());
            }
            continue;
        }

        let name = HeaderName::from_bytes(k.trim().as_bytes()).map_err(|e| {
            anyhow::Error::new(PermanentConfigError(format!(
                "Invalid HTTP header name '{k}': {e}"
            )))
        })?;
        let value = HeaderValue::from_str(v.trim()).map_err(|e| {
            anyhow::Error::new(PermanentConfigError(format!(
                "Invalid HTTP header value for '{k}': {e}"
            )))
        })?;
        parsed_headers.insert(name, value);
    }

    let request_timeout = Duration::from_millis(timeout_ms.max(1_000));

    let mut config = StreamableHttpClientTransportConfig::with_uri(trimmed_url)
        .custom_headers(parsed_headers)
        .reinit_on_expired_session(true);

    if let Some(token) = auth_header {
        if !token.is_empty() {
            config = config.auth_header(token);
        }
    }

    let http_client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .redirect(same_origin_redirect_policy())
        .connect_timeout(Duration::from_secs(10))
        .timeout(request_timeout)
        .build()
        .context("Failed to build reqwest client for MCP HTTP transport")?;

    let transport = StreamableHttpClientTransport::with_client(http_client, config);

    // Probe MCP 2026-07-28 `server/discover` first and transparently fall back to
    // the legacy `initialize` handshake (2025-11-25) when talking to older servers.
    let lifecycle = ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::LATEST],
        legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
    };

    let client = tokio::time::timeout(
        request_timeout,
        serve_client_with_lifecycle(handler, transport, lifecycle),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out after {}ms connecting to HTTP MCP server at '{url}'",
            request_timeout.as_millis()
        )
    })?
    .map_err(|e| {
        let detail = format_client_init_error(&e);
        anyhow::Error::new(e).context(format!(
            "Failed to initialize Streamable HTTP client: {detail}"
        ))
    })?;

    client
        .peer()
        .set_response_cache_config(ClientCacheConfig::default().with_serve_stale_on_error(false))
        .await;

    Ok(client)
}
