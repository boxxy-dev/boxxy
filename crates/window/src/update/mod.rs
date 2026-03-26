pub mod events;
pub mod split;
pub mod tabs;
pub mod window_state;

use gtk4::prelude::*;
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
        AppInput::SetClawActive(active) => {
            inner.claw_active = active;
            if active {
                inner
                    .claw_indicator
                    .remove_css_class("claw-indicator-inactive");
                inner
                    .claw_indicator
                    .set_tooltip_text(Some("Claw Agent Options (Enabled)"));
            } else {
                inner
                    .claw_indicator
                    .add_css_class("claw-indicator-inactive");
                inner
                    .claw_indicator
                    .set_tooltip_text(Some("Claw Agent Options (Disabled)"));
            }
            inner
                .claw_popover
                .update_ui(inner.claw_active, inner.claw_proactive);
            inner
                .claw
                .update_ui(inner.claw_active, inner.claw_proactive);
            for tab in &inner.tabs {
                tab.controller.set_claw_active(active);
            }
        }
        AppInput::SetClawProactive(proactive) => {
            inner.claw_proactive = proactive;
            inner
                .claw_popover
                .update_ui(inner.claw_active, inner.claw_proactive);
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
            // Note: cross-window sync is disabled in `settings_changed` to allow per-window modes.
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
            inner.notification_pill.set_notification(ready);
        }
        AppInput::DismissNotification(id) => {
            inner.notifications.retain(|n| n.id != id);
            if let Some(current) = inner.notification_pill.get_notification()
                && current.id == id
            {
                inner.notification_pill.clear();
                // Show next notification if any
                if let Some(next) = inner.notifications.first() {
                    inner.notification_pill.set_notification(next.clone());
                }
            }
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
            inner.notification_pill.set_notification(ready);
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
    }
}
