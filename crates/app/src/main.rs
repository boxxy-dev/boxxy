use boxxy_ai_core::utils;
use boxxy_window::{AppInit, AppWindow};
use gtk4::{gio, glib};
use libadwaita::prelude::*;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("zvariant", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("sqlx", log::LevelFilter::Warn)
        .filter_module("rig", log::LevelFilter::Warn)
        .init();
    // Enter the global tokio runtime context so tokio::spawn works everywhere.
    let _rt_guard = utils::runtime().enter();

    // Pre-load configuration into memory before GTK touches the window system.
    // This completely removes disk I/O from the critical window mapping path.
    boxxy_preferences::Settings::init();
    boxxy_preferences::AppState::init();

    // Initialize Telemetry
    tokio::spawn(async {
        boxxy_telemetry::init().await;

        // Track app.launch
        use sysinfo::System;
        let os = System::name().unwrap_or_else(|| "Unknown".to_string());
        let arch = System::cpu_arch();
        let version = env!("CARGO_PKG_VERSION");
        let pkg_type = if std::env::var("FLATPAK_ID").is_ok() {
            "flatpak"
        } else {
            "native"
        };
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());

        boxxy_telemetry::track_launch(&os, &arch, pkg_type, version, &shell).await;
    });

    // Ensure all default files are generated immediately on first run
    // in the background, without blocking the UI thread.
    tokio::spawn(async {
        boxxy_preferences::Settings::ensure_claw_skills();
    });

    // Fetch API keys from host environment in the background
    tokio::spawn(async {
        let agent = boxxy_terminal::get_agent().await;
        let keys_to_check = [
            ("GEMINI_API_KEY", "Gemini"),
            ("ANTHROPIC_API_KEY", "Anthropic"),
            ("OPENAI_API_KEY", "OpenAI"),
        ];

        let mut found_any = false;
        for (env_var, provider) in keys_to_check {
            if let Ok(key) = agent
                .proxy()
                .get_environment_variable(env_var.to_string())
                .await
            {
                if !key.is_empty() {
                    boxxy_preferences::Settings::set_env_api_key(provider, key);
                    found_any = true;
                }
            }
        }

        if found_any {
            let settings = boxxy_preferences::Settings::load();
            let _ = boxxy_preferences::SETTINGS_EVENT_BUS.send(settings);
        }
    });

    gstreamer::init().expect("Failed to initialize GStreamer.");

    gio::resources_register_include!("compiled.gresource").expect("Failed to register resources.");

    let app = libadwaita::Application::builder()
        .application_id("dev.boxxy.BoxxyTerminal")
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

    app.connect_shutdown(|_| {
        boxxy_bookmarks::manager::BookmarksManager::clean_runs_dir();
    });

    let exit_code = app.run();

    // Flush Telemetry after the main window is closed
    utils::runtime().block_on(async {
        boxxy_telemetry::shutdown().await;
    });

    std::process::exit(exit_code.into());
}
