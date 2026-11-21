# Core Toolbox Crate (`boxxy-core-toolbox`)

## Responsibility
Provides a library of high-level, structured tools for Boxxy agents. This crate decouples specialized capabilities (file management, workspace search, system monitoring, clipboard, network fetching, and web search) from the core reasoning engine, making agents faster, safer, and more autonomous.

## Architecture
The crate is organized into focused domain submodules re-exported from `src/lib.rs`:
- **`approval.rs`**: `ApprovalOutcome` (`Approved`, `Rejected`, `Interrupted(String)`), the `ApprovalHandler` trait, and shared lifecycle helpers (`begin_tool`, `finish_tool`, `report_error`, `await_proposal`, `resolve_live_cwd`) guaranteeing every `report_tool_started` call is paired with a structured `report_tool_result`.
- **`blacklist.rs`**: Mtime-cached `BLACKLIST.md` loader that merges custom rules onto the full built-in protected path list without erasing built-ins, enforcing path-segment boundaries plus lexical (`..`) and canonical symlink resolution.
- **`file/` (`mod.rs`, `read.rs`, `list.rs`, `write.rs`, `replace.rs`, `matcher.rs`, `delete.rs`)**:
  - `file_read`: 500-line default window, 1-based range validation (`end_line >= start_line`), and stale-edit FNV-1a hash registration.
  - `replace_in_file`: surgical single or batch edits with stale-file verification (`check_stale_file_before_write`), no-op edit rejection, dominant CRLF/LF detection, zero-allocation 3-tier matching (`matcher.rs`: exact, trailing-whitespace-insensitive, and typographic quote/dash/space normalized), and post-write `/undo` snapshot commitment.
  - `file_write`: full-file creation/overwrite with stale-file check, zero-clone `spawn_blocking` diffing (`FileDiff`), and post-write snapshot commitment.
  - `list_directory`: deterministic directory-first listing with zero-allocation ASCII case-insensitive sorting and tie-breaking, capped at 250 entries.
  - `file_delete`: user-approved permanent file deletion.
- **`workspace_search/` (`mod.rs`, `walker.rs`, `glob.rs`, `grep.rs`)**: `.gitignore`-aware workspace navigation (`ignore::WalkBuilder` on `spawn_blocking`), supporting bracket/brace globs, single-file `include` matching, symlink/blacklist pruning, 8 KiB binary header probing, reusable scratch read buffers, whole-file SIMD `multi_line` + `crlf` regex prefiltering, and automatic literal fallback on unescaped regex syntax.
- **`system/` (`mod.rs`, `info.rs`, `processes.rs`, `kill.rs`, `background.rs`)**: Host hardware/OS/GPU/thermal telemetry (`get_system_info`), allocation-free filtered and `total_cmp`-sorted process inspection (`list_processes`), PID/signal-validated process termination (`kill_process`), and detached background launching (`run_background_command` including `command` attribution for turn traces).
- **`clipboard.rs`**: User-approved system clipboard read/write (`get_clipboard`, `set_clipboard`).
- **`network/` (`mod.rs`, `html.rs`)**: Read-only (`GET`/`HEAD`) `http_fetch` over a shared `reqwest::Client` with cross-origin SSRF redirect guards (blocking external redirects to loopback/link-local/RFC1918 addresses), header allowlisting, UTF-8 char-boundary-safe truncation, and single-pass HTML-to-Markdown extraction (`html.rs`: raw `<script>`/`<style>`/`<svg>` skipping, `<!-- -->` comment handling, `<pre>` indentation preservation, and non-double-unescaping entity decoding).
- **`search/` (`mod.rs`, `tavily.rs`, `tool.rs`)**: `SearchProvider` trait, `WebSearchTool` (input validation and clamped limits), and `TavilyProvider` backed by a shared `reqwest::Client` pool and bounded error body reporting.
- **`utils.rs`**: Cached `~` home expansion, `CurDir`-normalized `resolve_path` / `resolve_dir_path` (preserving `..` for downstream symlink guards), and zero-allocation case-insensitive string helpers.

## Approval Protocol
Sensitive tools (`file_write`, `replace_in_file`, `file_delete`, `kill_process`, `run_background_command`, `get_clipboard`, `set_clipboard`) require user consent via `ApprovalHandler`:
- When the user types a new instruction while a proposal is pending, `ApprovalOutcome::Interrupted(msg)` returns a `[USER_INTERRUPTION]` payload so the active LLM turn pivots immediately.
- Explicit rejection returns `[USER_EXPLICIT_REJECT]`, allowing `ClawSession` to abort cleanly with zero wasted tokens.
- Snapshots and updated file content hashes (`record_file_snapshot_before_write`) are committed strictly after `write_file` succeeds on disk so failed writes never poison `/undo` or stale-file state.
- Pure read-only tools (`file_read`, `list_directory`, `glob_search`, `grep_search`, `get_system_info`, `list_processes`, `http_fetch`, `web_search`) declare `ToolAnnotations::default().with_read_only(true)` so `ordered_by_footprint(true)` executes them concurrently, while all mutating and approval-gated tools (`file_write`, `replace_in_file`, `file_delete`, `kill_process`, `run_background_command`, `get_clipboard`, `set_clipboard`) retain `with_read_only(false)` as full serialization barriers.
