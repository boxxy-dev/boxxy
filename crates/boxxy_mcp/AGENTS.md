# Boxxy-MCP Architecture & Guidelines

## Component Overview
The `boxxy-mcp` crate provides a robust, generic Model Context Protocol (MCP) client integration for Boxxy-Terminal. It acts as the bridge between standard MCP servers (which use JSON-RPC and JSON Schema Draft 7) and `rig-core` (the agentic reasoning framework used by Boxxy).

### Design Philosophy
- **Strict Modularity:** Boxxy's core UI (`boxxy-preferences`) and agent reasoning (`boxxy-claw`) remain completely decoupled from the underlying MCP SDK (`rmcp`).
- **Dynamic Ingestion:** Tools are discovered dynamically at runtime and converted into `rig::tool::DynamicTool` via `to_dynamic_tool()`, translating the MCP JSON Schema to Rig's expected format on the fly.
- **Namespacing:** All MCP tools injected into an agent are prefixed with `{server_name}__` to prevent naming collisions with Boxxy's native tools (e.g. `github__search_code`).

### The Lazy Boot Initialization
To prevent severe startup performance degradation (especially when spawning multiple Node.js processes via `npx`), `boxxy-mcp` employs a **Lazy Boot** strategy.
1. The `McpClientManager` acts as a global Resource.
2. During agent instantiation, `build_rig_tools()` is called.
3. Instead of immediately spawning the child process and calling `ListTools`, the manager reads from a local tool cache.
4. The actual Stdio/HTTP connection is only fully established when the agent first attempts to `call()` the tool.

### Process Safety (Zombification Prevention)
Node.js and Python processes spawned via Stdio are notoriously difficult to clean up if the parent process crashes. 
- The `client::stdio` module uses `tokio::process::Command` wrapped with `.kill_on_drop(true)`. 
- This guarantees that if the Rust `McpClient` is dropped or the application exits unexpectedly, the OS will immediately terminate the child process, preventing orphaned processes.
