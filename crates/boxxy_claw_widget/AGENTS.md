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
│   ├── mod.rs         MsgBarComponent constructor + status / update_ui
│   ├── attachment.rs  clipboard paste → image/text chips
│   ├── autocomplete.rs @agent / /resume completion providers
│   └── history.rs     lazy-loaded MsgHistory backed by boxxy-db
├── dispatch.rs        spawn_dispatch(rx, host, overlay, …) event loop
├── claw_host.rs       ClawHost trait — the decoupling boundary
├── proposal.rs        Proposal enum (UI shape)
└── lib.rs             re-exports the public surface
```

### The `ClawHost` trait (`claw_host.rs`)
The single boundary between widget code and surface-specific code. Ten methods: `host_id`, `inject_line`, `execute_script`, `snapshot` (async), `grab_focus`, `set_focusable`, `is_busy`, `working_dir`, `send_claw`, `focus_sidebar`, `cd`, `notify`, `forward_event`. The trait is `!Send` / `!Sync` — GTK widgets are single-threaded and every call happens on the main thread; a `Send` bound would be a lie and would force `Arc<Mutex<…>>` ceremony in impls.

### The drawer (`overlay.rs`)
Defines `TerminalOverlay` and `OverlayMode { Claw, Bookmark }`. Constructed via `TerminalOverlay::new(indicator_widget, msg_bar, host)`. The `.ui` file wires most of the layout; Rust code attaches the `ClawIndicator::widget()` into the header, appends `msg_bar.widget` + `msg_bar.send_btn` into the `msgbar_slot`, and stitches up button handlers (each one is two lines of `host.xyz()`).

- `show_chat_only(agent_name)` — opens the drawer in an empty-proposal state (used when the agent starts thinking before producing output).
- `show_input_only(agent_name)` — the `Ctrl+/` entry point. Reveals the drawer if closed; always focuses the input; never hides. The Okay/Reject/Accept buttons are the explicit dismiss paths — silent hiding would strand the user when the agent is waiting for a message.
- `enter_waiting_state()` — collapses proposal controls so the drawer reads as "thinking…" after the user sends a reply.
- `set_history_mode(enabled)` — toggles between the single-message view and a full scrollable conversation log (user preference `maintain_overlay_history`).

### History list + the GTK4 `ListView` virtualization gotcha
The drawer's scrollable history is a `gtk::ListView` over a `gio::ListStore`. Virtualization means `adj.upper()` reflects only the *realized* portion of the list — so snapping to "bottom" via `set_value(upper - page_size)` lands on a false bottom (scrollbar looks at bottom, but unmeasured rows are still below). This is [GNOME/gtk#2971](https://gitlab.gnome.org/GNOME/gtk/-/issues/2971), partially fixed, partially live.

The pragmatic workaround: arm a `Cell<u32>` counter on `items_changed` (+ on `scroll_to_latest()`), then retry `ListView::scroll_to(last, FOCUS, None)` + `adj.set_value(upper - page_size)` across ~10 frames via `glib::timeout_add_local`. Each tick lets more rows realize / markdown re-measure; by frame 10 the layout has settled at the true bottom.

### The merged msgbar (`msgbar/`)
Formerly the `boxxy-msgbar` crate — folded in during phase 2. The `MsgBarComponent` is a horizontal bar with the 4 status toggles on the left (`claw`, `sleep`, `pin`, `web_search`) and the entry on the right. The send button is deliberately **not** a child of the bar — the drawer appends it as a sibling into `msgbar_slot`, so the bar renders as a single rounded field and the send icon floats alongside with no background.

- **Embedded flag**: `msg_bar.set_embedded(true)` suppresses the bar's self-hide on Enter / Escape. The drawer owns visibility; the bar stays visible as long as the drawer is revealed. `is_active` stays latched while embedded.
- **Attachments**: `AttachmentManager` intercepts Ctrl+V to convert long text / images into chips above the input. `build_payload(base_text)` merges everything into `(text, Vec<base64_image>)` for the agent.
- **Autocomplete**: three providers sorted by trigger-length descending — `AgentCompletionProvider` (@agent, fetched from `boxxy_claw::registry::workspace`), `CommandCompletionProvider` (/resume etc.), `ResumeCompletionProvider` (lists recent sessions from the DB with LLM-generated titles + relative timestamps + agent colors).
- **History**: `MsgHistory` — lazily loaded from `boxxy-db` on the first up/down key. Capped at ~100 entries in memory, images pruned after 20, to keep RAM flat even after thousands of sessions.

### The `ClawIndicator` (`claw_indicator.rs`)
Two widgets in one: a small floating badge (over the terminal, shows the agent's name + color) and a detailed pill (inside the drawer header, shows "Thinking…" / "Diagnosis Ready" / "Error Detected" with action buttons). It dynamically updates to reflect the active tool during execution using a curated list of pretty names and matching icons, keeping the user informed of the agent's real-time actions.

**Quiet-by-default policy**: the badge starts with its revealer retracted. It only appears once `set_identity(name)` arrives with a non-empty name. `set_mode(AgentStatus)` only drives status icons + color tint; it never writes placeholder labels like "CLAW" / "WORKING". Dormant panes show nothing.

### The dispatch loop (`dispatch.rs`)
`spawn_dispatch(rx, host, overlay, indicator, msg_bar, sidebar_store, id, session_status, is_pinned, is_web_search, agent_name, total_tokens)` — a single `while let Ok(event) = rx.recv().await` that matches every `ClawEngineEvent` variant. Each branch either mutates widget UI, appends rows to both the sidebar store and the overlay's history store (when `history_mode()` is on), or delegates to the host. Crucially, it acts as a dual-store router: it maintains the full unfiltered history in the `sidebar_store` for debugging, while applying a `is_overlay_visible()` filter to keep noisy `ToolResult` blocks and raw process lists out of the `overlay_store`, ensuring the user's primary view remains clean and focused. Every event, including ones with no widget reaction (`RequestSpawnAgent`, `InjectKeystrokes`), is forwarded via `host.forward_event(event)` at the tail so non-UI consumers (tab badges, swarm router) see the full stream.

**User-typed error handling.** On `LazyErrorIndicator`, dispatch calls `indicator.show_lazy_error()` — a small red "Error Detected" pill appears at the pane corner. **No drawer opens, no LLM call.** The user clicks "Diagnose" on the pill, which fires the indicator's `on_lazy_click` → the pane sends `ClawMessage::RequestLazyDiagnosis` → engine drains its stashed prompt and runs the diagnosis pipeline → next `DiagnosisComplete` opens the drawer normally. Agent-originated command failures (the accept → error loop) still auto-diagnose via the engine's proactive path, emerging as `DiagnosisComplete` directly. The dispatch code doesn't need to know which mode it's in — origin is the engine's concern.

### The neutral `Proposal` enum (`proposal.rs`)
UI-shaped, host-agnostic variants: `None`, `Command(String)`, `Bookmark { filename, script, placeholders }`, `FileWrite { path, content }`, `FileDelete { path }`, `KillProcess { pid, name }`, `GetClipboard`, `SetClipboard(String)`. The terminal's own `TerminalProposal` converts via `.into()` at the call-site boundary in `pane/claw.rs` (the `From` impl lives in `crates/terminal/src/claw_proposal.rs` because `TerminalProposal` is local to that crate — orphan rule).

## Dependencies

```
boxxy-claw-widget
├── async-channel
├── base64, chrono, serde, serde_json, lazy_static, uuid (msgbar transitives)
├── glib, gtk4
├── log, tokio
├── boxxy-ai-core          (autocomplete fetches live agents)
├── boxxy-claw             (agent registry for @agent completion)
├── boxxy-claw-protocol    (DTOs: ClawMessage, ClawEngineEvent, AgentStatus)
├── boxxy-claw-ui          (sidebar row helpers; same factory reused here)
├── boxxy-core-widgets     (AutocompleteController + CompletionProvider trait)
├── boxxy-db               (MsgHistory persistence)
├── boxxy-preferences      (read settings: maintain_overlay_history, …)
└── boxxy-viewer           (StructuredViewer for diagnosis rendering)
```

Verified via `cargo tree -p boxxy-claw-widget`: **no** `boxxy-vte`, **no** `boxxy-agent`, **no** `boxxy-terminal` back-edges. The dep graph is acyclic.
