# Core Toolbox Crate (`boxxy-core-toolbox`)

## Responsibility
Provides a library of high-level, structured tools for Boxxy agents. This crate decouples specialized capabilities (file management, system monitoring, network fetching) from the core reasoning engine, making agents faster, safer, and more autonomous.

## Architecture
The crate is modular, with tools grouped by functionality:
- **`file.rs`**: Tools for file reading (with `start_line` and `end_line` support to save context), writing, deletion, and directory listing.
- **`system.rs`**: Tools for system information retrieval (returning structured JSON OS/Memory/CPU data) and process management (list/kill).
- **`clipboard.rs`**: Tools for reading and writing to the system clipboard.
- **`network.rs`**: Tools for web fetching (`http_fetch`) with built-in timeouts and 1MB response size limits to prevent context flooding.
- **`search/mod.rs`**: Web search functionality with a `SearchProvider` trait abstraction. Includes the `WebSearchTool` and currently implements `TavilyProvider`.
- **`utils.rs`**: Shared utilities like robust path resolution (handling absolute, relative, and `~` home directory paths).

## Approval Protocol
Sensitive tools (e.g., file deletion, process termination, clipboard access) require user consent. This is handled via the `ApprovalHandler` trait:
- The toolbox defines the trait.
- `boxxy-claw` implements the trait to bridge with the GTK UI and `ClawEngineEvent` system.
- The protocol supports **Structured Result Reporting**, allowing tools to send back JSON data (like process lists or system info) for rich UI rendering before the agent continues its turn.

## Key Features
- **Structured Output**: Returns JSON/data-driven results instead of raw terminal strings, preventing parsing errors.
- **Context Protection**: Built-in limits for file reading (line ranges) and network requests (1MB cutoff) ensure the LLM's context window isn't overwhelmed.
- **Lazy Load Integration**: Tools are conditionally registered based on user preferences in `boxxy-preferences` (e.g., `enable_system_tools`, `enable_web_tools`).
- **Host Awareness**: Leverages `boxxy-agent` to bypass Flatpak sandboxing for system-level operations.
