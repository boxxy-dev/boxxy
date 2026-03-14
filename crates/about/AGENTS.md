# boxxy-about Agents & Architecture

## Role
This crate provides the "About" dialog for Boxxy Terminal, utilizing `adw::AboutDialog` (Libadwaita 1.5+ style).

## Responsibilities
- Provide an `AboutComponent` that manages an `adw::AboutDialog`.
- Display application metadata (version, license, credits).
- Display the application icon (loaded via GResources).

## Public Interface
- `AboutComponent`: A GTK4 component wrapping the dialog.
- Input: `Show(gtk4::Window)`, `Hide`.
