use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::ProtocolVersion;
use rmcp::service::{
    ClientCacheConfig, ClientLifecycleMode, RunningService, serve_client_with_lifecycle,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::RoleClient;

use super::handler::BoxxyClientHandler;

pub async fn build_streamable_http_client(
    handler: BoxxyClientHandler,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_ms: u64,
) -> Result<RunningService<RoleClient, BoxxyClientHandler>> {
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

        let name = HeaderName::from_bytes(k.trim().as_bytes())
            .with_context(|| format!("Invalid HTTP header name: {k}"))?;
        let value = HeaderValue::from_str(v.trim())
            .with_context(|| format!("Invalid HTTP header value for '{k}'"))?;
        parsed_headers.insert(name, value);
    }

    let request_timeout = Duration::from_millis(timeout_ms.max(1_000));

    let mut config = StreamableHttpClientTransportConfig::with_uri(url)
        .custom_headers(parsed_headers)
        .reinit_on_expired_session(true);

    if let Some(token) = auth_header {
        if !token.is_empty() {
            config = config.auth_header(token);
        }
    }

    let http_client = reqwest::Client::builder()
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
    .map_err(|e| anyhow::anyhow!("Failed to initialize Streamable HTTP client: {e}"))?;

    client
        .peer()
        .set_response_cache_config(ClientCacheConfig::default().with_serve_stale_on_error(false))
        .await;

    Ok(client)
}
