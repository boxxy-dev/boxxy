# Window Crate (`boxxy-window`)

## Responsibility
Acts as the main UI orchestrator for Boxxy-Terminal. It manages the application window, sidebar, and high-level routing of messages between components.

## Architecture
This crate follows a modular **Model-View-Update (MVU)** architecture:

- **`state.rs`**: Central state definition (`AppWindowInner`) and the `AppInput` message enum.
- **`ui.rs`**: GTK widget tree construction and signal-to-message mapping.
- **`update/` Module**: Pure state transition logic:
    - **`update/mod.rs`**: Primary message dispatcher.
    - **`update/tabs.rs`**: Tab lifecycle (open, close, adopt, transfer).
    - **`update/split.rs`**: Split management and layout control.
    - **`update/events.rs`**: Terminal signal routing and agent event handling.
    - **`update/window_state.rs`**: Persistence and global state updates.

## Key Features
- **Multi-Window Support**: Native support for splitting tabs across multiple windows.
- **Advanced Sidebar**: Houses the AI Chat, Claw Logs, and Theme Selector using a unified `AdwOverlaySplitView`.
- **Global Context Propagation**: The window acts as the primary orchestrator for the **Global Workspace Radar**. It manages the state of active agents across all tabs and ensures that global intents and orchestration messages are correctly routed to all peers, regardless of their workspace.
- **Intelligent Default State**: Respects the `claw_on_by_default` setting during window and tab initialization. This ensures a "Lightweight First" experience while allowing power users to toggle full agentic assistance as their starting point.
- **Global Event Bus**: Routes asynchronous background events (CWD changes, AI responses) to the correct UI components.
