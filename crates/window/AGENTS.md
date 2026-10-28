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
- **Task-Aware Closing Protection & Background Detach**: The `handle_close_request` dialog (in `update/window_state.rs`) and tab-close dialog (`update/tabs.rs`) prevent accidental loss of active work. They query `boxxy-agent` for running OS processes, query the `boxxy-claw` `WorkspaceRegistry` for active scheduled AI tasks, AND check `get_active_claw_panes()` for any active AI agent sessions in the tab/window, displaying a unified 3-button alert: **Cancel**, **Run in Background** (detaches running shells and active AI sessions into `boxxy-agent` via `detach_on_close_sync()`), and **Terminate** / **Terminate All** (kills the process group via `terminate_on_close_sync()`). When the "Always Keep Running Processes in Background" (`pty_persistence`) setting is on, the dialog is skipped and running shells / active AI sessions automatically detach into the daemon while idle shells are terminated cleanly.
- **HeaderBar Background Shells Badge**: Whenever one or more detached shells are running in `boxxy-agent`, a flat terminal badge (`detached_btn`) with the active count appears in the HeaderBar next to the menu button. Clicking the HeaderBar badge opens the unified **Background Shells** `adw::AlertDialog` listing each background shell with **Reattach**, **Terminate**, and **Restore All**.
- **Credential Sync on Save**: Whenever the user saves preferences, `settings_changed` pushes `settings.get_effective_api_keys()` to the daemon via `update_credentials` so the daemon's in-memory state tracks the on-disk file without waiting for the engine's disk-fallback path.
- **Task Indicators**: Automatically manages `adw::TabPage` indicators (e.g., displaying a `boxxy-running-symbolic` clock icon) when a tab contains an agent with pending scheduled tasks, driven by `TaskStatusChanged` events.
- **Multi-Window Support**: Native support for splitting tabs across multiple windows.
- **Advanced Sidebar**: Houses the AI Chat, Claw Logs, and Theme Selector using a unified `AdwOverlaySplitView`.
- **Frosted Glass**: `widgets/frosted_glass.rs` wraps the entire split view, emits a padded copy/blur/paste backdrop marker (radius 40 for GTK 4.25 compatibility), then paints a separate flat tint. Native `AdwApplicationWindow` clipping owns window corners and CSD geometry; do not add sidebar-dependent corner radii or blur the tint itself.
- **Unified Notification Pipeline & Post-Update "What's New" Dialog**: Manages both local in-app toasts (`ShowToast`) and native OS desktop alerts (`PushGlobalNotification`), using `gtk4::gio::Notification` to ensure proper delivery through Flatpak portals while intelligently suppressing desktop alerts when the window is focused. Update notifications show `"There is a new Boxxy version available"` in the toast and link to the GitHub compare view (`Current Commit` -> `New Commit`) in the details dialog. The first launch after an update consumes `~/.local/boxxy-terminal/.whats_new_pending` (or `BOXXY_SIMULATE_UPDATE="commit_a..commit_b"`) to present the "What's New in Boxxy" `adw::Dialog` with zero-diff GitHub compare stats (commits, files, lines, and contributor avatars), an interactive commit list that pages the full dialog to show the complete commit message and GitHub commit link, **Read What's New** (starts the Claw overlay with the default character on an idle pane; disabled if no Claw model is configured), and **Ask Another AI**.
- **Global Context Propagation**: The window acts as the primary orchestrator for the **Global Workspace Radar**. It coordinates global intents and orchestration messages to all active terminal peers, regardless of their workspace. Note that Claw mode itself is active **per-terminal pane**, not globally per window. The window observes the active pane's state to update its UI.
- **Intelligent Default State**: Ensures a "Lightweight First" experience while allowing power users to have full agentic assistance by manually activating the agent. New panes start with the agent in an inactive state.
- **Global Event Bus**: Routes asynchronous background events (CWD changes, AI responses) to the correct UI components.
