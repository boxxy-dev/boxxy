# Claw Engine Tools (`crates/boxxy_claw/src/engine/tools/`)

## Responsibilities
Houses session-aware tools that interact directly with `SessionState`, `WorkspaceRegistry`, `SkillRegistry`, PTY scrollback/command injection, and character-scoped capabilities. Host-level generic tools (`file_read`, `replace_in_file`, `glob_search`, `http_fetch`, etc.) live in `boxxy-core-toolbox`.

## Footprint & Concurrency Rules
`ClawAgent` executes tool calls with `.tool_concurrency(4).ordered_by_footprint(true)`:
- **Read-only pane/workspace inspection tools** (`ReadScrollbackTool`, `ReadPaneTool`, `ListActiveAgentsTool`, `ListTasksTool`) declare `ToolAnnotations::default().with_read_only(true)` and pure read footprints so multiple pane/scrollback reads run concurrently.
- **Mutating, external-side-effect, and approval-gated tools** (`SysShellTool`, `TerminalCommandTool`, `GenerateImageTool`, `InviteCharacterTool`, `SpawnAgentTool`, `SendKeystrokesTool`, `ScheduleTaskTool`, lock/orchestration tools) MUST keep `with_read_only(false)` / default `Footprint::unknown()` so UI approval popovers, PTY injections, and heavy API calls serialize deterministically and never race on `SessionState` oneshot channels.

## Key Modules
- **`mod.rs` (`ClawApprovalHandler`, `SysShellTool`)**: Implements `boxxy_core_toolbox::ApprovalHandler` and `McpToolObserver` over `SessionState` oneshot channels, Vibe Mode path checks (`is_safe_vibe_path`), stale-file FNV-1a verification, `/undo` pre-turn baseline snapshots (`current_turn_snapshots`), and live `TurnTraceProgress` emission.
- **`image.rs` (`GenerateImageTool`)**: Registered exclusively when a character's `ModelProvider::supports_image_generation()` is `true` (`Gemini`, `OpenAI`, `xAI`, `MiniMax`, `OpenRouter`). Calls `boxxy_ai_core::generate_image_with_provider` with the character's own credentials, supports local or turn-attached reference images on Gemini/OpenRouter, writes decoded bytes atomically to `<config_dir>/boxxyclaw/generated/`, gates explicit `output_path` saves through `BLACKLIST.md` + `propose_file_write` (`DiffKind::Binary`) + `/undo` snapshots, and records `(caption, path)` in `SessionState.generated_images_this_turn`.
- **`orchestration.rs` (`InviteCharacterTool`, `SubscribeToPaneTool`, `AcquireLockTool`, `ReleaseLockTool`, `AwaitTasksTool`, `RequestAssistanceTool`, `OrchestrateAgentTool`)**: Swarm pub/sub, distributed locks, capability routing, and multi-character overlay invitations (dynamically advertising image-capable peers when the host character uses a text-only model, and supporting 1-turn collaborative handoffs via `debate: false`).
- **`terminal.rs` (`TerminalCommandTool`)**: Proposes or Vibe-executes interactive shell commands in the home pane or a targeted companion pane (`target_pane`).
- **`workspace.rs` (`SpawnAgentTool`, `CloseAgentTool`, `SendKeystrokesTool`, `ReadPaneTool`, `DelegateTaskTool`, `DelegateTaskAsyncTool`, …)**: Multi-pane lifecycle, TUI keystroke control, and map-reduce task delegation.
- **`scrollback.rs`, `skills.rs`, `tasks.rs`, `set_state.rs`, `summon_headless.rs`**: Paged terminal scrollback (`read_scrollback_page`), on-demand skill loading (`activate_skill`), persistent scheduled tasks & interval timers (`schedule_task`), autonomous sleep transitions (`set_agent_state`), and transient background workers (`summon_headless_worker`).
