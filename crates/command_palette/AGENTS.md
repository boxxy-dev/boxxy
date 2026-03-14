# `boxxy-command-palette`

## Role
Provides the `CommandPaletteComponent`, a searchable list of application commands activated via `Ctrl+Shift+P`.

## Responsibilities
- Renders a `gtk4::Popover` containing a `gtk4::SearchEntry` and `gtk4::ListBox`.
- Filters available commands based on user input.
- Triggers `win.*` actions on the main application window upon selection.
