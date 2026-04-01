use boxxy_preferences::Settings;
use gtk4::gio;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::init::AppInit;
use crate::state::AppWindowInner;
use crate::ui::AppWindow;

pub fn save_window_state(inner: &mut AppWindowInner, width: i32, height: i32, is_maximized: bool) {
    inner.app_state.window_width = width;
    inner.app_state.window_height = height;
    inner.app_state.is_maximized = is_maximized;
    inner.app_state.save();
}

pub fn handle_close_request(inner_ref: &Rc<RefCell<AppWindowInner>>, inner: &mut AppWindowInner) {
    let mut pids = Vec::new();
    for tab in &inner.tabs {
        pids.extend(tab.controller.get_pids());
    }

    let inner_clone = inner_ref.clone();
    gtk4::glib::spawn_future_local(async move {
        let agent = boxxy_terminal::get_agent().await;
        let mut running_apps = Vec::new();
        for pid in pids {
            log::debug!(
                "handle_close_request: Querying running processes for PID {}",
                pid
            );
            if let Ok(Ok(mut procs)) = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                agent.get_running_processes(pid),
            )
            .await
            {
                // Ignore the shell process itself if it's the only thing running,
                // but since we get descendants, the shell itself isn't included.
                running_apps.append(&mut procs);
            } else {
                log::warn!(
                    "handle_close_request: Timeout or error querying processes for PID {}",
                    pid
                );
            }
        }

        let workspace = boxxy_claw::registry::workspace::global_workspace().await;
        let pending_tasks = workspace.get_all_pending_tasks().await;

        if running_apps.is_empty() && pending_tasks.is_empty() {
            inner_clone.borrow().force_close.set(true);
            inner_clone.borrow().window.close();
            return;
        }

        let dialog = libadwaita::AlertDialog::builder()
            .heading("Close Window?")
            .body("Some processes or AI tasks are still running.")
            .build();

        let group = libadwaita::PreferencesGroup::new();
        for (pid, comm) in running_apps {
            let row = libadwaita::ActionRow::builder()
                .title(&comm)
                .subtitle(format!("Process {}", pid))
                .build();
            group.add(&row);
        }

        for (agent_name, task) in pending_tasks {
            let type_str = match task.task_type {
                boxxy_claw::engine::TaskType::Notification => "Reminder",
                boxxy_claw::engine::TaskType::Command => "Scheduled Command",
                boxxy_claw::engine::TaskType::Query => "Scheduled Query",
            };
            let row = libadwaita::ActionRow::builder()
                .title(&task.payload)
                .subtitle(format!("{} by {}", type_str, agent_name))
                .build();
            group.add(&row);
        }

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .max_content_height(250)
            .propagate_natural_height(true)
            .child(&group)
            .build();

        dialog.set_extra_child(Some(&scrolled));

        dialog.add_response("cancel", "Cancel");
        dialog.add_response("close", "Close All");
        dialog.set_response_appearance("close", libadwaita::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let window = inner_clone.borrow().window.clone();
        let response = dialog.choose_future(Some(&window)).await;
        if response == "close" {
            inner_clone.borrow().force_close.set(true);
            inner_clone.borrow().window.close();
        }
    });
}

pub fn new_window(inner: &AppWindowInner) {
    if let Some(app) =
        gio::Application::default().and_then(|a| a.downcast::<libadwaita::Application>().ok())
    {
        let working_dir = if inner.current_settings.preserve_working_dir {
            inner.tab_view.selected_page().and_then(|page| {
                let child = page.child();
                inner
                    .tabs
                    .iter()
                    .find(|c| c.controller.widget() == &child)
                    .and_then(|tc| tc.cwd.clone())
            })
        } else {
            None
        };

        AppWindow::new(
            &app,
            AppInit {
                incoming_tab_view: None,
                working_dir,
            },
        );
    }
}

pub fn settings_changed(inner: &mut AppWindowInner, settings: Settings) {
    inner.current_settings = settings.clone();
    inner.preferences.sync_settings(&settings);

    let style_manager = libadwaita::StyleManager::default();
    let scheme = match settings.color_scheme {
        boxxy_preferences::config::ColorScheme::Default => libadwaita::ColorScheme::Default,
        boxxy_preferences::config::ColorScheme::Light => libadwaita::ColorScheme::ForceLight,
        boxxy_preferences::config::ColorScheme::Dark => libadwaita::ColorScheme::ForceDark,
    };
    if style_manager.color_scheme() != scheme {
        style_manager.set_color_scheme(scheme);
    }

    let parsed = boxxy_themes::load_palette(settings.theme.as_str());
    let is_dark = libadwaita::StyleManager::default().is_dark();
    boxxy_themes::apply_palette(parsed.as_ref(), is_dark);

    let variant = parsed
        .as_ref()
        .map(|p| if is_dark { p.dark } else { p.light });

    for tab in &inner.tabs {
        tab.controller.update_settings(settings.clone(), variant);
    }

    if inner.sidebar_toolbar.width_request() != settings.ai_chat_width {
        inner
            .sidebar_toolbar
            .set_width_request(settings.ai_chat_width);
    }

    // from global settings here. Those are window-local states that can be toggled independently per window.

    inner
        .claw
        .update_ui(inner.claw_active, inner.claw_proactive);

    super::tabs::sync_header_title(inner);
}

pub fn theme_selected(inner: &mut AppWindowInner, palette: boxxy_themes::ParsedPaletteStatic) {
    inner.current_settings.theme = palette.id.to_string();
    inner.current_settings.save();

    let is_dark = libadwaita::StyleManager::default().is_dark();
    let variant = if is_dark { palette.dark } else { palette.light };

    for tab in &inner.tabs {
        tab.controller
            .update_settings(inner.current_settings.clone(), Some(variant));
    }

    inner.preferences.sync_settings(&inner.current_settings);

    boxxy_themes::apply_palette(Some(&palette), is_dark);
}

pub fn sidebar_visible_changed(inner: &mut AppWindowInner, visible: bool) {
    inner.sidebar_visible = visible;
    inner.app_state.sidebar_visible = visible;
    inner.app_state.save();
    if !visible {
        super::tabs::focus_active_terminal(inner);
    }
}

pub fn sidebar_page_changed(inner: &mut AppWindowInner, name: String) {
    inner.app_state.active_sidebar_page = name;
    inner.app_state.save();
}

pub fn sidebar_width_changed(inner: &mut AppWindowInner, width: i32) {
    if inner.current_settings.ai_chat_width != width {
        let mut settings = boxxy_preferences::Settings::load();
        settings.ai_chat_width = width;
        settings.save();

        inner.current_settings.ai_chat_width = width;
    }
}

pub fn toggle_sidebar(inner: &mut AppWindowInner) {
    let is_visible = inner.split_view.shows_sidebar();
    inner.split_view.set_show_sidebar(!is_visible);
}

pub fn show_themes_sidebar(inner: &mut AppWindowInner) {
    if !inner.sidebar_visible {
        inner.split_view.set_show_sidebar(true);
    }
    inner.view_stack.set_visible_child_name("themes");
}

pub fn show_ai_chat(inner: &mut AppWindowInner) {
    if !inner.sidebar_visible {
        inner.split_view.set_show_sidebar(true);
    }
    inner.view_stack.set_visible_child_name("assistant");
    inner.ai_chat.grab_focus();
}

pub fn show_claw_sidebar(inner: &mut AppWindowInner) {
    if !inner.sidebar_visible {
        inner.split_view.set_show_sidebar(true);
    }
    inner.view_stack.set_visible_child_name("claw");
}

pub fn show_bookmarks_sidebar(inner: &mut AppWindowInner) {
    if !inner.sidebar_visible {
        inner.split_view.set_show_sidebar(true);
    }
    inner.view_stack.set_visible_child_name("bookmarks");
}
