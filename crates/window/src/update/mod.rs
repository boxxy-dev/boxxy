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

fn is_active_tab_claw_drawer_visible(inner: &AppWindowInner) -> bool {
    let Some(page) = inner.tab_view.selected_page() else {
        return false;
    };
    let child = page.child();
    inner
        .tabs
        .iter()
        .find(|t| t.controller.widget() == &child)
        .is_some_and(|t| t.controller.is_active_claw_drawer_visible())
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
        AppInput::NewTabInDir(dir) => {
            tabs::new_tab_in_dir(&mut inner, dir);
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
        AppInput::OpenApisPreferences => {
            inner.preferences.show_page("apis");
            inner.preferences.show(&inner.window.clone().upcast());
        }
        AppInput::OpenMcpPreferences => {
            inner.preferences.show_page("mcp");
            inner.preferences.show(&inner.window.clone().upcast());
        }
        AppInput::OpenBookmarks => {
            tabs::open_bookmarks(&mut inner);
        }
        AppInput::OpenShortcuts => {
            inner.preferences.show_page("shortcuts");
            inner.preferences.show(&inner.window.clone().upcast());
        }
        AppInput::OpenWhatsNew => {
            let pending = crate::updater::whats_new::load_last_whats_new();
            let tx_wn = inner.tx.clone();
            tokio::spawn(async move {
                let summary = crate::updater::whats_new::fetch_changelog_summary(
                    &pending.from_commit,
                    &pending.to_commit,
                )
                .await
                .ok();
                let _ = tx_wn
                    .send(AppInput::ShowWhatsNewDialog(pending, summary))
                    .await;
            });
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
            if is_active_tab_claw_drawer_visible(&inner) {
                boxxy_terminal::TerminalComponent::zoom_in_claw_overlay();
                return;
            }
            let settings = boxxy_preferences::Settings::update(|s| {
                s.font_size += 1;
            });
            let _ = inner
                .tx
                .send_blocking(AppInput::SettingsChanged((*settings).clone()));
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
                let _ = inner
                    .tx
                    .send_blocking(AppInput::SettingsChanged((*settings).clone()));
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
            let tx_apis = inner.tx.clone();
            let dialog = boxxy_model_selection::GlobalModelSelectorDialog::new_with_custom(
                inner.current_settings.ai_chat_model.clone(),
                inner.current_settings.claw_model.clone(),
                inner.current_settings.memory_model.clone(),
                inner.current_settings.ollama_base_url.clone(),
                inner.current_settings.get_effective_api_keys(),
                inner.current_settings.openai_compatible_endpoints.clone(),
                move |ai_provider, claw_provider, memory_provider| {
                    boxxy_preferences::Settings::update(|settings| {
                        settings.ai_chat_model = ai_provider.clone();
                        settings.claw_model = claw_provider.clone();
                        settings.memory_model = memory_provider.clone();
                    });
                },
            );
            dialog.set_on_open_apis(move || {
                let _ = tx_apis.send_blocking(AppInput::OpenApisPreferences);
            });
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
            let is_update = ready.level == crate::widgets::notification::NotificationLevel::Update;
            if is_update {
                if inner
                    .notifications
                    .iter()
                    .any(|n| n.id == ready.id && n.message == ready.message && n.details == ready.details)
                {
                    return;
                }
                inner.notifications.retain(|n| n.id != ready.id);
            }

            let title = ready.title.clone();
            let msg = ready.message.clone();
            // Agent reminders and long-task desktop notifications are emitted directly
            // by `boxxy-agent` over `org.freedesktop.Notifications` so they work reliably
            // on non-GNOME desktops (COSMIC, KDE, Sway) as well as GNOME. Only send
            // `gio::Notification` here for app-level update alerts when unfocused.
            let should_notify = is_update && !inner.window.is_active();

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
                    let current_commit = notification
                        .details
                        .iter()
                        .find(|(k, _)| k == "Current Commit")
                        .map(|(_, v)| v.as_str())
                        .unwrap_or_default();
                    let new_commit = notification
                        .details
                        .iter()
                        .find(|(k, _)| k == "New Commit")
                        .map(|(_, v)| v.clone())
                        .unwrap_or_else(|| "nightly".to_string());

                    let body_markup = if !current_commit.is_empty()
                        && current_commit != "unknown"
                        && !new_commit.is_empty()
                        && new_commit != "nightly"
                    {
                        let compare_url = format!(
                            "https://github.com/{}/{}/compare/{}...{}",
                            crate::updater::REPO_OWNER,
                            crate::updater::REPO_NAME,
                            gtk4::glib::markup_escape_text(current_commit),
                            gtk4::glib::markup_escape_text(&new_commit),
                        );
                        format!(
                            "<a href=\"{compare_url}\"><span underline=\"none\">See commit changes on GitHub</span></a>"
                        )
                    } else {
                        gtk4::glib::markup_escape_text(&notification.message).to_string()
                    };

                    let dialog = adw::AlertDialog::builder()
                        .heading(&notification.title)
                        .body(&body_markup)
                        .body_use_markup(true)
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
                                        new_commit.clone(),
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
        AppInput::StartUpdateDownload(url, date, new_commit, checksum_url) => {
            let tx_download = inner.tx.clone();
            tokio::spawn(async move {
                match crate::updater::Updater::download_update(url, date, new_commit, checksum_url)
                    .await
                {
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
        AppInput::ShowWhatsNewDialog(pending, summary) => {
            crate::updater::whats_new::save_last_whats_new(&pending);

            let claw_ready = matches!(
                inner
                    .current_settings
                    .resolve_model_state(&inner.current_settings.claw_model),
                boxxy_preferences::config::ModelConfigState::Ready(_)
            );

            let builder =
                gtk4::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/whats_new_dialog.ui");
            let Some(dialog) = builder.object::<adw::Dialog>("whats_new_dialog") else {
                return;
            };

            if let Some(banner_picture) = builder.object::<gtk4::Picture>("banner_picture") {
                banner_picture
                    .set_resource(Some("/dev/boxxy/BoxxyTerminal/icons/whats_new_banner.png"));
            }

            if let Some(hero_subtitle) = builder.object::<gtk4::Label>("hero_subtitle_label") {
                if claw_ready {
                    hero_subtitle.set_label("Hot out of the compiler! Let Boxxy give you the tour.");
                } else {
                    hero_subtitle.set_label(
                        "Hot out of the compiler! Copy the prompt below for your favorite AI.",
                    );
                }
            }

            let commits_stack = builder.object::<gtk4::Stack>("commits_stack");
            let detail_scrolled_for_back =
                builder.object::<gtk4::ScrolledWindow>("detail_scrolled");
            if let Some(back_btn) = builder.object::<gtk4::Button>("commit_back_btn") {
                let stack_weak = commits_stack.as_ref().map(gtk4::prelude::ObjectExt::downgrade);
                let detail_sc_weak = detail_scrolled_for_back
                    .as_ref()
                    .map(gtk4::prelude::ObjectExt::downgrade);
                back_btn.connect_clicked(move |_| {
                    if let Some(ref scw) = detail_sc_weak
                        && let Some(sc) = scw.upgrade()
                    {
                        sc.set_min_content_height(-1);
                        sc.set_height_request(-1);
                    }
                    if let Some(ref sw) = stack_weak
                        && let Some(stack) = sw.upgrade()
                    {
                        stack.set_visible_child_name("list");
                    }
                });
            }

            let state_rc = Rc::new(RefCell::new((pending.clone(), summary.clone())));
            populate_whats_new_summary(&builder, &pending, summary.as_ref());

            let range_stack = builder.object::<gtk4::Stack>("commit_range_stack");
            let from_entry = builder.object::<gtk4::Entry>("from_commit_entry");
            let to_entry = builder.object::<gtk4::Entry>("to_commit_entry");
            let edit_range_btn = builder.object::<gtk4::Button>("edit_range_btn");
            let refresh_range_btn = builder.object::<gtk4::Button>("refresh_range_btn");

            if let Some(ref edit_btn) = edit_range_btn {
                let range_stack_c = range_stack.clone();
                let from_entry_c = from_entry.clone();
                let to_entry_c = to_entry.clone();
                let state_c = state_rc.clone();
                edit_btn.connect_clicked(move |_| {
                    let (ref cur_pending, _) = *state_c.borrow();
                    if let Some(ref fe) = from_entry_c {
                        fe.set_text(&cur_pending.from_commit);
                    }
                    if let Some(ref te) = to_entry_c {
                        te.set_text(&cur_pending.to_commit);
                    }
                    if let Some(ref st) = range_stack_c {
                        st.set_visible_child_name("edit");
                    }
                    if let Some(ref fe) = from_entry_c {
                        fe.grab_focus();
                    }
                });
            }

            let trigger_refresh: Rc<dyn Fn()> = {
                let builder_c = builder.clone();
                let range_stack_c = range_stack.clone();
                let from_entry_c = from_entry.clone();
                let to_entry_c = to_entry.clone();
                let state_c = state_rc.clone();
                let tx_toast = inner.tx.clone();
                Rc::new(move || {
                    let from_val = from_entry_c
                        .as_ref()
                        .map(|e| e.text().trim().to_string())
                        .unwrap_or_default();
                    let to_val = to_entry_c
                        .as_ref()
                        .map(|e| e.text().trim().to_string())
                        .unwrap_or_default();

                    if from_val.is_empty() || to_val.is_empty() {
                        return;
                    }

                    if let Some(ref st) = range_stack_c {
                        st.set_visible_child_name("display");
                    }
                    if let Some(commit_pill) = builder_c.object::<gtk4::Label>("commit_pill_label")
                    {
                        commit_pill.set_label(&format!("{from_val} → {to_val} …"));
                    }

                    let new_pending = crate::updater::whats_new::WhatsNewPending {
                        from_commit: crate::updater::whats_new::shorten_sha(&from_val),
                        to_commit: crate::updater::whats_new::shorten_sha(&to_val),
                    };

                    let builder_async = builder_c.clone();
                    let state_async = state_c.clone();
                    let tx_toast_async = tx_toast.clone();
                    gtk4::glib::spawn_future_local(async move {
                        let p_fetch = new_pending.clone();
                        let res = boxxy_sys_utils::runtime()
                            .spawn(async move {
                                crate::updater::whats_new::fetch_changelog_summary(
                                    &p_fetch.from_commit,
                                    &p_fetch.to_commit,
                                )
                                .await
                            })
                            .await;

                        match res {
                            Ok(Ok(new_summary)) if new_summary.total_commits > 0 => {
                                crate::updater::whats_new::save_last_whats_new(&new_pending);
                                populate_whats_new_summary(
                                    &builder_async,
                                    &new_pending,
                                    Some(&new_summary),
                                );
                                *state_async.borrow_mut() = (new_pending, Some(new_summary));
                            }
                            _ => {
                                let (ref prev_pending, _) = *state_async.borrow();
                                if let Some(commit_pill) =
                                    builder_async.object::<gtk4::Label>("commit_pill_label")
                                {
                                    commit_pill.set_label(&format!(
                                        "{} → {}",
                                        prev_pending.from_commit, prev_pending.to_commit
                                    ));
                                }
                                let _ = tx_toast_async.send_blocking(AppInput::ShowToast(
                                    crate::state::ToastRequest::ephemeral(format!(
                                        "Could not fetch commits for {}...{}",
                                        new_pending.from_commit, new_pending.to_commit
                                    )),
                                ));
                            }
                        }
                    });
                })
            };

            if let Some(ref refresh_btn) = refresh_range_btn {
                let trig = trigger_refresh.clone();
                refresh_btn.connect_clicked(move |_| trig());
            }
            if let Some(ref fe) = from_entry {
                let trig = trigger_refresh.clone();
                fe.connect_activate(move |_| trig());
            }
            if let Some(ref te) = to_entry {
                let trig = trigger_refresh.clone();
                te.connect_activate(move |_| trig());
            }

            if let Some(copy_btn) = builder.object::<gtk4::Button>("copy_prompt_btn") {
                let copy_icon = builder.object::<gtk4::Image>("copy_prompt_icon");
                let tx_copy = inner.tx.clone();
                let state_copy = state_rc.clone();
                copy_btn.connect_clicked(move |_| {
                    let (ref cur_pending, ref cur_summary) = *state_copy.borrow();
                    let clip_text = crate::updater::whats_new::build_clipboard_whats_new_prompt(
                        cur_pending,
                        cur_summary.as_ref(),
                    );
                    if let Some(display) = gtk4::gdk::Display::default() {
                        display.clipboard().set_text(&clip_text);
                    }
                    if let Some(ref icon) = copy_icon {
                        icon.set_icon_name(Some("boxxy-object-select-symbolic"));
                    }
                    let _ = tx_copy.send_blocking(AppInput::ShowToast(
                        crate::state::ToastRequest::ephemeral(
                            "What's New prompt copied to clipboard",
                        ),
                    ));
                });
            }

            if let Some(close_btn) = builder.object::<gtk4::Button>("close_btn") {
                let dialog_weak = dialog.downgrade();
                close_btn.connect_clicked(move |_| {
                    if let Some(d) = dialog_weak.upgrade() {
                        d.close();
                    }
                });
            }

            if let Some(read_btn) = builder.object::<gtk4::Button>("read_btn") {
                let read_btn_label = builder.object::<gtk4::Label>("read_btn_label");
                let update_read_btn_state = {
                    let read_btn_c = read_btn.clone();
                    let read_btn_label_c = read_btn_label.clone();
                    move || {
                        let chars_available =
                            boxxy_claw_protocol::characters::has_available_character(None);
                        if !claw_ready {
                            read_btn_c.set_sensitive(false);
                            if let Some(ref lbl) = read_btn_label_c {
                                lbl.set_label("Read What's New");
                            }
                            read_btn_c.set_tooltip_text(Some(
                                "Configure a Claw AI model in Preferences to let Boxxy read what's new",
                            ));
                        } else if !chars_available {
                            read_btn_c.set_sensitive(false);
                            if let Some(ref lbl) = read_btn_label_c {
                                lbl.set_label("No Characters Free");
                            }
                            read_btn_c.set_tooltip_text(Some(
                                "All characters are currently active in other sessions",
                            ));
                        } else {
                            read_btn_c.set_sensitive(true);
                            if let Some(ref lbl) = read_btn_label_c {
                                lbl.set_label("Read What's New");
                            }
                            read_btn_c.set_tooltip_text(None);
                        }
                    }
                };
                update_read_btn_state();

                // Reactively update the button if a character claim is released or taken while the dialog is open.
                let read_btn_weak = read_btn.downgrade();
                let mut rx_reg = boxxy_claw_protocol::characters::subscribe_registry_changes();
                gtk4::glib::spawn_future_local(async move {
                    while rx_reg.changed().await.is_ok() {
                        if read_btn_weak.upgrade().is_none() {
                            break;
                        }
                        update_read_btn_state();
                    }
                });

                let dialog_weak = dialog.downgrade();
                let tx_read = inner.tx.clone();
                let state_read = state_rc.clone();
                read_btn.connect_clicked(move |_| {
                    if !boxxy_claw_protocol::characters::has_available_character(None) {
                        return;
                    }
                    if let Some(d) = dialog_weak.upgrade() {
                        d.close();
                    }
                    let (ref cur_pending, ref cur_summary) = *state_read.borrow();
                    let prompt = crate::updater::whats_new::build_claw_whats_new_prompt(
                        cur_pending,
                        cur_summary.as_ref(),
                    );
                    let _ = tx_read.send_blocking(AppInput::StartWhatsNewClaw(prompt));
                });
            }

            dialog.present(Some(&inner.window));
        }
        AppInput::StartWhatsNewClaw(prompt) => {
            if !boxxy_claw_protocol::characters::has_available_character(None) {
                let toast = adw::Toast::new("No characters available to read What's New");
                toast.set_timeout(3);
                inner.toast_overlay.add_toast(toast);
                return;
            }
            if inner.tabs.is_empty() {
                tabs::new_tab(&mut inner);
            }

            // Ensure we target an idle pane that does not already have a running Claw session.
            let active_tab_has_claw = inner
                .tab_view
                .selected_page()
                .and_then(|page| {
                    let target_ptr = page.child().as_ptr() as usize;
                    inner
                        .tabs
                        .iter()
                        .find(|t| t.controller.widget().as_ptr() as usize == target_ptr)
                })
                .is_some_and(|t| t.is_claw_active());

            if active_tab_has_claw {
                tabs::new_tab(&mut inner);
            }

            if let Some(page) = inner.tab_view.selected_page() {
                let target_ptr = page.child().as_ptr() as usize;
                if let Some(tab) = inner
                    .tabs
                    .iter()
                    .find(|t| t.controller.widget().as_ptr() as usize == target_ptr)
                {
                    let _ = tab.start_claw_query_with_default_character(prompt);
                }
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
        AppInput::ResumeClawSession(session_id) => {
            if let Some(page) = inner.tab_view.selected_page() {
                let child = page.child();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget() == &child)
                {
                    inner.tabs[pos].controller.resume_session(&session_id);
                }
            }
        }
        AppInput::RefreshMaintenanceStatus(status, on_battery) => {
            inner.claw.set_maintenance_status(&status, on_battery);
        }
        AppInput::ReattachDetachedSession(pid, cwd) => {
            let _ = tabs::new_tab_reattach(&mut inner, pid, cwd);
            let tx = inner.tx.clone();
            gtk4::glib::spawn_future_local(async move {
                let _ = tx.send(AppInput::RefreshDetachedSessions).await;
                let _ = tx.send(AppInput::FocusActiveTerminal).await;
            });
        }
        AppInput::ReattachDetachedBatch(sessions) => {
            tabs::reattach_sessions_grouped(&mut inner, sessions);
            let tx = inner.tx.clone();
            gtk4::glib::spawn_future_local(async move {
                let _ = tx.send(AppInput::RefreshDetachedSessions).await;
                let _ = tx.send(AppInput::FocusActiveTerminal).await;
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

                let remaining_rows = std::rc::Rc::new(std::cell::Cell::new(sessions.len()));
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

                    let mut subtitle_str = format!("{} • {}", cwd_display, idle_display);
                    if !s.character_name.is_empty()
                        && !title
                            .to_lowercase()
                            .starts_with(&s.character_name.to_lowercase())
                    {
                        subtitle_str.push_str(&format!(" • {}", s.character_name));
                    }
                    if s.pending_task_count > 0 {
                        subtitle_str.push_str(&format!(
                            " • {} reminder{}",
                            s.pending_task_count,
                            if s.pending_task_count == 1 { "" } else { "s" }
                        ));
                    }

                    let row = adw::ActionRow::builder()
                        .title(gtk4::glib::markup_escape_text(&title))
                        .subtitle(gtk4::glib::markup_escape_text(&subtitle_str))
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
                    let dialog_weak_reattach = dialog.downgrade();
                    let rem_reattach = remaining_rows.clone();
                    reattach_btn.connect_clicked(move |btn| {
                        btn.set_sensitive(false);
                        if let Some(r) = row_weak_reattach.upgrade() {
                            r.set_visible(false);
                        }
                        let left = rem_reattach.get().saturating_sub(1);
                        rem_reattach.set(left);
                        if left == 0
                            && let Some(dlg) = dialog_weak_reattach.upgrade()
                        {
                            dlg.close();
                        }
                        let _ = tx_reattach.send_blocking(AppInput::ReattachDetachedSession(
                            pid,
                            cwd_opt.clone(),
                        ));
                    });

                    let tx_kill = tx.clone();
                    let row_weak_kill = row.downgrade();
                    let dialog_weak_kill = dialog.downgrade();
                    let rem_kill = remaining_rows.clone();
                    kill_btn.connect_clicked(move |btn| {
                        btn.set_sensitive(false);
                        if let Some(r) = row_weak_kill.upgrade() {
                            r.set_visible(false);
                        }
                        let left = rem_kill.get().saturating_sub(1);
                        rem_kill.set(left);
                        if left == 0
                            && let Some(dlg) = dialog_weak_kill.upgrade()
                        {
                            dlg.close();
                        }
                        let tx_after_kill = tx_kill.clone();
                        gtk4::glib::spawn_future_local(async move {
                            let agent = boxxy_terminal::get_agent().await;
                            let _ = agent.terminate_detached_session(pid).await;
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
                        if let Ok(remaining) = agent.list_detached_sessions_detailed().await
                            && !remaining.is_empty()
                        {
                            let _ = tx
                                .send(AppInput::ReattachDetachedBatch(remaining))
                                .await;
                        }
                    }
                    "terminate-all" => {
                        let _ = agent.terminate_detached_session(0).await;
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    _ => {}
                }
                let _ = tx.send(AppInput::RefreshDetachedSessions).await;
                let _ = tx.send(AppInput::FocusActiveTerminal).await;
            });
        }
    }
}

fn populate_whats_new_summary(
    builder: &gtk4::Builder,
    pending: &crate::updater::whats_new::WhatsNewPending,
    summary: Option<&crate::updater::whats_new::ChangelogSummary>,
) {
    if let Some(commit_pill) = builder.object::<gtk4::Label>("commit_pill_label") {
        commit_pill.set_label(&format!("{} → {}", pending.from_commit, pending.to_commit));
    }

    if let Some(s) = summary
        && s.total_commits > 0
    {
        if let Some(stats_card) = builder.object::<gtk4::Box>("stats_card") {
            stats_card.set_visible(true);
        }
        if let Some(commits_val) = builder.object::<gtk4::Label>("commits_val") {
            commits_val.set_label(&format!("📦 {}", s.total_commits));
        }
        if let Some(commits_caption) = builder.object::<gtk4::Label>("commits_caption") {
            let word = if s.total_commits == 1 {
                "Commit Shipped"
            } else {
                "Commits Shipped"
            };
            commits_caption.set_label(word);
        }
        if let Some(files_val) = builder.object::<gtk4::Label>("files_val") {
            files_val.set_label(&format!("🗂️ {}", s.files_changed));
        }
        if let Some(files_caption) = builder.object::<gtk4::Label>("files_caption") {
            let word = if s.files_changed == 1 {
                "File Touched"
            } else {
                "Files Touched"
            };
            files_caption.set_label(word);
        }
        if let Some(lines_val) = builder.object::<gtk4::Label>("lines_val") {
            lines_val.set_markup(&format!(
                "<span foreground=\"#4ade80\">+{}</span> <span alpha=\"50%\">/</span> <span foreground=\"#f87171\">-{}</span>",
                s.additions, s.deletions
            ));
        }
        let contributor_count = s.contributors.len().max(1);
        if let Some(contributors_val) = builder.object::<gtk4::Label>("contributors_val") {
            if s.contributors.is_empty() {
                contributors_val.set_label(&format!("👥 {contributor_count}"));
            } else {
                contributors_val.set_label(&contributor_count.to_string());
            }
        }
        if let Some(contributors_caption) = builder.object::<gtk4::Label>("contributors_caption") {
            let word = if contributor_count == 1 {
                "Contributor"
            } else {
                "Contributors"
            };
            let contributors_url = format!(
                "https://github.com/{}/{}/graphs/contributors",
                crate::updater::REPO_OWNER,
                crate::updater::REPO_NAME,
            );
            contributors_caption.set_markup(&format!(
                "<a href=\"{contributors_url}\"><span underline=\"none\">{word}</span></a>"
            ));
        }
        if let Some(avatars_box) = builder.object::<gtk4::Box>("contributors_avatars_box") {
            while let Some(child) = avatars_box.first_child() {
                avatars_box.remove(&child);
            }
            for contributor in s.contributors.iter().take(5) {
                let avatar = adw::Avatar::builder()
                    .size(22)
                    .text(&contributor.login)
                    .show_initials(true)
                    .tooltip_text(format!("@{}", contributor.login))
                    .build();
                if let Some(ref bytes) = contributor.avatar_bytes {
                    let gbytes = gtk4::glib::Bytes::from(bytes);
                    if let Ok(texture) = gtk4::gdk::Texture::from_bytes(&gbytes) {
                        avatar.set_custom_image(Some(&texture));
                    }
                }
                avatars_box.append(&avatar);
            }
        }
        if let Some(see_diff_label) = builder.object::<gtk4::Label>("see_diff_label") {
            let compare_url = format!(
                "https://github.com/{}/{}/compare/{}...{}",
                crate::updater::REPO_OWNER,
                crate::updater::REPO_NAME,
                gtk4::glib::markup_escape_text(&pending.from_commit),
                gtk4::glib::markup_escape_text(&pending.to_commit),
            );
            see_diff_label.set_markup(&format!(
                "<a href=\"{compare_url}\"><span underline=\"none\">See diff</span></a>"
            ));
        }

        let commits_stack = builder.object::<gtk4::Stack>("commits_stack");
        let detail_sha_label = builder.object::<gtk4::Label>("detail_sha_label");
        let detail_message_label = builder.object::<gtk4::Label>("detail_message_label");
        let detail_scrolled = builder.object::<gtk4::ScrolledWindow>("detail_scrolled");

        if let Some(commits_scrolled) = builder.object::<gtk4::ScrolledWindow>("commits_scrolled") {
            commits_scrolled.vadjustment().set_value(0.0);
        }

        if let Some(commits_list) = builder.object::<gtk4::Box>("commits_list") {
            while let Some(child) = commits_list.first_child() {
                commits_list.remove(&child);
            }
            for entry in s.commits.iter().rev() {
                let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);

                let sha_label = gtk4::Label::builder()
                    .label(&entry.short_sha)
                    .valign(gtk4::Align::Center)
                    .css_classes(["monospace", "whats-new-commit-sha"])
                    .build();

                let title_label = gtk4::Label::builder()
                    .label(entry.title())
                    .halign(gtk4::Align::Start)
                    .valign(gtk4::Align::Center)
                    .hexpand(true)
                    .xalign(0.0)
                    .ellipsize(gtk4::pango::EllipsizeMode::End)
                    .single_line_mode(true)
                    .css_classes(["whats-new-commit-title"])
                    .build();

                let chevron = gtk4::Image::builder()
                    .icon_name("boxxy-right-symbolic")
                    .valign(gtk4::Align::Center)
                    .opacity(0.45)
                    .build();

                row_box.append(&sha_label);
                row_box.append(&title_label);
                row_box.append(&chevron);

                let row_btn = gtk4::Button::builder()
                    .child(&row_box)
                    .focus_on_click(false)
                    .css_classes(["flat", "whats-new-commit-row"])
                    .build();

                let stack_weak =
                    commits_stack.as_ref().map(gtk4::prelude::ObjectExt::downgrade);
                let sha_weak = detail_sha_label
                    .as_ref()
                    .map(gtk4::prelude::ObjectExt::downgrade);
                let msg_weak = detail_message_label
                    .as_ref()
                    .map(gtk4::prelude::ObjectExt::downgrade);
                let scrolled_weak = detail_scrolled
                    .as_ref()
                    .map(gtk4::prelude::ObjectExt::downgrade);
                let short_sha = entry.short_sha.clone();
                let full_message = entry.message.clone();

                row_btn.connect_clicked(move |_| {
                    if let Some(ref sw) = sha_weak
                        && let Some(lbl) = sw.upgrade()
                    {
                        let escaped_sha = gtk4::glib::markup_escape_text(&short_sha);
                        let commit_url = format!(
                            "https://github.com/{}/{}/commit/{}",
                            crate::updater::REPO_OWNER,
                            crate::updater::REPO_NAME,
                            escaped_sha
                        );
                        lbl.set_markup(&format!(
                            "<a href=\"{commit_url}\"><span underline=\"none\">See {escaped_sha} on GitHub</span></a>"
                        ));
                    }
                    if let Some(ref mw) = msg_weak
                        && let Some(lbl) = mw.upgrade()
                    {
                        let mut lines = full_message.lines();
                        let first_line = lines.next().unwrap_or(&full_message).trim();
                        let rest = lines.collect::<Vec<_>>().join("\n").trim().to_string();
                        let markup = if rest.is_empty() {
                            format!("<b>{}</b>", gtk4::glib::markup_escape_text(first_line))
                        } else {
                            format!(
                                "<b>{}</b>\n\n<span alpha=\"85%\">{}</span>",
                                gtk4::glib::markup_escape_text(first_line),
                                gtk4::glib::markup_escape_text(&rest)
                            )
                        };
                        lbl.set_markup(&markup);
                    }
                    if let Some(ref scw) = scrolled_weak
                        && let Some(sc) = scw.upgrade()
                    {
                        sc.set_min_content_height(420);
                        sc.set_height_request(420);
                        sc.vadjustment().set_value(0.0);
                    }
                    if let Some(ref stw) = stack_weak
                        && let Some(stack) = stw.upgrade()
                    {
                        stack.set_visible_child_name("detail");
                    }
                });

                commits_list.append(&row_btn);
            }
        }
    } else if let Some(stats_card) = builder.object::<gtk4::Box>("stats_card") {
        stats_card.set_visible(false);
    }
}
