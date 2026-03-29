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
- **Task-Aware Closing Protection**: The `handle_close_request` dialog (in `update/window_state.rs`) prevents accidental loss of active work. It queries the `boxxy-agent` for running OS processes AND queries the `boxxy-claw` `WorkspaceRegistry` for active scheduled AI tasks (reminders/commands), displaying them all in a unified "Still Running" alert before closing a tab or window.
- **Task Indicators**: Automatically manages `adw::TabPage` indicators (e.g., displaying a `boxxy-running-symbolic` clock icon) when a tab contains an agent with pending scheduled tasks, driven by `TaskStatusChanged` events.
- **Multi-Window Support**: Native support for splitting tabs across multiple windows.
- **Advanced Sidebar**: Houses the AI Chat, Claw Logs, and Theme Selector using a unified `AdwOverlaySplitView`.
- **Global Context Propagation**: The window acts as the primary orchestrator for the **Global Workspace Radar**. It coordinates global intents and orchestration messages to all active terminal peers, regardless of their workspace. Note that Claw mode itself is active **per-terminal pane**, not globally per window. The window observes the active pane's state to update its UI.
- **Intelligent Default State**: Respects the `claw_on_by_default` setting during pane initialization. This ensures a "Lightweight First" experience while allowing power users to have full agentic assistance as their starting point for new terminals.
- **Global Event Bus**: Routes asynchronous background events (CWD changes, AI responses) to the correct UI components.
