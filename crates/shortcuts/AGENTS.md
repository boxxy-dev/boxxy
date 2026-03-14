# boxxy-shortcuts Agents & Architecture

## Role
This crate provides a keyboard shortcuts reference dialog for Boxxy Terminal.

## Responsibilities
- Provide a `ShortcutsComponent` that manages an `adw::Dialog`.
- Implement a Libadwaita 1.8 style shortcuts view manually using `adw::PreferencesPage` and `adw::ActionRow`.
- Organize shortcuts into groups (Window, Terminal).
- Dynamically load accelerators from `boxxy-keybindings`.

## Public Interface
- `ShortcutsComponent`: A GTK4 component wrapping the dialog.
- Input: `Show(gtk4::Window)`, `Hide`.
