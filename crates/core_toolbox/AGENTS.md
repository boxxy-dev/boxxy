# Core Toolbox Crate (`boxxy-core-toolbox`)

## Responsibility
Provides a library of high-level, structured tools for Boxxy agents. This crate decouples specialized capabilities (file management, system monitoring, network fetching) from the core reasoning engine, making agents faster, safer, and more autonomous.

## Architecture
The crate is modular, with tools grouped by functionality:
- **`file.rs`**: Tools for file reading (`file_read` with default 500-line window), surgical editing (`replace_in_file` with CRLF preservation and uniqueness verification), full-file writing (`file_write`), deletion (`file_delete`), and deterministic directory listing (`list_directory`, directories first, capped at 250 entries).
- **`workspace_search.rs`**: Fast workspace navigation tools (`glob_search` and regex `grep_search`) running on `tokio::task::spawn_blocking`, automatically skipping `.git`, `target`, `node_modules`, and binary files while respecting `BLACKLIST.md`.
- **`system.rs` & `system/background.rs`**: Tools for system information retrieval (`get_system_info` including OS, CPU, RAM, GPUs, disks, and thermal sensors), filtered/sorted/bounded process listing (`list_processes` with `filter`, `sort_by` supporting `cpu`/`memory`/`gpu`/`gpu_memory`/`io`/`pid`, per-process GPU usage/VRAM, and `limit`), process termination (`kill_process`), and detached background application/server launching (`run_background_command`).
- **`clipboard.rs`**: Tools for reading and writing to the system clipboard with `[USER_EXPLICIT_REJECT]` feedback on denial.
- **`network.rs`**: Tools for web fetching (`http_fetch`) with a shared `reqwest::Client`, 10s timeouts, 1MB response size limits, and Markdown-like HTML extraction to prevent context flooding.
- **`search/mod.rs`**: Web search functionality with a `SearchProvider` trait abstraction. Includes the `WebSearchTool` and currently implements `TavilyProvider`.
- **`utils.rs`**: Shared utilities like robust path resolution (handling absolute, relative, and `~` home directory paths).

## Approval Protocol
Sensitive tools (e.g., file writes/edits, file deletion, process termination, background commands, clipboard access) require user consent. This is handled via the `ApprovalHandler` trait and `ApprovalOutcome` enum (`Approved`, `Rejected`, `Interrupted(String)`):
- The toolbox defines the trait (including `current_dir()` so cached tools dynamically track terminal `cd` changes across turns).
- `boxxy-claw` implements the trait (`ClawApprovalHandler`) to bridge with the GTK UI and `ClawEngineEvent` system.
- When the user types a new instruction instead of clicking Approve/Reject while a proposal is on screen, `ApprovalHandler` resolves with `ApprovalOutcome::Interrupted(msg)` and the tool returns a structured `[USER_INTERRUPTION]` message so the active LLM turn immediately pivots to the new instruction without tearing down and restarting the turn.
- When the user explicitly rejects a proposal, `ClawSession` aborts the in-flight turn immediately (zero wasted LLM tokens) and closes the drawer.
- The protocol supports **Structured Result Reporting**, allowing tools to send back JSON data (like process lists or system info) for rich UI rendering before the agent continues its turn.

## Key Features
- **Structured Output**: Returns JSON/data-driven results instead of raw terminal strings, preventing parsing errors.
- **Context Protection**: Built-in limits for file reading (line ranges) and network requests (1MB cutoff) ensure the LLM's context window isn't overwhelmed.
- **Lazy Load Integration**: Tools are conditionally registered based on user preferences in `boxxy-preferences` (e.g., `enable_system_tools`, `enable_web_tools`).
- **Host Awareness**: Leverages `boxxy-agent` to bypass Flatpak sandboxing for system-level operations.
