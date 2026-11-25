# Scenario Runner (`scenario-runner`) — Deterministic Headless E2E Harness

## Overview

`scenario-runner` is Boxxy's GTK-free, deterministic End-to-End (E2E) workflow test harness (`crates/scenario_runner`). Instead of relying on live external LLM API keys or untyped YAML fixtures, it drives the **real** headless Boxxy stack in standard `cargo test` runs:
- Real `ClawSession` actors (`boxxy-claw`), `rig 0.47` streaming + tool concurrency, and `WorkspaceRegistry`
- Real `ClawSubsystem` (`boxxy-agent`) sharing a single `AgentState` with `PtyRegistry`
- Real `boxxy-db` SQLite WAL + FTS5 persistence inside a process-isolated `TempDir` config root
- Real Linux POSIX PTYs (`/bin/bash` + `OSC 7` / `OSC 133` shell integration + 4 MB detached `RingBuffer`)
- Headless `boxxy-cli` `+doctor` and `+env` verification

## Architecture & Modules

- **`src/config_env.rs`**: Bootstraps a process-scoped temporary config root via `boxxy_sys_utils::configure_dev_options(CliDevOverrides { dev: true, instance: Some("e2e"), ephemeral: true, config_dir: Some(...), debug_context: env_truthy("BOXXY_DEBUG_CONTEXT"), .. })`, initializes single-init `env_logger` (honoring `BOXXY_DEBUG_CONTEXT=1 RUST_LOG=model-context=info`), disables telemetry, self-updates, and ambient network fetches (`enable_os_context = false`), seeds bundled Characters v2 packages (`load_characters`), and wires `Settings` to local `OpenAiCompatible` endpoints (`mock-claw-model` + `mock-memory-model`). Never touches `~/.config/boxxy-terminal/`.
- **`src/mock_llm.rs` (`MockLlmServer`, `MockStep`, `MockToolCall`, `TurnGate`, `RecordedChatRequest`)**: In-process `tokio::net::TcpListener` (`127.0.0.1:0`) OpenAI `/v1/chat/completions` HTTP/SSE server. Parses multi-kilobyte request bodies (`Content-Length` up to 4 MiB), exposes `system_prompt_text()` and `conversation_text()` inspectors across string and array content blocks, isolates background `mock-memory-model` calls (Turn-1 session title generator, `flush_history`) and `/btw` side-channel calls (`tool_choice == "none"`) from main conversation turns, streams spec-compliant SSE `reasoning_content`, `tool_calls`, `content`, and `usage` chunks, and provides zero-sleep `TurnGate` barriers for deterministic mid-turn concurrency tests.
- **`src/pane_driver.rs` (`PaneDriver`, `CompanionPane`, `CompanionTable`)**: Non-dropping event collector (`Arc<Mutex<Vec<ClawEngineEvent>>>` + `tokio::sync::Notify`) with a monotonic read cursor so sequential `wait_for_next` assertions never drop interleaved events or re-match stale turns. Handles correlation-ID side effects (`RequestSpawnAgent`, `RequestScrollback`, `InjectKeystrokes`, `RequestCloseAgent`, and companion-pane `ProposeTerminalCommand`).
- **`src/pty_driver.rs` (`PtyShellSession`)**: Real POSIX pseudoterminal (`posix_openpt` / `grantpt` / `unlockpt`) + `/bin/bash` child running with a synthetic `HOME` (never sourcing host `~/.bashrc`) and Boxxy's `apply_shell_integration` scripts. Exercises `PtyRegistry::release_owner`, detached `RingBuffer` capture, `reattach`, and process-group `SIGKILL` cleanup on `Drop`.
- **`src/harness.rs` (`TestWorkspace`)**: High-level test fixture combining `MockLlmServer`, shared `AgentState` (`ClawSubsystem` + `PtyRegistry`), `Db`, `WorkspaceRegistry::reset_all()` isolation, `/incognito` pane spawning (`spawn_incognito_pane`), temporary workspace files, Vibe Mode directory trust, and `BLACKLIST.md` configuration.
- **`src/main.rs`**: Standalone CLI smoke check (`cargo run -p scenario-runner`) verifying edit proposal, approval, `/undo`, and real PTY `OSC 133` round-trip in `< 200ms`.

## Automated Real User Workflow Suites (`tests/workflows.rs` & `tests/workflows/`)

Organized as modular submodules inside a single integration test binary (`tests/workflows.rs`) so Cargo links once while executing every workflow under `cargo test -p scenario-runner`:
1. **`tests/workflows/edit_and_undo.rs`**: `@file` Turn-1 hash pre-loading, `replace_in_file` diff proposals, zero-token user rejection (`ProposalResolved { approved: false }`) with same-pane Turn-2 `@file` & rejection-note continuity, mid-proposal user steering with `@file` preload, approval `TurnTraceItem::Edit`, and `/undo` byte-exact file restoration.
2. **`tests/workflows/error_diagnosis.rs`**: Origin-aware terminal error diagnosis — ignores benign exit codes (`grep` 1, `SIGINT` 130), emits `LazyErrorIndicator` at **0 LLM tokens** on user-typed errors until `RequestLazyDiagnosis`, and auto-diagnoses agent-originated command failures.
3. **`tests/workflows/queue_btw_and_abort.rs`**: In-flight `TurnGate` synchronization, FIFO follow-up prompt queueing (`QueueUpdated`), draft recall (`QueuedPromptRecalled`), concurrent `/btw` side-channel (`ForbidToolsHook` blocking rogue tool calls, warm `in_flight_snapshot` reuse, zero `claw_events` mutation), and `Abort` (`Esc Esc`) queue parking + `[Previous task was interrupted by the user]` alternation bridging.
4. **`tests/workflows/persistence_and_resume.rs`**: Multi-tool turn trace & catalog-priced token persistence in `boxxy-db`, Turn-1 background session title generation, visual-only `/clear` (`SoftClearHistory`), and cross-pane `/resume` with `Evicted` + coalesced `RestoreHistory`.
5. **`tests/workflows/companion_panes_and_vibe.rs`**: `1 Home Pane + Companion Terminal` (`spawn_agent` -> `pane-2` -> `terminal_exec` -> `read_pane_buffer` -> `send_keystrokes_to_pane` -> `close_agent`) and Vibe Mode ("Scoped YOLO") guardrails (trusted workspace auto-approve vs out-of-workspace manual approval fallback vs `BLACKLIST.md` hard block).
6. **`tests/workflows/pty_and_cli.rs`**: Real PTY + `/bin/bash` `OSC 7` / `OSC 133` markers, `PtyRegistry` detach + 4 MB `RingBuffer` replay + `reattach`, and isolated `--dev --ephemeral --instance=e2e` headless `boxxy-cli` `+doctor --json` & `+env` execution.
7. **`tests/workflows/context_hygiene_and_compaction.rs`**: Static system prompt & compact turn-context formatting, `Pane Alias` / `Swarm Petname` identity injection, `[TERMINAL_SNAPSHOT: EMPTY]` & `[TERMINAL_SNAPSHOT: NO_CHANGE]` deduplication, unified `@file` `<config_dir>/auth` blacklist enforcement, `EXTERNALLY MODIFIED FILES NOTICE`, 4-turn freshness stripping + `TRANSCRIPT_CLIP`, and `>= 75%` `input_limit` exchange-boundary compaction (`[SESSION_CHECKPOINT]` + `DemotionExtractor` `flush_history`).
8. **`tests/workflows/incognito_and_roundtable.rs`**: `/incognito` (`Inco Gnito`) zero-tool, zero-context, zero-DB guarantee and multi-character `/debate :levi-kujo` perspective-flipping (`[Character Niko Una said]:` with zero persona bleed) + autonomous turn-limit termination.

## Running Tests

```bash
# Run all deterministic E2E workflow suites
cargo test -p scenario-runner

# Run the standalone smoke check binary
cargo run -p scenario-runner
```
