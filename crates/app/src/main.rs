#![allow(clippy::redundant_pub_crate)]

mod launch;
mod logging;
mod restart_guard;
mod startup;
mod wayland_guard;

use boxxy_cli::ParseOutcome;
use boxxy_window::{AppInit, AppWindow};
use gtk4::gio;
use libadwaita::prelude::*;
use std::process::ExitCode;

fn main() -> ExitCode {
    // Parse CLI flags and dispatch any headless `+action`, `--help`, or `--version`
    // immediately before touching GTK, GResources, or background runtimes.
    let launch_req = match boxxy_cli::dispatch_from_env() {
        ParseOutcome::Exit(code) => return code,
        ParseOutcome::Launch(req) => req,
    };

    // Apply isolated dev-mode / config-dir / debug-context overrides first so both
    // `logging::init()` and `gtk_app_id()` observe the resolved instance flags.
    boxxy_sys_utils::configure_dev_options(launch_req.dev.clone());
    logging::init();

    // When spawned with `--gapplication-replace` (e.g. `Updater::apply_update_and_restart()`),
    // wait for any exiting parent `boxxy-terminal` to release the session D-Bus name.
    restart_guard::wait_for_restarting_parent(launch_req.replace_existing);

    // Enter the global Tokio runtime context so `tokio::spawn` works everywhere on the main thread.
    let _rt_guard = boxxy_sys_utils::runtime().enter();

    let app = libadwaita::Application::builder()
        .application_id(boxxy_sys_utils::gtk_app_id())
        .flags(
            gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::ALLOW_REPLACEMENT,
        )
        .build();

    app.connect_startup(|_| startup::on_startup());
    app.connect_command_line(launch::handle_command_line);

    let inspector_action = gio::SimpleAction::new("inspector", None);
    inspector_action.connect_activate(|_, _| {
        gtk4::Window::set_interactive_debugging(true);
    });
    app.add_action(&inspector_action);

    app.connect_activate(|app| {
        AppWindow::new(app, AppInit::new());
    });

    app.connect_shutdown(|_| {
        wayland_guard::mark_shutting_down();
        boxxy_bookmarks::manager::BookmarksManager::clean_runs_dir();
    });

    let gapp_args = launch_req.to_gapplication_args("boxxy-terminal");
    let exit_code: i32 = app.run_with_args(&gapp_args).into();

    launch::to_process_exit_code(exit_code)
}
