# boxxy-app Agents & Architecture

## Responsibilities
This crate is the binary entry point for the application. It is responsible for:
- Initializing the GTK environment.
- Setting up the application state.
- **Compiling and registering GResources** (icons, CSS) via `build.rs`.
- Bootstrapping the main application window.

## Entry Point

### `main()`
- Registers compiled resources (`compiled.gresource`).
- Initializes `libadwaita::Application`.
- Creates the `RelmApp`.
- Runs the application with `boxxy_window::AppWindow` as the root component.
