pub mod claw;
pub mod close_dialog;
pub mod detached_sessions;
pub mod events;
pub mod notifications;
pub mod split;
pub mod tab_bar_sync;
pub mod tabs;
pub mod whats_new_dialog;
pub mod window_state;

use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

use crate::state::{AppInput, AppWindowInner, ToastRequest};

fn is_active_tab_claw_drawer_visible(inner: &AppWindowInner) -> bool {
    inner
        .active_tab()
        .is_some_and(|t| t.controller.is_active_claw_drawer_visible())
}

fn open_preferences_page(inner: &mut AppWindowInner, page: Option<&str>) {
    let win: gtk4::Window = inner.window.clone().upcast();
    let prefs = inner.ensure_preferences();
    if let Some(page_name) = page {
        prefs.show_page(page_name);
    }
    prefs.show(&win);
}

pub fn update(inner_ref: &Rc<RefCell<AppWindowInner>>, input: AppInput) {
    let mut inner = inner_ref.borrow_mut();
    match input {
        AppInput::NewWindow => {
            window_state::new_window(&inner);
        }
        AppInput::NewTab => {
            tabs::new_tab(&mut inner);
        }
        AppInput::EnsureInitialTab => {
            if inner.tabs.is_empty() {
                tabs::new_tab(&mut inner);
            }
        }
        AppInput::NewTabLaunch(spec) => {
            tabs::new_tab_launch(&mut inner, spec);
        }
        AppInput::CloseTabRequest(key) => {
            tabs::close_tab_request(inner_ref, &mut inner, key);
        }
        AppInput::CloseTab(id) => {
            tabs::close_tab(&mut inner, id);
        }
        AppInput::CloseActiveTab => {
            if let Some(page) = inner.tab_view.selected_page() {
                inner.tab_view.close_page(&page);
            }
        }
        AppInput::HandleTerminalEvent(event) => {
            events::handle_terminal_event(&mut inner, *event);
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
            open_preferences_page(&mut inner, None);
        }
        AppInput::OpenPreferencesPage(ref page) => {
            open_preferences_page(&mut inner, Some(page));
        }
        AppInput::OpenThemesPreferences => {
            open_preferences_page(&mut inner, Some("themes"));
        }
        AppInput::OpenApisPreferences => {
            open_preferences_page(&mut inner, Some("apis"));
        }
        AppInput::OpenMcpPreferences => {
            open_preferences_page(&mut inner, Some("mcp"));
        }
        AppInput::OpenShortcuts => {
            open_preferences_page(&mut inner, Some("shortcuts"));
        }
        AppInput::OpenAbout => {
            open_preferences_page(&mut inner, Some("about"));
        }
        AppInput::OpenBookmarks => {
            tabs::open_bookmarks(&mut inner);
        }
        AppInput::OpenWhatsNew => {
            whats_new_dialog::open_whats_new(&inner);
        }
        AppInput::OpenInFiles => {
            if let Some(tc) = inner.active_tab() {
                tc.controller.open_in_files();
            }
        }
        AppInput::CommandPalette => {
            inner.command_palette.show(&inner.window);
        }
        AppInput::SettingsChanged(settings) => {
            window_state::settings_changed(&mut inner, *settings);
        }
        AppInput::ToggleClawCodeBlocks => {
            let settings = boxxy_preferences::Settings::update(|s| {
                s.always_show_code_blocks = !s.always_show_code_blocks;
            });
            let enabled = settings.always_show_code_blocks;
            window_state::settings_changed(&mut inner, (*settings).clone());
            let toast_msg = if enabled {
                "Claw Code & Diff Blocks: Always Expanded"
            } else {
                "Claw Code & Diff Blocks: Collapsed in Trace"
            };
            let toast = adw::Toast::new(toast_msg);
            toast.set_use_markup(false);
            toast.set_timeout(3);
            inner.toast_overlay.add_toast(toast);
        }
        AppInput::ZoomIn => {
            if is_active_tab_claw_drawer_visible(&inner) {
                boxxy_terminal::TerminalComponent::zoom_in_claw_overlay();
                return;
            }
            let settings = boxxy_preferences::Settings::update(|s| {
                s.font_size += 1;
            });
            window_state::settings_changed(&mut inner, (*settings).clone());
        }
        AppInput::ZoomOut => {
            if is_active_tab_claw_drawer_visible(&inner) {
                boxxy_terminal::TerminalComponent::zoom_out_claw_overlay();
                return;
            }
            if boxxy_preferences::Settings::load().font_size > 4 {
                let settings = boxxy_preferences::Settings::update(|s| {
                    if s.font_size > 4 {
                        s.font_size -= 1;
                    }
                });
                window_state::settings_changed(&mut inner, (*settings).clone());
            }
        }
        AppInput::ResetZoom => {
            if is_active_tab_claw_drawer_visible(&inner) {
                boxxy_terminal::TerminalComponent::reset_claw_overlay_zoom();
                return;
            }
            let default_font_size = boxxy_preferences::Settings::default().font_size;
            let settings = boxxy_preferences::Settings::update(|s| {
                s.font_size = default_font_size;
            });
            window_state::settings_changed(&mut inner, (*settings).clone());
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
        AppInput::MovePaneToSplit {
            source_pane_id,
            target_pane_id,
            direction,
        } => {
            split::move_pane_to_split(&mut inner, source_pane_id, target_pane_id, direction);
        }
        AppInput::DropPaneOnTab {
            source_pane_id,
            target_page_key,
        } => {
            split::drop_pane_on_tab(&mut inner, source_pane_id, target_page_key);
        }
        AppInput::ShowThemesSidebar => {
            window_state::show_themes_sidebar(&mut inner);
        }
        AppInput::ShowClawSidebar => {
            window_state::show_claw_sidebar(&mut inner);
        }
        AppInput::ShowBookmarksSidebar => {
            window_state::show_bookmarks_sidebar(&mut inner);
        }
        AppInput::ExecuteBookmark(name, filename, script) => {
            let template = boxxy_bookmarks::parser::BookmarkTemplate::parse(&script);
            if let Some(tab) = inner.active_tab() {
                tab.controller.show_bookmark_proposal(
                    &name,
                    &filename,
                    &script,
                    template.placeholders,
                );
            }
        }
        AppInput::ExecuteInNewTab(name, filename, script) => {
            let template = boxxy_bookmarks::parser::BookmarkTemplate::parse(&script);
            let placeholders = template.placeholders;
            tabs::new_tab(&mut inner);
            tab_bar_sync::sync_header_title(&inner);

            if let Some(tab) = inner.active_tab() {
                let tc = tab.controller.clone();
                gtk4::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(150),
                    move || {
                        tc.show_bookmark_proposal(&name, &filename, &script, placeholders);
                    },
                );
            }
        }
        AppInput::SetClawActive(active, pane_id) => {
            claw::set_claw_active(&mut inner, active, pane_id);
        }
        AppInput::SetClawActiveGlobal(active) => {
            claw::set_claw_active_global(&mut inner, active);
        }
        AppInput::ThemeSelected(palette) => {
            window_state::theme_selected(&mut inner, *palette);
        }
        AppInput::ReloadEngine => {
            claw::reload_engine(&mut inner);
        }
        AppInput::ModelSelection => {
            claw::open_model_selection(&inner, None);
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
        AppInput::ShowToast(ToastRequest {
            message,
            timeout_secs,
        }) => {
            let toast = adw::Toast::new(&message);
            toast.set_use_markup(false);
            toast.set_timeout(timeout_secs);
            inner.toast_overlay.add_toast(toast);
        }
        AppInput::PushGlobalNotification(ready) => {
            notifications::push_global_notification(&mut inner, *ready);
        }
        AppInput::DismissNotification(id) => {
            notifications::dismiss_notification(&mut inner, &id);
        }
        AppInput::StartUpdateDownload(url, date, new_commit, checksum_url) => {
            notifications::start_update_download(&inner, url, date, new_commit, checksum_url);
        }
        AppInput::UpdateDownloaded(_path) => {
            notifications::update_downloaded(&mut inner);
        }
        AppInput::ApplyUpdateAndRestart => {
            notifications::apply_update_and_restart(&inner);
        }
        AppInput::ShowWhatsNewDialog(pending, summary) => {
            whats_new_dialog::show_whats_new_dialog(&inner, pending, summary.map(|b| *b));
        }
        AppInput::StartWhatsNewClaw(prompt) => {
            whats_new_dialog::start_whats_new_claw(&mut inner, prompt);
        }
        AppInput::CancelTask(task_id, pane_id) => {
            claw::cancel_task(&inner, task_id, pane_id);
        }
        AppInput::TogglePauseTask(task_id, pane_id) => {
            claw::toggle_pause_task(&inner, task_id, pane_id);
        }
        AppInput::CancelAllTasks(pane_id) => {
            claw::cancel_all_tasks(&inner, pane_id);
        }
        AppInput::ClearClawHistory(pane_id) => {
            claw::clear_claw_history(&inner, pane_id);
        }
        AppInput::ResumeClawSession(session_id, override_character_id) => {
            claw::resume_claw_session(&inner, &session_id, override_character_id);
        }
        AppInput::RefreshMaintenanceStatus(status, on_battery) => {
            inner.claw.set_maintenance_status(&status, on_battery);
        }
        AppInput::RefreshMcpStatuses(statuses) => {
            inner.claw.set_mcp_statuses(statuses);
        }
        AppInput::PrewarmDeferredUi => {
            let _ = inner.ensure_preferences();
            inner.theme_selector.ensure_populated();
        }
        AppInput::ReattachDetachedSession(pid, cwd) => {
            detached_sessions::reattach_session(&mut inner, pid, cwd);
        }
        AppInput::ReattachDetachedBatch(sessions) => {
            detached_sessions::reattach_batch(&mut inner, sessions);
        }
        AppInput::SpawnCharacterTab(character_id, cwd) => {
            let _ = tabs::new_tab_with_character(&mut inner, character_id, cwd);
        }
        AppInput::RefreshDetachedSessions => {
            detached_sessions::refresh_detached_sessions(&inner);
        }
        AppInput::UpdateDetachedSessionsBadge(sessions) => {
            detached_sessions::update_badge(&inner, &sessions);
        }
        AppInput::ShowDetachedSessions => {
            detached_sessions::show_detached_sessions_dialog(&inner);
        }
    }
}
