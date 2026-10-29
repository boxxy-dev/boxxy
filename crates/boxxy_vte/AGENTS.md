# boxxy-vte — Headless Terminal Emulator Widget

## Overview

`boxxy-vte` is a 100% Rust, headless GTK4 terminal emulator widget. It features a fully internalized, async-first ANSI state machine and terminal engine (absorbed from an upstream project and customized). It replaces the C-based `libvte` with a custom `gtk4::Widget` subclass that renders the terminal grid directly via GTK4's GSK scene graph (`snapshot()`).

The crate exposes a single public widget — `TerminalWidget` — that provides a lock-free, double-buffered rendering model for silky smooth performance.

---

## Architecture

```
┌─────────────────────────────────────────┐
│            TerminalWidget               │  ← public GObject wrapper (mod.rs)
│  (ObjectSubclass<imp::TerminalWidget>)  │
│  @implements gtk::Scrollable            │
└───────────────────┬─────────────────────┘
                    │ sends messages to
        ┌───────────▼────────────┐
        │      EventLoop         │  ← engine/event_loop.rs
        │  (PTY reader/parser)   │  ← Background Tokio Task
        │  Owns Term state       │
        └───────────┬────────────┘
                    │ pushes RenderState
        ┌───────────▼────────────┐
        │      RenderState       │  ← Lock-free snapshot
        │      (ArcSwap)         │  ← engine/sync.rs
        └────────────────────────┘
```

### Internalized ANSI Engine

The core terminal logic is located in `src/engine/`, with `src/engine/term/` organized into focused modules (`mod.rs`, `render_state.rs`, `modes.rs`, `cursor.rs`, `buffer.rs`, `handler.rs`, `cell.rs`, `color.rs`, `search.rs`, `tests.rs`) and the GTK widget implementation in `src/terminal/imp/` (`mod.rs`, `painter.rs`, `gestures.rs`, `kitty.rs`). We have fully absorbed and refactored the terminal engine to be:
1. **Asynchronous:** Uses `tokio::io::unix::AsyncFd` for non-blocking PTY I/O.
2. **Lock-Free:** The UI thread never blocks on the PTY thread. Instead, it reads a cheap, atomic snapshot of the visible grid (`RenderState`) via `arc-swap`.
3. **Event-Driven CWD:** Natively parses **OSC 7** sequences to provide instantaneous directory tracking that works over SSH and through Flatpak sandboxes.

### `src/terminal/mod.rs` — Public API

The `TerminalWidget` GObject wrapper.

**Key public methods:**

| Method | Description |
|---|---|
| `new()` | Construct an empty terminal widget. |
| `attach_pty(master_fd)` | Wrap a provided PTY master file descriptor and start the async event loop. |
| `set_vadjustment(adj)` | Connect a `gtk::Adjustment` for vertical scrolling. |
| `set_font(desc)` | Set the Pango font; triggers a resize recalculation. |
| `set_colors(...)` | Apply a 16-colour ANSI palette + default fg/bg. |
| `on_cwd_changed(f)` | Register a callback for OSC 7 (primary) and /proc (fallback) CWD updates. |
| `on_title_changed(f)` | Fired on OSC 0/2 title events. |
| `on_bell(f)` / `on_exit(f)` | Fired on terminal bell and child process exit. |
| `on_context_menu(f)` | Register a callback fired when the terminal decides a right-click belongs to the terminal (not the running app). Receives `(x, y)` in widget coordinates. |
| `copy_clipboard()` | Copies the full current selection (including off-screen scrollback history) to the clipboard via `RenderState::selection_text()` and `Msg::CopySelection`. |
| `select_all()` | Selects all non-empty scrollback and viewport lines (`Msg::SelectAll`) without appending trailing blank rows below the cursor. |
| `clear_scrollback()` / `reset_terminal()` | Clears scrollback history (`CSI 3 J`) or performs a full terminal state reset (`RIS`, clearing stuck mouse modes and alternate screen). |
| `dump_buffer(include_scrollback)` | Asynchronously extracts the visible screen or full scrollback + screen plain text. |
| `paste_clipboard()` / `paste_primary()` | Paste from clipboard/primary selection with automatic bracketed-paste wrapping (`\x1b[200~...\x1b[201~`) when the app has enabled `BRACKETED_PASTE` mode. |
| `is_mouse_mode()` | Returns `true` if the running app has enabled any mouse-reporting mode (1000/1002/1003). |

---

## Threading & Synchronization

- **GTK Main Thread:** Handles rendering (`snapshot`), input events, and UI state. It consumes a `RenderState` snapshot produced by the background thread.
- **Tokio Background Task:** Runs the `EventLoop`. It reads from the PTY, updates the `Term` state machine, and pushes `RenderState` snapshots to the UI thread using a dual-priority scheme (`RenderPriority`):
  - **`Immediate`:** User-driven interactions (mouse selection, viewport scrolling, search, resize, process exit) publish a snapshot immediately with zero throttle latency.
  - **`Coalesced`:** High-throughput PTY parser updates are coalesced to an 8ms minimum interval (`MIN_SNAPSHOT_INTERVAL`, ~120 Hz) with an automatic trailing flush timer so bursts of terminal output don't allocate redundant intermediate snapshots between GTK frames.
- **Lock-Free RCU with Buffer Reuse:** We use Read-Copy-Update (RCU) via `arc-swap`. The background thread retains the swapped-out `Arc<RenderState>` (`spare_render_state`) and reuses its inner `Vec<Cell>` allocation in-place via `Arc::get_mut` whenever the UI thread is not actively reading it, achieving zero heap allocations in steady-state rendering. Each published snapshot carries a monotonic `generation: u64` counter.
- **Viewport Search & Font Metrics Caching:** `TerminalWidget` caches character glyph metrics (`cached_char_size`, snapped to physical device pixels using the surface's true fractional `gdk::Surface::scale()`) and a reusable `pango::Layout` (invalidated on font, cell-scale, or fractional DPI scale changes), and caches viewport regex search matches (`SearchViewportCache`) keyed by `(generation, display_offset, query, case_sensitive)` so cursor blinks and hover repaints do not re-run regex scans.
- **Hidden-Tab Resource Release (`map` / `unmap`):** When a terminal widget is unmapped (e.g. inside a background tab), it pauses its cursor blink timer, drops cached GPU `kitty_textures` and row `gsk::RenderNode`s (`row_node_cache`), and suppresses redundant `queue_draw()` calls on background PTY wakeups until mapped again.

---

## Features

- **Bidirectional Reflow & Unicode Grapheme Clustering (`DECSET 2027`):** Full word wrapping and unwrapping on resize, paired with `TermMode::GRAPHEME_CLUSTERING` (`?2027h` / `?2027$p`, enabled by default) to cluster Emoji ZWJ sequences (`\u{200D}`), Fitzpatrick skin-tone modifiers (`\u{1F3FB}`..`\u{1F3FF}`), Variation Selector-16 (`\u{FE0F}` emoji widening), and Regional Indicator flag pairs (`\u{1F1E6}`..`\u{1F1FF}`) into accurate 2-column cells without increasing `sizeof(Cell)` (24 bytes).
- **Advanced Regex Search:** Uses `regex-automata` 0.4 with full Unicode support. Features zero-cost wrap-around, look-behind context for word boundaries (`\b`), and automatic viewport scrolling to matches.
- **OSC 8 Hyperlinks:** Hover-only underlines and native URI support.
- **OSC 9;4 Progress Bars:** Intercepts progress sequences (e.g. from Flatpak, Cargo) to render a native GTK progress bar indicating normal, error, warning, or indeterminate states.
- **OSC 7 CWD Tracking:** Robust, event-driven directory tracking.
- **OSC 133 Semantic Prompt Tracking & Navigation:** Native support for shell integration markers. The terminal engine automatically tracks semantic boundaries (Prompt, Command, Output) directly within the cell grid (`Flags::SEMANTIC_*`), enabling structured context extraction for AI agents, `Ctrl+Shift+PageUp` / `Ctrl+Shift+PageDown` prompt jumping (`scroll_to_prev_prompt` / `scroll_to_next_prompt`), and command output extraction (`copy_last_command_output`).
- **Smooth High-Resolution Touchpad Scrolling:** `EventControllerScroll` processes continuous `gdk::ScrollUnit::Surface` pixel deltas on Wayland touchpads (alongside discrete `ScrollUnit::Wheel` ticks) via fractional line accumulation (`scroll_accumulator`) with instant direction-reversal reset across history scrolling, `MOUSE_MODE`, and `ALTERNATE_SCROLL`.
- **Kitty Graphics Protocol:** Natively supports Kitty images (`_G` APC sequences) including zero-copy shared memory (`t=s`) rendering for raw RGB/RGBA buffers, `z-index` background/foreground layer ordering, image deletion, and High-DPI scaling directly through GTK4 memory textures.
- **GSK Rendering & Per-Row `RenderNode` Caching:** `TerminalWidget` caches row-local `gsk::RenderNode`s (`row_node_cache`) keyed by a 64-bit content hash of each row's `Cell` slice and search highlight spans, invalidated only when a row's cells change or on global epoch changes (`RowCacheEpochKey`: font, scale, palette, dimming). Cursor blinking, selection dragging, link hover underlines, and scrolling reuse unchanged row `gsk::RenderNode`s with zero Pango re-shaping. Custom GSK geometry (`gsk::PathBuilder` + `gsk::Stroke`) renders distinct single, double, undercurl, dotted, and dashed underlines with SGR 58/59 underline colors, while the cursor overlay honors `DECTCEM` (`?25l`/`?25h`), `DECSCUSR` (`CSI Ps SP q`), reverse-video text legibility under solid block cursors, and a hollow block outline when unfocused.
- **Keyboard Protocols & Selection Shortcuts:** Supports `Ctrl+Shift+A` (Select All), `Ctrl+Shift+C`/`Ctrl+Insert` (Copy), `Ctrl+Shift+V`/`Shift+Insert` (Paste), and `Shift+Arrow`/`Shift+Home`/`Shift+End` keyboard selection adjustment (`SelectionAdjust`) whenever a selection is active. Also supports legacy xterm/VT100 key encodings (including `Shift+Tab` via `ISO_Left_Tab`, `Alt+Backspace`, `Ctrl+Backspace`, `Alt+Return`, modified `F1`–`F12`, non-alpha C0 control combinations, and `DECBKM` backarrow mode `67`), `xterm` `modifyOtherKeys` (`CSI > 4 ; Pv m` and `CSI ? 4 m` query), and the full Kitty Keyboard Protocol (`CSI u` via `translate_key_event` supporting `DISAMBIGUATE_ESC_CODES`, `REPORT_EVENT_TYPES` press/repeat/release, `REPORT_ALTERNATE_KEYS` base:shifted codepoints, `REPORT_ALL_KEYS_AS_ESC` including standalone modifier keys, `REPORT_ASSOCIATED_TEXT`, and `Super`/`Meta` modifier bits).
- **Focus, Visibility, Alternate Scroll, Color Scheme, `DECSTR`, `XTGETTCAP`, `DECSET 2048` & `OSC 3008`:** Emits `\x1b[I` / `\x1b[O` on widget focus transitions when `TermMode::FOCUS_IN_OUT` (DECSET `1004`) is active, supports `DECSET 2033` (`TermMode::VISIBILITY_REPORTS`) + `CSI ? 998 n` surface visibility reports (`\x1b[?999;1n` when mapped/visible, `\x1b[?999;2n` when unmapped/hidden or Wayland-suspended), translates mouse wheel ticks on the alternate screen into `Up`/`Down` cursor keys when `TermMode::ALTERNATE_SCROLL` (DECSET `1007`) is active without `MOUSE_MODE`, and supports `DECSET 2031` (`TermMode::COLOR_SCHEME_UPDATES`) + `CSI ? 996 n` dark/light mode queries (`\x1b[?997;1n` for dark, `\x1b[?997;2n` for light) with live notifications on theme change. Supports soft terminal reset (`DECSTR`, `CSI ! p`), extended cursor position reports (`DECXCPR`, `CSI ? 6 n`), screen character size reports (`CSI 19 t`), responds to `XTGETTCAP` (`DCS + q <hex> ST`) in-band capability queries (`TN`, `RGB`, `Tc`, `Smulx`, `Setulc`, `Ss`, `Se`, `Ms`, `Sync`), emits `DECSET 2048` (`\x1b[48;rows;cols;h;wt`) in-band resize notifications, and parses `OSC 3008` (`systemd` v258+ / `sudo`) hierarchical process context metadata.
- **Zero-Paint Idle Hover & Mouse Autohide:** Idle pointer motion over the terminal only queues a redraw when the hovered OSC 8 hyperlink or `Ctrl+hover` regex match span actually changes, and regex row matching (`match_row_cells`, with wide-character and combining-mark byte-to-column mapping) only runs when `Control` is held. `set_mouse_autohide(true)` hides the pointer while typing and restores it on pointer motion; `set_scroll_on_keystroke` and `set_scroll_on_output` control automatic viewport scrolling.
- **Headless Conformance Harness (`terminal/headless.rs`):** Provides `HeadlessTerminal` for deterministic chunk-boundary, query-response, and grid regression testing without a GTK display.
- **Mouse Ownership:** The internal `click_gesture` always calls `gesture.set_state(Claimed)` to prevent click events from bubbling to parent widgets. Ownership is decided by `MOUSE_MODE` only (not `is_alt_screen`) — apps must explicitly enable mouse reporting to receive clicks. Cell motion events (`1002` / `1003`) are deduplicated by `(col, row)`.
