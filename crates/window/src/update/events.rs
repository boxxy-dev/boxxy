use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::state::{AppInput, AppWindowInner};
use boxxy_claw_protocol::*;
use boxxy_terminal::{TerminalEvent, TerminalEventKind};

pub fn handle_terminal_event(
    _inner_ref: &Rc<RefCell<AppWindowInner>>,
    inner: &mut AppWindowInner,
    event: TerminalEvent,
) {
    // Global events (no pane id) — handle before the per-pane match.
    if event.id.is_empty() {
        if let TerminalEventKind::Notification(message) = event.kind {
            let _ =
                inner
                    .tx
                    .send_blocking(AppInput::ShowToast(crate::state::ToastRequest::ephemeral(
                        message,
                    )));
        }
        return;
    }

    if let Some(pos) = inner.tabs.iter().position(|c| c.id == event.id) {
        match event.kind {
            TerminalEventKind::TitleChanged(title) => {
                if inner.tabs[pos].custom_title.is_none() {
                    let widget = inner.tabs[pos].controller.widget();
                    let page = inner.tab_view.page(widget);
                    page.set_title(&title);
                    if Some(&page) != inner.tab_view.selected_page().as_ref() {
                        page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                            "boxxy-visual-bell-symbolic",
                        )));
                        page.set_indicator_activatable(false);
                    }
                    super::tabs::sync_header_title(inner);
                }
            }
            TerminalEventKind::DirectoryChanged(path) => {
                inner.tabs[pos].cwd = Some(path);
            }
            TerminalEventKind::Exited(_code) => {
                let id = event.id;
                super::tabs::close_tab(inner, id);
            }
            TerminalEventKind::BellRung => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);
                if Some(&page) != inner.tab_view.selected_page().as_ref() {
                    page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                        "boxxy-visual-bell-symbolic",
                    )));
                    page.set_indicator_activatable(false);
                } else {
                    inner.bell_indicator.set_visible(true);
                }
            }
            TerminalEventKind::Osc133A
            | TerminalEventKind::Osc133B
            | TerminalEventKind::Osc133C
            | TerminalEventKind::Osc133D(_, _)
            | TerminalEventKind::ForegroundProcessChanged(_) => {}
            TerminalEventKind::Notification(message) => {
                let _ = inner.tx.send_blocking(AppInput::PushGlobalNotification(
                    crate::widgets::notification::Notification::new_info(message),
                ));
            }
            TerminalEventKind::ClawStateChanged(active, _sleep) => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);

                // Update tab icon (swap: claw is indicator, timer is main icon)
                if active {
                    page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                        "boxxy-boxxyclaw-symbolic",
                    )));
                    page.set_indicator_activatable(false);
                } else {
                    page.set_indicator_icon(None::<&gtk4::gio::Icon>);
                }

                if Some(&page) == inner.tab_view.selected_page().as_ref() {
                    inner.claw_active = active;
                }
            }
            TerminalEventKind::PaneFocused(_) => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);

                // Update tab icon
                let is_claw_active = inner.tabs[pos].controller.is_claw_active();
                if is_claw_active {
                    page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                        "boxxy-boxxyclaw-symbolic",
                    )));
                    page.set_indicator_activatable(false);
                } else {
                    page.set_indicator_icon(None::<&gtk4::gio::Icon>);
                }

                if Some(&page) == inner.tab_view.selected_page().as_ref() {
                    inner.claw_active = is_claw_active;

                    inner.claw.set_history_widget(
                        &inner.tabs[pos].controller.claw_history_widget(),
                        &inner.tabs[pos].controller.agent_name(),
                        inner.tabs[pos].controller.is_pinned(),
                        inner.tabs[pos].controller.is_web_search(),
                    );
                    inner
                        .claw
                        .set_token_usage(inner.tabs[pos].controller.get_total_tokens());
                }
            }
            TerminalEventKind::FocusClawSidebar => {
                if !inner.sidebar_visible {
                    inner.sidebar_visible = true;
                    inner.app_state.sidebar_visible = true;
                    inner.app_state.save();
                    if let Some(split) = inner
                        .window
                        .content()
                        .and_then(|c| c.downcast::<libadwaita::OverlaySplitView>().ok())
                    {
                        split.set_show_sidebar(true);
                    }
                }
                inner.view_stack.set_visible_child_name("claw");
            }
            TerminalEventKind::ClawEvent(p_id, claw_event) => {
                // Update token usage if this is the active tab/pane
                let total_tokens = inner.tabs[pos].controller.get_total_tokens();
                if let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if inner.tabs[pos].controller.widget() == &child {
                        inner.claw.set_token_usage(total_tokens);
                    }
                }

                match claw_event {
                    ClawEngineEvent::SessionStateChanged { status, .. } => {
                        inner.tabs[pos]
                            .controller
                            .set_session_status_for_pane(&p_id, status);
                    }
                    ClawEngineEvent::Identity {
                        agent_name,
                        pinned,
                        web_search_enabled,
                        ..
                    } => {
                        // If we got an identity, ensure the sidebar UI reflects that this pane is now active
                        if let Some(page) = inner.tab_view.selected_page() {
                            let child = page.child();
                            if inner.tabs[pos].controller.widget() == &child {
                                let active = inner.tabs[pos].controller.is_claw_active();
                                let _sleep = inner.tabs[pos].controller.is_sleep();
                                inner.claw_active = active;
                                inner.claw.set_history_widget(
                                    &inner.tabs[pos].controller.claw_history_widget(),
                                    &agent_name,
                                    pinned,
                                    web_search_enabled,
                                );
                            }
                        }
                    }
                    ClawEngineEvent::PinStatusChanged(pinned) => {
                        if let Some(page) = inner.tab_view.selected_page() {
                            let child = page.child();
                            if inner.tabs[pos].controller.widget() == &child {
                                inner.claw.set_history_widget(
                                    &inner.tabs[pos].controller.claw_history_widget(),
                                    &inner.tabs[pos].controller.agent_name(),
                                    pinned,
                                    inner.tabs[pos].controller.is_web_search(),
                                );
                            }
                        }
                    }
                    ClawEngineEvent::WebSearchStatusChanged(enabled) => {
                        if let Some(page) = inner.tab_view.selected_page() {
                            let child = page.child();
                            if inner.tabs[pos].controller.widget() == &child {
                                inner.claw.set_history_widget(
                                    &inner.tabs[pos].controller.claw_history_widget(),
                                    &inner.tabs[pos].controller.agent_name(),
                                    inner.tabs[pos].controller.is_pinned(),
                                    enabled,
                                );
                            }
                        }
                    }
                    ClawEngineEvent::DismissDrawer => {
                        inner.claw.refresh_visibility();
                    }
                    ClawEngineEvent::FocusPane => {
                        // Raise the window
                        inner.window.present();

                        // Select the tab containing this pane
                        let widget = inner.tabs[pos].controller.widget();
                        let page = inner.tab_view.page(widget);
                        inner.tab_view.set_selected_page(&page);

                        // Focus the specific pane within the terminal component
                        inner.tabs[pos].controller.focus_pane_by_id(&p_id);
                    }
                    ClawEngineEvent::DiagnosisComplete { .. }
                    | ClawEngineEvent::InjectCommand { .. }
                    | ClawEngineEvent::ProposeFileWrite { .. }
                    | ClawEngineEvent::RestoreHistory(..)
                    | ClawEngineEvent::ProposeTerminalCommand { .. } => {
                        inner.claw.refresh_visibility();
                    }
                    ClawEngineEvent::RequestSpawnAgent {
                        location, intent, ..
                    } => match location {
                        SpawnLocation::NewTab => {
                            super::tabs::new_tab_with_intent(inner, intent);
                        }
                        SpawnLocation::VerticalSplit => {
                            inner.tabs[pos].controller.split_vertical(intent);
                        }
                        SpawnLocation::HorizontalSplit => {
                            inner.tabs[pos].controller.split_horizontal(intent);
                        }
                    },
                    ClawEngineEvent::RequestCloseAgent { target_agent_name } => {
                        let inner_clone = _inner_ref.clone();
                        let target_name = target_agent_name.clone();
                        gtk4::glib::spawn_future_local(async move {
                            let workspace =
                                boxxy_claw::registry::workspace::global_workspace().await;
                            if let Some(pane_id) =
                                workspace.resolve_pane_id_by_name(&target_name).await
                            {
                                let inner = inner_clone.borrow_mut();
                                // Search all tabs for this pane
                                for tab in &inner.tabs {
                                    if tab.controller.close_pane_by_id(&pane_id) {
                                        break;
                                    }
                                }
                            }
                        });
                    }
                    ClawEngineEvent::InjectKeystrokes {
                        target_agent_name,
                        keys,
                    } => {
                        let inner_clone = _inner_ref.clone();
                        let target_name = target_agent_name.clone();
                        let keys = keys.clone();
                        gtk4::glib::spawn_future_local(async move {
                            let workspace =
                                boxxy_claw::registry::workspace::global_workspace().await;
                            if let Some(pane_id) =
                                workspace.resolve_pane_id_by_name(&target_name).await
                            {
                                let inner = inner_clone.borrow();
                                for tab in &inner.tabs {
                                    if tab.controller.inject_keystrokes_by_id(&pane_id, &keys) {
                                        break;
                                    }
                                }
                            }
                        });
                    }
                    ClawEngineEvent::TaskStatusChanged { tasks, .. } => {
                        let has_pending = tasks.iter().any(|t| t.status == TaskStatus::Pending);
                        let widget = inner.tabs[pos].controller.widget();
                        let page = inner.tab_view.page(widget);

                        if has_pending {
                            // Replace claw with colored timer composite in the indicator slot
                            page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                                "boxxy-timer-symbolic",
                            )));
                            page.set_indicator_activatable(false);
                        } else {
                            // Revert to normal claw indicator
                            page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                                "boxxy-boxxyclaw-symbolic",
                            )));
                            page.set_indicator_activatable(false);
                        }

                        // Update sidebar if this is the active tab
                        if let Some(selected) = inner.tab_view.selected_page() {
                            if selected == page {
                                inner.claw.update_tasks(tasks);
                            }
                        }
                    }
                    ClawEngineEvent::TaskCompleted {
                        agent_name,
                        character_id,
                        message,
                        ..
                    } => {
                        let settings = boxxy_preferences::Settings::load();
                        if settings.enable_timer_sounds {
                            crate::sound::play_timer_completion_sound();
                        }

                        let first_name =
                            agent_name.split_whitespace().next().unwrap_or(&agent_name);
                        let mut icon_path = "boxxyclaw-symbolic".to_string();

                        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
                        if let Some(info) = registry.iter().find(|c| c.config.id == *character_id) {
                            if info.has_avatar {
                                let mut path = gtk4::glib::user_config_dir();
                                path.push("boxxy-terminal");
                                path.push("boxxyclaw");
                                path.push("characters");
                                path.push(&info.config.name);
                                path.push("AVATAR.png");
                                icon_path = path.to_string_lossy().to_string();
                            }
                        }

                        let display_message = message
                            .clone()
                            .unwrap_or_else(|| format!("{} has finished a timer.", first_name));

                        let _ = inner.tx.send_blocking(AppInput::PushGlobalNotification(
                            crate::widgets::notification::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                level: crate::widgets::notification::NotificationLevel::Info,
                                title: format!("{} sends a reminder", first_name),
                                message: display_message,
                                icon_name: icon_path,
                                actions: vec![crate::widgets::notification::NotificationAction {
                                    label: "Dismiss".to_string(),
                                    action_name: "win.dismiss-notification".to_string(),
                                    is_primary: false,
                                }],
                                details: Vec::new(),
                            },
                        ));
                    }
                    ClawEngineEvent::LongTaskCompleted {
                        agent_name,
                        character_id,
                        message,
                        ..
                    } => {
                        let settings = boxxy_preferences::Settings::load();
                        if settings.enable_task_sounds {
                            crate::sound::play_task_completion_sound();
                        }

                        let first_name =
                            agent_name.split_whitespace().next().unwrap_or(&agent_name);
                        let mut icon_path = "boxxyclaw-symbolic".to_string();

                        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
                        if let Some(info) = registry.iter().find(|c| c.config.id == *character_id) {
                            if info.has_avatar {
                                let mut path = gtk4::glib::user_config_dir();
                                path.push("boxxy-terminal");
                                path.push("boxxyclaw");
                                path.push("characters");
                                path.push(&info.config.name);
                                path.push("AVATAR.png");
                                icon_path = path.to_string_lossy().to_string();
                            }
                        }

                        let _ = inner.tx.send_blocking(AppInput::PushGlobalNotification(
                            crate::widgets::notification::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                level: crate::widgets::notification::NotificationLevel::Info,
                                title: format!("{} completed a task", first_name),
                                message,
                                icon_name: icon_path,
                                actions: vec![crate::widgets::notification::NotificationAction {
                                    label: "Dismiss".to_string(),
                                    action_name: "win.dismiss-notification".to_string(),
                                    is_primary: false,
                                }],
                                details: Vec::new(),
                            },
                        ));
                    }
                    ClawEngineEvent::PushGlobalNotification { title, message } => {
                        let _ = inner.tx.send_blocking(AppInput::PushGlobalNotification(
                            crate::widgets::notification::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                level: crate::widgets::notification::NotificationLevel::Info,
                                title: title.clone(),
                                message: message.clone(),
                                icon_name: "boxxyclaw-symbolic".to_string(),
                                actions: vec![crate::widgets::notification::NotificationAction {
                                    label: "Dismiss".to_string(),
                                    action_name: "win.dismiss-notification".to_string(),
                                    is_primary: false,
                                }],
                                details: Vec::new(),
                            },
                        ));
                    }
                    _ => {} // Other events like AgentThinking or FileWrite are handled strictly by the Pane UI
                }
            }
            TerminalEventKind::ZoomIn => {
                let _ = inner.tx.send_blocking(AppInput::ZoomIn);
            }
            TerminalEventKind::ZoomOut => {
                let _ = inner.tx.send_blocking(AppInput::ZoomOut);
            }
            TerminalEventKind::ResetZoom => {
                let _ = inner.tx.send_blocking(AppInput::ResetZoom);
            }
        }
    }
}
