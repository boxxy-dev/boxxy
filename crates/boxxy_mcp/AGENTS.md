# Boxxy-MCP Architecture & Guidelines

## Component Overview
The `boxxy-mcp` crate provides a generic Model Context Protocol (MCP) client integration for Boxxy-Terminal built on `rmcp 3.5.1` (supporting the MCP `2026-07-28` specification as well as `2025-11-25` and earlier servers). It bridges standard MCP servers (JSON-RPC + JSON Schema Draft 7 / 2020-12) and `rig-core` (the agentic reasoning framework used by Boxxy).

### Design Philosophy
- **Strict Modularity:** Boxxy's core UI (`boxxy-preferences`) and agent reasoning (`boxxy-claw`) remain completely decoupled from the underlying MCP SDK (`rmcp`).
- **Dynamic Ingestion & Schema Normalization:** Tools are discovered dynamically via `list_all_tools()` (automatically walking paginated catalogs) and converted into `rig::tool::DynamicTool` via `to_dynamic_tool()`. `translate_schema()` strips `$schema` dialect keys and ensures top-level object schemas carry `"properties": {}` for strict LLM function-calling compatibility.
- **Namespacing:** All MCP tools injected into an agent are prefixed with `{server_name}__` and normalized to `[a-zA-Z0-9_-]+` to prevent naming collisions with Boxxy's native tools (e.g. `github__search_code`).
- **UI & Telemetry Observability (`McpToolObserver`):** `DynamicMcpTool` invokes `McpToolObserver::on_tool_started` and `on_tool_result` (implemented by `ClawApprovalHandler` in `boxxy-claw`) so MCP tool calls appear in the real-time Claw indicator badge, the read-only Claw sidebar debug log, and telemetry.

### Connection Lifecycle & Resilience
1. **Protocol Negotiation (`ClientLifecycleMode`):**
   - Streamable HTTP clients use `ClientLifecycleMode::Auto`, probing MCP `2026-07-28` (`server/discover` with inline per-request `_meta` and SEP-2243 headers) and transparently falling back to the legacy `initialize` handshake (`ProtocolVersion::LATEST_WITH_INITIALIZE`) for older servers.
   - Stdio child-process clients use the `initialize` handshake with configurable timeouts.
2. **Client Identity & Cache Invalidation (`BoxxyClientHandler`):**
   - Advertises `Implementation::new("boxxy-terminal", env!("CARGO_PKG_VERSION"))` via `ClientConfig`.
   - Listens for `notifications/tools/list_changed` and immediately invalidates the server's entry in `McpClientManager::tool_cache`.
   - Configures `rmcp`'s per-peer response cache with `with_serve_stale_on_error(false)` so transport failures surface cleanly.
3. **Full Config Diffing & Auto-Reconnection (`McpClientManager`):**
   - `update_configs()` compares full `McpServerConfig` structs. Any server that is removed, disabled, or modified (URL, command, args, headers, env, timeout) is gracefully closed via `close_with_timeout` and evicted from `tool_cache`.
   - `ensure_client()` checks `client.is_closed() || client.is_transport_closed()` before every tool discovery or execution, automatically reconnecting with exponential backoff (`max_retries` + `BackoffConfig`) if a child process or HTTP session died.

### Process Safety (Zombification Prevention)
Node.js and Python processes spawned via Stdio are notoriously difficult to clean up if the parent process crashes.
- The `client::stdio` module uses `rmcp::transport::which_command` + `tokio::process::Command` wrapped with `.kill_on_drop(true)` and `process-wrap` 10.0 via `TokioChildProcess`.
- This guarantees that if the Rust client is dropped or the application exits unexpectedly, the OS terminates the child process immediately.
