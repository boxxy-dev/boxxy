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
- **Task-Aware Closing Protection**: The `handle_close_request` dialog (in `update/window_state.rs`) prevents accidental loss of active work. It queries the `boxxy-agent` for running OS processes AND queries the `boxxy-claw` `WorkspaceRegistry` for active scheduled AI tasks (reminders/commands), displaying them all in a unified "Still Running" alert before closing a tab or window. When the `pty_persistence` setting is on the dialog is skipped entirely — shells detach into the daemon via `PaneInner::drop` and AI tasks continue running with desktop-notification delivery on completion, so there's nothing to warn about.
- **Credential Sync on Save**: Whenever the user saves preferences, `settings_changed` pushes `settings.get_effective_api_keys()` to the daemon via `update_credentials` so the daemon's in-memory state tracks the on-disk file without waiting for the engine's disk-fallback path.
- **Task Indicators**: Automatically manages `adw::TabPage` indicators (e.g., displaying a `boxxy-running-symbolic` clock icon) when a tab contains an agent with pending scheduled tasks, driven by `TaskStatusChanged` events.
- **Multi-Window Support**: Native support for splitting tabs across multiple windows.
- **Advanced Sidebar**: Houses the AI Chat, Claw Logs, and Theme Selector using a unified `AdwOverlaySplitView`.
- **Unified Notification Pipeline**: Manages both local in-app toasts (`ShowToast`) and native OS desktop alerts (`PushGlobalNotification`), using `gtk4::gio::Notification` to ensure proper delivery through Flatpak portals while intelligently suppressing desktop alerts when the window is focused.
- **Global Context Propagation**: The window acts as the primary orchestrator for the **Global Workspace Radar**. It coordinates global intents and orchestration messages to all active terminal peers, regardless of their workspace. Note that Claw mode itself is active **per-terminal pane**, not globally per window. The window observes the active pane's state to update its UI.
- **Intelligent Default State**: Ensures a "Lightweight First" experience while allowing power users to have full agentic assistance by manually activating the agent. New panes start with the agent in an inactive state.
- **Global Event Bus**: Routes asynchronous background events (CWD changes, AI responses) to the correct UI components.
