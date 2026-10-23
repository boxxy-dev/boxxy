use boxxy_claw_protocol::TaskType;
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
    let mut window_pane_ids = std::collections::HashSet::new();
    for tab in &inner.tabs {
        pids.extend(tab.controller.get_pids());
        for pid_str in tab.controller.get_pane_ids() {
            window_pane_ids.insert(pid_str);
        }
    }

    let inner_clone = inner_ref.clone();
    gtk4::glib::spawn_future_local(async move {
        let agent = boxxy_terminal::get_agent().await;
        let mut running_apps = Vec::new();
        for pid in &pids {
            log::debug!(
                "handle_close_request: Querying running processes for PID {}",
                pid
            );
            if let Ok(Ok(mut procs)) = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                agent.get_running_processes(*pid),
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

        let pending_tasks: Vec<_> = agent
            .get_pending_tasks()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|t| !t.is_detached && window_pane_ids.contains(&t.pane_id))
            .collect();

        if running_apps.is_empty() && pending_tasks.is_empty() {
            let tabs: Vec<_> = inner_clone
                .borrow()
                .tabs
                .iter()
                .map(|t| t.controller.clone())
                .collect();
            for tab in tabs {
                tab.terminate_on_close_sync().await;
            }

            inner_clone.borrow().force_close.set(true);
            inner_clone.borrow().window.close();
            return;
        }

        // If "Always Keep Running Processes in Background" (`pty_persistence`) is on,
        // detach all tabs (the daemon automatically terminates idle shells and keeps
        // busy shells alive) and close without showing the confirmation dialog.
        if Settings::load().pty_persistence {
            let tabs: Vec<_> = inner_clone
                .borrow()
                .tabs
                .iter()
                .map(|t| t.controller.clone())
                .collect();
            for tab in tabs {
                tab.detach_on_close_sync().await;
            }

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

        for task in pending_tasks {
            let type_str = match task.task_type {
                TaskType::Notification => "Reminder",
                TaskType::Command => "Scheduled Command",
                TaskType::Query => "Scheduled Query",
            };
            let agent_name = if task.character_name.is_empty() {
                "AI Agent"
            } else {
                &task.character_name
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

        let extra_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
        extra_vbox.set_width_request(440);
        extra_vbox.append(&scrolled);

        let actions_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        actions_row.set_homogeneous(true);

        let cancel_btn = gtk4::Button::builder()
            .label("Cancel")
            .height_request(38)
            .build();
        let terminate_btn = gtk4::Button::builder()
            .label("Terminate All")
            .css_classes(["destructive-action"])
            .height_request(38)
            .build();
        let bg_btn = gtk4::Button::builder()
            .label("Run in Background")
            .css_classes(["suggested-action"])
            .height_request(38)
            .build();

        let chosen = std::rc::Rc::new(std::cell::Cell::new("cancel"));
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            cancel_btn.connect_clicked(move |_| {
                c.set("cancel");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            terminate_btn.connect_clicked(move |_| {
                c.set("close");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            bg_btn.connect_clicked(move |_| {
                c.set("background");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }

        actions_row.append(&cancel_btn);
        actions_row.append(&terminate_btn);
        actions_row.append(&bg_btn);
        extra_vbox.append(&actions_row);

        dialog.set_extra_child(Some(&extra_vbox));

        let window = inner_clone.borrow().window.clone();
        let _ = dialog.choose_future(Some(&window)).await;
        let response = chosen.get();
        if response == "background" {
            let tabs: Vec<_> = inner_clone
                .borrow()
                .tabs
                .iter()
                .map(|t| t.controller.clone())
                .collect();
            for tab in tabs {
                tab.detach_on_close_sync().await;
            }

            inner_clone.borrow().force_close.set(true);
            inner_clone.borrow().window.close();
        } else if response == "close" {
            let tabs: Vec<_> = inner_clone
                .borrow()
                .tabs
                .iter()
                .map(|t| t.controller.clone())
                .collect();
            for tab in tabs {
                tab.terminate_on_close_sync().await;
            }

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

    inner.claw.refresh_mcp_list(&settings);

    for tab in &inner.tabs {
        tab.controller.update_settings(settings.clone(), variant);
        tab.controller.notify_settings_invalidated();
    }

    let bg_rgba = if let Some(ref p) = parsed {
        let variant = if is_dark { &p.dark } else { &p.light };
        gtk4::gdk::RGBA::parse(variant.background)
            .unwrap_or_else(|_| gtk4::gdk::RGBA::new(0.08, 0.08, 0.1, 1.0))
    } else {
        gtk4::gdk::RGBA::new(0.08, 0.08, 0.1, 1.0)
    };
    inner.frosted_container.set_tint_color(bg_rgba);
    inner.frosted_container.set_opacity(settings.opacity);
    inner.frosted_container.set_frosted_glass(settings.frosted_glass);

    if settings.frosted_glass && settings.opacity < 1.0 {
        inner.window.add_css_class("frosted-glass-active");
    } else {
        inner.window.remove_css_class("frosted-glass-active");
    }

    // Re-push credentials so the daemon's in-memory `core.state.api_keys`
    // track any changes the user just made in Settings → APIs.
    // (The engine also falls back to disk, but keeping the IPC state
    // current avoids that fallback path on every turn.)
    let effective_keys = settings.get_effective_api_keys();
    let ollama_url = settings.ollama_base_url.clone();

    gtk4::glib::spawn_future_local(async move {
        let agent = boxxy_terminal::get_agent().await;
        let _ = agent.update_credentials(effective_keys, ollama_url).await;
        let _ = agent.notify_settings_invalidated().await;
    });

    if inner.sidebar_toolbar.width_request() != settings.ai_chat_width {
        inner
            .sidebar_toolbar
            .set_width_request(settings.ai_chat_width);
    }

    // from global settings here. Those are window-local states that can be toggled independently per window.

    super::tabs::sync_header_title(inner);
}

pub fn theme_selected(inner: &mut AppWindowInner, palette: boxxy_themes::ParsedPaletteStatic) {
    inner.current_settings.theme = palette.id.to_string();
    inner.current_settings.save();

    let is_dark = libadwaita::StyleManager::default().is_dark();
    let variant = if is_dark { palette.dark } else { palette.light };

    if let Ok(rgba) = gtk4::gdk::RGBA::parse(variant.background) {
        inner.frosted_container.set_tint_color(rgba);
    }

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
    inner.sidebar_overlay.set_visible(visible);
    if !visible {
        super::tabs::focus_active_terminal(inner);
    }
}

pub fn sidebar_page_changed(inner: &mut AppWindowInner, name: String) {
    if name == "claw" {
        refresh_claw_maintenance_status(inner.tx.clone());
    }
    inner.app_state.active_sidebar_page = name;
    inner.app_state.save();
}

fn refresh_claw_maintenance_status(tx: async_channel::Sender< crate::state::AppInput>) {
    gtk4::glib::spawn_future_local(async move {
        let agent = boxxy_terminal::get_agent().await;
        if let Ok((status, on_battery)) = agent.get_maintenance_status().await {
            let _ = tx
                .send(crate::state::AppInput::RefreshMaintenanceStatus(
                    status, on_battery,
                ))
                .await;
        }
    });
}

pub fn sidebar_width_changed(inner: &mut AppWindowInner, width: i32) {
    if inner.current_settings.ai_chat_width != width {
        boxxy_preferences::Settings::update(|settings| {
            settings.ai_chat_width = width;
        });

        inner.current_settings.ai_chat_width = width;
    }
}

pub fn toggle_sidebar(inner: &mut AppWindowInner) {
    let is_visible = inner.split_view.shows_sidebar();
    if !is_visible {
        inner.sidebar_overlay.set_visible(true);
    }
    inner.split_view.set_show_sidebar(!is_visible);
}

pub fn show_themes_sidebar(inner: &mut AppWindowInner) {
    if !inner.sidebar_visible {
        inner.sidebar_overlay.set_visible(true);
        inner.split_view.set_show_sidebar(true);
    }
    inner.view_stack.set_visible_child_name("themes");
}

pub fn show_ai_chat(inner: &mut AppWindowInner) {
    if !inner.sidebar_visible {
        inner.sidebar_overlay.set_visible(true);
        inner.split_view.set_show_sidebar(true);
    }
    inner.view_stack.set_visible_child_name("assistant");
    inner.ai_chat.grab_focus();
}

pub fn show_claw_sidebar(inner: &mut AppWindowInner) {
    if inner.split_view.shows_sidebar()
        && inner.view_stack.visible_child_name().as_deref() == Some("claw")
    {
        inner.split_view.set_show_sidebar(false);
    } else {
        if !inner.sidebar_visible {
            inner.sidebar_overlay.set_visible(true);
            inner.split_view.set_show_sidebar(true);
        }
        inner.view_stack.set_visible_child_name("claw");
        refresh_claw_maintenance_status(inner.tx.clone());
    }
}

pub fn show_bookmarks_sidebar(inner: &mut AppWindowInner) {
    if !inner.sidebar_visible {
        inner.sidebar_overlay.set_visible(true);
        inner.split_view.set_show_sidebar(true);
    }
    inner.view_stack.set_visible_child_name("bookmarks");
}
