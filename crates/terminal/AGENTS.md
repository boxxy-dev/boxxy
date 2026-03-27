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
- **`pane/claw.rs`**: Integration with the `boxxy-claw` actor model. Manages in-terminal popovers, status indicators, and handles structured `ToolResult` events for rendering a read-only debug log in the sidebar.
- **pane/preview.rs**: OSC 8 hyperlink media previews (hover/click detection).

- **Session Observability**: Each pane maintains an internal counter of cumulative token usage for its dedicated Claw agent. This data is synced to the global Claw sidebar, ensuring users can track the context cost of each terminal session independently.

### `TerminalComponent` (`component.rs`)
The container component representing a single Tab. 
- It uses a `gtk::Overlay` as its root widget to layer a `gtk::Picture` (for tab-wide background images) underneath a `gtk::Stack` (containing the split-pane tree).
- Manages the recursive split-pane tree (`gtk::Paned`). Handles focus navigation, pane spawning, and maximization logic.

## Key Features
- **Dynamic Splits**: Supports infinite vertical and horizontal terminal splits.
- **MsgBar Input**: Uses `boxxy_msgbar::MsgBarComponent` triggered by `Ctrl+/` to provide an inline, native GTK input directly over the active terminal cursor. This is the primary method for sending queries securely to the agent, providing rich autocompletion.
- **Seamless Background Images**: A single background image spans the entire tab seamlessly across all transparent terminal splits. 
- **Agent Integration**: Seamlessly routes terminal context (CWD, snapshots) and real-time foreground process changes (via D-Bus signals) to the Claw agent. Explicitly manages tracking lifecycle to ensure zero overhead when Claw is disabled.
- **Modern Hyperlinks**: Native OSC 8 support with robust media previews.
- **OSD Indicators**: Interactive overlays for terminal size and agent status.
