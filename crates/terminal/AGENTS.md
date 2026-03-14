# Terminal Crate (`boxxy-terminal`)

## Responsibility
Manages the terminal environment, including split-pane layouts, PTY integration, and the Boxxy-Claw agent UI. It wraps the headless `boxxy-vte` widget and provides high-level terminal features.

## Architecture
The crate uses a deeply modular structure to manage the complexity of terminal panes:

### `TerminalPaneComponent` (`pane/` module)
The leaf component representing a single terminal instance. Modularized into:
- **`pane/mod.rs`**: Main component entry and public API.
- **`pane/ui.rs`**: Core widget initialization (Overlays, ScrolledWindow, SearchBar).
- **`pane/gestures.rs`**: Input handling, including middle-click paste, context menus, and focus tracking.
- **`pane/events.rs`**: VTE signal wiring and PTY event routing.
- **`pane/claw.rs`**: Integration with the `boxxy-claw` actor model (popovers, indicators, event loops).
- **`pane/preview.rs`**: OSC 8 hyperlink media previews (hover/click detection).

### `TerminalComponent` (`component.rs`)
The container component that manages the recursive split-pane tree (`gtk::Paned`). Handles focus navigation, pane spawning, and maximization logic.

## Key Features
- **Dynamic Splits**: Supports infinite vertical and horizontal terminal splits.
- **Agent Integration**: Seamlessly routes terminal context (CWD, snapshots) to the Claw agent.
- **Modern Hyperlinks**: Native OSC 8 support with robust media previews.
- **OSD Indicators**: Interactive overlays for terminal size and agent status.
