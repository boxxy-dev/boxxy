use boxxy_window::{AppWindow, AppInit};
use libadwaita::prelude::*;
use gtk4::{gio, glib};
use boxxy_ai_core::utils;

fn main() {
    env_logger::init();
    // Enter the global tokio runtime context so tokio::spawn works everywhere.
    let _rt_guard = utils::runtime().enter();

    // Pre-load configuration into memory before GTK touches the window system.
    // This completely removes disk I/O from the critical window mapping path.
    boxxy_preferences::Settings::init();
    boxxy_preferences::AppState::init();

    // Ensure all default files and the SQLite database are generated immediately on first run
    // in the background, without blocking the UI thread.
    tokio::spawn(async {
        boxxy_preferences::Settings::ensure_claw_skills();
        if let Err(e) = boxxy_db::Db::new().await {
            log::error!("Failed to initialize Claw Memory Database on startup: {}", e);
        }
    });

    gstreamer::init().expect("Failed to initialize GStreamer.");

    gio::resources_register_include!("compiled.gresource")
        .expect("Failed to register resources.");

    let app = libadwaita::Application::builder()
        .application_id("play.mii.Boxxy")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.add_main_option(
        "new-window",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Open a new window",
        None,
    );

    app.connect_command_line(|app, cmdline| {
        let options = cmdline.options_dict();
        let new_window = options.contains("new-window");
        
        let has_window = app.active_window().is_some();

        if new_window && has_window {
            // A secondary instance passed --new-window.
            // Tell the primary instance's existing window to open another one.
            if let Some(window) = app.active_window() {
                let _ = window.activate_action("win.new-window", None);
            }
        } else if !has_window {
            // First launch, or no active windows. Just activate normally to create the first window.
            app.activate();
        } else {
            // Subsequent launch with no special flags. Just present the existing window.
            if let Some(window) = app.active_window() {
                window.present();
            }
        }
        
        0.into()
    });

    app.connect_activate(|app| {
        let app_init = AppInit::new();
        AppWindow::new(app, app_init);
        
        let inspector_action = gio::SimpleAction::new("inspector", None);
        inspector_action.connect_activate(move |_, _| {
            gtk4::Window::set_interactive_debugging(true);
        });
        app.add_action(&inspector_action);
    });

    let exit_code = app.run();
    std::process::exit(exit_code.into());
}



