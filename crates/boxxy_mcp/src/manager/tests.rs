use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::future::join_all;

use super::{CachedToolList, CircuitBreakerOpenError, McpClientManager};
use crate::client::error::PermanentConfigError;
use crate::client::handler::BoxxyClientHandler;
use crate::config::{BackoffConfig, McpServerConfig, McpTransport};

fn sample_stdio_config(name: &str, cmd: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        transport: McpTransport::Stdio {
            command: cmd.to_string(),
            args: vec![],
            env: HashMap::new(),
        },
        enabled: true,
        timeout_ms: 5_000,
        max_retries: 1,
        backoff: BackoffConfig::default(),
    }
}

#[tokio::test]
async fn update_configs_filters_disabled_and_invalidates_modified() {
    let manager = McpClientManager::new();

    let cfg1 = sample_stdio_config("alpha", "echo");
    let mut cfg2 = sample_stdio_config("beta", "echo");
    cfg2.enabled = false;

    manager.update_configs(vec![cfg1.clone(), cfg2]).await;

    assert!(manager.configs.read().await.contains_key("alpha"));
    assert!(!manager.configs.read().await.contains_key("beta"));

    manager.tool_cache.write().await.insert(
        "alpha".to_string(),
        CachedToolList {
            tools: vec![],
            expires_at: None,
        },
    );

    // Updating with identical config preserves the tool cache.
    manager.update_configs(vec![cfg1]).await;
    assert!(manager.tool_cache.read().await.contains_key("alpha"));

    // Modifying transport command invalidates the cache and bumps generation.
    let gen_before = manager.current_generation("alpha");
    let modified = sample_stdio_config("alpha", "cat");
    manager.update_configs(vec![modified]).await;
    assert!(!manager.tool_cache.read().await.contains_key("alpha"));
    assert!(manager.current_generation("alpha") > gen_before);
}

#[tokio::test]
async fn tool_list_changed_notification_invalidates_manager_tool_cache_and_blocks_stale_commit() {
    use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let manager = McpClientManager::new();
    manager.tool_cache.write().await.insert(
        "live-srv".to_string(),
        CachedToolList {
            tools: vec![],
            expires_at: None,
        },
    );

    let (server_io, client_io) = tokio::io::duplex(8192);
    let (server_read, mut server_write) = tokio::io::split(server_io);
    let mut reader = BufReader::new(server_read).lines();

    let server_task = tokio::spawn(async move {
        let line = reader.next_line().await.unwrap().expect("discover line");
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        let discover_resp = json!({
            "jsonrpc": "2.0",
            "id": req["id"],
            "result": {
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": { "tools": { "listChanged": true } },
                "ttlMs": 0,
                "cacheScope": "private"
            }
        });
        server_write
            .write_all(format!("{discover_resp}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/tools/list_changed"
        });
        server_write
            .write_all(format!("{notif}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();
    });

    let stale_gen = manager.current_generation("live-srv");
    let handler = BoxxyClientHandler::new("live-srv", Arc::downgrade(&manager));
    let client = Arc::new(
        serve_client_with_lifecycle(
            handler,
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover succeeded"),
    );
    manager
        .active_clients
        .write()
        .await
        .insert("live-srv".to_string(), client.clone());

    server_task.await.unwrap();

    for _ in 0..20 {
        if !manager.tool_cache.read().await.contains_key("live-srv") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        !manager.tool_cache.read().await.contains_key("live-srv"),
        "tool_cache must be invalidated on notifications/tools/list_changed"
    );

    // A late in-flight `tools/list` started under `stale_gen` must NOT repopulate the cache.
    manager
        .commit_tools_if_current(
            "live-srv",
            &client,
            stale_gen,
            CachedToolList {
                tools: vec![],
                expires_at: None,
            },
        )
        .await;
    assert!(
        !manager.tool_cache.read().await.contains_key("live-srv"),
        "stale generation commit must be rejected"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn paginated_tools_list_honors_merged_ttl_zero_and_positive_ttl_hints() {
    use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let manager = McpClientManager::new();
    let (server_io, client_io) = tokio::io::duplex(16_384);
    let (server_read, mut server_write) = tokio::io::split(server_io);
    let mut reader = BufReader::new(server_read).lines();

    let server_task = tokio::spawn(async move {
        // 1. server/discover
        let line = reader.next_line().await.unwrap().expect("discover line");
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        let discover_resp = json!({
            "jsonrpc": "2.0",
            "id": req["id"],
            "result": {
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": { "tools": {} },
                "ttlMs": 0,
                "cacheScope": "private"
            }
        });
        server_write
            .write_all(format!("{discover_resp}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        // 2. First `get_tools` call: two pages where page 2 sets `ttlMs: 0` (merged ttl = 0 -> uncached!)
        let p1_line = reader
            .next_line()
            .await
            .unwrap()
            .expect("tools/list page 1");
        let p1_req: serde_json::Value = serde_json::from_str(&p1_line).unwrap();
        let p1_resp = json!({
            "jsonrpc": "2.0",
            "id": p1_req["id"],
            "result": {
                "resultType": "complete",
                "tools": [{ "name": "alpha", "inputSchema": { "type": "object", "properties": {} } }],
                "nextCursor": "page-2",
                "ttlMs": 60000,
                "cacheScope": "public"
            }
        });
        server_write
            .write_all(format!("{p1_resp}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        let p2_line = reader
            .next_line()
            .await
            .unwrap()
            .expect("tools/list page 2");
        let p2_req: serde_json::Value = serde_json::from_str(&p2_line).unwrap();
        assert_eq!(p2_req["params"]["cursor"], "page-2");
        let p2_resp = json!({
            "jsonrpc": "2.0",
            "id": p2_req["id"],
            "result": {
                "resultType": "complete",
                "tools": [{ "name": "beta", "inputSchema": { "type": "object", "properties": {} } }],
                "ttlMs": 0,
                "cacheScope": "private"
            }
        });
        server_write
            .write_all(format!("{p2_resp}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        // 3. Second `get_tools` call MUST hit the wire only for expired page 2 (`cursor: "page-2"`)
        // while `rmcp 3.11` reuses fresh page 1 (`[alpha]`) from `PeerResponseCache`!
        // Returning `ttlMs: 60_000` on page 2 makes the merged TTL positive so a 3rd call hits `tool_cache`.
        let refetch_line = reader
            .next_line()
            .await
            .unwrap()
            .expect("second tools/list for expired page-2");
        let refetch_req: serde_json::Value = serde_json::from_str(&refetch_line).unwrap();
        assert_eq!(refetch_req["params"]["cursor"], "page-2");
        let refetch_resp = json!({
            "jsonrpc": "2.0",
            "id": refetch_req["id"],
            "result": {
                "resultType": "complete",
                "tools": [
                    { "name": "beta", "inputSchema": { "type": "object", "properties": {} } },
                    { "name": "gamma", "inputSchema": { "type": "object", "properties": {} } }
                ],
                "ttlMs": 60000,
                "cacheScope": "private"
            }
        });
        server_write
            .write_all(format!("{refetch_resp}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();
        // Keep `server_write` alive briefly so `is_transport_closed()` stays false for the cached read.
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let handler = BoxxyClientHandler::new("paged-srv", Arc::downgrade(&manager));
    let client = Arc::new(
        serve_client_with_lifecycle(
            handler,
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover succeeded"),
    );

    manager.configs.write().await.insert(
        "paged-srv".to_string(),
        sample_stdio_config("paged-srv", "echo"),
    );
    manager
        .active_clients
        .write()
        .await
        .insert("paged-srv".to_string(), client);

    let first = manager.get_tools("paged-srv").await.expect("first fetch");
    assert_eq!(first.len(), 2);
    assert!(
        !manager.tool_cache.read().await.contains_key("paged-srv"),
        "Merged ttlMs=0 across pages must not be retained in tool_cache"
    );

    let second = manager.get_tools("paged-srv").await.expect("second fetch");
    assert_eq!(second.len(), 3);
    assert!(
        manager.tool_cache.read().await.contains_key("paged-srv"),
        "Positive ttlMs must populate tool_cache"
    );

    // Third call is served from cache with zero additional wire messages.
    let third = manager.get_tools("paged-srv").await.expect("cached fetch");
    assert_eq!(third.len(), 3);

    server_task.await.unwrap();
}

#[tokio::test]
async fn single_flight_ensure_client_coalesces_concurrent_connection_attempts() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let accepted_clone = accepted_connections.clone();

    let server_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            accepted_clone.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                tokio::time::sleep(Duration::from_millis(80)).await;
                let resp = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"mcp\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });

    let manager = McpClientManager::new();
    manager
        .update_configs(vec![McpServerConfig {
            name: "coalesced-srv".to_string(),
            transport: McpTransport::Http {
                url: format!("http://{addr}/mcp"),
                headers: HashMap::new(),
                streamable: true,
            },
            enabled: true,
            timeout_ms: 3_000,
            max_retries: 3,
            backoff: BackoffConfig::default(),
        }])
        .await;

    let results = join_all((0..6).map(|_| manager.ensure_client("coalesced-srv"))).await;
    assert!(results.iter().all(Result::is_err));
    assert_eq!(
        accepted_connections.load(Ordering::SeqCst),
        1,
        "Concurrent ensure_client calls must coalesce onto a single handshake"
    );

    server_task.abort();
}

#[tokio::test]
async fn permanent_config_error_fails_fast_trips_circuit_breaker_and_resets_on_update() {
    let manager = McpClientManager::new();
    let bad_http_cfg = McpServerConfig {
        name: "bad-scheme-srv".to_string(),
        transport: McpTransport::Http {
            url: "tcp://example.com:9000".to_string(),
            headers: HashMap::new(),
            streamable: true,
        },
        enabled: true,
        timeout_ms: 5_000,
        max_retries: 3,
        backoff: BackoffConfig {
            initial_ms: 1_000,
            max_ms: 5_000,
            multiplier: 2.0,
        },
    };

    manager.update_configs(vec![bad_http_cfg.clone()]).await;

    let start = Instant::now();
    let err1 = manager
        .ensure_client("bad-scheme-srv")
        .await
        .err()
        .expect("non-http scheme must fail");
    assert!(
        start.elapsed() < Duration::from_millis(200),
        "Permanent config error should skip retry sleeps"
    );
    assert!(err1.downcast_ref::<PermanentConfigError>().is_some());

    assert!(
        manager
            .failed_servers
            .read()
            .await
            .contains_key("bad-scheme-srv")
    );

    let err2 = manager
        .ensure_client("bad-scheme-srv")
        .await
        .err()
        .expect("circuit breaker should short-circuit");
    assert!(err2.downcast_ref::<CircuitBreakerOpenError>().is_some());

    let updated_cfg = McpServerConfig {
        name: "bad-scheme-srv".to_string(),
        transport: McpTransport::Http {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            streamable: true,
        },
        ..bad_http_cfg
    };
    manager.update_configs(vec![updated_cfg]).await;
    assert!(
        !manager
            .failed_servers
            .read()
            .await
            .contains_key("bad-scheme-srv"),
        "Updating config must clear the circuit breaker"
    );
}

#[tokio::test]
async fn offline_loopback_http_server_fails_fast_without_retry_backoff() {
    let closed_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };

    let manager = McpClientManager::new();
    let offline_local_cfg = McpServerConfig {
        name: "offline-local-srv".to_string(),
        transport: McpTransport::Http {
            url: format!("http://127.0.0.1:{closed_port}/mcp"),
            headers: HashMap::new(),
            streamable: true,
        },
        enabled: true,
        timeout_ms: 5_000,
        max_retries: 3,
        backoff: BackoffConfig {
            initial_ms: 1_000,
            max_ms: 5_000,
            multiplier: 2.0,
        },
    };

    manager.update_configs(vec![offline_local_cfg]).await;

    let start = Instant::now();
    let statuses = manager.get_server_statuses().await;
    assert_eq!(statuses, vec![("offline-local-srv".to_string(), false)]);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "Offline loopback server must skip 3,000ms retry sleeps (elapsed: {:?})",
        start.elapsed()
    );
}

#[tokio::test]
async fn resource_only_server_skips_tools_list_request_and_caches_empty() {
    use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let manager = McpClientManager::new();
    let (server_io, client_io) = tokio::io::duplex(8192);
    let (server_read, mut server_write) = tokio::io::split(server_io);
    let mut reader = BufReader::new(server_read).lines();

    let server_task = tokio::spawn(async move {
        let line = reader.next_line().await.unwrap().expect("discover line");
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        let discover_resp = json!({
            "jsonrpc": "2.0",
            "id": req["id"],
            "result": {
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": { "resources": {} },
                "ttlMs": 0,
                "cacheScope": "private"
            }
        });
        server_write
            .write_all(format!("{discover_resp}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();
        // Server deliberately handles NO `tools/list` message; if client sent one, `get_tools` would fail!
    });

    let handler = BoxxyClientHandler::new("res-only", Arc::downgrade(&manager));
    let client = Arc::new(
        serve_client_with_lifecycle(
            handler,
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover succeeded"),
    );
    server_task.await.unwrap();

    manager.configs.write().await.insert(
        "res-only".to_string(),
        sample_stdio_config("res-only", "echo"),
    );
    manager
        .active_clients
        .write()
        .await
        .insert("res-only".to_string(), client);

    let tools = manager
        .get_tools("res-only")
        .await
        .expect("empty tools vec");
    assert_eq!(tools, [] as [rmcp::model::Tool; 0]);
    assert_eq!(
        manager
            .tool_cache
            .read()
            .await
            .get("res-only")
            .map(|c| c.tools.as_slice()),
        Some(&[][..])
    );
}

#[tokio::test]
async fn mid_session_auth_challenge_on_tools_list_evicts_client_and_trips_breaker() {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let req_count = Arc::new(AtomicUsize::new(0));
    let req_count_bg = req_count.clone();

    let server_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let idx = req_count_bg.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let raw = String::from_utf8_lossy(&buf[..n]);
                let body_str = raw.split("\r\n\r\n").nth(1).unwrap_or("{}");
                let rpc: serde_json::Value = serde_json::from_str(body_str).unwrap_or_default();

                if idx == 0 {
                    let payload = json!({
                        "jsonrpc": "2.0",
                        "id": rpc["id"],
                        "result": {
                            "resultType": "complete",
                            "supportedVersions": ["2026-07-28"],
                            "capabilities": { "tools": {} },
                            "ttlMs": 0,
                            "cacheScope": "private"
                        }
                    })
                    .to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else {
                    let resp = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"mcp\", error=\"invalid_token\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            });
        }
    });

    let manager = McpClientManager::new();
    manager
        .update_configs(vec![McpServerConfig {
            name: "revoked-http".to_string(),
            transport: McpTransport::Http {
                url: format!("http://{addr}/mcp"),
                headers: HashMap::new(),
                streamable: true,
            },
            enabled: true,
            timeout_ms: 3_000,
            max_retries: 1,
            backoff: BackoffConfig::default(),
        }])
        .await;

    assert!(manager.get_tools("revoked-http").await.is_none());
    assert!(
        !manager
            .active_clients
            .read()
            .await
            .contains_key("revoked-http"),
        "Revoked session must be evicted from active_clients"
    );
    let breaker_err = manager
        .ensure_client("revoked-http")
        .await
        .err()
        .expect("circuit breaker must be open");
    assert!(
        breaker_err
            .downcast_ref::<CircuitBreakerOpenError>()
            .is_some()
    );
    assert!(breaker_err.to_string().contains("invalid_token"));

    server_task.abort();
}

#[tokio::test]
async fn numeric_float_ttl_and_unknown_server_capabilities_preserve_typed_tool_catalog() {
    use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let manager = McpClientManager::new();
    let (server_io, client_io) = tokio::io::duplex(8192);
    let (server_read, mut server_write) = tokio::io::split(server_io);
    let mut reader = BufReader::new(server_read).lines();

    let server_task = tokio::spawn(async move {
        // 1. `server/discover` carrying an unknown capability (`events`) and float `ttlMs: 5000.0`
        let line1 = reader.next_line().await.unwrap().expect("discover line");
        let req1: serde_json::Value = serde_json::from_str(&line1).unwrap();
        let discover_resp = json!({
            "jsonrpc": "2.0",
            "id": req1["id"],
            "result": {
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {
                    "tools": {},
                    "events": { "subscribe": true }
                },
                "ttlMs": 5000.0,
                "cacheScope": "private"
            }
        });
        server_write
            .write_all(format!("{discover_resp}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();

        // 2. `tools/list` returning scientific/float `ttlMs: 1.5e4` (15,000 ms)
        let line2 = reader.next_line().await.unwrap().expect("tools/list line");
        let req2: serde_json::Value = serde_json::from_str(&line2).unwrap();
        let list_resp = json!({
            "jsonrpc": "2.0",
            "id": req2["id"],
            "result": {
                "resultType": "complete",
                "tools": [{
                    "name": "ping_peer",
                    "description": "Pings a peer",
                    "inputSchema": { "type": "object" }
                }],
                "ttlMs": 1.5e4,
                "cacheScope": "private"
            }
        });
        server_write
            .write_all(format!("{list_resp}\n").as_bytes())
            .await
            .unwrap();
        server_write.flush().await.unwrap();
    });

    let handler = BoxxyClientHandler::new("ext-caps", Arc::downgrade(&manager));
    let client = Arc::new(
        serve_client_with_lifecycle(
            handler,
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover succeeded"),
    );

    assert!(
        client
            .peer_info()
            .is_some_and(|info| info.capabilities.extra.contains_key("events")),
        "Unknown server capabilities should survive round-trip in rmcp 3.12"
    );

    manager.configs.write().await.insert(
        "ext-caps".to_string(),
        sample_stdio_config("ext-caps", "echo"),
    );
    manager
        .active_clients
        .write()
        .await
        .insert("ext-caps".to_string(), client);

    let tools = manager.get_tools("ext-caps").await.expect("tools list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_ref(), "ping_peer");
    assert!(
        manager
            .tool_cache
            .read()
            .await
            .get("ext-caps")
            .and_then(|c| c.expires_at)
            .is_some(),
        "Float/scientific ttlMs should populate bounded cache expiry"
    );

    server_task.await.unwrap();
}
