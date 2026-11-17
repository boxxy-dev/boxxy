//! Single-instance `GApplication` command-line routing, window lookup, and exit-code conversion.

use boxxy_cli::{LaunchRequest, LaunchTarget};
use boxxy_window::{AppInit, AppWindow, TabLaunchSpec};
use gtk4::gio;
use libadwaita::prelude::*;
use std::process::ExitCode;

#[derive(Debug)]
pub(crate) enum RouteDecision {
    /// `--gapplication-service`: warm D-Bus service without opening an unsolicited window.
    ServiceWarmup,
    /// Construct and present a new `AppWindow` synchronously (preserves Wayland activation token).
    OpenWindow(TabLaunchSpec),
    /// Focus/raise the most recently focused existing `AppWindow`.
    PresentExisting,
    /// Open a new tab inside the existing `AppWindow` via `win.new-tab-launch`.
    OpenTabInExisting(TabLaunchSpec),
    /// Ask the existing `AppWindow` to open a sibling window inheriting its active tab's CWD.
    OpenWindowInheritingCwd(TabLaunchSpec),
}

pub(crate) fn handle_command_line(
    app: &libadwaita::Application,
    cmdline: &gio::ApplicationCommandLine,
) -> gtk4::glib::ExitCode {
    let remote_cwd = cmdline.cwd();
    let req = match boxxy_cli::parse_remote_launch(cmdline.arguments(), remote_cwd.as_deref()) {
        Ok(r) => r,
        Err(err) => {
            log::error!("Invalid remote command-line payload: {err}");
            return 2.into();
        }
    };

    let existing_window = find_active_boxxy_window(app);
    let preserve_working_dir = boxxy_preferences::Settings::load().preserve_working_dir;
    let decision = plan_launch_route(req, existing_window.is_some(), preserve_working_dir);

    match decision {
        RouteDecision::ServiceWarmup => {}
        RouteDecision::OpenWindow(spec) => {
            AppWindow::new(app, AppInit::from_tab_launch(spec));
        }
        RouteDecision::PresentExisting => {
            if let Some(window) = existing_window {
                window.present();
            } else {
                AppWindow::new(app, AppInit::new());
            }
        }
        RouteDecision::OpenTabInExisting(spec) => {
            if let Some(window) = existing_window {
                match serde_json::to_string(&spec) {
                    Ok(json) => {
                        let variant = json.to_variant();
                        if WidgetExt::activate_action(&window, "win.new-tab-launch", Some(&variant))
                            .is_ok()
                        {
                            window.present();
                        } else {
                            AppWindow::new(app, AppInit::from_tab_launch(spec));
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to serialize TabLaunchSpec: {e}");
                        AppWindow::new(app, AppInit::from_tab_launch(spec));
                    }
                }
            } else {
                AppWindow::new(app, AppInit::from_tab_launch(spec));
            }
        }
        RouteDecision::OpenWindowInheritingCwd(spec) => {
            if let Some(window) = existing_window {
                if WidgetExt::activate_action(&window, "win.new-window", None).is_err() {
                    AppWindow::new(app, AppInit::from_tab_launch(spec));
                }
            } else {
                AppWindow::new(app, AppInit::from_tab_launch(spec));
            }
        }
    }

    0.into()
}

/// Finds the most-recently-focused Boxxy [`libadwaita::ApplicationWindow`].
///
/// `app.windows()` is maintained in MRU focus order by GTK4; filtering with `downcast`
/// ensures auxiliary `GtkWindow`s (such as the interactive GTK Inspector) never intercept
/// and drop `win.new-tab-launch` or `win.new-window` actions.
fn find_active_boxxy_window(
    app: &libadwaita::Application,
) -> Option<libadwaita::ApplicationWindow> {
    app.windows()
        .into_iter()
        .find_map(|w| w.downcast::<libadwaita::ApplicationWindow>().ok())
}

pub(crate) fn plan_launch_route(
    req: LaunchRequest,
    has_existing_window: bool,
    preserve_working_dir: bool,
) -> RouteDecision {
    if req.gapplication_service {
        return RouteDecision::ServiceWarmup;
    }

    let spec = TabLaunchSpec {
        working_dir: req.working_dir.map(|p| p.to_string_lossy().into_owned()),
        command: req.command,
        hold: req.hold,
        title: req.title,
    };
    let has_explicit_payload =
        spec.working_dir.is_some() || spec.command.is_some() || spec.title.is_some() || spec.hold;

    if !has_existing_window {
        return RouteDecision::OpenWindow(spec);
    }

    match req.target {
        LaunchTarget::Present => RouteDecision::PresentExisting,
        LaunchTarget::NewTab => RouteDecision::OpenTabInExisting(spec),
        LaunchTarget::Default | LaunchTarget::NewWindow => {
            if has_explicit_payload || !preserve_working_dir {
                RouteDecision::OpenWindow(spec)
            } else {
                RouteDecision::OpenWindowInheritingCwd(spec)
            }
        }
    }
}

pub(crate) fn to_process_exit_code(gtk_code: i32) -> ExitCode {
    let clamped = gtk_code.clamp(0, 255);
    ExitCode::from(u8::try_from(clamped).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn service_activation_never_opens_unsolicited_window() {
        let req = LaunchRequest {
            gapplication_service: true,
            ..LaunchRequest::default()
        };
        assert!(matches!(
            plan_launch_route(req, false, true),
            RouteDecision::ServiceWarmup
        ));
    }

    #[test]
    fn first_launch_always_opens_window_synchronously() {
        let req = LaunchRequest {
            target: LaunchTarget::NewTab,
            title: Some("Build".to_string()),
            ..LaunchRequest::default()
        };
        match plan_launch_route(req, false, true) {
            RouteDecision::OpenWindow(spec) => assert_eq!(spec.title.as_deref(), Some("Build")),
            other => panic!("expected OpenWindow, got {other:?}"),
        }
    }

    #[test]
    fn secondary_launches_respect_present_tab_payload_and_cwd_inheritance() {
        let present_req = LaunchRequest {
            target: LaunchTarget::Present,
            ..LaunchRequest::default()
        };
        assert!(matches!(
            plan_launch_route(present_req, true, true),
            RouteDecision::PresentExisting
        ));

        let tab_req = LaunchRequest {
            target: LaunchTarget::NewTab,
            working_dir: Some(PathBuf::from("/tmp/synthetic-workspace")),
            ..LaunchRequest::default()
        };
        match plan_launch_route(tab_req, true, true) {
            RouteDecision::OpenTabInExisting(spec) => {
                assert_eq!(
                    spec.working_dir.as_deref(),
                    Some("/tmp/synthetic-workspace")
                );
            }
            other => panic!("expected OpenTabInExisting, got {other:?}"),
        }

        // Bare secondary launch with `preserve_working_dir = true` delegates to `win.new-window`.
        assert!(matches!(
            plan_launch_route(LaunchRequest::default(), true, true),
            RouteDecision::OpenWindowInheritingCwd(_)
        ));

        // Bare secondary launch with `preserve_working_dir = false` opens synchronously
        // so Wayland `XDG_ACTIVATION_TOKEN` focus handoff stays intact.
        assert!(matches!(
            plan_launch_route(LaunchRequest::default(), true, false),
            RouteDecision::OpenWindow(_)
        ));

        // Explicit `--hold` counts as an explicit launch payload.
        let hold_req = LaunchRequest {
            hold: true,
            ..LaunchRequest::default()
        };
        assert!(matches!(
            plan_launch_route(hold_req, true, true),
            RouteDecision::OpenWindow(spec) if spec.hold
        ));
    }

    #[test]
    fn exit_code_clamps_out_of_range_values_cleanly() {
        assert_eq!(to_process_exit_code(0), ExitCode::SUCCESS);
        assert_eq!(to_process_exit_code(2), ExitCode::from(2));
        assert_eq!(to_process_exit_code(-5), ExitCode::SUCCESS);
        assert_eq!(to_process_exit_code(999), ExitCode::from(255));
    }
}
