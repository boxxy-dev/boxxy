# `boxxy-keybindings` Agents & Architecture

## Responsibilities
Centralizes keyboard shortcut definitions, custom `keybindings.toml` persistence, conflict and override diagnostics, and GTK4 `PropagationPhase::Capture` key dispatch across all windows, panes, and overlays.

## Architecture & Modules

- **`src/action.rs` (`ShortcutId`, `ShortcutCategory`, `ActionScope`, `TerminalInputMode`):**
  - Defines all 49 user-bindable actions across `General`, `Terminal`, `Split Panes`, and `Tabs` (including `Alt+1`..`Alt+0` tab switching).
  - Declares explicit `ActionScope` tiers (`GlobalWindow` -> `PaneOrOverlay` -> `TerminalFocused`) so terminal-specific shortcuts (`Copy`, `Paste`, `Select All`, `PrevPrompt`, `NextPrompt`) never hijack standard GTK text editing inside `MsgBar` or `Preferences` entries even when customized.
  - Includes `TerminalInputMode` (`Normal`, `Search`, `ViNormal`, `ViVisual`) hooks prepared for Stage 2 Vi/Copy Mode.
- **`src/defaults.rs` (`ACTION_SPECS`, `spec_for`):**
  - Canonical static catalog of titles, `win.*` action targets, default chords, search keywords, and automatic numpad/layout fallback aliases (`KP_Add`, `equal`, `KP_Subtract`, `KP_0`, `Ctrl+Insert`).
- **`src/trigger.rs` (`Chord`):**
  - Headless parser and formatter built on `gdk::Key::from_name` (requiring zero `gtk::init()`, safe in `boxxy-cli` and `spawn_blocking`).
  - Accepts both human-readable strings (`"Ctrl+Shift+V"`, `"Ctrl+/"`, `"Ctrl++"`, `"performable:Ctrl+C"`) and GTK accelerator strings (`"<Control><Shift>v"`, `"<Ctrl>slash"`).
  - Supports Ghostty-style `performable:` conditional pass-through (only consuming the key event when `WindowShortcutHost::can_perform` is `true`, enabling Smart Copy on `performable:Ctrl+C`).
- **`src/conflicts.rs` (`analyze_diagnostics`, `ChordStatus`, `OutgoingOverride`):**
  - Deterministic override and conflict analyzer computing both action-vs-action shadowing (such as `Swap Up` `Ctrl+Shift+Up` overriding `Jump to Previous Command Block` while `Ctrl+Shift+PageUp` remains active) and terminal built-in signal shadows (`SIGINT` `^C`, `EOF` `^D`, bare `F1`–`F12` TUI keys, scrollback/selection keys).
- **`src/store.rs` (`keybindings.toml`):**
  - Sparse override persistence in `<config_dir>/keybindings.toml` (only customized or `"none"` unbound entries are written so untouched actions automatically track future defaults).
  - Atomic `0600` `.tmp` + `sync_data` + `rename` writes, mtime/size stamp hot-reloading, non-destructive syntax warning reporting, and one-time migration of legacy `settings.claw_msgbar_shortcut`.
- **`src/controller.rs` (`attach_window_controller`, `WindowShortcutHost`):**
  - Unified `PropagationPhase::Capture` dispatcher using GTK4's `gdk::KeyEvent::matches()`, supporting both Latin layouts (QWERTY, Dvorak, Colemak, AZERTY, QWERTZ via `KeyMatch::Exact`) and non-Latin layouts (Greek, Cyrillic, Arabic, Hebrew via XKB cross-group `KeyMatch::Partial`) before Kitty `CSI u` or terminal handlers can swallow modified keys.
- **`src/lib.rs` (`KEYBINDINGS`, `KEYBINDINGS_EVENT_BUS`):**
  - Lock-free `ArcSwap<CompiledKeymap>` read directly on key press with zero controller rebuilds, paired with `KEYBINDINGS_EVENT_BUS` for live UI synchronization in `Preferences` and `CommandPalette`.
