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
