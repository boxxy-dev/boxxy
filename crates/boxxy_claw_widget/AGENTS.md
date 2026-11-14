# Claw Widget Crate (`boxxy-claw-widget`)

## Responsibility
The reusable Claw drawer UI. Owns every widget the user sees when interacting with an agent in a pane: the slide-down overlay drawer, the floating `ClawIndicator` badge, the merged input bar (formerly the standalone `boxxy-msgbar` crate), the neutral `Proposal` enum, and the event-dispatch loop that turns a `ClawEngineEvent` stream into widget mutations.

The crate is **surface-agnostic**. It talks to its host — the thing that owns the PTY / scrollback / focus target / agent channel — exclusively through the `ClawHost` trait. Today the terminal pane is the only implementor (`boxxy_terminal::PaneClawHost`), but any future surface (a standalone chat window, a mobile shell, a remote-session viewer) can drive the same UI by implementing the trait.

## Architecture

```
boxxy_claw_widget/
├── overlay.rs         drawer UI (TerminalOverlay) — loaded from
│                      /dev/boxxy/BoxxyTerminal/ui/claw_overlay.ui
├── claw_indicator.rs  floating status pill / badge (ClawIndicator)
├── msgbar/            merged input bar (formerly a crate)
│   ├── mod.rs         MsgBarComponent constructor + status / update_ui + TextTag highlighting
│   ├── attachment.rs  clipboard paste → image/text chips
│   ├── autocomplete.rs @file / :character / /debate / /drama / /resume completion providers
│   ├── file_index.rs  async ignore::WalkBuilder file indexer + frizbee fuzzy matcher + frecency
│   └── history.rs     lazy-loaded MsgHistory backed by boxxy-db
├── dispatch.rs        spawn_dispatch(rx, host, overlay, …) event loop
├── claw_host.rs       ClawHost trait — the decoupling boundary
├── proposal.rs        Proposal enum (UI shape)
└── lib.rs             re-exports the public surface
```

### The `ClawHost` trait (`claw_host.rs`)
The single boundary between widget code and surface-specific code. Ten methods: `host_id`, `inject_line`, `execute_script`, `snapshot` (async), `grab_focus`, `set_focusable`, `is_busy`, `working_dir`, `send_claw`, `focus_sidebar`, `cd`, `notify`, `forward_event`. The trait is `!Send` / `!Sync` — GTK widgets are single-threaded and every call happens on the main thread; a `Send` bound would be a lie and would force `Arc<Mutex<…>>` ceremony in impls.

### The drawer (`overlay.rs`)
Defines `TerminalOverlay` and `OverlayMode { Claw, Bookmark }`. Constructed via `TerminalOverlay::new(indicator, msg_bar, host, pending_character)`. The `.ui` file wires most of the layout; Rust code attaches `ClawIndicator::widget()` into `indicator_slot`, appends `msg_bar.widget` into `msgbar_slot`, and stitches up button handlers (each one is two lines of `host.xyz()`).

`TerminalOverlay::sync_action_state()` is the single source of truth for the bottom action row (`file_action_box`, `action_box`, `ok_btn`, `accept_btn`, `reject_btn`, `indicator_slot`, `tips_cycle`, and `msg_bar.set_input_sensitive`):
- `OverlayState::Idle` — `ok_btn` visible, `indicator_slot` hidden, input sensitive.
- `OverlayState::Thinking` — `indicator_slot` visible (spinner pill + Cancel `×`), `ok_btn` and proposal buttons hidden, input disabled until the turn finishes, pauses on a proposal, or is cancelled.
- `OverlayState::Pending` — `ok_btn` visible + pending tips, `indicator_slot` hidden, input sensitive.
- `OverlayState::Action(Proposal)` — only the proposal's approval/reject buttons visible, `ok_btn` and `indicator_slot` hidden, input sensitive so the user can steer the proposal by typing a follow-up instruction.

- `show_chat_only(agent_name)` — opens the drawer in an empty-proposal state (used when the agent starts thinking before producing output).
- `show_input_only(agent_name)` — the `Ctrl+/` entry point. Reveals the drawer if closed; focuses the input if sensitive; never hides or clobbers an active `Thinking` or `Action` state. The Okay/Reject/Accept buttons are the explicit dismiss paths — silent hiding would strand the user when the agent is waiting for a message.
- `enter_waiting_state()` — collapses proposal controls and transitions immediately to `OverlayState::Thinking` so the drawer reads as "thinking…" the moment the user submits a reply or approves an in-turn tool proposal.

The character selector renders from `CHARACTER_CACHE` and `CLAIMS_CACHE`; it is refreshed via `RegistrySnapshot` push signals, not polling.

### History list + sticky-bottom auto-scroll
The drawer's scrollable history is a `gtk::ListView` over a `gio::ListStore` (`history_store`, synced from `full_history_store` via a `clear_offset` tail slice so temporary `/clear` preserves session history). Virtualization means `adj.upper()` grows asynchronously as rows realize and Markdown/diff blocks measure.

To keep streaming pinned to the true bottom without stealing focus across split panes: `arm_sticky_scroll` maintains a single cancellable 1200 ms `SourceId` on `items_changed` and `scroll_to_latest()`, snapping `adj.set_value(new_upper - page_size)` on `notify::upper` while sticky, and immediately cancelling sticky mode if the user scrolls upward manually.

### The merged msgbar (`msgbar/`)
Formerly the `boxxy-msgbar` crate — folded in during phase 2. The `MsgBarComponent` is a full-width horizontal input bar with the user avatar/options popover on the left, an auto-wrapping `gtk::TextView` (`Enter` submits, `Shift+Enter` inserts a newline), and trailing Sleep/Vibe status icons on the right.

- **Embedded flag**: `msg_bar.set_embedded(true)` suppresses self-hide and terminal focus handoff on Enter / Escape so the drawer owns visibility and proposal Escape resolution (`OverlayState::Action` rejects active proposals on `Escape`).
- **Attachments**: `AttachmentManager` intercepts Ctrl+V and file drops off the GTK main thread (validating magic bytes & encoding raw PNG/JPEG/WebP files or clipboard textures asynchronously on the Tokio runtime) to render chips above the input. `build_payload(base_text)` merges everything into `(text, Vec<base64_image>)` for the agent.
- **Autocomplete**: six providers sorted by trigger-length descending — `FileCompletionProvider` (`@file`, backed by `file_index.rs` using `Arc<[WorkspaceEntry]>` bounded root caches, `ignore 0.4.34`, `frizbee 0.13.0` SIMD Smith-Waterman fuzzy matching, merged `BLACKLIST.md` rules, frecency boosting, `#L1-L2` / `:L1-L2` line-range preservation, and directory drilling), `AgentCompletionProvider` (`:character`), `DebateCompletionProvider` (`/debate `), `DramaCompletionProvider` (`/drama `), `CommandCompletionProvider` (`/resume`, `/vibe`, etc.), and `ResumeCompletionProvider` (lists recent sessions from the DB with LLM-generated titles + relative timestamps + agent colors).
- **History**: `MsgHistory` — lazily loaded from `boxxy-db` on the first up/down key. Capped at 100 entries in memory, dropping image attachments older than the 20 most recent entries so recalled items never contain invalid base64 data.

### The `ClawIndicator` (`claw_indicator.rs`)
Two surfaces:
1. **Floating top-right terminal overlay (`badge()`)**: hosts both the passive character badge (pass-through `can_target: false`) and the interactive `alert_revealer` pill (`"Error Detected · Diagnose"` / `"Diagnosis Ready · View"`, temporarily enabling `can_target: true` only while an alert is visible over a closed drawer).
2. **In-drawer thinking pill (`widget()`)**: embedded inside `indicator_slot` in the drawer's bottom action row, displaying the animated `gtk::Svg` spinner (paused automatically when unmapped/hidden), real-time tool names, and the Cancel (`×`) button.

Badge and spinner colors use `boxxy_claw_ui`'s shared `DynamicColorRegistry` (`is_safe_hex_color`-validated classes) so tool calls never reload display-wide `GtkCssProvider`s.

### The dispatch loop (`dispatch.rs`)
`spawn_dispatch(rx, host, overlay, indicator, msg_bar, id, session_status, is_pinned, is_web_search, is_vibe, agent_name, session_usage, pending_tasks)` — a single `while let Ok(event) = rx.recv().await` that matches every `ClawEngineEvent` variant. Each branch either mutates widget UI, appends overlay-visible rows to the overlay's full-session backing store (which syncs to the visible `history_store`), or delegates to the host. Resuming a session restores the full conversation history. Every event, including ones with no widget reaction (`RequestSpawnAgent`, `InjectKeystrokes`), is forwarded via `host.forward_event(event)` at the tail so non-UI consumers (tab badges, swarm router) see the full stream.

**User-typed error handling.** On `LazyErrorIndicator`, dispatch calls `indicator.show_lazy_error()` — a small red `"Error Detected · Diagnose"` pill appears on `indicator.badge()` at the top-right corner of the terminal pane while the drawer remains closed. **No drawer opens, no LLM call.** Clicking `"Diagnose"` dismisses the alert pill and fires `on_lazy_click` → the pane sends `ClawMessage::RequestLazyDiagnosis` → engine drains its stashed prompt and runs the diagnosis pipeline → next `DiagnosisComplete` opens the drawer normally.

### The neutral `Proposal` enum (`proposal.rs`)
UI-shaped, host-agnostic variants: `None`, `Command`, `Bookmark`, `FileWrite`, `FileDelete`, `KillProcess`, `BackgroundCommand`, `GetClipboard`, `SetClipboard`. The terminal's own `TerminalProposal` converts via `.into()` at the call-site boundary in `pane/claw.rs` (the `From` impl lives in `crates/terminal/src/claw_proposal.rs` because `TerminalProposal` is local to that crate — orphan rule).

## Dependencies

```
boxxy-claw-widget
├── async-channel, parking_lot, frizbee, ignore, home
├── chrono, serde, serde_json, uuid
├── glib, gtk4, libadwaita
├── log, tokio
├── boxxy-sys-utils        (Tokio runtime & config dir)
├── boxxy-claw-protocol    (DTOs: ClawMessage, ClawEngineEvent, AgentStatus, CHARACTER_CACHE)
├── boxxy-claw-ui          (virtualized message list, shared character CSS & avatar caches)
├── boxxy-core-widgets     (AutocompleteController + CompletionProvider trait)
├── boxxy-db               (MsgHistory & ResumeCompletionProvider persistence)
├── boxxy-preferences      (read settings: enable_tips, overlay_font_scale, …)
├── boxxy-themes           (active mention token colors)
└── boxxy-viewer           (StructuredViewer for diagnosis rendering)
```

Verified via `cargo tree -p boxxy-claw-widget`: **no** `boxxy-claw`, **no** `boxxy-vte`, **no** `boxxy-agent`, **no** `boxxy-terminal` back-edges. The dep graph is acyclic and free of reasoning-engine SDKs.
