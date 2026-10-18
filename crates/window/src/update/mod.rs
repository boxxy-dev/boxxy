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
            let settings = boxxy_preferences::Settings::update(|s| {
                s.font_size += 1;
            });
            let _ = inner
                .tx
                .send_blocking(AppInput::SettingsChanged((*settings).clone()));
        }
        AppInput::ZoomOut => {
            if boxxy_preferences::Settings::load().font_size > 4 {
                let settings = boxxy_preferences::Settings::update(|s| {
                    if s.font_size > 4 {
                        s.font_size -= 1;
                    }
                });
                let _ = inner
                    .tx
                    .send_blocking(AppInput::SettingsChanged((*settings).clone()));
            }
        }
        AppInput::ResetZoom => {
            let default_font_size = boxxy_preferences::Settings::default().font_size;
            let settings = boxxy_preferences::Settings::update(|s| {
                s.font_size = default_font_size;
            });
            let _ = inner
                .tx
                .send_blocking(AppInput::SettingsChanged((*settings).clone()));
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
                    }
                }
            } else {
                // If no pane identified (e.g. no tabs open), just update the window state
                inner.claw_active = active;
            }
        }
        AppInput::SetClawActiveGlobal(active) => {
            inner.claw_active = active;

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
                inner.current_settings.get_effective_api_keys(),
                move |ai_provider, claw_provider, memory_provider| {
                    boxxy_preferences::Settings::update(|settings| {
                        settings.ai_chat_model = ai_provider.clone();
                        settings.claw_model = claw_provider.clone();
                        settings.memory_model = memory_provider.clone();
                    });
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
        AppInput::ShowToast(req) => {
            let toast = adw::Toast::new(&req.message);
            toast.set_timeout(req.timeout_secs);
            inner.toast_overlay.add_toast(toast);
        }
        AppInput::PushGlobalNotification(ready) => {
            let title = ready.title.clone();
            let msg = ready.message.clone();

            let is_update = ready.level == crate::widgets::notification::NotificationLevel::Update;
            let should_notify = !is_update || !inner.window.is_active();

            if should_notify {
                if let Some(app) = inner.window.application() {
                    let notif = gtk4::gio::Notification::new(&title);
                    notif.set_body(Some(&msg));

                    if ready.icon_name == "boxxyclaw-symbolic" {
                        if let Ok(bytes) = gtk4::gio::resources_lookup_data(
                            "/dev/boxxy/BoxxyTerminal/icons/boxxyclaw.svg",
                            gtk4::gio::ResourceLookupFlags::NONE,
                        ) {
                            notif.set_icon(&gtk4::gio::BytesIcon::new(&bytes));
                        }
                    } else if ready.icon_name == "boxxy-up-arrow-in-a-star-symbolic" {
                        let icon = gtk4::gio::ThemedIcon::new("software-update-available-symbolic");
                        notif.set_icon(&icon);
                    } else if ready.icon_name.starts_with("/") {
                        // Assume absolute file path for custom character avatars
                        let file = gtk4::gio::File::for_path(&ready.icon_name);
                        let icon = gtk4::gio::FileIcon::new(&file);
                        notif.set_icon(&icon);
                    } else if !ready.icon_name.is_empty() {
                        // Fallback for themed icons if needed
                        let icon = gtk4::gio::ThemedIcon::new(&ready.icon_name);
                        notif.set_icon(&icon);
                    }

                    app.send_notification(None, &notif);
                }
            }

            inner.notifications.push(ready.clone());

            let toast = adw::Toast::new(&ready.message);

            if is_update {
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

            // Only show the in-app toast if it's not suppressed.
            // This avoids UI spam for events that the user can already see in the active Claw drawer.
            if !ready.suppress_toast {
                inner.toast_overlay.add_toast(toast);
            }
        }
        AppInput::DismissNotification(id) => {
            inner.notifications.retain(|n| n.id != id);
        }
        AppInput::StartUpdateDownload(url, date, checksum_url) => {
            let tx_download = inner.tx.clone();
            tokio::spawn(async move {
                match crate::updater::Updater::download_update(url, date, checksum_url).await {
                    Ok(path) => {
                        let _ = tx_download
                            .send(AppInput::UpdateDownloaded(
                                path.to_string_lossy().to_string(),
                            ))
                            .await;
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
            if let Err(e) = crate::updater::Updater::apply_update_and_restart() {
                log::error!("Failed to apply update and restart: {e}");
            }
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
        AppInput::RefreshMaintenanceStatus(status, on_battery) => {
            inner.claw.set_maintenance_status(&status, on_battery);
        }
        AppInput::ReattachDetachedSession(pid, cwd) => {
            tabs::new_tab_reattach(&mut inner, pid, cwd);
            let tx = inner.tx.clone();
            gtk4::glib::spawn_future_local(async move {
                let _ = tx.send(AppInput::RefreshDetachedSessions).await;
            });
        }
        AppInput::RefreshDetachedSessions => {
            let tx = inner.tx.clone();
            gtk4::glib::spawn_future_local(async move {
                let agent = boxxy_terminal::get_agent().await;
                if let Ok(sessions) = agent.list_detached_sessions_detailed().await {
                    let _ = tx
                        .send(AppInput::UpdateDetachedSessionsBadge(sessions))
                        .await;
                }
            });
        }
        AppInput::UpdateDetachedSessionsBadge(sessions) => {
            if sessions.is_empty() {
                inner.detached_btn.set_visible(false);
                inner.detached_btn.remove_css_class("warning");
                return;
            }

            inner
                .detached_count_lbl
                .set_label(&sessions.len().to_string());
            inner.detached_btn.set_visible(true);
            if sessions.iter().any(|s| s.waiting_for_input) {
                inner.detached_btn.add_css_class("warning");
            } else {
                inner.detached_btn.remove_css_class("warning");
            }
        }
        AppInput::ShowDetachedSessions => {
            let window = inner.window.clone();
            let toast_overlay = inner.toast_overlay.clone();
            let tx = inner.tx.clone();
            gtk4::glib::spawn_future_local(async move {
                let agent = boxxy_terminal::get_agent().await;
                let sessions = match agent.list_detached_sessions_detailed().await {
                    Ok(list) => list,
                    Err(e) => {
                        log::warn!("Failed to list detached sessions: {e}");
                        Vec::new()
                    }
                };

                let _ = tx
                    .send(AppInput::UpdateDetachedSessionsBadge(sessions.clone()))
                    .await;

                if sessions.is_empty() {
                    let toast = adw::Toast::new("No detached background shells.");
                    toast.set_timeout(3);
                    toast_overlay.add_toast(toast);
                    return;
                }

                let dialog = adw::AlertDialog::builder()
                    .heading("Background Shells")
                    .body("Detached terminal sessions kept alive by the Boxxy daemon.")
                    .build();

                let group = adw::PreferencesGroup::new();
                for s in &sessions {
                    let proc_name = if s.foreground_process.is_empty() {
                        "Background Process".to_string()
                    } else {
                        s.foreground_process.clone()
                    };
                    let title = if s.waiting_for_input {
                        format!("{proc_name} — Waiting for input")
                    } else {
                        proc_name
                    };
                    let cwd_display = if s.cwd.is_empty() {
                        "~".to_string()
                    } else {
                        s.cwd.clone()
                    };
                    let idle_display = if s.idle_secs < 60 {
                        format!("{}s idle", s.idle_secs)
                    } else if s.idle_secs < 3600 {
                        format!("{}m idle", s.idle_secs / 60)
                    } else {
                        format!("{}h idle", s.idle_secs / 3600)
                    };

                    let row = adw::ActionRow::builder()
                        .title(gtk4::glib::markup_escape_text(&title))
                        .subtitle(gtk4::glib::markup_escape_text(&format!(
                            "PID {} • {} • {}",
                            s.pid, cwd_display, idle_display
                        )))
                        .build();

                    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                    btn_box.set_valign(gtk4::Align::Center);

                    let reattach_btn = gtk4::Button::builder()
                        .label("Reattach")
                        .css_classes(["suggested-action", "flat"])
                        .build();
                    let kill_btn = gtk4::Button::builder()
                        .label("Terminate")
                        .css_classes(["destructive-action", "flat"])
                        .build();

                    let pid = s.pid;
                    let cwd_opt = (!s.cwd.is_empty()).then(|| s.cwd.clone());
                    let tx_reattach = tx.clone();
                    let row_weak_reattach = row.downgrade();
                    reattach_btn.connect_clicked(move |btn| {
                        btn.set_sensitive(false);
                        if let Some(r) = row_weak_reattach.upgrade() {
                            r.set_visible(false);
                        }
                        let _ = tx_reattach.send_blocking(AppInput::ReattachDetachedSession(
                            pid,
                            cwd_opt.clone(),
                        ));
                    });

                    let tx_kill = tx.clone();
                    let row_weak_kill = row.downgrade();
                    kill_btn.connect_clicked(move |btn| {
                        btn.set_sensitive(false);
                        if let Some(r) = row_weak_kill.upgrade() {
                            r.set_visible(false);
                        }
                        let tx_after_kill = tx_kill.clone();
                        gtk4::glib::spawn_future_local(async move {
                            let agent = boxxy_terminal::get_agent().await;
                            let _ = agent.set_persistence(pid, false).await;
                            let _ = agent.signal_process_group(pid, 15).await;
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            let _ = tx_after_kill.send(AppInput::RefreshDetachedSessions).await;
                        });
                    });

                    btn_box.append(&reattach_btn);
                    btn_box.append(&kill_btn);
                    row.add_suffix(&btn_box);
                    group.add(&row);
                }

                let scrolled = gtk4::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk4::PolicyType::Never)
                    .vscrollbar_policy(gtk4::PolicyType::Automatic)
                    .max_content_height(280)
                    .propagate_natural_height(true)
                    .child(&group)
                    .build();

                let extra_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
                extra_vbox.set_width_request(440);
                extra_vbox.append(&scrolled);

                let actions_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
                actions_row.set_homogeneous(true);

                let close_btn = gtk4::Button::builder()
                    .label("Close")
                    .height_request(38)
                    .build();
                let terminate_all_btn = gtk4::Button::builder()
                    .label("Terminate All")
                    .css_classes(["destructive-action"])
                    .height_request(38)
                    .build();
                let restore_all_btn = gtk4::Button::builder()
                    .label("Restore All")
                    .css_classes(["suggested-action"])
                    .height_request(38)
                    .build();

                let chosen = std::rc::Rc::new(std::cell::Cell::new("close"));
                {
                    let c = chosen.clone();
                    let d = dialog.downgrade();
                    close_btn.connect_clicked(move |_| {
                        c.set("close");
                        if let Some(dlg) = d.upgrade() {
                            dlg.close();
                        }
                    });
                }
                {
                    let c = chosen.clone();
                    let d = dialog.downgrade();
                    terminate_all_btn.connect_clicked(move |_| {
                        c.set("terminate-all");
                        if let Some(dlg) = d.upgrade() {
                            dlg.close();
                        }
                    });
                }
                {
                    let c = chosen.clone();
                    let d = dialog.downgrade();
                    restore_all_btn.connect_clicked(move |_| {
                        c.set("restore-all");
                        if let Some(dlg) = d.upgrade() {
                            dlg.close();
                        }
                    });
                }

                actions_row.append(&close_btn);
                actions_row.append(&terminate_all_btn);
                actions_row.append(&restore_all_btn);
                extra_vbox.append(&actions_row);

                dialog.set_extra_child(Some(&extra_vbox));

                let _ = dialog.choose_future(Some(&window)).await;
                match chosen.get() {
                    "restore-all" => {
                        if let Ok(remaining) = agent.list_detached_sessions_detailed().await {
                            for s in remaining {
                                let cwd_opt = (!s.cwd.is_empty()).then_some(s.cwd);
                                let _ = tx
                                    .send(AppInput::ReattachDetachedSession(s.pid, cwd_opt))
                                    .await;
                            }
                        }
                    }
                    "terminate-all" => {
                        if let Ok(remaining) = agent.list_detached_sessions_detailed().await {
                            for s in remaining {
                                let _ = agent.set_persistence(s.pid, false).await;
                                let _ = agent.signal_process_group(s.pid, 15).await;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                    _ => {}
                }
                let _ = tx.send(AppInput::RefreshDetachedSessions).await;
            });
        }
    }
}
