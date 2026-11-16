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

/// Walks an error source chain to detect a TCP connection-refused failure.
#[must_use]
pub fn is_connection_refused(mut source: Option<&(dyn StdError + 'static)>) -> bool {
    while let Some(err) = source {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>()
            && io_err.kind() == std::io::ErrorKind::ConnectionRefused
        {
            return true;
        }
        if let Some(req_err) = err.downcast_ref::<reqwest::Error>()
            && req_err.is_connect()
        {
            return true;
        }
        if let Some(init_err) = err.downcast_ref::<ClientInitializeError>() {
            match init_err {
                ClientInitializeError::TransportError { error, .. }
                    if is_connection_refused(Some(error.error.as_ref())) =>
                {
                    return true;
                }
                ClientInitializeError::LegacyFallbackFailed {
                    discover, fallback, ..
                } if is_connection_refused(Some(discover.as_ref()))
                    || is_connection_refused(Some(fallback.as_ref())) =>
                {
                    return true;
                }
                _ => {}
            }
        }
        source = err.source();
    }
    false
}

/// Checks an `anyhow::Error` chain for a TCP `ConnectionRefused` failure,
/// including `rmcp::transport::worker` errors that stringify transport failures when closing the channel.
#[must_use]
pub fn is_anyhow_connection_refused(err: &anyhow::Error) -> bool {
    if is_connection_refused(Some(err.as_ref())) || is_connection_refused(Some(err.root_cause())) {
        return true;
    }
    let full = format!("{err:#}");
    full.contains("Connection refused") || full.contains("ConnectionRefused")
}

/// Walks an error source chain to detect `StreamableHttpError::ReservedHeaderConflict`.
#[must_use]
pub fn has_reserved_header_conflict(mut source: Option<&(dyn StdError + 'static)>) -> bool {
    while let Some(err) = source {
        if matches!(
            err.downcast_ref::<StreamableHttpError<reqwest::Error>>(),
            Some(StreamableHttpError::ReservedHeaderConflict(_))
        ) {
            return true;
        }
        if let Some(init_err) = err.downcast_ref::<ClientInitializeError>() {
            match init_err {
                ClientInitializeError::TransportError { error, .. }
                    if has_reserved_header_conflict(Some(error.error.as_ref())) =>
                {
                    return true;
                }
                ClientInitializeError::LegacyFallbackFailed {
                    discover, fallback, ..
                } if has_reserved_header_conflict(Some(discover.as_ref()))
                    || has_reserved_header_conflict(Some(fallback.as_ref())) =>
                {
                    return true;
                }
                _ => {}
            }
        }
        source = err.source();
    }
    false
}

/// Returns `true` if `err` represents a permanent configuration, protocol-mismatch,
/// or 401/403 authorization error where immediate retries cannot succeed.
#[must_use]
pub fn is_permanent_failure(err: &anyhow::Error) -> bool {
    if err.downcast_ref::<PermanentConfigError>().is_some() {
        return true;
    }
    if err.downcast_ref::<ClientInitializeError>().is_some_and(|init| {
        init.is_authorization_required()
            || matches!(
                init,
                ClientInitializeError::NoCompatibleProtocolVersion { .. }
                    | ClientInitializeError::NoPreferredProtocolVersion
            )
    }) {
        return true;
    }
    has_reserved_header_conflict(Some(err.as_ref()))
        || has_reserved_header_conflict(Some(err.root_cause()))
}

/// Walks an error source chain to find `rmcp`'s body-free [`HttpStatusError`]
/// (emitted for HTTP 429 and 5xx responses with optional `Retry-After` metadata).
#[must_use]
pub fn find_http_status_error<'a>(
    mut source: Option<&'a (dyn StdError + 'static)>,
) -> Option<&'a HttpStatusError> {
    while let Some(err) = source {
        if let Some(http_status) = err.downcast_ref::<HttpStatusError>() {
            return Some(http_status);
        }
        if let Some(init_err) = err.downcast_ref::<ClientInitializeError>() {
            match init_err {
                ClientInitializeError::TransportError { error, .. } => {
                    if let Some(found) = find_http_status_error(Some(error.error.as_ref())) {
                        return Some(found);
                    }
                }
                ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
                    if let Some(found) = find_http_status_error(Some(fallback.as_ref())) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        source = err.source();
    }
    None
}

/// Extracts numeric `Retry-After` seconds from an error chain and clamps to `[min_ms, max_ms]`.
#[must_use]
pub fn parse_retry_after_ms(err: &anyhow::Error, min_ms: u64, max_ms: u64) -> Option<u64> {
    find_http_status_error(Some(err.as_ref()))
        .or_else(|| find_http_status_error(Some(err.root_cause())))
        .and_then(|s| s.retry_after.as_deref())
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(|secs| secs.saturating_mul(1_000).clamp(min_ms, max_ms))
}

/// Walks an error source chain to detect HTTP 401/403 authorization challenges.
#[must_use]
pub fn find_auth_error_detail(mut source: Option<&(dyn StdError + 'static)>) -> Option<String> {
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
#[must_use]
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
#[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn retry_after_parsing_clamps_to_backoff_bounds() {
        let status_err = anyhow::Error::new(HttpStatusError::new(429, Some("12".to_string())));
        assert_eq!(parse_retry_after_ms(&status_err, 1_000, 30_000), Some(12_000));
        assert_eq!(parse_retry_after_ms(&status_err, 1_000, 5_000), Some(5_000));

        let non_numeric = anyhow::Error::new(HttpStatusError::new(
            503,
            Some("Wed, 21 Oct 2026 07:28:00 GMT".to_string()),
        ));
        assert_eq!(parse_retry_after_ms(&non_numeric, 1_000, 30_000), None);
    }
}
