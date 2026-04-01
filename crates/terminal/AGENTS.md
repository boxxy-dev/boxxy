# Terminal Crate (`boxxy-terminal`)

## Responsibility
Manages the terminal environment, including split-pane layouts, PTY integration, and the Boxxy-Claw agent UI. It wraps the headless `boxxy-vte` widget and provides high-level terminal features.

## Architecture
The crate uses a deeply modular structure to manage the complexity of terminal panes:

### `TerminalPaneComponent` (`pane/` module)
The leaf component representing a single terminal instance. Modularized into:
- **`pane/mod.rs`**: Main component entry and public API. Handles configuration updates and state synchronization (e.g. inactive pane dimming).
- **`pane/ui.rs`**: Core widget initialization using `gtk::Overlay`, `gtk::ScrolledWindow`, and the `SearchBarComponent`.
- **`pane/gestures.rs`**: Input handling, including middle-click paste, focus tracking, and context menu wiring. Right-click ownership logic lives entirely in `boxxy-vte`; this file registers a callback via `terminal.on_context_menu(...)` to receive the event only when the terminal (not the running app) owns the click.
- **`pane/events.rs`**: VTE signal wiring and PTY event routing.
- **pane/claw.rs**: Integration with the `boxxy-claw` actor model. Manages in-terminal popovers, status indicators, and handles structured `ToolResult` and `RestoreHistory` events for rendering a persistent debug log in the sidebar.
- **pane/preview.rs**: OSC 8 hyperlink media previews (hover/click detection).

- **Session Observability**: Each pane maintains an internal counter of cumulative token usage for its dedicated Claw agent. This data is synced to the global Claw sidebar, ensuring users can track the context cost of each terminal session independently.

### `TerminalComponent` (`component.rs`)
The container component representing a single Tab. 
- It uses a `gtk::Overlay` as its root widget to layer a `gtk::Picture` (for tab-wide background images) underneath a `gtk::Stack` (containing the split-pane tree).
- Manages the recursive split-pane tree (`gtk::Paned`). Handles focus navigation, pane spawning, and maximization logic.

## Key Features
- **Dynamic Splits**: Supports infinite vertical and horizontal terminal splits.
- **Per-Pane Claw State**: Claw activation, Proactive Mode, and Session Pinning are tracked **per-pane**. The `boxxy_msgbar::MsgBarComponent` triggered by `Ctrl+/` acts as the primary interface for this pane's agent. Sending a query automatically enables Claw mode for the pane if it was disabled. The pane provides rich autocompletion and maintains its own badge and UI state.
- **Seamless Background Images**: A single background image spans the entire tab seamlessly across all transparent terminal splits. 
- **Agent Badge Indicators**: Each pane features a color-coded `AgentBadge` representing its active AI session. This badge now includes a dynamic clock icon (`boxxy-running-symbolic`) that appears automatically when the agent has pending scheduled tasks, managed via `TaskStatusChanged` events.
- **Task Proposals**: Handles `TerminalProposal::Reminder` to display scheduled tasks directly in the terminal's interactive overlay popup, maintaining a consistent UI for all agent actions.
- **Agent Integration**: Seamlessly routes terminal context (CWD, snapshots) and real-time foreground process changes (via D-Bus signals) to the Claw agent. Explicitly manages tracking lifecycle to ensure zero overhead when Claw is disabled. Supports **Full State Restoration** upon session resumption, including visual history playback, terminal CWD synchronization, and task re-hydration.
- **Eviction Awareness**: The terminal UI proactively handles session eviction, displaying grayscale indicators and system messages when an agent is moved to a different pane.
- **Modern Hyperlinks**: Native OSC 8 support with robust media previews.
- **OSD Indicators**: Interactive overlays for terminal size and agent status.
