use gtk4 as gtk;
use libadwaita as adw;
use gtk::prelude::*;
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use boxxy_sidebar::AiSidebarComponent;
use boxxy_app_menu::AppMenuComponent;
use boxxy_claw::ClawSidebarComponent;
use boxxy_command_palette::CommandPaletteComponent;
use boxxy_preferences::{AppState, PreferencesComponent, Settings};
use boxxy_themes::ThemeSelectorComponent;

use crate::init::AppInit;
use crate::state::{AppInput, AppWindowInner};
use crate::actions::setup_actions;
use crate::tab_menu::TabContextMenu;

pub struct AppWindow {
    _window: adw::ApplicationWindow,
}

impl AppWindow {
    pub fn new(app: &adw::Application, init: AppInit) -> Self {
        static CSS_REGISTERED: OnceLock<()> = OnceLock::new();
        CSS_REGISTERED.get_or_init(|| {
            let provider = gtk::CssProvider::new();
            provider.load_from_resource("/play/mii/Boxxy/style.css");
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
                gtk::IconTheme::for_display(&display).add_resource_path("/play/mii/Boxxy/icons");
            }
        });

        let current_settings = Settings::load();
        let app_state = AppState::load();

        let palette = boxxy_themes::load_palette(current_settings.theme.as_str());
        let is_dark = adw::StyleManager::default().is_dark();
        boxxy_themes::apply_palette(palette.as_ref(), is_dark);

        let is_drag_window = init.incoming_tab_view.is_some();
        let tab_view = init.incoming_tab_view.unwrap_or_default();

        let (tx, rx) = async_channel::bounded::<AppInput>(100);

        tab_view.connect_create_window(move |_| {
            let new_tab_view = adw::TabView::new();
            if let Some(app) = gtk::gio::Application::default()
                .and_then(|a| a.downcast::<adw::Application>().ok())
            {
                AppWindow::new(
                    &app,
                    AppInit {
                        incoming_tab_view: Some(new_tab_view.clone()),
                        working_dir: None,
                    },
                );
            }
            Some(new_tab_view)
        });

        let tx_pref = tx.clone();
        let tx_themes_nav = tx.clone();
        let tx_reload = tx.clone();
        let preferences = PreferencesComponent::new(
            move |settings| {
                let _ = tx_pref.send_blocking(AppInput::SettingsChanged(settings));
            },
            move || {
                let _ = tx_themes_nav.send_blocking(AppInput::ShowThemesSidebar);
            },
            move || {
                let _ = tx_reload.send_blocking(AppInput::ReloadEngine);
            },
        );

        let app_menu = AppMenuComponent::new();
        let ai_chat = AiSidebarComponent::new();
        
        let tx_claw_active = tx.clone();
        let claw = ClawSidebarComponent::new(move |active| {
            let _ = tx_claw_active.send_blocking(AppInput::SetClawActive(active));
        });
        claw.update_diagnosis_mode(&current_settings.claw_auto_diagnosis_mode);

        let tx_theme = tx.clone();
        let theme_selector = ThemeSelectorComponent::new(move |palette| {
            let _ = tx_theme.send_blocking(AppInput::ThemeSelected(Box::new(palette)));
        });
        theme_selector.select_theme(current_settings.theme.as_str());

        let command_palette = CommandPaletteComponent::new();

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(app_state.window_width)
            .default_height(app_state.window_height)
            .maximized(app_state.is_maximized)
            .title("Boxxy Terminal")
            .build();
        window.add_css_class("main-window");

        app_menu.widget().set_parent(&window);

        let tx_cp = tx.clone();
        command_palette.widget().connect_closed(move |_| {
            let _ = tx_cp.send_blocking(AppInput::FocusActiveTerminal);
        });

        let tx_pref_close = tx.clone();
        preferences.widget().connect_closed(move |_| {
            let _ = tx_pref_close.send_blocking(AppInput::FocusActiveTerminal);
        });

        let tx_am = tx.clone();
        app_menu.widget().connect_closed(move |_| {
            let _ = tx_am.send_blocking(AppInput::FocusActiveTerminal);
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
            if win.is_active() {
                let _ = tx_focus.send_blocking(AppInput::FocusActiveTerminal);
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

        let gesture = gtk::GestureClick::new();
        gesture.set_button(0);
        let tx_menu = tx.clone();
        gesture.connect_pressed(move |gesture, n_press, x, y| {
            if n_press == 1 && gesture.current_button() == gtk::gdk::BUTTON_SECONDARY {
                let _ = tx_menu.send_blocking(AppInput::ShowAppMenu(x, y));
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        split_view.add_controller(gesture);

        let (sidebar_toolbar, view_stack) = Self::build_sidebar(&tx, &ai_chat, &claw, &theme_selector, &app_state, &current_settings);
        split_view.set_sidebar(Some(&sidebar_toolbar));

        let (content_toolbar, content_header, bell_indicator, single_tab_title, header_title_stack, menu_btn, tab_bar, claw_indicator, claw_popover) = Self::build_content_area(&tx, &tab_view, &current_settings);
        
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&content_toolbar));

        let notification_pill = crate::widgets::notification_pill::BoxxyNotificationPill::new();
        notification_pill.set_visible(false);
        overlay.add_overlay(&notification_pill);

        split_view.set_content(Some(&overlay));
        window.set_content(Some(&split_view));

        let tx_pill = tx.clone();
        let pill_clone = notification_pill.clone();
        notification_pill.connect_clicked(move |_| {
            if let Some(notification) = pill_clone.get_notification() {
                let popover = gtk::Popover::new();
                popover.set_position(gtk::PositionType::Top);
                let details = crate::widgets::notification_details::BoxxyNotificationDetails::new(&notification, tx_pill.clone());
                popover.set_child(Some(&details));
                popover.set_parent(&pill_clone);
                popover.popup();
            }
        });

        let tx_focus2 = tx.clone();
        tab_view.connect_selected_page_notify(move |_| {
            let _ = tx_focus2.send_blocking(AppInput::FocusActiveTerminal);
        });

        let tv_weak = tab_view.downgrade();
        let tx_move = tx.clone();
        let _ = TabContextMenu::new(
            &tab_view,
            &window,
            move |page| {
                if let Some(tv) = tv_weak.upgrade() {
                    tv.close_page(&page);
                }
            },
            move |page| {
                let key = page.child().as_ptr() as usize;
                let _ = tx_move.send_blocking(AppInput::MoveTabToNewWindowRequest(key));
            },
        );

        setup_actions(&window, tx.clone());
        boxxy_keybindings::bind_shortcuts(app);

        let mut event_rx = boxxy_terminal::TERMINAL_EVENT_BUS.subscribe();
        let tx_event = tx.clone();
        boxxy_ai_core::utils::runtime().spawn(async move {
            while let Ok(event) = event_rx.recv().await {
                let _ = tx_event.send(AppInput::HandleTerminalEvent(Some(event))).await;
            }
        });

        let mut settings_rx = boxxy_preferences::SETTINGS_EVENT_BUS.subscribe();
        let tx_settings = tx.clone();
        boxxy_ai_core::utils::runtime().spawn(async move {
            while let Ok(settings) = settings_rx.recv().await {
                let _ = tx_settings.send_blocking(AppInput::SettingsChanged(settings));
            }
        });

        let inner = AppWindowInner {
            window: window.clone(),
            tabs: Vec::new(),
            boxxy_apps_controller: None,
            boxxy_apps_page: None,
            tab_view,
            tab_bar,
            single_tab_title,
            header_title_stack,
            content_header,
            _split_view: split_view,
            sidebar_toolbar,
            menu_btn,
            view_stack,
            next_id: 1,
            sidebar_visible: app_state.sidebar_visible,
            preferences,
            app_menu,
            ai_chat,
            claw,
            theme_selector,
            command_palette,
            current_settings,
            app_state,
            bell_indicator,
            claw_indicator,
            claw_popover,
            notification_pill,
            notifications: Vec::new(),
            initial_working_dir: init.working_dir.clone(),
            force_close,
            tx: tx.clone(),
        };

        let inner_ref = Rc::new(RefCell::new(inner));

        let tab_bar_opt = inner_ref.borrow().header_title_stack.child_by_name("tabs")
            .and_then(|w| w.downcast::<adw::TabBar>().ok());

        if let Some(tab_bar) = tab_bar_opt {
            inner_ref.borrow_mut().tab_bar = tab_bar;
        }

        let tab_view_clone1 = inner_ref.borrow().tab_view.clone();
        let tx_detach = tx.clone();
        let inner_detach = inner_ref.clone();
        tab_view_clone1.connect_page_detached(move |_, page, _| {
            let key = page.child().as_ptr() as usize;
            if let Ok(mut inner) = inner_detach.try_borrow_mut() {
                crate::update::tabs::tab_page_detached(&mut inner, key);
            } else {
                let _ = tx_detach.send_blocking(AppInput::TabPageDetached(key));
            }
        });

        let tab_view_clone2 = inner_ref.borrow().tab_view.clone();
        let tx_attach = tx.clone();
        let inner_attach = inner_ref.clone();
        tab_view_clone2.connect_page_attached(move |_, page, _| {
            let key = page.child().as_ptr() as usize;
            if let Ok(mut inner) = inner_attach.try_borrow_mut() {
                crate::update::tabs::tab_page_attached(&mut inner, key);
            } else {
                let _ = tx_attach.send_blocking(AppInput::TabPageAttached(key));
            }
        });

        if is_drag_window {
            crate::update::update(&inner_ref, AppInput::AdoptOrphanTabs);
        }

        if !is_drag_window && inner_ref.borrow().tabs.is_empty() {
            crate::update::update(&inner_ref, AppInput::NewTab);
        }

        // Background update check
        let tx_update = tx.clone();
        tokio::spawn(async move {
            // Wait 10 seconds after startup to not interfere with boot
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            if let Ok(Some((version, url))) = crate::updater::Updater::check_for_update().await {
                let notification = crate::widgets::notification::Notification::new_update(&version, &url);
                let _ = tx_update.send(AppInput::PushNotification(notification)).await;
            }
        });

        window.present();

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
        ai_chat: &AiSidebarComponent,
        claw: &ClawSidebarComponent,
        theme_selector: &ThemeSelectorComponent,
        app_state: &AppState,
        current_settings: &Settings,
    ) -> (adw::ToolbarView, adw::ViewStack) {
        let sidebar_toolbar = adw::ToolbarView::new();
        sidebar_toolbar.set_width_request(current_settings.ai_chat_width);

        let view_stack = adw::ViewStack::new();

        let assistant_page = view_stack.add_titled(ai_chat.widget(), Some("assistant"), "AI Chat");
        assistant_page.set_icon_name(Some("ai-slop-symbolic"));

        let claw_page = view_stack.add_titled(claw.widget(), Some("claw"), "Claw");
        claw_page.set_icon_name(Some("boxxyclaw"));

        let themes_page = view_stack.add_titled(theme_selector.widget(), Some("themes"), "Colors");
        themes_page.set_icon_name(Some("appearance-symbolic"));

        view_stack.set_visible_child_name(&app_state.active_sidebar_page);

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

        let sidebar_header = adw::HeaderBar::builder()
            .title_widget(&switcher)
            .build();
        sidebar_header.add_css_class("flat");
        sidebar_header.add_css_class("sidebar-header");
        
        let sidebar_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        
        let handle_event_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        handle_event_box.set_width_request(6);
        handle_event_box.add_css_class("sidebar-resize-handle");
        handle_event_box.set_cursor_from_name(Some("col-resize"));

        let drag = gtk::GestureDrag::new();
        
        let start_width = Rc::new(Cell::new(0));
        let last_width = Rc::new(Cell::new(0));

        drag.connect_drag_begin(glib::clone!(
            #[weak] sidebar_toolbar,
            #[strong] start_width,
            #[strong] last_width,
            move |_, _, _| {
                let w = sidebar_toolbar.width();
                start_width.set(w);
                last_width.set(w);
            }
        ));

        let tx_drag_update = tx.clone();
        drag.connect_drag_update(glib::clone!(
            #[weak] sidebar_toolbar,
            #[strong] start_width,
            #[strong] last_width,
            move |_, offset_x, _| {
                let true_offset_x = offset_x + (last_width.get() - start_width.get()) as f64;
                let offset_snapped = (true_offset_x as i32 / 10) * 10;
                
                if offset_snapped == 0 {
                    return;
                }
                
                let target_width = (start_width.get() + offset_snapped).clamp(200, 800);

                if target_width != last_width.get() {
                    sidebar_toolbar.set_width_request(target_width);
                    last_width.set(target_width);
                    let _ = tx_drag_update.send_blocking(AppInput::SidebarWidthChanged(target_width));
                }
            }
        ));
        let tx_drag_end = tx.clone();
        drag.connect_drag_end(glib::clone!(
            #[weak] sidebar_toolbar,
            #[strong] start_width,
            #[strong] last_width,
            move |_, offset_x, _| {
                let true_offset_x = offset_x + (last_width.get() - start_width.get()) as f64;
                let offset_snapped = (true_offset_x as i32 / 10) * 10;
                let target_width = (start_width.get() + offset_snapped).clamp(200, 800);
                sidebar_toolbar.set_width_request(target_width);
                let _ = tx_drag_end.send_blocking(AppInput::SidebarWidthChanged(target_width));
            }
        ));        
        handle_event_box.add_controller(drag);
        
        let sidebar_content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar_content_box.set_hexpand(true);
        sidebar_content_box.append(&sidebar_header);
        sidebar_content_box.append(&view_stack);
        
        sidebar_box.append(&sidebar_content_box);
        sidebar_box.append(&handle_event_box);

        sidebar_toolbar.set_content(Some(&sidebar_box));
        
        (sidebar_toolbar, view_stack)
    }

    fn build_content_area(
        tx: &async_channel::Sender<AppInput>,
        tab_view: &adw::TabView,
        current_settings: &Settings,
    ) -> (adw::ToolbarView, adw::HeaderBar, gtk::Image, adw::WindowTitle, gtk::Stack, gtk::Button, adw::TabBar, gtk::Button, crate::boxxyclaw_indicator_popover::BoxxyclawIndicatorPopover) {
        let content_toolbar = adw::ToolbarView::new();
        let content_header = adw::HeaderBar::builder().build();
        content_header.add_css_class("flat");
        content_header.add_css_class("terminal-header");
        content_header.add_css_class("content-header");

        let tab_bar = adw::TabBar::builder()
            .autohide(!current_settings.always_show_tabs)
            .expand_tabs(!current_settings.fixed_width_tabs)
            .view(tab_view)
            .build();

        let single_tab_title = adw::WindowTitle::builder()
            .title("Terminal")
            .build();

        let header_title_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::None)
            .build();
        header_title_stack.add_named(&tab_bar, Some("tabs"));
        header_title_stack.add_named(&single_tab_title, Some("title"));
        header_title_stack.set_visible_child_name("title");

        content_header.set_title_widget(Some(&header_title_stack));

        let toggle_sidebar_btn = gtk::Button::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Toggle Sidebar")
            .build();
        let tx_toggle = tx.clone();
        toggle_sidebar_btn.connect_clicked(move |_| {
            let _ = tx_toggle.send_blocking(AppInput::ToggleSidebar);
        });
        content_header.pack_start(&toggle_sidebar_btn);

        let menu_btn = gtk::Button::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Menu")
            .build();
        let tx_menu_btn = tx.clone();
        menu_btn.connect_clicked(move |_| {
            let _ = tx_menu_btn.send_blocking(AppInput::ShowCommandPaletteMenu);
        });
        content_header.pack_end(&menu_btn);

        let claw_img = gtk::Image::builder()
            .icon_name("boxxyclaw")
            .pixel_size(20)
            .build();

        let claw_indicator = gtk::Button::builder()
            .child(&claw_img)
            .tooltip_text("Claw Agent Options")
            .css_classes(["flat", "claw-indicator-inactive", "image-button"])
            .build();

        let tx_enable = tx.clone();

        let claw_popover = crate::boxxyclaw_indicator_popover::BoxxyclawIndicatorPopover::new(
            move |enabled| {
                let _ = tx_enable.send_blocking(AppInput::SetClawActive(enabled));
            },
            move |proactive| {
                let mut s = boxxy_preferences::Settings::load();
                s.claw_auto_diagnosis_mode = if proactive {
                    boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
                } else {
                    boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
                };
                s.save();
                // Saving triggers the settings event bus to update everything else
            },
            move |terminal| {
                let mut s = boxxy_preferences::Settings::load();
                s.claw_terminal_suggestions = terminal;
                s.save();
            }
        );
        
        let pop_clone = claw_popover.popover().clone();
        let btn_clone = claw_indicator.clone();
        claw_indicator.connect_clicked(move |_| {
            pop_clone.set_parent(&btn_clone);
            pop_clone.popup();
        });
        content_header.pack_end(&claw_indicator);

        let bell_indicator = gtk::Image::builder()
            .icon_name("visual-bell-symbolic")
            .visible(false)
            .margin_start(6)
            .margin_end(6)
            .build();
        content_header.pack_start(&bell_indicator);

        content_toolbar.add_top_bar(&content_header);
        content_toolbar.set_content(Some(tab_view));

        let tx_closetab = tx.clone();
        tab_view.connect_close_page(move |_, page| {
            let key = page.child().as_ptr() as usize;
            let _ = tx_closetab.send_blocking(AppInput::CloseTabRequest(key));
            gtk::glib::Propagation::Stop
        });
        
        (content_toolbar, content_header, bell_indicator, single_tab_title, header_title_stack, menu_btn, tab_bar, claw_indicator, claw_popover)
    }
}
