# boxxy-app Agents & Architecture

## Responsibilities
This crate is the binary entry point for the application. It is responsible for:
- Initializing the GTK environment.
- Setting up the application state.
- **Compiling and registering GResources** (icons, CSS) via `build.rs`.
- Bootstrapping the main application window.

## Entry Point

### `main()`
- Invokes `boxxy_cli::dispatch_from_env()` before initializing GTK or GResources: headless `+actions` (`+version`, `+doctor`, `+env`, `+agent`, `+completions`), `-h`/`--help`, and `-V`/`--version` execute immediately and exit.
- Applies `LaunchRequest.dev` (`--dev`, `--instance <slug>`, `--ephemeral`, `--sync-settings`, `--config-dir <path>`, `--debug-context`) and prepares the isolated or ephemeral configuration directory via `boxxy_sys_utils::prepare_dev_environment()`.
- Registers compiled resources (`compiled.gresource`).
- Initializes `libadwaita::Application` with `boxxy_sys_utils::gtk_app_id()` (`dev.boxxy.BoxxyTerminal` or `dev.boxxy.BoxxyTerminal.Devel`), forwarding validated `LaunchRequest` payloads (`working_dir`, `-e` `command`, `--hold`, `--title`, window/tab target) through `app.run_with_args` and `connect_command_line` with `boxxy_window::AppWindow`.
