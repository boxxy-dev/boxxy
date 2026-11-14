# Claw UI Crate (`boxxy-claw-ui`)

## Role

Every GTK widget that renders into the "Claw" page of the right-hand sidebar and the virtualized conversation history list lives here. The crate is the glue between `boxxy-claw-protocol`'s DTOs and GTK4/libadwaita rendering.

Two direct consumers:
- `boxxy-window` embeds `ClawSidebarComponent` as one page of its right-hand `ViewStack`.
- `boxxy-claw-widget` (`overlay.rs` and `dispatch.rs`) uses `create_claw_message_list()`, `get_claw_viewer_registry()`, `ClawRowObject`, and the `add_*_row` helpers to render the per-pane conversation history.

## Structure

- **`sidebar/` (`mod.rs`, `tasks.rs`, `past_sessions.rs`)** — outer window-scoped `ClawSidebarComponent` (one per `AppWindow`, retargeted via `set_pane_state(agent_name, sleep, pinned, web_search_enabled, vibe_enabled)`):
  - **`sidebar/mod.rs`** — arranges active character sessions into a 3-row vertical layout: Row 1 (Top, `vexpand = false`) anchors the character hero/identity header (avatar with sleep/vibe badges, colored character name, duties, `Vibe • Sleep • Pin • Web`, and deduplicated memory/tool counters); Row 2 (Middle, `vexpand = true`) hosts `TasksSection`; Row 3 (Bottom, `vexpand = false`) anchors the `Models`, `Context`, and `MCP` panels.
  - **`sidebar/tasks.rs`** — `TasksSection`: occupies the middle sidebar zone (`vexpand = true`) during active non-incognito character sessions (centering the slim `"No pending tasks"` / `"Ask {character_name} to set a reminder"` card when empty, and filling the middle region with a full-height scrollable task list when populated). Supports both compact single-shot reminder/task cards and dedicated **Multi-Round Interval Cards** (`.claw-round-card` for boxing laps, Pomodoro, HIIT, and study blocks, featuring `ROUND X/N` / `BREAK` phase pills, visual round pip capsules, a live `MM:SS left` stopwatch countdown, and a live `GtkProgressBar`), plus 1-click `×` dismissal and `"Dismiss all"`.
  - **`sidebar/past_sessions.rs`** — borderless `.navigation-sidebar` **Past Conversations** resume list shown on blank panes (`agent_name` empty), sorting pinned conversations first, tracking live `CharacterRegistry` claims, and offering Pin/Unpin, Continue with…, and Delete options.
- **`message_list.rs`** — `create_claw_message_list()`, `format_exchange_for_copy()`, and read-only `add_*_row` helpers. Shares a single cached `Rc<ViewerRegistry>` across all recycled `SignalListItemFactory` slots, stores slot widgets via `boxxy_core_widgets::ObjectExtSafe`, borrows row payloads zero-copy via `ClawRowObject::with_row`, recycles child blocks via `StructuredViewer::park_for_recycle()` on `connect_unbind`, and drives typewriter reveals via incremental `set_streaming_content`.
- **`character_style.rs`** — shared character display-name formatting, Pango/CSS hex-color sanitization, deduplicated display-level CSS color classes (`avatar_ring_color_class` and `character_btn_color_class` backed by a single `GtkCssProvider` so scrolling never triggers global CSS cascade invalidations), and mtime-aware `gdk::Texture` avatar caching (2s stat TTL) so `ListView` scrolling and pane switches avoid repeated synchronous disk PNG decoding on the GTK main thread.
- **`process_renderer.rs`** — `ProcessListRenderer` custom `BlockRenderer` for `list_processes` payloads (supporting both structured `ProcessSample` and legacy tuple JSON schemas) and thread-cached `get_claw_viewer_registry()`.
- **`row_object.rs`** — `ClawRowObject` GObject wrapper around `PersistentClawRow` with zero-clone `with_row` access and lazily cached `FileDiff` JSON `Arc<str>`.

## Visual Language

Each virtualized history row shares the same layout (avatar/icon + heading + dim status label + `StructuredViewer` + optional monospace command line) with per-variant styling:

| Row variant | Icon/Avatar | Heading | Extra styling |
|---|---|---|---|
| SystemMessage | — (hidden) | — | `system-message`, `dimmed` |
| User | User Avatar | User Display Name | Optional image attachment thumbnails |
| Diagnosis | Character Avatar | Character Name | Optional typewriter reveal; fallback to `boxxyclaw` icon |
| Suggested | Character Avatar | Character Name | Fallback to `boxxy-dialog-warning-symbolic`; command in monospace |
| ProcessList | Character Avatar | Character Name | Custom `ProcessListRenderer` table |
| ToolCall | Character Avatar | `Used tool: {name}` | Compact single-row label |
| Command | User Avatar | User Display Name | `"Command Execution"` / `"Command Failed"` subtitle |
| FileDiff | Character Avatar | Character Name | `file_diff` / `file_diff_compact` block |

## Dependencies

`boxxy-claw-protocol` (DTOs), `boxxy-db` (past-session & memory count queries), `boxxy-preferences` (settings & user profile), `boxxy-sys-utils` (async Tokio runtime), `boxxy-viewer` (`StructuredViewer` + renderer trait), `boxxy-core-widgets` (`ObjectExtSafe`), `gtk4`, `libadwaita`, `serde`, `serde_json`, `uuid`, `chrono`. Explicitly **not** `boxxy-claw` or `rig` — the UI crate has zero dependency on LLM provider SDKs or reasoning internals.
