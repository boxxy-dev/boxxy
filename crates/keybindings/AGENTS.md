# boxxy-keybindings Agents & Architecture

## Responsibilities
This crate centralizes the definition and registration of keyboard shortcuts (accelerators) for the application.

## Public API

### `bind_shortcuts(app: &libadwaita::Application)`
Registers all defined keybindings with the application instance.

### `Keybinding` Struct
Defines a shortcut:
- `trigger`: Accelerator string (e.g., `"<Ctrl><Shift>t"`).
- `action_name`: The generic action name (e.g., `"win.new-tab"`).

### Constants
Predefined `Keybinding` instances:
- `NEW_TAB`
- `CLOSE_TAB`
- `PREFERENCES`
- `ZOOM_IN`
- `ZOOM_OUT`
- `COPY`
- `PASTE`
- `SELECT_ALL`
- `OPEN_SCROLLBACK`

## International Keyboard Layouts
To ensure shortcuts (like Zoom In/Out) work across diverse keyboard layouts (e.g., AZERTY, QWERTZ, Brazilian ABNT2) without manual scancode sniffing, we rely on GTK's native layout-aware accelerator parsing. 

When binding shortcuts via `set_accels_for_action` or `ShortcutTrigger::parse_string` (using `|`), we provide an array/list of **logical fallback keysyms**. 

For example, `Zoom In` is bound to:
- `<Ctrl>plus` (The semantic intent)
- `<Ctrl>equal` (US QWERTY fallback, as `+` requires `Shift`)
- `<Ctrl>KP_Add` (Universal Numpad)
- `<Ctrl><Shift>plus` (AZERTY fallback where `+` requires `Shift`)

This casts a wider net, allowing GTK to resolve the intended action based on the user's active keyboard map.
