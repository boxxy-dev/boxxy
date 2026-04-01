pub mod events;
pub mod split;
pub mod tabs;
pub mod window_state;

use gtk4::gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::state::{AppInput, AppWindowInner};

pub fn update(inner_ref: &Rc<RefCell<AppWindowInner>>, input: AppInput) {
    let mut inner = inner_ref.borrow_mut();
    match input {
        AppInput::NewWindow => {
            window_state::new_window(&inner);
        }
        AppInput::NewTab => {
            tabs::new_tab(&mut inner);
        }
        AppInput::CloseTabRequest(key) => {
            tabs::close_tab_request(&mut inner, key);
        }
        AppInput::CloseTab(id) => {
            tabs::close_tab(&mut inner, id);
        }
        AppInput::CloseActiveTab => {
            if let Some(page) = inner.tab_view.selected_page() {
                let key = page.child().as_ptr() as usize;
                tabs::close_tab_request(&mut inner, key);
            }
        }
        AppInput::HandleTerminalEvent(event) => {
            if let Some(event) = event {
                events::handle_terminal_event(inner_ref, &mut inner, event);
            }
        }
        AppInput::MoveTabToNewWindowRequest(key) => {
            tabs::move_tab_to_new_window_request(&mut inner, key);
        }
        AppInput::SetTabColor(key, color) => {
            tabs::set_tab_color(&mut inner, key, color);
        }
        AppInput::SetTabTitle(key, title) => {
            tabs::set_tab_title(&mut inner, key, title);
        }
        AppInput::SyncTabColors => {
            tabs::sync_tab_colors(&mut inner);
        }
        AppInput::AdoptOrphanTabs => {
            tabs::adopt_orphan_tabs(&mut inner);
        }
        AppInput::FocusActiveTerminal => {
            tabs::focus_active_terminal(&mut inner);
        }
        AppInput::TabPageAttached(key) => {
            tabs::tab_page_attached(&mut inner, key);
        }
        AppInput::TabPageDetached(key) => {
            tabs::tab_page_detached(&mut inner, key);
        }
        AppInput::ToggleSidebar => {
            window_state::toggle_sidebar(&mut inner);
        }
        AppInput::SidebarVisibleChanged(visible) => {
            window_state::sidebar_visible_changed(&mut inner, visible);
        }
        AppInput::SidebarPageChanged(page) => {
            window_state::sidebar_page_changed(&mut inner, page);
        }
        AppInput::OpenPreferences => {
            inner.preferences.show(&inner.window.clone().upcast());
        }
        AppInput::OpenBookmarks => {
            tabs::open_bookmarks(&mut inner);
        }
        AppInput::OpenShortcuts => {
            inner.preferences.show_page("shortcuts");
            inner.preferences.show(&inner.window.clone().upcast());
        }
        AppInput::OpenAbout => {
            inner.preferences.show_page("about");
            inner.preferences.show(&inner.window.clone().upcast());
        }
        AppInput::OpenInFiles => {
            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(tc) = inner.tabs.iter().find(|c| c.controller.widget() == &child) {
                    tc.controller.open_in_files();
                }
            }
        }
        AppInput::ShowAppMenu(x, y) => {
            let rect = gtk4::gdk::Rectangle::new(x as i32, y as i32, 0, 0);

            let has_selection = if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                inner
                    .tabs
                    .iter()
                    .find(|c| c.controller.widget() == &child)
                    .map(|tc| tc.controller.has_selection())
                    .unwrap_or(false)
            } else {
                false
            };

            let ctx = crate::app_menu::AppMenuContext {
                is_maximized: inner.app_state.is_maximized,
                path_to_copy: None,
                has_selection,
            };
            inner.app_menu.show(rect, ctx);
        }
        AppInput::ShowCommandPaletteMenu => {
            inner.command_palette.show(&inner.window);
        }
        AppInput::SettingsChanged(settings) => {
            window_state::settings_changed(&mut inner, settings);
        }
        AppInput::ZoomIn => {
            let mut settings = boxxy_preferences::Settings::load();
            settings.font_size += 1;
            settings.save();
            let _ = inner.tx.send_blocking(AppInput::SettingsChanged(settings));
        }
        AppInput::ZoomOut => {
            let mut settings = boxxy_preferences::Settings::load();
            if settings.font_size > 4 {
                settings.font_size -= 1;
                settings.save();
                let _ = inner.tx.send_blocking(AppInput::SettingsChanged(settings));
            }
        }
        AppInput::ResetZoom => {
            let mut settings = boxxy_preferences::Settings::load();
            let default_settings = boxxy_preferences::Settings::default();
            settings.font_size = default_settings.font_size;
            settings.save();
            let _ = inner.tx.send_blocking(AppInput::SettingsChanged(settings));
        }
        AppInput::Copy => {
            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget() == &child)
                {
                    inner.tabs[pos].controller.copy();
                }
            }
        }
        AppInput::Paste => {
            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget() == &child)
                {
                    inner.tabs[pos].controller.paste();
                }
            }
        }
        AppInput::SplitVertical => {
            split::split_vertical(&mut inner);
        }
        AppInput::SplitHorizontal => {
            split::split_horizontal(&mut inner);
        }
        AppInput::CloseSplit => {
            split::close_split(&mut inner);
        }
        AppInput::ToggleMaximize => {
            split::toggle_maximize(&mut inner);
        }
        AppInput::FocusLeft => {
            split::focus(&mut inner, boxxy_terminal::Direction::Left);
        }
        AppInput::FocusRight => {
            split::focus(&mut inner, boxxy_terminal::Direction::Right);
        }
        AppInput::FocusUp => {
            split::focus(&mut inner, boxxy_terminal::Direction::Up);
        }
        AppInput::FocusDown => {
            split::focus(&mut inner, boxxy_terminal::Direction::Down);
        }
        AppInput::SwapLeft => {
            split::swap(&mut inner, boxxy_terminal::Direction::Left);
        }
        AppInput::SwapRight => {
            split::swap(&mut inner, boxxy_terminal::Direction::Right);
        }
        AppInput::SwapUp => {
            split::swap(&mut inner, boxxy_terminal::Direction::Up);
        }
        AppInput::SwapDown => {
            split::swap(&mut inner, boxxy_terminal::Direction::Down);
        }
        AppInput::ShowThemesSidebar => {
            window_state::show_themes_sidebar(&mut inner);
        }
        AppInput::ShowAiChat => {
            window_state::show_ai_chat(&mut inner);
        }
        AppInput::ShowClawSidebar => {
            window_state::show_claw_sidebar(&mut inner);
        }
        AppInput::ShowBookmarksSidebar => {
            window_state::show_bookmarks_sidebar(&mut inner);
        }
        AppInput::ExecuteBookmark(name, filename, script) => {
            let template = boxxy_bookmarks::parser::BookmarkTemplate::parse(&script);
            let placeholders = template.placeholders;
            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget() == &child)
                {
                    inner.tabs[pos].controller.show_bookmark_proposal(
                        &name,
                        &filename,
                        &script,
                        placeholders,
                    );
                }
            }
        }
        AppInput::ExecuteInNewTab(name, filename, script) => {
            let template = boxxy_bookmarks::parser::BookmarkTemplate::parse(&script);
            let placeholders = template.placeholders;
            tabs::new_tab(&mut inner);
            tabs::sync_header_title(&inner);

            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget() == &child)
                {
                    let tc = inner.tabs[pos].controller.clone();
                    gtk4::glib::timeout_add_local_once(
                        std::time::Duration::from_millis(150),
                        move || {
                            tc.show_bookmark_proposal(&name, &filename, &script, placeholders);
                        },
                    );
                }
            }
        }
        AppInput::SetClawActive(active, pane_id) => {
            let id = if let Some(id) = pane_id {
                Some(id)
            } else {
                // Get active pane ID from selected tab
                if let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if let Some(pos) = inner
                        .tabs
                        .iter()
                        .position(|c| c.controller.widget() == &child)
                    {
                        Some(inner.tabs[pos].controller.active_pane_id())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(id) = id {
                let mut found = false;
                for tab in &inner.tabs {
                    if tab.controller.set_claw_active_for_pane(&id, active) {
                        found = true;
                        break;
                    }
                }

                // If we updated the active pane, update the UI
                if found && let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if let Some(pos) = inner
                        .tabs
                        .iter()
                        .position(|c| c.controller.widget() == &child)
                        && inner.tabs[pos].controller.active_pane_id() == id
                    {
                        inner.claw_active = active;
                        if active {
                            page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                                "boxxy-boxxyclaw-symbolic",
                            )));
                            page.set_indicator_activatable(false);
                        } else {
                            page.set_indicator_icon(None::<&gio::Icon>);
                        }
                        inner
                            .claw
                            .update_ui(inner.claw_active, inner.claw_proactive);
                    }
                }
            } else {
                // If no pane identified (e.g. no tabs open), just update the window state
                inner.claw_active = active;
                inner
                    .claw
                    .update_ui(inner.claw_active, inner.claw_proactive);
            }
        }
        AppInput::SetClawActiveGlobal(active) => {
            inner.claw_active = active;
            inner
                .claw
                .update_ui(inner.claw_active, inner.claw_proactive);

            for tab in &inner.tabs {
                tab.controller.set_claw_active(active);

                let widget = tab.controller.widget();
                let page = inner.tab_view.page(widget);
                if active {
                    page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                        "boxxy-boxxyclaw-symbolic",
                    )));
                    page.set_indicator_activatable(false);
                } else {
                    page.set_indicator_icon(None::<&gio::Icon>);
                }
            }
        }
        AppInput::SetClawProactive(proactive, pane_id) => {
            let mode = if proactive {
                boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
            } else {
                boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
            };

            let id = if let Some(id) = pane_id {
                Some(id)
            } else {
                // Get active pane ID from selected tab
                if let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if let Some(pos) = inner
                        .tabs
                        .iter()
                        .position(|c| c.controller.widget() == &child)
                    {
                        Some(inner.tabs[pos].controller.active_pane_id())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(id) = id {
                let mut found = false;
                for tab in &inner.tabs {
                    if tab.controller.update_diagnosis_mode_for_pane(&id, &mode) {
                        found = true;
                        break;
                    }
                }

                // If we updated the active pane, update the UI
                if found && let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if let Some(pos) = inner
                        .tabs
                        .iter()
                        .position(|c| c.controller.widget() == &child)
                        && inner.tabs[pos].controller.active_pane_id() == id
                    {
                        inner.claw_proactive = proactive;
                        inner
                            .claw
                            .update_ui(inner.claw_active, inner.claw_proactive);
                    }
                }
            } else {
                inner.claw_proactive = proactive;
                inner
                    .claw
                    .update_ui(inner.claw_active, inner.claw_proactive);
            }
        }
        AppInput::SetClawProactiveGlobal(proactive) => {
            inner.claw_proactive = proactive;
            inner
                .claw
                .update_ui(inner.claw_active, inner.claw_proactive);
            let mode = if proactive {
                boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
            } else {
                boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
            };
            for tab in &inner.tabs {
                tab.controller.update_diagnosis_mode(&mode);
            }

            // Save settings for persistence (default for new windows)
            let mut settings = boxxy_preferences::Settings::load();
            settings.claw_auto_diagnosis_mode = mode;
            settings.save();
        }
        AppInput::ThemeSelected(palette) => {
            window_state::theme_selected(&mut inner, *palette);
        }
        AppInput::CommandPalette => {
            inner.command_palette.show(&inner.window);
        }
        AppInput::ReloadEngine => {
            // TODO: Signal all tabs to reload their claw session?
        }
        AppInput::ModelSelection => {
            let dialog = boxxy_model_selection::GlobalModelSelectorDialog::new(
                inner.current_settings.ai_chat_model.clone(),
                inner.current_settings.claw_model.clone(),
                inner.current_settings.memory_model.clone(),
                inner.current_settings.ollama_base_url.clone(),
                inner.current_settings.api_keys.clone(),
                move |provider| {
                    let mut settings = boxxy_preferences::Settings::load();
                    settings.ai_chat_model = provider;
                    settings.save();
                },
                move |provider| {
                    let mut settings = boxxy_preferences::Settings::load();
                    settings.claw_model = provider;
                    settings.save();
                },
                move |provider| {
                    let mut settings = boxxy_preferences::Settings::load();
                    settings.memory_model = provider;
                    settings.save();
                },
            );
            dialog.present(Some(&inner.window));
        }
        AppInput::CloseRequested => {
            window_state::handle_close_request(inner_ref, &mut inner);
        }
        AppInput::SidebarWidthChanged(width) => {
            window_state::sidebar_width_changed(&mut inner, width);
        }
        AppInput::SaveWindowState {
            width,
            height,
            is_maximized,
        } => {
            window_state::save_window_state(&mut inner, width, height, is_maximized);
        }
        AppInput::PushNotification(ready) => {
            inner.notifications.push(ready.clone());

            let toast = adw::Toast::new(&ready.message);

            if ready.level == crate::widgets::notification::NotificationLevel::Update {
                toast.set_timeout(0); // Permanent until dismissed
                toast.set_button_label(Some("Details"));

                let tx = inner.tx.clone();
                let notification = ready.clone();
                let toast_overlay = inner.toast_overlay.clone();
                toast.connect_button_clicked(move |_| {
                    let dialog = adw::AlertDialog::builder()
                        .heading(&notification.title)
                        .body(&notification.message)
                        .build();

                    let details_grid = gtk4::Grid::new();
                    details_grid.set_column_spacing(12);
                    details_grid.set_row_spacing(6);

                    let mut row = 0;
                    for (key, value) in &notification.details {
                        if key == "Url" || key == "ChecksumUrl" {
                            continue;
                        }
                        let key_label = gtk4::Label::builder()
                            .label(key)
                            .halign(gtk4::Align::Start)
                            .css_classes(["dim-label"])
                            .build();
                        let value_label = gtk4::Label::builder()
                            .label(value)
                            .halign(gtk4::Align::Start)
                            .build();

                        details_grid.attach(&key_label, 0, row, 1, 1);
                        details_grid.attach(&value_label, 1, row, 1, 1);
                        row += 1;
                    }
                    dialog.set_extra_child(Some(&details_grid));

                    for action in &notification.actions {
                        dialog.add_response(&action.action_name, &action.label);
                        if action.is_primary {
                            dialog.set_response_appearance(
                                &action.action_name,
                                adw::ResponseAppearance::Suggested,
                            );
                        }
                    }

                    let url = notification
                        .details
                        .iter()
                        .find(|(k, _)| k == "Url")
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let date = notification
                        .details
                        .iter()
                        .find(|(k, _)| k == "Date")
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let checksum_url = notification
                        .details
                        .iter()
                        .find(|(k, _)| k == "ChecksumUrl")
                        .map(|(_, v)| v.clone());
                    let id = notification.id.clone();
                    let tx_dialog = tx.clone();

                    if let Some(root) = toast_overlay.root().and_downcast::<gtk4::Window>() {
                        dialog.choose(Some(&root), gtk4::gio::Cancellable::NONE, move |response| {
                            if response == "win.start-download" {
                                let _ = tx_dialog.send_blocking(
                                    crate::state::AppInput::StartUpdateDownload(
                                        url.clone(),
                                        date.clone(),
                                        checksum_url.clone(),
                                    ),
                                );
                                let _ = tx_dialog.send_blocking(
                                    crate::state::AppInput::DismissNotification(id.clone()),
                                );
                            } else if response == "win.apply-update" {
                                let _ = tx_dialog
                                    .send_blocking(crate::state::AppInput::ApplyUpdateAndRestart);
                            } else if response == "win.dismiss-notification" {
                                let _ = tx_dialog.send_blocking(
                                    crate::state::AppInput::DismissNotification(id.clone()),
                                );
                            }
                        });
                    }
                });
            } else {
                toast.set_timeout(5);
            }

            inner.toast_overlay.add_toast(toast);
        }
        AppInput::DismissNotification(id) => {
            inner.notifications.retain(|n| n.id != id);
        }
        AppInput::StartUpdateDownload(url, date, checksum_url) => {
            let tx_download = inner.tx.clone();
            gtk4::glib::spawn_future_local(async move {
                match crate::updater::Updater::download_update(url, date, checksum_url).await {
                    Ok(path) => {
                        let _ = tx_download.send_blocking(AppInput::UpdateDownloaded(
                            path.to_string_lossy().to_string(),
                        ));
                    }
                    Err(e) => {
                        log::error!("Failed to download update: {}", e);
                    }
                }
            });
        }
        AppInput::UpdateDownloaded(_path) => {
            // Show the "Update Ready" notification
            let ready = crate::widgets::notification::Notification::new_update_ready("nightly");
            inner.notifications.push(ready.clone());

            let toast = adw::Toast::new(&ready.message);
            toast.set_timeout(0); // Permanent until dismissed
            toast.set_button_label(Some("Restart"));

            let tx = inner.tx.clone();
            toast.connect_button_clicked(move |_| {
                let _ = tx.send_blocking(AppInput::ApplyUpdateAndRestart);
            });

            inner.toast_overlay.add_toast(toast);
        }
        AppInput::ApplyUpdateAndRestart => {
            let _ = crate::updater::Updater::apply_update_and_restart();
        }
        AppInput::GrabFocus => {
            if let Some(page) = inner.tab_view.selected_page() {
                let target_ptr = page.child().as_ptr() as usize;
                if let Some(tab) = inner
                    .tabs
                    .iter()
                    .find(|t| t.controller.widget().as_ptr() as usize == target_ptr)
                {
                    tab.controller.grab_focus();
                }
            }
        }
        AppInput::CancelTask(task_id, pane_id) => {
            let id = if let Some(id) = pane_id {
                Some(id)
            } else {
                // Get active pane ID from selected tab
                if let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if let Some(pos) = inner
                        .tabs
                        .iter()
                        .position(|c| c.controller.widget() == &child)
                    {
                        Some(inner.tabs[pos].controller.active_pane_id())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(id) = id {
                for tab in &inner.tabs {
                    if tab.controller.cancel_task_by_id(&id, task_id) {
                        break;
                    }
                }
            }
        }
        AppInput::ClearClawHistory(pane_id) => {
            let id = if let Some(id) = pane_id {
                Some(id)
            } else {
                // Get active pane ID from selected tab
                if let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if let Some(pos) = inner
                        .tabs
                        .iter()
                        .position(|c| c.controller.widget() == &child)
                    {
                        Some(inner.tabs[pos].controller.active_pane_id())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(id) = id {
                for tab in &inner.tabs {
                    if tab.controller.soft_clear_claw_history(&id) {
                        break;
                    }
                }
            }
        }
    }
}
