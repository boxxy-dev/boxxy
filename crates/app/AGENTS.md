# boxxy-app Agents & Architecture

## Responsibilities
This crate is the binary entry point for the application. It is responsible for:
- Initializing the GTK environment.
- Setting up the application state.
- **Compiling and registering GResources** (icons, CSS) via `build.rs`.
- Bootstrapping the main application window.

## Entry Point

### `main()`
- Parses early isolated dev flags (`--dev`, `--instance <slug>`, `--ephemeral`, `--sync-settings`) and prepares the isolated or ephemeral configuration directory via `boxxy_sys_utils::prepare_dev_environment()`.
- Registers compiled resources (`compiled.gresource`).
- Initializes `libadwaita::Application` with `boxxy_sys_utils::gtk_app_id()` (`dev.boxxy.BoxxyTerminal` or `dev.boxxy.BoxxyTerminal.Devel`).
- Runs the application with `boxxy_window::AppWindow` as the root component.
