# boxxy-app-menu Agents & Architecture

## Responsibilities
This crate provides the popover context menu component shown on right-click within the application.

## Public Components

### `AppMenuComponent`
A GTK4 component that wraps a `gtk::PopoverMenu`.

**Inputs (`AppMenuInput`):**
- `Show(gdk::Rectangle)`: Displays the menu at the specified position.
- `Hide`: Hides the menu.

**Menu Items:**
- Copy (`win.copy`)
- Paste (`win.paste`)
- Mini Apps (`win.mini-apps`)
- Open in Files (`win.open-in-files`)
- Keyboard Shortcuts (`win.shortcuts`)
- Preferences (`win.preferences`)
- About Boxxy Terminal (`win.about`)

**State:**
- Manages a `gio::Menu` model.
- All actions are defined at the window level (`win.*` prefix) and handled by `boxxy-window`.
