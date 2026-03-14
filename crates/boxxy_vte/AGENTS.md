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

The core terminal logic is located in `src/engine/`. We have fully absorbed and refactored the terminal engine to be:
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

---

## Threading & Synchronization

- **GTK Main Thread:** Handles rendering (`snapshot`), input events, and UI state. It consumes a `RenderState` snapshot produced by the background thread.
- **Tokio Background Task:** Runs the `EventLoop`. It reads from the PTY, updates the `Term` state machine, and periodically pushes a new `RenderState` to the UI thread.
- **Lock-Free RCU:** We use Read-Copy-Update (RCU) via `arc-swap`. The background thread creates a new `RenderState` and "swaps" it into a global pointer. The UI thread "loads" the latest pointer without ever waiting for a lock.

---

## Features

- **Bidirectional Reflow:** Full word wrapping and unwrapping on resize.
- **Advanced Regex Search:** Uses `regex-automata` 0.4 with full Unicode support. Features zero-cost wrap-around, look-behind context for word boundaries (`\b`), and automatic viewport scrolling to matches.
- **OSC 8 Hyperlinks:** Hover-only underlines and native URI support.
- **OSC 7 CWD Tracking:** Robust, event-driven directory tracking.
- **Kitty Graphics Protocol:** Natively supports Kitty images (`_G` APC sequences) including zero-copy shared memory (`t=s`) rendering for raw RGB/RGBA buffers, `z-index` background/foreground layer ordering, image deletion, and High-DPI scaling directly through GTK4 memory textures.
- **GSK Rendering:** Optimized GTK4 scene graph snapshots for high-performance text rendering.
- **Keyboard Protocols:** Supports modern terminal keyboard protocols including CSI u.
