use std::collections::HashMap;
use std::hash::BuildHasher;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};
use rmcp::RoleClient;
use rmcp::model::ProtocolVersion;
use rmcp::service::{
    ClientCacheConfig, ClientLifecycleMode, RunningService, serve_client_with_lifecycle,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};

pub use super::error::{
    PermanentConfigError, find_auth_error_detail, find_http_status_error, format_client_init_error,
    format_service_error, is_anyhow_connection_refused, is_connection_refused,
};
use super::handler::BoxxyClientHandler;
use crate::config::MIN_TIMEOUT_MS;

/// Maximum number of same-origin 307/308 redirects followed per request.
const MAX_REDIRECTS: usize = 10;

/// Redirect policy matching `rmcp 3.11.0`'s Streamable HTTP semantics:
/// follows only same-origin 307/308 redirects (up to 10 hops) so JSON-RPC POST
/// bodies are preserved and custom headers / session IDs are never leaked to
/// another origin or rewritten into bodyless GET requests (301/302/303).
/// Each followed hop is logged at `debug` level (paths only, never query strings).
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
            log::debug!(
                "MCP HTTP following same-origin redirect ({}): {} -> {}",
                attempt.status(),
                attempt.previous().last().map_or("", reqwest::Url::path),
                attempt.url().path(),
            );
            attempt.follow()
        }
    })
}

fn is_localhost_domain(host: &str) -> bool {
    let bare = host.strip_suffix('.').unwrap_or(host);
    if bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    bare.len() > ".localhost".len()
        && bare.as_bytes()[bare.len() - ".localhost".len()..].eq_ignore_ascii_case(b".localhost")
}

/// Returns `true` if `url` points to a loopback host (`localhost`, `*.localhost`, `127.0.0.0/8`, `[::1]`).
#[must_use]
pub fn is_loopback_http_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    let Some(host_str) = parsed.host_str() else {
        return false;
    };
    if is_localhost_domain(host_str) {
        return true;
    }
    let trimmed = host_str.trim_start_matches('[').trim_end_matches(']');
    trimmed.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Performs a sub-millisecond non-blocking TCP probe to a loopback HTTP URL's port
/// so the health monitor can detect when a local MCP server starts or stops without
/// generating `rmcp` worker log noise.
pub async fn probe_loopback_http_port(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    let Some(host_str) = parsed.host_str() else {
        return false;
    };
    let port = parsed.port_or_known_default().unwrap_or(80);
    let trimmed_host = host_str.trim_start_matches('[').trim_end_matches(']');

    if is_localhost_domain(trimmed_host) {
        let v4 = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let v6 = std::net::SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port));
        tokio::time::timeout(
            Duration::from_millis(150),
            tokio::net::TcpStream::connect(&[v4, v6][..]),
        )
        .await
        .is_ok_and(|r| r.is_ok())
    } else if let Ok(ip) = trimmed_host.parse::<IpAddr>() {
        let addr = std::net::SocketAddr::new(ip, port);
        tokio::time::timeout(
            Duration::from_millis(150),
            tokio::net::TcpStream::connect(addr),
        )
        .await
        .is_ok_and(|r| r.is_ok())
    } else {
        false
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AuthorizationDisposition {
    /// Empty header value — omit completely.
    Skip,
    /// Bearer token (without `"Bearer "` prefix) routed to `StreamableHttpClientTransportConfig::auth_header`.
    BearerToken(String),
    /// Non-Bearer scheme (`Basic ...`, `ApiKey ...`, `Token ...`) preserved verbatim in `custom_headers`.
    CustomHeader(HeaderValue),
}

/// Parses an `Authorization` header value safely without UTF-8 byte-slice panics
/// and without corrupting non-Bearer authentication schemes (`Basic`, `ApiKey`, `Token`).
fn parse_authorization_header(
    raw_value: &str,
) -> Result<AuthorizationDisposition, PermanentConfigError> {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("bearer") {
        return Ok(AuthorizationDisposition::Skip);
    }

    if let Some((scheme, rest)) = trimmed.split_once(char::is_whitespace) {
        if scheme.eq_ignore_ascii_case("bearer") {
            let token = rest.trim();
            if token.is_empty() {
                return Ok(AuthorizationDisposition::Skip);
            }
            HeaderValue::from_str(token).map_err(|e| {
                PermanentConfigError(format!("Invalid HTTP Authorization bearer token: {e}"))
            })?;
            return Ok(AuthorizationDisposition::BearerToken(token.to_string()));
        }

        // Preserve non-Bearer schemes (`Basic ...`, `ApiKey ...`, `Token ...`) verbatim.
        let value = HeaderValue::from_str(trimmed).map_err(|e| {
            PermanentConfigError(format!("Invalid HTTP Authorization header value: {e}"))
        })?;
        return Ok(AuthorizationDisposition::CustomHeader(value));
    }

    // Bare single-token value (user supplied raw token without "Bearer " prefix):
    // validate it can form a legal HTTP header value before passing to `rmcp`.
    HeaderValue::from_str(trimmed).map_err(|e| {
        PermanentConfigError(format!("Invalid HTTP Authorization token value: {e}"))
    })?;
    Ok(AuthorizationDisposition::BearerToken(trimmed.to_string()))
}

/// Connects to a Streamable HTTP MCP server, probing MCP `2026-07-28` (`server/discover`)
/// first and automatically falling back to `2025-11-25` (`initialize`).
///
/// # Errors
///
/// Returns an error if the URL/scheme or HTTP headers are invalid (`PermanentConfigError`),
/// the HTTP client cannot be constructed, or the MCP handshake fails or times out.
pub async fn build_streamable_http_client<S: BuildHasher + Sync>(
    handler: BoxxyClientHandler,
    url: &str,
    headers: &HashMap<String, String, S>,
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
        let key_trimmed = k.trim();
        if key_trimmed.is_empty() {
            continue;
        }

        if key_trimmed.eq_ignore_ascii_case("authorization") {
            match parse_authorization_header(v).map_err(anyhow::Error::new)? {
                AuthorizationDisposition::Skip => {}
                AuthorizationDisposition::BearerToken(token) => {
                    auth_header = Some(token);
                }
                AuthorizationDisposition::CustomHeader(header_val) => {
                    parsed_headers.insert(AUTHORIZATION, header_val);
                }
            }
            continue;
        }

        let name = HeaderName::from_bytes(key_trimmed.as_bytes()).map_err(|e| {
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

    let request_timeout = Duration::from_millis(timeout_ms.max(MIN_TIMEOUT_MS));

    let mut config = StreamableHttpClientTransportConfig::with_uri(trimmed_url)
        .custom_headers(parsed_headers)
        .reinit_on_expired_session(true);

    if let Some(token) = auth_header
        && !token.is_empty()
    {
        config = config.auth_header(token);
    }

    let is_loopback = is_loopback_http_url(trimmed_url);
    // Deliberately omit a client-wide `.timeout(request_timeout)` here: in `reqwest`,
    // `.timeout()` caps total body stream lifetime and would prematurely sever stateful
    // `2025-11-25` long-lived SSE GET notification streams every `timeout_ms`. Individual
    // handshakes, `tools/list`, and `tools/call` enforce `tokio::time::timeout` directly
    // (dropping `rmcp`'s `AbandonedRequestGuard` to cancel the exact in-flight POST).
    let mut client_builder = reqwest::Client::builder()
        .redirect(same_origin_redirect_policy())
        .connect_timeout(request_timeout.min(Duration::from_secs(10)))
        .tcp_nodelay(true);

    if is_loopback {
        // Avoid retaining idle sockets against local dev servers that may restart frequently.
        client_builder = client_builder.pool_max_idle_per_host(0);
    } else {
        // Reuse warm TLS 1.3 / HTTP/2 sessions across repeated tool calls on remote servers.
        client_builder = client_builder
            .pool_idle_timeout(Duration::from_secs(60))
            .tcp_keepalive(Duration::from_secs(30));
    }

    let http_client = client_builder
        .build()
        .context("Failed to build reqwest client for MCP HTTP transport")?;

    let transport = StreamableHttpClientTransport::with_client(http_client, config);

    // Probe MCP 2026-07-28 `server/discover` first and transparently fall back to
    // the legacy `initialize` handshake (`2025-11-25`) when talking to older servers.
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
            "Timed out after {}ms connecting to HTTP MCP server at '{trimmed_url}'",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_authorization_header_handles_bearer_basic_and_multibyte_utf8_safely() {
        assert_eq!(
            parse_authorization_header("Bearer sk-test-123").unwrap(),
            AuthorizationDisposition::BearerToken("sk-test-123".to_string())
        );
        assert_eq!(
            parse_authorization_header("bearer   tok_abc  ").unwrap(),
            AuthorizationDisposition::BearerToken("tok_abc".to_string())
        );
        assert_eq!(
            parse_authorization_header("raw_token_without_scheme").unwrap(),
            AuthorizationDisposition::BearerToken("raw_token_without_scheme".to_string())
        );
        assert_eq!(
            parse_authorization_header("Basic dXNlcjpwYXNz").unwrap(),
            AuthorizationDisposition::CustomHeader(HeaderValue::from_static("Basic dXNlcjpwYXNz"))
        );
        assert_eq!(
            parse_authorization_header("ApiKey secret-key-42").unwrap(),
            AuthorizationDisposition::CustomHeader(HeaderValue::from_static(
                "ApiKey secret-key-42"
            ))
        );
        assert_eq!(
            parse_authorization_header("Bearer   ").unwrap(),
            AuthorizationDisposition::Skip
        );

        // Multi-byte UTF-8 crossing byte index 7 ("éééé..." has 2-byte chars crossing index 7)
        // must never panic, and control characters must return `PermanentConfigError`.
        let multibyte = "éééé-token";
        assert_eq!(
            parse_authorization_header(multibyte).unwrap(),
            AuthorizationDisposition::BearerToken(multibyte.to_string())
        );
        assert!(parse_authorization_header("Bearer bad\ntoken").is_err());
    }

    #[test]
    fn loopback_url_detection_covers_ipv4_ipv6_and_rfc6761_localhost_subdomains() {
        assert!(is_loopback_http_url("http://localhost:3000/mcp"));
        assert!(is_loopback_http_url("http://mcp.LOCALHOST:3000/mcp"));
        assert!(is_loopback_http_url("http://127.0.0.1:8080/mcp"));
        assert!(is_loopback_http_url("http://[::1]:9000/mcp"));
        assert!(!is_loopback_http_url("https://notlocalhost.com/mcp"));
        assert!(!is_loopback_http_url("https://example.com/mcp"));
        assert!(!is_loopback_http_url("not-a-url"));
    }
}
