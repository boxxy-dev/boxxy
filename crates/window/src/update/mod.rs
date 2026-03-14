pub mod events;
pub mod window_state;
pub mod tabs;
pub mod split;

use std::cell::RefCell;
use std::rc::Rc;
use gtk4::prelude::*;

use crate::state::{AppInput, AppWindowInner};

pub fn update(inner_ref: &Rc<RefCell<AppWindowInner>>, msg: AppInput) {
    let mut inner = inner_ref.borrow_mut();

    match msg {
        // AppWindow Level
        AppInput::SaveWindowState { width, height, is_maximized } => {
            window_state::save_window_state(&mut inner, width, height, is_maximized);
        }
        AppInput::CloseRequested => {
            window_state::handle_close_request(inner_ref, &mut inner);
        }
        AppInput::NewWindow => {
            window_state::new_window(&inner);
        }
        AppInput::SettingsChanged(settings) => {
            window_state::settings_changed(&mut inner, settings);
        }
        AppInput::ThemeSelected(theme) => {
            window_state::theme_selected(&mut inner, *theme);
        }
        AppInput::SidebarVisibleChanged(visible) => {
            window_state::sidebar_visible_changed(&mut inner, visible);
        }
        AppInput::SidebarPageChanged(name) => {
            window_state::sidebar_page_changed(&mut inner, name);
        }
        AppInput::SidebarWidthChanged(width) => {
            window_state::sidebar_width_changed(&mut inner, width);
        }
        AppInput::ToggleSidebar => {
            window_state::toggle_sidebar(&mut inner);
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
        AppInput::SetClawActive(active) => {
            if active {
                inner.claw_indicator.remove_css_class("claw-indicator-inactive");
                inner.claw_indicator.set_tooltip_text(Some("Claw Agent Options (Enabled)"));
            } else {
                inner.claw_indicator.add_css_class("claw-indicator-inactive");
                inner.claw_indicator.set_tooltip_text(Some("Claw Agent Options (Disabled)"));
            }
            inner.claw_popover.update_ui(active, &inner.current_settings);
            for tab in &inner.tabs {
                tab.controller.set_claw_active(active);
            }
            inner.claw.update_active(active);
        }

        // Tabs
        AppInput::NewTab => {
            tabs::new_tab(&mut inner);
            tabs::sync_header_title(&inner);
        }
        AppInput::CloseTabRequest(key) => {
            tabs::close_tab_request(&mut inner, key);
            tabs::sync_header_title(&inner);
        }
        AppInput::CloseTab(id) => {
            tabs::close_tab(&mut inner, id);
        }
        AppInput::CloseActiveTab => {
            if let Some(page) = inner.tab_view.selected_page() {
                inner.tab_view.close_page(&page);
            }
        }
        AppInput::MoveTabToNewWindowRequest(key) => {
            tabs::move_tab_to_new_window_request(&mut inner, key);
            tabs::sync_header_title(&inner);
        }
        AppInput::AdoptOrphanTabs => {
            tabs::adopt_orphan_tabs(&mut inner);
            tabs::sync_header_title(&inner);
        }
        AppInput::TabPageDetached(key) => {
            tabs::tab_page_detached(&mut inner, key);
            tabs::sync_header_title(&inner);
        }
        AppInput::TabPageAttached(key) => {
            tabs::tab_page_attached(&mut inner, key);
            tabs::sync_header_title(&inner);
        }

        // Terminal Events
        AppInput::HandleTerminalEvent(Some(event)) => {
            events::handle_terminal_event(inner_ref, &mut inner, event);
        }
        AppInput::HandleTerminalEvent(None) => {}
        
        // Terminal Focus/Zoom
        AppInput::FocusActiveTerminal => {
            tabs::focus_active_terminal(&mut inner);
        }
        AppInput::ZoomIn => {
            let mut settings = inner.current_settings.clone();
            settings.font_size = (settings.font_size + 1).min(72);
            settings.save();
        }
        AppInput::ZoomOut => {
            let mut settings = inner.current_settings.clone();
            settings.font_size = (settings.font_size - 1).max(6);
            settings.save();
        }
        AppInput::Copy => {
            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
                    inner.tabs[pos].controller.copy();
                }
            }
        }
        AppInput::Paste => {
            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
                    inner.tabs[pos].controller.paste();
                }
            }
        }
        AppInput::OpenInFiles => {
            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(pos) = inner.tabs.iter().position(|c| c.controller.widget() == &child) {
                    inner.tabs[pos].controller.open_in_files();
                }
            }
        }

        // Splits
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

        // Dialogs/Menus
        AppInput::OpenPreferences => {
            inner.preferences.show(&inner.window.clone().upcast());
        }
        AppInput::OpenBoxxyApps => {
            tabs::open_boxxy_apps(&mut inner);
        }
        AppInput::OpenShortcuts => {
            inner.shortcuts.show(&inner.window.clone().upcast());
        }
        AppInput::OpenAbout => {
            inner.about.show(&inner.window.clone().upcast());
        }
        AppInput::ShowAppMenu(x, y) => {
            let rect = gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            let ctx = boxxy_app_menu::AppMenuContext {
                is_maximized: false,
                path_to_copy: None,
                has_selection: false,
            };
            inner.app_menu.show(rect, ctx);
        }
        AppInput::ShowCommandPaletteMenu => {
            inner.command_palette.show_as_menu(&inner.menu_btn);
        }
        AppInput::ModelSelection => {
            let win = inner.window.clone();
            let tx_ai = inner.tx.clone();
            let tx_claw = inner.tx.clone();
            
            let tx_mem = inner.tx.clone();

            let dialog = boxxy_model_selection::GlobalModelSelectorDialog::new(
                inner.current_settings.ai_chat_model.clone(),
                inner.current_settings.claw_model.clone(),
                inner.current_settings.memory_model.clone(),
                inner.current_settings.ollama_base_url.clone(),
                move |m| {
                    let mut s = boxxy_preferences::Settings::load();
                    s.ai_chat_model = m;
                    s.save();
                    let _ = tx_ai.send_blocking(AppInput::SettingsChanged(s));
                },
                move |m| {
                    let mut s = boxxy_preferences::Settings::load();
                    s.claw_model = m;
                    s.save();
                    let _ = tx_claw.send_blocking(AppInput::SettingsChanged(s));
                },
                move |m| {
                    let mut s = boxxy_preferences::Settings::load();
                    s.memory_model = m;
                    s.save();
                    let _ = tx_mem.send_blocking(AppInput::SettingsChanged(s));
                }
            );
            dialog.present(Some(&win));
        }
        AppInput::CommandPalette => {
            inner.command_palette.show(&inner.window);
        }
        AppInput::ReloadEngine => {
            for tab in &inner.tabs {
                tab.controller.reload_claw();
            }
        }
    }
}
