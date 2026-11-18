use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use boxxy_bookmarks::manager::{BOOKMARKS_EVENT_BUS, BookmarksManager};
use boxxy_bookmarks::sidebar::BookmarksSidebarComponent;
use boxxy_claw_ui::ClawSidebarComponent;
use boxxy_command_palette::CommandPaletteComponent;
use boxxy_preferences::{AppState, Settings};
use boxxy_themes::ThemeSelectorComponent;

use crate::actions::setup_actions;
use crate::init::AppInit;
use crate::state::{AppInput, AppWindowInner};
use crate::tab_menu::{TabContextCallbacks, TabContextMenu};
use crate::widgets::tab_bar::BoxxyTabBar;

async fn sync_bookmarks_to_palette(cp: &CommandPaletteComponent) {
    let loaded = boxxy_sys_utils::runtime()
        .spawn_blocking(|| {
            let bookmarks = BookmarksManager::list();
            bookmarks
                .into_iter()
                .map(|bm| {
                    let script = BookmarksManager::get_script(&bm.filename).unwrap_or_default();
                    (bm.name, bm.filename, script)
                })
                .collect::<Vec<_>>()
        })
        .await;

    if let Ok(entries) = loaded {
        let commands = entries
            .into_iter()
            .map(|(name, filename, script)| {
                let param = (name.clone(), filename.clone(), script).to_variant();
                boxxy_command_palette::CommandItem::bookmark(&name, filename, param)
            })
            .collect();
        cp.set_dynamic_commands(commands);
    }
}

pub struct AppWindow {
    _window: adw::ApplicationWindow,
}

impl AppWindow {
    pub fn new(app: &adw::Application, init: AppInit) -> Self {
        static APP_UI_INIT: OnceLock<()> = OnceLock::new();
        APP_UI_INIT.get_or_init(|| {
            let provider = gtk::CssProvider::new();
            provider.load_from_resource("/dev/boxxy/BoxxyTerminal/style.css");
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
                gtk::IconTheme::for_display(&display)
                    .add_resource_path("/dev/boxxy/BoxxyTerminal/icons");
            }
            boxxy_keybindings::bind_shortcuts(app);
        });

        let current_settings = (*Settings::load()).clone();
        let app_state = AppState::load();

        let style_manager = adw::StyleManager::default();
        let fallback_scheme = match current_settings.color_scheme {
            boxxy_preferences::config::ColorScheme::Default => adw::ColorScheme::Default,
            boxxy_preferences::config::ColorScheme::Light => adw::ColorScheme::ForceLight,
            boxxy_preferences::config::ColorScheme::Dark => adw::ColorScheme::ForceDark,
        };
        boxxy_themes::app_theme::apply_app_theme(
            current_settings.app_theme_mode == boxxy_preferences::AppThemeMode::Boxxy,
            &current_settings.app_theme_id,
            current_settings.custom_accent_color.as_deref(),
            fallback_scheme,
        );

        let palette = boxxy_themes::load_palette(current_settings.theme.as_str());
        let is_dark = style_manager.is_dark();
        boxxy_themes::apply_palette(palette.as_ref(), is_dark);

        let is_drag_window = init.incoming_tab_view.is_some();
        let tab_view = init.incoming_tab_view.unwrap_or_default();

        // Unbounded single-consumer UI channel prevents main-thread deadlocks when
        // synchronous GTK signal handlers dispatch multiple `AppInput` messages in a burst.
        let (tx, rx) = async_channel::unbounded::<AppInput>();

        let tx_claw_cancel = tx.clone();
        let tx_claw_pause = tx.clone();
        let tx_claw_cancel_all = tx.clone();
        let tx_claw_soft_clear = tx.clone();
        let tx_claw_resume = tx.clone();
        let claw = ClawSidebarComponent::new(
            move |task_id| {
                let _ = tx_claw_cancel.send_blocking(AppInput::CancelTask(task_id, None));
            },
            move || {
                let _ = tx_claw_soft_clear.send_blocking(AppInput::ClearClawHistory(None));
            },
        );
        claw.set_on_toggle_pause_task(move |task_id| {
            let _ = tx_claw_pause.send_blocking(AppInput::TogglePauseTask(task_id, None));
        });
        claw.set_on_cancel_all_tasks(move || {
            let _ = tx_claw_cancel_all.send_blocking(AppInput::CancelAllTasks(None));
        });
        claw.set_on_resume_session(move |session_id, override_character_id| {
            let _ = tx_claw_resume.send_blocking(AppInput::ResumeClawSession(
                session_id,
                override_character_id,
            ));
        });

        let tx_bookmarks = tx.clone();
        let tx_open_bookmarks = tx.clone();
        let bookmarks_sidebar = BookmarksSidebarComponent::new(
            move |name, filename, script| {
                let _ =
                    tx_bookmarks.send_blocking(AppInput::ExecuteBookmark(name, filename, script));
            },
            move || {
                let _ = tx_open_bookmarks.send_blocking(AppInput::OpenBookmarks);
            },
        );

        let tx_theme = tx.clone();
        let theme_selector = ThemeSelectorComponent::new(move |palette| {
            let _ = tx_theme.send_blocking(AppInput::ThemeSelected(Box::new(palette)));
        });
        theme_selector.select_theme(current_settings.theme.as_str());
        if app_state.sidebar_visible && app_state.active_sidebar_page == "themes" {
            theme_selector.ensure_populated();
        }

        let command_palette = CommandPaletteComponent::new();

        let cp_sync = command_palette.clone();
        let tx_bm_guard = tx.clone();
        let mut bookmarks_rx = BOOKMARKS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            loop {
                match bookmarks_rx.recv().await {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
                if tx_bm_guard.is_closed() {
                    break;
                }
                sync_bookmarks_to_palette(&cp_sync).await;
            }
        });

        let window_title = match boxxy_sys_utils::dev_header_tag() {
            Some(ref tag) => format!("Boxxy Terminal {tag}"),
            None => "Boxxy Terminal".to_string(),
        };
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(app_state.window_width)
            .default_height(app_state.window_height)
            .maximized(app_state.is_maximized)
            .title(window_title)
            .build();
        window.add_css_class("main-window");

        if current_settings.frosted_glass && current_settings.opacity < 1.0 {
            window.add_css_class("frosted-glass-active");
        }

        let tx_cp = tx.clone();
        command_palette.widget().connect_closed(move |_| {
            let _ = tx_cp.send_blocking(AppInput::FocusActiveTerminal);
        });

        let force_close = Rc::new(Cell::new(false));

        let tx_close = tx.clone();
        let force_close_clone = force_close.clone();
        window.connect_close_request(move |win| {
            if force_close_clone.get() {
                return gtk::glib::Propagation::Proceed;
            }
            let width = win.width();
            let height = win.height();
            let is_maximized = win.is_maximized();
            let _ = tx_close.send_blocking(AppInput::SaveWindowState {
                width,
                height,
                is_maximized,
            });
            let _ = tx_close.send_blocking(AppInput::CloseRequested);
            gtk::glib::Propagation::Stop
        });

        let tx_focus = tx.clone();
        window.connect_notify(Some("is-active"), move |win, _| {
            if win.is_active() && !AppWindowInner::is_dialog_or_popover_open(win) {
                let _ = tx_focus.send_blocking(AppInput::FocusActiveTerminal);
            }
        });

        // Centralized focus restoration whenever any `AdwDialog` / `AdwAlertDialog`
        // finishes closing on this window.
        let tx_dialog_focus = tx.clone();
        window.connect_visible_dialog_notify(move |win| {
            use libadwaita::prelude::AdwApplicationWindowExt;
            if win.visible_dialog().is_none() {
                let tx_idle = tx_dialog_focus.clone();
                gtk::glib::idle_add_local_once(move || {
                    let _ = tx_idle.send_blocking(AppInput::FocusActiveTerminal);
                });
            }
        });

        let split_view = adw::OverlaySplitView::builder()
            .show_sidebar(app_state.sidebar_visible)
            .build();
        split_view.add_css_class("main-split-view");

        let tx_sidebar = tx.clone();
        split_view.connect_show_sidebar_notify(move |view| {
            let _ = tx_sidebar.send_blocking(AppInput::SidebarVisibleChanged(view.shows_sidebar()));
        });

        let (sidebar_overlay, sidebar_toolbar, view_stack) = Self::build_sidebar(
            &tx,
            &claw,
            &bookmarks_sidebar,
            &theme_selector,
            &app_state,
            &current_settings,
        );
        sidebar_overlay.set_visible(app_state.sidebar_visible);
        split_view.set_sidebar(Some(&sidebar_overlay));
        let (
            content_toolbar,
            content_header,
            detached_btn,
            detached_count_lbl,
            tab_bar,
        ) = Self::build_content_area(&tx, &tab_view, &current_settings);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&content_toolbar));
        split_view.set_content(Some(&overlay));

        let frosted_container = crate::widgets::frosted_glass::FrostedGlassContainer::new();
        frosted_container.set_child(Some(&split_view));
        frosted_container.set_frosted_glass(current_settings.frosted_glass);
        frosted_container.set_opacity(current_settings.opacity);

        let fallback_bg = if is_dark {
            gtk4::gdk::RGBA::new(0.08, 0.08, 0.1, 1.0)
        } else {
            gtk4::gdk::RGBA::new(0.98, 0.98, 0.99, 1.0)
        };
        let bg_rgba = if let Some(ref p) = palette {
            let variant = if is_dark { &p.dark } else { &p.light };
            gtk4::gdk::RGBA::parse(variant.background).unwrap_or(fallback_bg)
        } else {
            fallback_bg
        };
        frosted_container.set_tint_color(bg_rgba);

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&frosted_container));

        window.set_content(Some(&toast_overlay));

        let tx_focus2 = tx.clone();
        let tab_view_for_focus = tab_view.clone();
        tab_view_for_focus.connect_selected_page_notify(move |_| {
            let _ = tx_focus2.send_blocking(AppInput::FocusActiveTerminal);
        });

        let mut event_rx = boxxy_terminal::TERMINAL_EVENT_BUS.subscribe();
        let tx_event = tx.clone();
        boxxy_sys_utils::runtime().spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        if tx_event
                            .send(AppInput::HandleTerminalEvent(Box::new(event)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        log::warn!("TERMINAL_EVENT_BUS lagged by {skipped} events; continuing");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let mut settings_rx = boxxy_preferences::SETTINGS_EVENT_BUS.subscribe();
        let tx_settings = tx.clone();
        boxxy_sys_utils::runtime().spawn(async move {
            loop {
                let settings = match settings_rx.recv().await {
                    Ok(s) => s,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        (*boxxy_preferences::Settings::load()).clone()
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if tx_settings
                    .send(AppInput::SettingsChanged(Box::new(settings)))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let inner = AppWindowInner {
            window: window.clone(),
            tabs: Vec::new(),
            tab_view,
            tab_bar: tab_bar.clone(),
            content_header,
            split_view,
            sidebar_overlay,
            sidebar_toolbar,
            detached_btn,
            detached_count_lbl,
            view_stack,
            sidebar_visible: app_state.sidebar_visible,
            preferences: None,
            tab_menu: None,
            claw,
            bookmarks_sidebar,
            bookmarks_controller: None,
            bookmarks_page: None,
            theme_selector,
            command_palette: command_palette.clone(),
            current_settings,
            app_state,
            claw_active: false,
            toast_overlay,
            frosted_container,
            active_update_notification: None,
            last_notified_broken_default: None,
            initial_launch: Some(crate::init::TabLaunchSpec {
                working_dir: init.working_dir,
                command: init.initial_command.clone(),
                hold: init.hold,
                title: init.initial_title.clone(),
            }),
            force_close,
            tx: tx.clone(),
        };

        let is_blank_initial_tab = init.initial_command.is_none()
            && inner
                .initial_launch
                .as_ref()
                .is_none_or(|l| l.working_dir.is_none())
            && init.initial_title.is_none();

        let inner_ref = Rc::new(RefCell::new(inner));
        setup_actions(&window, &inner_ref, tx.clone());

        let tv_weak = inner_ref.borrow().tab_view.downgrade();
        let tx_move = tx.clone();
        let tx_color = tx.clone();
        let tx_title = tx.clone();
        let tx_menu_focus = tx.clone();
        let inner_for_menu = Rc::downgrade(&inner_ref);
        let inner_for_menu_cwd = Rc::downgrade(&inner_ref);
        let context_tab_bar = inner_ref.borrow().tab_bar.clone();
        let context_tab_view = inner_ref.borrow().tab_view.clone();
        let tab_menu = TabContextMenu::new(
            &context_tab_bar,
            &context_tab_view,
            TabContextCallbacks {
                on_close_page: Box::new(move |page| {
                    if let Some(tv) = tv_weak.upgrade() {
                        tv.close_page(&page);
                    }
                }),
                on_move_to_new_window: Box::new(move |page| {
                    let key = page.child().as_ptr() as usize;
                    let _ = tx_move.send_blocking(AppInput::MoveTabToNewWindowRequest(key));
                }),
                on_set_color: Box::new(move |page, color| {
                    let key = page.child().as_ptr() as usize;
                    let _ = tx_color.send_blocking(AppInput::SetTabColor(key, color));
                }),
                on_set_title: Box::new(move |page, title| {
                    let key = page.child().as_ptr() as usize;
                    let _ = tx_title.send_blocking(AppInput::SetTabTitle(key, title));
                }),
                get_custom_title: Box::new(move |page| {
                    let inner_rc = inner_for_menu.upgrade()?;
                    let inner = inner_rc.borrow();
                    let key = page.child().as_ptr() as usize;
                    inner
                        .tab_by_widget_key(key)
                        .and_then(|t| t.custom_title.clone())
                }),
                get_cwd: Box::new(move |page| {
                    let inner_rc = inner_for_menu_cwd.upgrade()?;
                    let inner = inner_rc.borrow();
                    let key = page.child().as_ptr() as usize;
                    inner
                        .tab_by_widget_key(key)
                        .and_then(|t| t.cwd.clone().or_else(|| t.controller.working_dir()))
                }),
                on_menu_closed: Box::new(move || {
                    let _ = tx_menu_focus.send_blocking(AppInput::FocusActiveTerminal);
                }),
            },
        );
        inner_ref.borrow_mut().tab_menu = Some(tab_menu);

        // Close `rx` and unparent custom popovers on window destruction so the
        // `rx.recv()` task, subscriber tasks, and `AppWindowInner` are freed cleanly.
        {
            let rx_destroy = rx.clone();
            let inner_weak_destroy = Rc::downgrade(&inner_ref);
            window.connect_destroy(move |_| {
                if let Some(inner_rc) = inner_weak_destroy.upgrade()
                    && let Ok(mut inner) = inner_rc.try_borrow_mut()
                {
                    inner.command_palette.unparent();
                    if let Some(tm) = inner.tab_menu.take() {
                        tm.unparent();
                    }
                }
                rx_destroy.close();
            });
        }

        let tab_view_clone1 = inner_ref.borrow().tab_view.clone();
        let tx_detach = tx.clone();
        let inner_detach = Rc::downgrade(&inner_ref);
        tab_view_clone1.connect_page_detached(move |_, page, _| {
            let key = page.child().as_ptr() as usize;
            if let Some(inner_rc) = inner_detach.upgrade()
                && let Ok(mut inner) = inner_rc.try_borrow_mut()
            {
                crate::update::tabs::tab_page_detached(&mut inner, key);
            } else {
                let _ = tx_detach.send_blocking(AppInput::TabPageDetached(key));
            }
        });

        let tab_view_clone2 = inner_ref.borrow().tab_view.clone();
        let tx_attach = tx.clone();
        let inner_attach = Rc::downgrade(&inner_ref);
        tab_view_clone2.connect_page_attached(move |_, page, _| {
            let key = page.child().as_ptr() as usize;
            if let Some(inner_rc) = inner_attach.upgrade()
                && let Ok(mut inner) = inner_rc.try_borrow_mut()
            {
                crate::update::tabs::tab_page_attached(&mut inner, key);
            } else {
                let _ = tx_attach.send_blocking(AppInput::TabPageAttached(key));
            }
        });

        if is_drag_window {
            crate::update::update(&inner_ref, AppInput::AdoptOrphanTabs);
        }

        // Create the initial terminal tab synchronously before `window.present()` so the
        // terminal widget and its background color/opacity are rendered on Frame 1 with
        // zero transparent gap.
        let initial_tab_id = if !is_drag_window && inner_ref.borrow().tabs.is_empty() {
            crate::update::update(&inner_ref, AppInput::NewTab);
            is_blank_initial_tab
                .then(|| inner_ref.borrow().tabs.first().map(|t| t.id.clone()))
                .flatten()
        } else {
            None
        };

        if !is_drag_window {
            let tx_startup = tx.clone();
            let window_weak = window.downgrade();
            gtk::glib::spawn_future_local(async move {
                let agent = boxxy_terminal::get_agent().await;

                {
                    let agent_bg = agent.clone();
                    let tx_bg = tx_startup.clone();
                    gtk::glib::spawn_future_local(async move {
                        if let Ok((status, on_battery)) = agent_bg.get_maintenance_status().await {
                            let _ = tx_bg
                                .send(AppInput::RefreshMaintenanceStatus(status, on_battery))
                                .await;
                        }
                        if let Ok(mcp_statuses) = agent_bg.get_mcp_statuses().await {
                            let _ = tx_bg.send(AppInput::RefreshMcpStatuses(mcp_statuses)).await;
                        }
                    });
                }

                let mut restored_any = false;
                if let Ok(pending_restores) = agent.take_pending_restores().await
                    && !pending_restores.is_empty()
                {
                    if pending_restores.iter().any(|(pid, _)| *pid == 0) {
                        if let Ok(all_detached) = agent.list_detached_sessions_detailed().await
                            && !all_detached.is_empty()
                        {
                            restored_any = true;
                            let _ = tx_startup
                                .send(AppInput::ReattachDetachedBatch(all_detached))
                                .await;
                        }
                    } else {
                        for (pid, cwd) in pending_restores {
                            restored_any = true;
                            let cwd_opt = (!cwd.is_empty()).then_some(cwd);
                            let _ = tx_startup
                                .send(AppInput::ReattachDetachedSession(pid, cwd_opt))
                                .await;
                        }
                    }
                }
                if let Ok(pending_spawns) = agent.take_pending_character_spawns().await
                    && !pending_spawns.is_empty()
                {
                    for (char_id, cwd) in pending_spawns {
                        restored_any = true;
                        let cwd_opt = (!cwd.is_empty()).then_some(cwd);
                        let _ = tx_startup
                            .send(AppInput::SpawnCharacterTab(char_id, cwd_opt))
                            .await;
                    }
                }
                if restored_any {
                    if let Some(init_id) = initial_tab_id {
                        let _ = tx_startup.send(AppInput::CloseTab(init_id)).await;
                    }
                } else {
                    let _ = tx_startup.send(AppInput::EnsureInitialTab).await;
                }

                // Perform What's New marker/cache inspection off the GTK main thread.
                let whats_new_opt = boxxy_sys_utils::runtime()
                    .spawn_blocking(|| {
                        crate::updater::whats_new::take_whats_new_pending().map(|pending| {
                            let cached =
                                crate::updater::whats_new::load_cached_changelog_summary(&pending);
                            (pending, cached)
                        })
                    })
                    .await
                    .ok()
                    .flatten();
                if let Some((pending, cached)) = whats_new_opt {
                    let _ = tx_startup
                        .send(AppInput::ShowWhatsNewDialog(pending, cached.map(Box::new)))
                        .await;
                }

                if let Ok(detached) = agent.list_detached_sessions_detailed().await {
                    let _ = tx_startup
                        .send(AppInput::UpdateDetachedSessionsBadge(detached))
                        .await;
                }

                loop {
                    gtk::glib::timeout_future(std::time::Duration::from_secs(3)).await;
                    if window_weak.upgrade().is_none() || tx_startup.is_closed() {
                        break;
                    }
                    if tx_startup
                        .send(AppInput::RefreshDetachedSessions)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }

        // Background update check (10s after startup, then once every hour)
        if !is_drag_window && boxxy_sys_utils::can_self_update() {
            let tx_update = tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                loop {
                    if tx_update.is_closed() {
                        break;
                    }
                    match crate::updater::Updater::check_for_update().await {
                        Ok(Some(info)) => {
                            let notification =
                                crate::widgets::notification::Notification::new_update(
                                    &info.current_commit,
                                    &info.new_commit,
                                    &info.date,
                                    &info.download_url,
                                    info.checksum_source,
                                );
                            if tx_update
                                .send(AppInput::PushGlobalNotification(Box::new(notification)))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            log::warn!("Background update check failed: {e:#}");
                        }
                    }

                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                }
            });
        } else if !boxxy_sys_utils::can_self_update() {
            log::debug!("Self-updater is disabled. Skipping update check.");
        }

        window.present();

        if !is_drag_window && let Ok(mut inner) = inner_ref.try_borrow_mut() {
            crate::update::window_state::maybe_notify_broken_default_model(&mut inner);
        }

        // Hydrate Command Palette bookmarks asynchronously off the UI thread and pre-warm
        // deferred dialogs/models right after the window has presented on screen.
        let cp_initial = command_palette;
        gtk::glib::spawn_future_local(async move {
            sync_bookmarks_to_palette(&cp_initial).await;
        });

        let tx_prewarm = tx;
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            let _ = tx_prewarm.send_blocking(AppInput::PrewarmDeferredUi);
        });

        gtk::glib::spawn_future_local({
            let inner_ref = inner_ref.clone();
            async move {
                while let Ok(msg) = rx.recv().await {
                    crate::update::update(&inner_ref, msg);
                }
            }
        });

        Self { _window: window }
    }

    fn build_sidebar(
        tx: &async_channel::Sender<AppInput>,
        claw: &ClawSidebarComponent,
        bookmarks_sidebar: &BookmarksSidebarComponent,
        theme_selector: &ThemeSelectorComponent,
        app_state: &AppState,
        current_settings: &Settings,
    ) -> (gtk::Overlay, adw::ToolbarView, adw::ViewStack) {
        let sidebar_toolbar = adw::ToolbarView::new();
        sidebar_toolbar.add_css_class("sidebar-toolbar");
        sidebar_toolbar.set_width_request(current_settings.ai_chat_width);

        let view_stack = adw::ViewStack::new();

        let claw_page = view_stack.add_titled(claw.widget(), Some("claw"), "Claw");
        claw_page.set_icon_name(Some("boxxy-boxxyclaw-symbolic"));

        let bookmarks_page =
            view_stack.add_titled(bookmarks_sidebar.widget(), Some("bookmarks"), "Bookmarks");
        bookmarks_page.set_icon_name(Some("boxxy-bookmark-filled-symbolic"));

        let themes_page = view_stack.add_titled(theme_selector.widget(), Some("themes"), "Colors");
        themes_page.set_icon_name(Some("boxxy-appearance-symbolic"));

        let initial_page = match app_state.active_sidebar_page.as_str() {
            "claw" | "bookmarks" | "themes" => app_state.active_sidebar_page.as_str(),
            _ => "claw",
        };
        view_stack.set_visible_child_name(initial_page);

        let tx_page = tx.clone();
        view_stack.connect_visible_child_name_notify(move |stack| {
            if let Some(name) = stack.visible_child_name() {
                let _ = tx_page.send_blocking(AppInput::SidebarPageChanged(name.to_string()));
            }
        });

        let switcher = adw::ViewSwitcher::builder()
            .stack(&view_stack)
            .policy(adw::ViewSwitcherPolicy::Narrow)
            .build();

        let sidebar_header = adw::HeaderBar::builder().title_widget(&switcher).build();
        sidebar_header.add_css_class("flat");
        sidebar_header.add_css_class("sidebar-header");

        let sidebar_content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar_content_box.set_hexpand(true);
        sidebar_content_box.append(&sidebar_header);
        sidebar_content_box.append(&view_stack);

        sidebar_toolbar.set_content(Some(&sidebar_content_box));

        // Keep width at 4px so it sits strictly inside the 5px right margin of sidebar scrollbars
        // and never blocks grabbing or dragging the scrollbar thumb.
        let handle_event_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        handle_event_box.set_width_request(4);
        handle_event_box.set_halign(gtk::Align::End);
        handle_event_box.set_vexpand(true);
        handle_event_box.add_css_class("sidebar-resize-handle");
        handle_event_box.set_cursor_from_name(Some("col-resize"));

        let drag = gtk::GestureDrag::new();
        let start_width = Rc::new(Cell::new(0));
        let last_width = Rc::new(Cell::new(0));

        drag.connect_drag_begin(glib::clone!(
            #[weak]
            sidebar_toolbar,
            #[strong]
            start_width,
            #[strong]
            last_width,
            move |_, _, _| {
                let w = sidebar_toolbar.width();
                start_width.set(w);
                last_width.set(w);
            }
        ));

        // Resize the widget smoothly during drag without persisting settings to disk on every motion step.
        drag.connect_drag_update(glib::clone!(
            #[weak]
            sidebar_toolbar,
            #[strong]
            start_width,
            #[strong]
            last_width,
            move |_, offset_x, _| {
                let true_offset_x = offset_x + f64::from(last_width.get() - start_width.get());
                let offset_snapped = (true_offset_x as i32 / 10) * 10;
                if offset_snapped == 0 {
                    return;
                }
                let target_width = (start_width.get() + offset_snapped).clamp(200, 800);
                if target_width != last_width.get() {
                    sidebar_toolbar.set_width_request(target_width);
                    last_width.set(target_width);
                }
            }
        ));

        let tx_drag_end = tx.clone();
        drag.connect_drag_end(glib::clone!(
            #[weak]
            sidebar_toolbar,
            #[strong]
            start_width,
            #[strong]
            last_width,
            move |_, offset_x, _| {
                let true_offset_x = offset_x + f64::from(last_width.get() - start_width.get());
                let offset_snapped = (true_offset_x as i32 / 10) * 10;
                let target_width = (start_width.get() + offset_snapped).clamp(200, 800);
                sidebar_toolbar.set_width_request(target_width);
                last_width.set(target_width);
                let _ = tx_drag_end.send_blocking(AppInput::SidebarWidthChanged(target_width));
            }
        ));
        handle_event_box.add_controller(drag);

        let sidebar_overlay = gtk::Overlay::new();
        sidebar_overlay.set_child(Some(&sidebar_toolbar));
        sidebar_overlay.add_overlay(&handle_event_box);

        (sidebar_overlay, sidebar_toolbar, view_stack)
    }

    fn build_content_area(
        tx: &async_channel::Sender<AppInput>,
        tab_view: &adw::TabView,
        _current_settings: &Settings,
    ) -> (
        adw::ToolbarView,
        adw::HeaderBar,
        gtk::Button,
        gtk::Label,
        BoxxyTabBar,
    ) {
        tab_view.add_css_class("terminal-tab-view");

        let content_toolbar = adw::ToolbarView::new();
        content_toolbar.add_css_class("terminal-toolbar");
        content_toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);

        let content_header = adw::HeaderBar::builder().show_title(false).build();
        content_header.add_css_class("flat");
        content_header.add_css_class("terminal-header");
        content_header.add_css_class("content-header");

        let tab_bar = BoxxyTabBar::new(tab_view);
        tab_bar.set_hexpand(true);
        tab_bar.set_margin_start(36);

        let tx_tab_drop = tx.clone();
        tab_bar.set_on_pane_drop(move |source_pane_id, target_page_key| {
            let _ = tx_tab_drop.send_blocking(AppInput::DropPaneOnTab {
                source_pane_id,
                target_page_key,
            });
        });

        let tx_tab_detach = tx.clone();
        tab_bar.set_on_detach_to_new_window(move |page_key| {
            let _ = tx_tab_detach.send_blocking(AppInput::MoveTabToNewWindowRequest(page_key));
        });

        let toggle_sidebar_btn = gtk::Button::builder()
            .icon_name("boxxy-dock-left-symbolic")
            .tooltip_text("Toggle Sidebar")
            .action_name("win.toggle-sidebar")
            .build();
        content_header.pack_start(&toggle_sidebar_btn);

        // Slim status indicator pill for detached Background Shells
        let badge_content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        badge_content.set_valign(gtk::Align::Center);
        let badge_icon = gtk::Image::builder()
            .icon_name("utilities-terminal-symbolic")
            .pixel_size(13)
            .valign(gtk::Align::Center)
            .build();
        let detached_count_lbl = gtk::Label::builder()
            .label("0")
            .valign(gtk::Align::Center)
            .css_classes(["monospace", "detached-shells-count"])
            .build();
        badge_content.append(&badge_icon);
        badge_content.append(&detached_count_lbl);

        let detached_btn = gtk::Button::builder()
            .child(&badge_content)
            .css_classes(["flat", "detached-shells-pill"])
            .valign(gtk::Align::Center)
            .focus_on_click(false)
            .margin_end(4)
            .tooltip_text("Background Shells")
            .action_name("win.detached-sessions")
            .visible(false)
            .build();
        detached_btn.set_cursor_from_name(Some("pointer"));
        content_header.pack_end(&detached_btn);

        if let Some(tag_text) = boxxy_sys_utils::dev_header_tag() {
            let tooltip = format!(
                "Isolated development instance ({})",
                boxxy_sys_utils::get_config_dir().display()
            );
            let dev_tag = gtk::Label::builder()
                .label(&tag_text)
                .valign(gtk::Align::Center)
                .css_classes(["monospace", "dev-mode-tag"])
                .tooltip_text(&tooltip)
                .build();
            content_header.pack_end(&dev_tag);
        }

        content_header.pack_start(&tab_bar);

        if let Some(start_box) = tab_bar.parent() {
            start_box.set_halign(gtk::Align::Fill);
            start_box.set_hexpand(true);
        }

        content_toolbar.add_top_bar(&content_header);
        content_toolbar.set_content(Some(tab_view));
        tab_bar.install_header_drag_guard();

        let tx_closetab = tx.clone();
        tab_view.connect_close_page(move |_, page| {
            let key = page.child().as_ptr() as usize;
            let _ = tx_closetab.send_blocking(AppInput::CloseTabRequest(key));
            gtk::glib::Propagation::Stop
        });

        (
            content_toolbar,
            content_header,
            detached_btn,
            detached_count_lbl,
            tab_bar,
        )
    }
}
