# Claw Protocol Crate (`boxxy-claw-protocol`)

## Role

Shared, GTK-free, engine-free DTO layer used across the UI ↔ daemon D-Bus boundary. Anything the UI and the reasoning engine need to agree on lives here; anything heavier does not.

The crate is the single shared contract between the UI process and the daemon process, organized into focused domain submodules with a thin re-exporting `lib.rs` facade.

## Module Layout & Contents

- **`messages.rs` (`ClawMessage` & `ClawEngineEvent`)**:
  - **`ClawMessage`**: every request the UI can send down to an active `ClawSession`. Includes `Initialize`, `Deactivate`, `Reload`, `ClawQuery`, `UserMessage`, approval replies (`FileWriteReply`, `KillProcessReply`, …), sleep/pin/web-search/vibe toggles, delegated-task plumbing, correlation-ID replies (`ScrollbackReply`, `SpawnAgentReply`), and `SettingsInvalidated` for cross-process settings sync.
  - **`ClawEngineEvent`**: every event the engine emits upward — `AgentThinking`, `TurnTraceProgress`, `AssistantTextDelta`, `DiagnosisComplete`, proposal events (`ProposeFileWrite`, `ProposeKillProcess`, …), `Identity`, `PinStatusChanged`, `RequestScrollback`, `RestoreHistory`, `LongTaskCompleted`, `PushGlobalNotification`, etc. All agent-originated events carry a `character_id` to ensure consistent visual attribution. `RequestScrollback` uses a `request_id` with a matching `ClawMessage::ScrollbackReply` so no `oneshot::Sender` ever crosses the IPC boundary.
- **`rows.rs` & `trace.rs` (`PersistentClawRow` & `TurnTraceItem`)**: the row variants the sidebar/overlay render and the DB stores (`User`, `Diagnosis`, `Suggested`, `ToolCall`, `ProcessList`, `SystemMessage`, `Command`, `FileDiff`). `Diagnosis` and `Suggested` carry `trace: Vec<TurnTraceItem>` (`Thought`, `Tool`, `Edit`, `Skill`) summarized via `ActivitySummary` (`+ 4 commands, 19 edits, 8 thoughts, 2 reads, 1 tool`), `extract_reasoning_summary` / `live_reasoning_title` (zero-copy bold-title parsing & `[REDACTED]` stripping), single-pass JSON tool-result summarization (`tool_result_to_trace_item`), and speaker-aware `coalesce_restored_history` (folding legacy DB tool/diff rows into turn traces on `/resume`). Round-trips through JSON + SQLite losslessly.
- **`diff.rs` (`FileDiff`)**: local diff engine and serialisable DTOs (`FileDiff`, `DiffKind`, `DiffStats`, `DiffHunk`, `DiffPairRow`, `DiffCell`, `DiffSpan`) powered by `similar 3.2.1` (`Algorithm::Histogram` + word-level inline changes with `semantic_cleanup`). Bounds file sizes (`<= 2 MiB`) before CRLF normalization, caps rendered rows (`MAX_DIFF_RENDER_ROWS = 250`) before inline span allocation, handles created (`DiffKind::Created`) and binary (`DiffKind::Binary`) files, and generates `git apply`-compatible unified patches (`to_unified_patch`).
- **`status.rs` (`AgentStatus`, `ContextQuality`, `TriggerSource`, `TransitionRequest`, `SpawnLocation`, `ClawEvent`)**: live agent FSM states (`{Off, Sleep, Waiting, Working, Locking{resource}, Faulted{reason}}`), pane spawn placements, and inter-agent pub/sub event payloads.
- **`tasks.rs` (`ScheduledTask`, `TaskType`, `TaskStatus`, `IntervalPhase`, `IntervalSchedule`, `PendingTaskInfo`)**: scheduled-reminders and multi-round interval timer payloads with pending-guarded `toggle_pause_at`, plus the cross-agent `PendingTaskInfo` snapshot shape.
- **`usage.rs` (`UsageWrapper`, `SessionUsage`, `format_number_with_commas`, `format_usd`)**:
  - **`UsageWrapper`**: serde-safe DTO wrapper implementing `From<rig::completion::Usage>` and `From<&rig::completion::Usage>` (`{input_tokens, output_tokens, reasoning_tokens, cached_input_tokens, cost_usd, context_tokens, context_window}`) with saturated `total_tokens()` and `effective_context_tokens()`.
  - **`SessionUsage`**: `Copy` session-level context and cost state (`{context_tokens, context_window, total_tokens, cost_usd, tools_used}`) with `context_percent()` and `record_turn()`, plus stack-buffered `format_number_with_commas` and `format_usd`.
- **`environment.rs` (`ClawEnvironment` & `ProcessSample`)**: the async trait the reasoning engine calls for privileged host operations (`exec_shell`, `read_file`, `list_processes` returning `Vec<ProcessSample>`, `get_clipboard`, …). Implemented in-process on the daemon (`boxxy_agent::claw::ClawSubsystem`) and forwarded over D-Bus by `boxxy_terminal::agent_manager::DbusClawEnvironment`.
- **`characters.rs` (Character claim types & coherent caches)**:
  - `HolderKind { Pane = 0 }` (`#[repr(u8)]`) — extensible holder discriminator.
  - `CharacterClaim { holder_id, holder_kind, character_id, session_id, petname, owner_bus_name }` — one live claim (`petname` is the stable two-word Red Pony swarm address; `owner_bus_name` drives `NameOwnerChanged` crash cleanup).
  - `RegistrySnapshot { catalog, claims, revision }`, `ClaimedSession`, `ClaimError` (`Display` + `Error`), `StartupToken`, `PeerQuery`, `PeerInfo`.
  - `CHARACTER_CACHE: ArcSwap<Vec<CharacterInfo>>`, `CLAIMS_CACHE: ArcSwap<Vec<CharacterClaim>>`, and an internal coherent `(catalog, claims, revision)` `ArcSwap` (`load_coherent_registry`) so availability helpers (`find_first_available_character`, `get_available_characters`) never observe torn catalog/claim pairs, plus Unicode-aware case-insensitive `find_character_exact` / `resolve_character` and `MCP_STATUS_CACHE`.
- **`character_loader.rs`**: atomic disk I/O (`atomic_write` via hidden `.tmp` rename), multi-line-safe in-place `CHARACTER.toml` patching (`save_character_color`, empty/missing/duplicate UUID repair), single-pass `load_characters` / `load_characters_in`, and declarative `BUNDLED_CHARACTERS` extraction (`niko-una`, `levi-kujo`, `kuro`, plus `inco-gnito` avatar).
- **`skill_loader.rs`**: unified UTF-8 BOM- and boundary-safe YAML frontmatter parser (`parse_frontmatter_block`, `parse_skill_document`), stack-buffered 4 KiB header scanner (`read_skill_summary`, `discover_skill_summaries`), slug deduplication, and `is_skill_disabled` shared between `boxxy-preferences` and `boxxy-claw`.

## Design Rules

- **No GTK types.** This crate compiles without `gtk4`/`libadwaita`. The UI-side wrappers live in `boxxy-claw-ui`.
- **No `boxxy-claw` reasoning-engine internals.** `boxxy-claw` depends on this crate, never the reverse; only lightweight data conversions (such as `rig::completion::Usage` → `UsageWrapper`) belong here so SQLite and D-Bus wire schemas remain stable across upstream SDK updates.
- **All public DTOs implement `Serialize + Deserialize`**, even the large enums. IPC transport uses JSON; the types are wire-format-agnostic.
- **Correlation IDs over channels.** When the engine needs a response from the UI (scrollback, spawn-agent, delegated-task reply), the request carries a `uuid::Uuid` request ID and the reply is a distinct `ClawMessage` variant with the same ID. No `Sender<T>` ever crosses a process boundary.
