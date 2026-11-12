use boxxy_cli::{LaunchTarget, ParseOutcome};
use boxxy_window::{AppInit, AppWindow, TabLaunchSpec};
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

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("sqlx", log::LevelFilter::Warn)
        .filter_module("rig", log::LevelFilter::Warn)
        .filter_module("rmcp", log::LevelFilter::Warn)
        .filter_module("h2", log::LevelFilter::Warn)
        .filter_module("hyper", log::LevelFilter::Warn)
        .filter_module("reqwest", log::LevelFilter::Warn)
        .filter_module("rustls", log::LevelFilter::Warn)
        .init();

    // Apply isolated dev-mode / config-dir / debug-context overrides so `gtk_app_id()`
    // resolves the right GApplication bus name before `app.run_with_args()`.
    boxxy_sys_utils::configure_dev_options(launch_req.dev.clone());

    // When spawned by `Updater::apply_update_and_restart()`, wait for the dying parent
    // `boxxy-terminal` process to release `dev.boxxy.BoxxyTerminal` on D-Bus so GIO
    // does not forward `--new-window` to the exiting parent and quit immediately.
    if !boxxy_sys_utils::is_dev_instance() {
        wait_for_restarting_parent(launch_req.replace_existing);
    }

    // Enter the global tokio runtime context so tokio::spawn works everywhere on the main thread.
    let _rt_guard = boxxy_sys_utils::runtime().enter();

    let app = libadwaita::Application::builder()
        .application_id(boxxy_sys_utils::gtk_app_id())
        .flags(
            gio::ApplicationFlags::HANDLES_COMMAND_LINE
                | gio::ApplicationFlags::ALLOW_REPLACEMENT,
        )
        .build();

    // `connect_startup` runs strictly on the primary GUI instance (never on secondary
    // `boxxy-terminal --tab` / `--present` forwarders), avoiding duplicate file seeding,
    // agent startup-token claims, GStreamer init, and `app.launch` telemetry.
    app.connect_startup(|_| {
        boxxy_sys_utils::prepare_dev_environment();

        gio::resources_register_include!("compiled.gresource")
            .expect("Failed to register resources.");
        gtk4::Window::set_default_icon_name(boxxy_sys_utils::STABLE_GTK_APP_ID);

        boxxy_preferences::Settings::init();
        boxxy_preferences::AppState::init();

        tokio::spawn(async {
            boxxy_bookmarks::manager::BookmarksManager::init();
            boxxy_preferences::Settings::ensure_claw_skills();
        });

        boxxy_terminal::register_agent_ensure_hook(|| async {
            if let Err(e) = boxxy_window::agent_deployer::ensure_agent_running().await {
                log::error!("Failed to ensure agent is running: {}", e);
            }
        });

        tokio::spawn(async {
            let agent = boxxy_terminal::get_agent().await;

            let is_flatpak = std::env::var("FLATPAK_ID").is_ok();
            let mut keys_to_check: Vec<(String, String)> =
                boxxy_sys_utils::env::PROVIDER_ENV_KEY_MAP
                    .iter()
                    .map(|&(k, v)| (k.to_string(), v.to_string()))
                    .collect();

            for ep in &boxxy_preferences::Settings::load().openai_compatible_endpoints {
                if let Some(env_var) = ep
                    .api_key_env
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    keys_to_check.push((env_var.to_string(), ep.name.clone()));
                }
            }

            let mut found_any = false;
            for (env_var, provider) in keys_to_check {
                let key_opt = if is_flatpak {
                    agent.proxy().get_environment_variable(env_var).await.ok()
                } else {
                    std::env::var(&env_var).ok()
                };
                if let Some(key) = key_opt
                    && !key.is_empty()
                {
                    boxxy_preferences::Settings::set_env_api_key(&provider, key);
                    found_any = true;
                }
            }

            if found_any {
                let settings = boxxy_preferences::Settings::load();
                let _ = boxxy_preferences::SETTINGS_EVENT_BUS.send((*settings).clone());
            }

            let settings = boxxy_preferences::Settings::load();
            let _ = agent
                .update_credentials(
                    settings.get_effective_api_keys(),
                    settings.ollama_base_url.clone(),
                )
                .await;

            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            boxxy_window::sound::ensure_gstreamer_init();

            boxxy_telemetry::init_db().await;
            {
                use sysinfo::System;
                let os = if is_flatpak {
                    std::fs::read_to_string("/run/host/os-release")
                        .or_else(|_| std::fs::read_to_string("/run/host/etc/os-release"))
                        .ok()
                        .and_then(|content| {
                            content
                                .lines()
                                .find(|l| l.starts_with("NAME="))
                                .map(|l| {
                                    l.trim_start_matches("NAME=")
                                        .trim_matches(['"', '\''])
                                        .trim()
                                        .to_string()
                                })
                        })
                        .unwrap_or_else(|| System::name().unwrap_or_else(|_| "Unknown".to_string()))
                } else {
                    System::name().unwrap_or_else(|_| "Unknown".to_string())
                };
                let arch = System::cpu_arch();
                let version = env!("CARGO_PKG_VERSION");
                let pkg_type = if is_flatpak { "flatpak" } else { "native" };
                let shell = agent
                    .get_preferred_shell()
                    .await
                    .ok()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| {
                        std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string())
                    });
                let desktop =
                    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_string());
                let theme = {
                    let s = boxxy_preferences::Settings::load();
                    boxxy_telemetry::resolve_theme_telemetry(s.app_theme_mode, &s.app_theme_id)
                };

                boxxy_telemetry::track_launch(&os, &arch, pkg_type, version, &shell, &desktop, theme)
                    .await;
            }
        });
    });

    app.connect_command_line(|app, cmdline| {
        let remote_cwd = cmdline.cwd();
        let req = match boxxy_cli::parse_remote_launch(cmdline.arguments(), remote_cwd.as_deref()) {
            Ok(r) => r,
            Err(err) => {
                log::error!("Invalid remote command-line payload: {err}");
                return 2.into();
            }
        };

        let spec = TabLaunchSpec {
            working_dir: req.working_dir.map(|p| p.to_string_lossy().into_owned()),
            command: req.command,
            hold: req.hold,
            title: req.title,
        };
        let has_explicit_payload =
            spec.working_dir.is_some() || spec.command.is_some() || spec.title.is_some();

        let has_window = app.active_window().is_some();

        if !has_window {
            AppWindow::new(app, AppInit::from_tab_launch(spec));
        } else if req.target == LaunchTarget::Present {
            if let Some(window) = app.active_window() {
                window.present();
            }
        } else if req.target == LaunchTarget::NewTab {
            if let Some(window) = app.active_window() {
                if let Ok(json) = serde_json::to_string(&spec) {
                    let variant = json.to_variant();
                    let _ = window.activate_action("win.new-tab-launch", Some(&variant));
                }
                window.present();
            }
        } else if has_explicit_payload {
            AppWindow::new(app, AppInit::from_tab_launch(spec));
        } else if let Some(window) = app.active_window() {
            // Secondary invocation with no explicit CWD/command opens a new window
            // (preserving the active tab's CWD when `preserve_working_dir` is enabled).
            let _ = window.activate_action("win.new-window", None);
        }

        0.into()
    });

    let inspector_action = gio::SimpleAction::new("inspector", None);
    inspector_action.connect_activate(move |_, _| {
        gtk4::Window::set_interactive_debugging(true);
    });
    app.add_action(&inspector_action);

    app.connect_activate(|app| {
        let app_init = AppInit::new();
        AppWindow::new(app, app_init);
    });

    app.connect_shutdown(|_| {
        boxxy_bookmarks::manager::BookmarksManager::clean_runs_dir();
    });

    let gapp_args = launch_req.to_gapplication_args("boxxy-terminal");
    let exit_code: i32 = app.run_with_args(&gapp_args).into();

    ExitCode::from(exit_code.clamp(0, 255) as u8)
}

#[cfg(unix)]
fn wait_for_restarting_parent(replace_existing: bool) {
    let initial_ppid = std::os::unix::process::parent_id();
    if initial_ppid <= 1 {
        return;
    }

    let comm_path = format!("/proc/{initial_ppid}/comm");
    let parent_comm = std::fs::read_to_string(&comm_path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if parent_comm == "boxxy-terminal" {
        log::info!(
            "Detected restarting parent boxxy-terminal (PID {initial_ppid}); waiting for it to exit..."
        );
        let proc_dir = std::path::PathBuf::from(format!("/proc/{initial_ppid}"));
        let start = std::time::Instant::now();
        while std::os::unix::process::parent_id() == initial_ppid
            && proc_dir.exists()
            && start.elapsed() < std::time::Duration::from_secs(5)
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Allow the session D-Bus daemon to process the parent's closed socket
        // and release `dev.boxxy.BoxxyTerminal` before GApplication registers.
        std::thread::sleep(std::time::Duration::from_millis(150));
    } else if replace_existing {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(not(unix))]
fn wait_for_restarting_parent(_replace_existing: bool) {}
