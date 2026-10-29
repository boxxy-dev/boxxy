# Msgbar Module (`boxxy_claw_widget::msgbar`)

## History
Formerly the standalone `boxxy-msgbar` crate. Folded into `boxxy-claw-widget` during the phase-2 extraction because the merged msgbar has exactly one consumer (the claw drawer) and a separate crate boundary no longer earned its keep.

## Responsibility
Provides `MsgBarComponent` — the input bar that lives inside the Claw overlay drawer's bottom area. Owns the entry, attachments, autocomplete, history navigation, Ctrl+V paste, and the four inline status toggles (`claw`, `sleep`, `pin`, `web_search`).

## Key Features

- **Embedded-in-drawer by design.** The old floating-over-cursor mode is gone; the bar always lives inside `TerminalOverlay`. `set_embedded(true)` disables the bar's self-hide on Enter/Escape so the drawer owns visibility. `is_active` stays latched while embedded — code outside this module (e.g. `TerminalPaneComponent::grab_focus`) checks `claw_popover.is_visible()` instead.
- **Auto-wrapping multi-line input & top attachment strip.** `AttachmentManager`'s `gtk::FlowBox` is placed in `msgbar_slot` directly above `.boxxy-msgbar` (hidden when empty so 0 attachments takes 0 height). When attachments are added via `Ctrl+V`, they render as compact pill tags (`Image ×`, `Text Snippet ×`) above the input bar. Inside `.boxxy-msgbar`, the left status/avatar badges are grouped in a box with `valign: Start` (always anchored to the first line), the `gtk::TextView` (`WrapMode::WordChar`) expands across the full width, and the sibling `send_btn` uses `valign: End` (anchored to the bottom/last line when multi-line).
- **Send button lives outside the bar.** `send_btn` is a public field on `MsgBarComponent` but is intentionally *not* appended to `widget`. The drawer appends it as a sibling into `msgbar_slot` so the bar renders as a single rounded field with the send icon floating next to it (no background). Click on `send_btn` invokes the shared submit closure alongside the `Enter` key path.
- **Dynamic Theming.** Styled by `crates/themes/build.rs` via the `.boxxy-msgbar` CSS class; inherits the active terminal palette's surface + foreground. Inside `.claw-drawer`, `resources/style.css` overrides the outer bar to be a rounded pill while the entry is transparent — giving the bar the field-shape and the send button zero chrome.
- **Unified Status Indicator.** The bot icon on the far left is both the Claw toggle and a real-time status monitor. Driven by `set_status(AgentStatus)`: Active / Sleep / Error / Off states map to the `status-active` / `status-sleep` / `status-error` CSS classes on the claw + sleep buttons.
- **Native Autocompletion.** `AutocompleteController` from `boxxy-core-widgets` with three providers sorted by trigger-length descending (`/resume ` must match before `/`):
  - `AgentCompletionProvider` — `@agent-name` — fetches live agents from `boxxy_claw::registry::workspace`.
  - `CommandCompletionProvider` — `/resume` ("Resume a past conversation"), `/models` ("Switch model"), `/exit` ("Withdraw character from terminal", disabled when no character is attached), and friends (slash commands rendered with aligned command names and dim subtitle descriptions).
  - `ResumeCompletionProvider` — `/resume <session>` — reads from an in-memory session cache refreshed asynchronously in the background (pre-warmed on `/` and refreshed with a 2s TTL) so `get_completions` never blocks the GTK main thread. Shows LLM-generated titles, agent-colored badges, relative timestamps (`10h`, `2d`); pinned sessions float to the top.
- **State Toggles.** `claw_toggle`, `sleep_toggle`, `pin_toggle`, `web_search_toggle`. Each fires the caller-supplied callback; per-pane state (`is_pinned`, `is_web_search`, `session_status`) is held on `TerminalPaneComponent` and synced back via `update_ui(status, pinned, web_search)`.
- **Persistent History.** `MsgHistory` loads asynchronously from `boxxy-db` (SQLite) via an explicit `HistoryState` state machine (`Uninitialized`, `Loading`, `Ready`, `Failed`) that merges any `pending_writes` submitted while loading. Capped at ~100 entries in memory, images pruned after 20 — keeps RAM flat even after thousands of sessions.

## Query Submission Flow
On Enter: build payload via `AttachmentManager::build_payload(entry_text)` → get `(text, Vec<base64_image>)` → push into `MsgHistory` → fire the caller's `on_submit` callback. The caller (pane code) wraps that as `ClawMessage::ClawQuery { query, snapshot, cwd, image_attachments }` and sends it via the agent channel. In embedded mode the bar does not self-hide — the drawer stays open through the agent's thinking phase so the user can follow up without reopening.
