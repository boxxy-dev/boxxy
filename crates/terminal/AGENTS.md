# Terminal Crate (`boxxy-terminal`)

## Responsibility
Manages the terminal environment: split-pane layouts, PTY integration, session resume, and the per-pane bridge to the Claw agent. Wraps the headless `boxxy-vte` widget.

**What does _not_ live here**: the Claw drawer UI (overlay, indicator, merged msgbar, event dispatch, `ClawHost` trait, `Proposal` enum). That surface lives in `boxxy-claw-widget`. This crate supplies `PaneClawHost` — a thin adapter that implements `ClawHost` against a `PaneInner` + `claw_sender` + `callback` — so the widget can drive the terminal without depending on `TerminalWidget` or `PaneInner`.

## Architecture

### `TerminalPaneComponent` (`pane/` module)
The leaf component representing a single terminal instance. Modularized into:
- **`pane/mod.rs`**: Main component entry and public API. Constructs the pane's `ClawIndicator` (from `boxxy-claw-widget`), the merged `MsgBarComponent`, and the D-Bus agent session. Registers the `Ctrl+/` shortcut to reveal the drawer. Handles configuration updates and state synchronization (e.g. inactive pane dimming). Character claiming is lazy: on the first Claw activation the pane calls `agent_manager::create_claw_session_with_sender(pane_id, char_id, event_tx)`, registering its event channel with the centralized `AgentConnection` signal router and storing the returned RAII `SubscriptionGuard` on `PaneInner`. On `PaneInner::drop`, per-pane channels (`claw_event_tx`, `claw_msg_tx`) are explicitly closed and all `SubscriptionGuard`s are dropped to immediately terminate background tasks and free Claw widgets.
- **`agent_manager.rs`**: Houses `AgentManager`, `AgentConnection`, `SignalRouter<K, V>`, and `SubscriptionGuard`. Multiplexes D-Bus signals (`Exited`, `ForegroundProcessChanged`, `ClawEvent`, `ClaimsChanged`, `DesktopNotification`) over single process-wide session-bus streams ($O(1)$ D-Bus match rules) and reconciles state on daemon reconnect.
- **`pane/claw.rs`**: ~130-line orchestrator. Builds a `PaneClawHost`, constructs `TerminalOverlay` (drawer) from the widget crate, wires the three `ClawIndicator` click callbacks, and hands the agent event channel off to `boxxy_claw_widget::spawn_dispatch(…)`. Returns the overlay + the `pending_proactive_diagnosis` cell so higher-level code can push diagnoses to the indicator button.
- **`pane/claw_host.rs`**: `PaneClawHost` — the only implementor of `ClawHost` today. Owns a `Weak<RefCell<PaneInner>>`, the agent `claw_sender`, the upstream `PaneOutput` callback, and the stable pane id. Translates the trait methods into concrete actions: `inject_line` → `terminal.write_all(bytes + '\r')`; `execute_script` → write to `$XDG_CONFIG_HOME/boxxy-terminal/cache/bookmarks/runs/<uuid>.sh`, `chmod +x`, inject the path; `cd` → busy-check + path-exists check + inject-or-notify; `snapshot` → `terminal.get_text_snapshot(…)`; `forward_event` → `PaneOutput::ClawEvent`; etc.
- **`pane/ui.rs`**: Core widget initialization using `gtk::Overlay`, `gtk::ScrolledWindow`, and the `SearchBarComponent`.
- **`pane/gestures.rs`**: Input handling (middle-click paste, focus tracking, context menu wiring). Right-click ownership lives in `boxxy-vte`; this file listens via `terminal.on_context_menu(…)`.
- **`pane/events.rs`**: VTE signal wiring and PTY event routing, including OSC 133 → `ClawMessage::CommandFinished` bridge.
- **`pane/preview.rs`**: OSC 8 hyperlink media previews (hover/click detection).
- **`claw_proposal.rs`** (crate root): `From<TerminalProposal> for boxxy_claw_widget::Proposal`. The single type-boundary conversion. The enum itself lives in the widget crate; the `From` impl stays here because `TerminalProposal` is local.

### `TerminalComponent` (`component.rs`)
The container component representing a single Tab.
- Uses a `gtk::Overlay` as its root widget to layer a `gtk::Picture` (for tab-wide background images) underneath a `gtk::Stack` (the split-pane tree).
- Manages the recursive split-pane tree (`gtk::Paned`). Handles focus navigation, pane spawning, and maximization.

## Key Features
- **Dynamic Splits**: Infinite vertical and horizontal terminal splits.
- **Per-Pane Claw State**: Claw activation, Proactive Mode, Session Pinning, and Web Search permissions are tracked **per-pane** via `Rc<Cell<bool>>` / `Rc<RefCell<AgentStatus>>` held on `TerminalPaneComponent` and passed to `spawn_dispatch`. Sending a query via the merged msgbar (inside the drawer) auto-enables Claw mode if it was off.
- **Ctrl+/ opens the full drawer** (not a floating bar). The shortcut refreshes the toggle visuals via `msg_bar.update_ui(…)` and calls `claw_popover.show_input_only(&agent_name)` — never hides.
- **Seamless Background Images**: A single background image spans the entire tab across all transparent terminal splits.
- **Agent Integration**: Routes terminal context (CWD, snapshots) and real-time foreground process changes (via D-Bus signals) to the Claw agent through the `ClawHost` trait. Tracking is lifecycle-managed so there's zero overhead when Claw is off. Supports **Full State Restoration** on session resumption (visual history, CWD sync, task re-hydration).
- **Session Observability**: Each pane maintains an internal `SessionUsage` state cell (`context_tokens`, `context_window`, `total_tokens`, `cost_usd`). The `dispatch` loop writes to it; `TerminalPaneComponent::get_session_usage()` reads it; the sidebar renders it.
- **Persistent-Shells Dispatch (Experimental)**: `PaneInner::drop` reads the live `pty_persistence` setting. Off → `signal_process_group(pid, SIGTERM)` kills the shell + its children. On → `set_persistence(pid, true)` + `detach(pid)` hands the shell to the daemon. Read at close-time so toggling takes effect on the very next close.
- **Eviction Awareness**: The dispatch loop reacts to `ClawEngineEvent::Evicted` by flipping the indicator to grayscale and pushing a system message into the sidebar log.
- **Modern Hyperlinks**: Native OSC 8 support with robust media previews.
- **OSD Indicators**: Interactive overlays for terminal size and agent status.
