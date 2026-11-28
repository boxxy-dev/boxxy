use std::error::Error as StdError;

use rmcp::service::{ClientInitializeError, ServiceError};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, HttpStatusError, InsufficientScopeError, StreamableHttpError,
};

/// Marker error indicating a permanent configuration issue (invalid URL, unsupported
/// scheme, invalid/reserved HTTP header, or missing Stdio binary) that cannot succeed on retry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PermanentConfigError(pub String);

/// Walks an error tree, descending into [`ClientInitializeError`] branches
/// (which do not expose `#[source]` directly) as well as standard `Error::source()` chains.
fn find_in_error_tree<'a, T>(
    root: &'a (dyn StdError + 'static),
    visitor: &impl Fn(&'a (dyn StdError + 'static)) -> Option<T>,
) -> Option<T> {
    let mut current = Some(root);
    while let Some(err) = current {
        if let Some(found) = visitor(err) {
            return Some(found);
        }
        if let Some(init_err) = err.downcast_ref::<ClientInitializeError>() {
            match init_err {
                ClientInitializeError::TransportError { error, .. } => {
                    if let Some(found) = find_in_error_tree(error.error.as_ref(), visitor) {
                        return Some(found);
                    }
                }
                ClientInitializeError::LegacyFallbackFailed {
                    discover, fallback, ..
                } => {
                    if let Some(found) = find_in_error_tree(fallback.as_ref(), visitor)
                        .or_else(|| find_in_error_tree(discover.as_ref(), visitor))
                    {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        current = err.source();
    }
    None
}

/// Walks an error source chain to detect a TCP connection-refused failure.
#[must_use]
pub fn is_connection_refused(source: Option<&(dyn StdError + 'static)>) -> bool {
    let Some(root) = source else {
        return false;
    };
    find_in_error_tree(root, &|err| {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>()
            && io_err.kind() == std::io::ErrorKind::ConnectionRefused
        {
            return Some(());
        }
        if let Some(req_err) = err.downcast_ref::<reqwest::Error>()
            && req_err.is_connect()
        {
            return Some(());
        }
        None
    })
    .is_some()
}

/// Checks an `anyhow::Error` chain for a TCP `ConnectionRefused` failure,
/// including `rmcp::transport::worker` errors that stringify transport failures when closing the channel.
#[must_use]
pub fn is_anyhow_connection_refused(err: &anyhow::Error) -> bool {
    if err.chain().any(|cause| is_connection_refused(Some(cause))) {
        return true;
    }
    let full = format!("{err:#}");
    full.contains("Connection refused") || full.contains("ConnectionRefused")
}

/// Walks an error source chain to detect `StreamableHttpError::ReservedHeaderConflict`.
#[must_use]
pub fn has_reserved_header_conflict(source: Option<&(dyn StdError + 'static)>) -> bool {
    let Some(root) = source else {
        return false;
    };
    find_in_error_tree(root, &|err| {
        matches!(
            err.downcast_ref::<StreamableHttpError<reqwest::Error>>(),
            Some(StreamableHttpError::ReservedHeaderConflict(_))
        )
        .then_some(())
    })
    .is_some()
}

fn is_permanent_init_error(init: &ClientInitializeError) -> bool {
    if init.is_authorization_required() {
        return true;
    }
    match init {
        ClientInitializeError::NoCompatibleProtocolVersion { .. }
        | ClientInitializeError::NoPreferredProtocolVersion => true,
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
            is_permanent_init_error(fallback)
        }
        _ => false,
    }
}

/// Returns `true` if `err` represents a permanent configuration, protocol-mismatch,
/// or 401/403 authorization error where immediate retries cannot succeed.
#[must_use]
pub fn is_permanent_failure(err: &anyhow::Error) -> bool {
    if err.downcast_ref::<PermanentConfigError>().is_some() {
        return true;
    }
    if err
        .downcast_ref::<ClientInitializeError>()
        .is_some_and(is_permanent_init_error)
    {
        return true;
    }
    if err
        .downcast_ref::<ServiceError>()
        .is_some_and(ServiceError::is_authorization_required)
    {
        return true;
    }
    err.chain()
        .any(|cause| has_reserved_header_conflict(Some(cause)))
}

/// Walks an error source chain to find `rmcp`'s body-free [`HttpStatusError`]
/// (emitted for HTTP 429 and 5xx responses with optional `Retry-After` metadata).
#[must_use]
pub fn find_http_status_error<'a>(
    source: Option<&'a (dyn StdError + 'static)>,
) -> Option<&'a HttpStatusError> {
    find_in_error_tree(source?, &|err| err.downcast_ref::<HttpStatusError>())
}

fn find_response_body_too_large(source: Option<&(dyn StdError + 'static)>) -> Option<usize> {
    find_in_error_tree(source?, &|err| {
        if let Some(StreamableHttpError::ResponseBodyTooLarge { max_size }) =
            err.downcast_ref::<StreamableHttpError<reqwest::Error>>()
        {
            Some(*max_size)
        } else {
            None
        }
    })
}

/// Extracts numeric `Retry-After` seconds from an error chain and clamps to `[min_ms, max_ms]`.
#[must_use]
pub fn parse_retry_after_ms(err: &anyhow::Error, min_ms: u64, max_ms: u64) -> Option<u64> {
    let upper = max_ms.max(min_ms);
    err.chain()
        .find_map(|cause| find_http_status_error(Some(cause)))
        .and_then(|s| s.retry_after.as_deref())
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(|secs| secs.saturating_mul(1_000).clamp(min_ms, upper))
}

/// Walks an error source chain to detect HTTP 401/403 authorization challenges,
/// preserving `InsufficientScopeError` scope hints when present.
#[must_use]
pub fn find_auth_error_detail(source: Option<&(dyn StdError + 'static)>) -> Option<String> {
    find_in_error_tree(source?, &|err| {
        if let Some(auth_req) = err.downcast_ref::<AuthRequiredError>() {
            return Some(format!(
                "HTTP 401 Unauthorized (WWW-Authenticate: {})",
                auth_req.www_authenticate_header
            ));
        }
        if let Some(scope_err) = err.downcast_ref::<InsufficientScopeError>() {
            if let Some(scope) = scope_err.get_required_scope() {
                return Some(format!(
                    "HTTP 403 Insufficient Scope (required scope: {scope}, WWW-Authenticate: {})",
                    scope_err.www_authenticate_header
                ));
            }
            return Some(format!(
                "HTTP 403 Insufficient Scope (WWW-Authenticate: {})",
                scope_err.www_authenticate_header
            ));
        }
        None
    })
}

/// Formats a [`ClientInitializeError`] with full transport, `HttpStatusError` (`Retry-After`),
/// response-size limit, and 401/403 `WWW-Authenticate` challenge details, unwrapping `LegacyFallbackFailed`.
#[must_use]
pub fn format_client_init_error(err: &ClientInitializeError) -> String {
    if let Some(auth_detail) = find_auth_error_detail(Some(err)) {
        return auth_detail;
    }
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
            if let Some(max_size) = find_response_body_too_large(Some(error.error.as_ref())) {
                return format!(
                    "MCP HTTP response body exceeded maximum size ({max_size} bytes) during {context}"
                );
            }
            err.to_string()
        }
        _ => err.to_string(),
    }
}

/// Returns `true` if `err` wraps `StreamableHttpError::SessionExpired` (e.g. HTTP 404 on a
/// session-bound POST or SSE resume GET when automatic re-initialization could not recover).
#[must_use]
pub fn is_session_expired(err: &ServiceError) -> bool {
    find_in_error_tree(err, &|cause| {
        matches!(
            cause.downcast_ref::<StreamableHttpError<reqwest::Error>>(),
            Some(StreamableHttpError::SessionExpired)
        )
        .then_some(())
    })
    .is_some()
}

/// Formats a [`ServiceError`] from an active MCP session.
///
/// Leverages `rmcp 3.12`'s `ServiceError::auth_challenge()` / `is_authorization_required()`,
/// `ListPaginationExceeded` cursor-loop guards, and `SessionExpired` detection, while enriching
/// transport errors with HTTP 429/5xx `Retry-After` or response-limit metadata.
#[must_use]
pub fn format_service_error(err: &ServiceError) -> String {
    if let Some(auth_detail) = find_auth_error_detail(Some(err)) {
        return auth_detail;
    }
    if let Some(challenge) = err.auth_challenge() {
        return format!("Authorization required by MCP server (WWW-Authenticate: {challenge})");
    }
    if err.is_authorization_required() {
        return "Authorization required by MCP server (HTTP 401/403)".to_string();
    }
    if let ServiceError::ListPaginationExceeded { max_pages } = err {
        return format!(
            "MCP list pagination exceeded safety cap ({max_pages} pages); server may have an infinite nextCursor loop"
        );
    }
    if is_session_expired(err) {
        return "MCP HTTP session expired on server (HTTP 404)".to_string();
    }
    if let ServiceError::TransportSend(dyn_err) = err {
        if let Some(status_err) = find_http_status_error(Some(dyn_err.error.as_ref())) {
            if let Some(retry_after) = &status_err.retry_after {
                return format!("HTTP {} (Retry-After: {retry_after})", status_err.status);
            }
            return format!("HTTP {}", status_err.status);
        }
        if let Some(max_size) = find_response_body_too_large(Some(dyn_err.error.as_ref())) {
            return format!("MCP HTTP response body exceeded maximum size ({max_size} bytes)");
        }
    }
    err.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::RoleClient;
    use rmcp::transport::{DynamicTransportError, StreamableHttpClientTransport};

    type ReqwestHttpTransport = StreamableHttpClientTransport<reqwest::Client>;

    #[test]
    fn generic_url_send_errors_are_not_misclassified_as_connection_refused() {
        let timeout_like = anyhow::anyhow!(
            "error sending request for url (http://127.0.0.1:8080/mcp): operation timed out"
        );
        assert!(!is_anyhow_connection_refused(&timeout_like));

        let io_refused = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "os error 111",
        ));
        assert!(is_anyhow_connection_refused(&io_refused));
    }

    #[test]
    fn retry_after_parsing_clamps_to_backoff_bounds_safely() {
        let status_err = anyhow::Error::new(HttpStatusError::new(429, Some("12".to_string())));
        assert_eq!(
            parse_retry_after_ms(&status_err, 1_000, 30_000),
            Some(12_000)
        );
        assert_eq!(parse_retry_after_ms(&status_err, 1_000, 5_000), Some(5_000));
        // Inverted bounds must never panic.
        assert_eq!(
            parse_retry_after_ms(&status_err, 10_000, 2_000),
            Some(10_000)
        );

        let non_numeric = anyhow::Error::new(HttpStatusError::new(
            503,
            Some("Wed, 21 Oct 2026 07:28:00 GMT".to_string()),
        ));
        assert_eq!(parse_retry_after_ms(&non_numeric, 1_000, 30_000), None);
    }

    #[test]
    fn service_error_auth_challenge_and_scope_are_surfaced_and_classified_permanent() {
        let auth_err = StreamableHttpError::<reqwest::Error>::AuthRequired(AuthRequiredError::new(
            "Bearer realm=\"mcp\", error=\"invalid_token\"".to_string(),
        ));
        let svc_unauth = ServiceError::TransportSend(DynamicTransportError::new::<
            ReqwestHttpTransport,
            RoleClient,
        >(auth_err));
        assert_eq!(
            svc_unauth.auth_challenge(),
            Some("Bearer realm=\"mcp\", error=\"invalid_token\"")
        );
        assert!(svc_unauth.is_authorization_required());
        let formatted_401 = format_service_error(&svc_unauth);
        assert!(formatted_401.contains("HTTP 401 Unauthorized"));
        assert!(formatted_401.contains("invalid_token"));
        assert!(is_permanent_failure(&anyhow::Error::new(svc_unauth)));

        let scope_err =
            StreamableHttpError::<reqwest::Error>::InsufficientScope(InsufficientScopeError::new(
                "Bearer scope=\"mcp:tools\"".to_string(),
                Some("mcp:tools".to_string()),
            ));
        let svc_forbidden = ServiceError::TransportSend(DynamicTransportError::new::<
            ReqwestHttpTransport,
            RoleClient,
        >(scope_err));
        assert_eq!(
            svc_forbidden.auth_challenge(),
            Some("Bearer scope=\"mcp:tools\"")
        );
        let formatted_403 = format_service_error(&svc_forbidden);
        assert!(formatted_403.contains("HTTP 403 Insufficient Scope"));
        assert!(formatted_403.contains("required scope: mcp:tools"));
    }

    #[test]
    fn response_body_too_large_formats_cleanly() {
        let oversize = StreamableHttpError::<reqwest::Error>::ResponseBodyTooLarge {
            max_size: 16 * 1024 * 1024,
        };
        let svc_err = ServiceError::TransportSend(DynamicTransportError::new::<
            ReqwestHttpTransport,
            RoleClient,
        >(oversize));
        assert_eq!(
            format_service_error(&svc_err),
            "MCP HTTP response body exceeded maximum size (16777216 bytes)"
        );
    }

    #[test]
    fn session_expired_and_pagination_cap_errors_are_detected_and_formatted() {
        let expired_http = StreamableHttpError::<reqwest::Error>::SessionExpired;
        let svc_expired = ServiceError::TransportSend(DynamicTransportError::new::<
            ReqwestHttpTransport,
            RoleClient,
        >(expired_http));
        assert!(is_session_expired(&svc_expired));
        assert!(!is_permanent_failure(&anyhow::Error::new(
            StreamableHttpError::<reqwest::Error>::SessionExpired
        )));
        assert_eq!(
            format_service_error(&svc_expired),
            "MCP HTTP session expired on server (HTTP 404)"
        );

        let pagination_err = ServiceError::ListPaginationExceeded { max_pages: 64 };
        assert!(!is_session_expired(&pagination_err));
        assert!(
            format_service_error(&pagination_err)
                .contains("exceeded safety cap (64 pages)")
        );
    }
}
