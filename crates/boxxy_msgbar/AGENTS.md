# Message Bar Crate (`boxxy-msgbar`)

## Responsibility
Provides the `MsgBarComponent`, a seamless GTK input overlay that allows users to interact with Boxxy-Claw. It serves as the primary entry point for sending queries to agents globally via the `Ctrl+/` shortcut.

## Key Features

- **Inline Positioning:** Uses `boxxy_vte`'s `get_cursor_rect()` to position itself precisely over the active terminal cursor, matching the prompt's Y-coordinate.
- **Dynamic Theming:** Deeply integrated into Boxxy's dynamic CSS engine (`crates/themes/build.rs`). It automatically inherits the background and foreground colors of the currently active terminal palette, applying the exact `{surface}` background for subtle contrast.
- **Native Autocompletion**: Provides a robust GTK-based autocomplete system (`AutocompleteController`) built specifically for this input. It features an IDE-like dropdown (styled after GNOME Builder) that natively fetches live agent names (`@agent`) from the `WorkspaceRegistry` and past sessions (`/resume`) from `boxxy-db`. Pinned sessions are highlighted with a specialized icon and kept at the top.
- **State Toggles**: Includes native interactive buttons for toggling **Claw Mode**, **Proactive/Lazy Diagnosis**, and **Session Pinning**. These states are synchronized in real-time with the active terminal pane and its background agent.
- **Persistent History**: Implements a highly efficient, lazy-loaded history system (`MsgHistory`). It seamlessly remembers prompts and their rich attachments (images/text) using `boxxy-db` (SQLite). To remain "Lightweight First," history is only read from the database on the very first up/down navigation. In-memory payloads are strictly capped (e.g., maximum 100 items, with images pruned after 20) to prevent RAM bloating.

## Architecture
This crate is designed to be instantiated by the `TerminalPaneComponent` and layered securely over the `TerminalWidget` inside a GTK Overlay. When a query is submitted (via Enter), it triggers a callback that grabs the terminal snapshot and dispatches a `ClawMessage::ClawQuery` to the background agent actor. Behind the scenes, the query and its attachments are also asynchronously saved to the `boxxy-db` SQLite history table.