use gtk4::prelude::*;
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
        match event.kind {
            TerminalEventKind::Notification(message) => {
                let _ = inner.tx.send_blocking(AppInput::ShowToast(
                    crate::state::ToastRequest::ephemeral(message),
                ));
            }
            TerminalEventKind::RestoreDetached(pid, cwd) => {
                if pid == 0 {
                    let tx = inner.tx.clone();
                    gtk4::glib::spawn_future_local(async move {
                        let agent = boxxy_terminal::get_agent().await;
                        if let Ok(remaining) = agent.list_detached_sessions_detailed().await
                            && !remaining.is_empty()
                        {
                            let _ = tx.send(AppInput::ReattachDetachedBatch(remaining)).await;
                        }
                    });
                } else {
                    let cwd_opt = (!cwd.is_empty()).then_some(cwd);
                    let _ = inner
                        .tx
                        .send_blocking(AppInput::ReattachDetachedSession(pid, cwd_opt));
                }
            }
            _ => {}
        }
        return;
    }

    if let Some(pos) = inner.tabs.iter().position(|c| c.id == event.id) {
        match event.kind {
            TerminalEventKind::TitleChanged(title) => {
                inner.tabs[pos].raw_title = Some(title);
                if inner.tabs[pos].custom_title.is_none() {
                    let widget = inner.tabs[pos].controller.widget();
                    let page = inner.tab_view.page(widget);
                    if Some(&page) != inner.tab_view.selected_page().as_ref() {
                        inner.tabs[pos].unread_bell = true;
                    }
                }
                super::tabs::sync_tab_titles(inner);
            }
            TerminalEventKind::DirectoryChanged(path) => {
                inner.tabs[pos].cwd = Some(path);
                super::tabs::sync_tab_titles(inner);
            }
            TerminalEventKind::Exited(_code) => {
                let id = event.id;
                super::tabs::close_tab(inner, id);
            }
            TerminalEventKind::BellRung => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);
                if Some(&page) != inner.tab_view.selected_page().as_ref() {
                    inner.tabs[pos].unread_bell = true;
                    super::tabs::sync_tab_running_icons(inner);
                } else {
                    inner.bell_indicator.set_visible(true);
                }
            }
            TerminalEventKind::ForegroundProcessChanged(proc_name) => {
                let new_fg = (!proc_name.is_empty()).then_some(proc_name);
                if new_fg.is_none() {
                    inner.tabs[pos].raw_title = inner.tabs[pos].controller.last_title();
                }
                inner.tabs[pos].foreground_process = new_fg;
                super::tabs::sync_tab_titles(inner);
            }
            TerminalEventKind::AltScreenChanged(is_alt) => {
                if !is_alt {
                    inner.tabs[pos].raw_title = inner.tabs[pos].controller.last_title();
                }
                super::tabs::sync_tab_titles(inner);
            }
            TerminalEventKind::Osc133A | TerminalEventKind::Osc133D(_, _) => {
                inner.tabs[pos].foreground_process =
                    inner.tabs[pos].controller.foreground_process();
                inner.tabs[pos].raw_title = inner.tabs[pos].controller.last_title();
                super::tabs::sync_tab_titles(inner);
            }
            TerminalEventKind::Osc133B | TerminalEventKind::Osc133C => {}
            TerminalEventKind::Notification(message) => {
                let _ = inner.tx.send_blocking(AppInput::PushGlobalNotification(
                    crate::widgets::notification::Notification::new_info(message),
                ));
            }
            TerminalEventKind::ClawStateChanged(active, _sleep) => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);

                super::tabs::sync_tab_running_icons(inner);

                if Some(&page) == inner.tab_view.selected_page().as_ref() {
                    inner.claw_active = active;
                    inner.claw.set_pane_state(
                        &inner.tabs[pos].controller.agent_name(),
                        inner.tabs[pos].controller.is_sleep(),
                        inner.tabs[pos].controller.is_pinned(),
                        inner.tabs[pos].controller.is_web_search(),
                        inner.tabs[pos].controller.is_vibe(),
                    );
                    inner
                        .claw
                        .set_session_usage(inner.tabs[pos].controller.get_session_usage());
                    inner.claw.update_tasks(inner.tabs[pos].get_pending_tasks());
                }
            }
            TerminalEventKind::PaneFocused(_) => {
                inner.tabs[pos].cwd = inner.tabs[pos].controller.working_dir();
                inner.tabs[pos].raw_title = inner.tabs[pos].controller.last_title();
                inner.tabs[pos].foreground_process =
                    inner.tabs[pos].controller.foreground_process();
                super::tabs::sync_tab_titles(inner);

                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);
                let is_claw_active = inner.tabs[pos].controller.is_claw_active();

                if Some(&page) == inner.tab_view.selected_page().as_ref() {
                    inner.claw_active = is_claw_active;

                    inner.claw.set_pane_state(
                        &inner.tabs[pos].controller.agent_name(),
                        inner.tabs[pos].controller.is_sleep(),
                        inner.tabs[pos].controller.is_pinned(),
                        inner.tabs[pos].controller.is_web_search(),
                        inner.tabs[pos].controller.is_vibe(),
                    );
                    inner
                        .claw
                        .set_session_usage(inner.tabs[pos].controller.get_session_usage());
                    inner.claw.update_tasks(inner.tabs[pos].get_pending_tasks());
                }
            }
            TerminalEventKind::FocusClawSidebar => {
                super::window_state::show_claw_sidebar(inner);
            }
            TerminalEventKind::ClawEvent(p_id, claw_event) => {
                // Update session usage if this is the active tab/pane
                let session_usage = inner.tabs[pos].controller.get_session_usage();
                if let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if inner.tabs[pos].controller.widget() == &child {
                        inner.claw.set_session_usage(session_usage);
                    }
                }

                match claw_event {
                    ClawEngineEvent::SessionStateChanged { status, .. } => {
                        inner.tabs[pos]
                            .controller
                            .set_session_status_for_pane(&p_id, status);
                        if let Some(page) = inner.tab_view.selected_page()
                            && inner.tabs[pos].controller.widget() == &page.child()
                        {
                            inner.claw.set_pane_state(
                                &inner.tabs[pos].controller.agent_name(),
                                inner.tabs[pos].controller.is_sleep(),
                                inner.tabs[pos].controller.is_pinned(),
                                inner.tabs[pos].controller.is_web_search(),
                                inner.tabs[pos].controller.is_vibe(),
                            );
                        }
                    }
                    ClawEngineEvent::Identity {
                        agent_name,
                        pinned,
                        web_search_enabled,
                        vibe_enabled,
                        ..
                    } if let Some(page) = inner.tab_view.selected_page()
                        && inner.tabs[pos].controller.widget() == &page.child() =>
                    {
                        // If we got an identity, ensure the sidebar UI reflects that this pane is now active
                        let active = inner.tabs[pos].controller.is_claw_active();
                        let sleep = inner.tabs[pos].controller.is_sleep();
                        inner.claw_active = active;
                        inner.claw.set_pane_state(
                            &agent_name,
                            sleep,
                            pinned,
                            web_search_enabled,
                            vibe_enabled,
                        );
                    }
                    ClawEngineEvent::PinStatusChanged(pinned)
                        if let Some(page) = inner.tab_view.selected_page()
                            && inner.tabs[pos].controller.widget() == &page.child() =>
                    {
                        inner.claw.set_pane_state(
                            &inner.tabs[pos].controller.agent_name(),
                            inner.tabs[pos].controller.is_sleep(),
                            pinned,
                            inner.tabs[pos].controller.is_web_search(),
                            inner.tabs[pos].controller.is_vibe(),
                        );
                    }
                    ClawEngineEvent::WebSearchStatusChanged(enabled)
                        if let Some(page) = inner.tab_view.selected_page()
                            && inner.tabs[pos].controller.widget() == &page.child() =>
                    {
                        inner.claw.set_pane_state(
                            &inner.tabs[pos].controller.agent_name(),
                            inner.tabs[pos].controller.is_sleep(),
                            inner.tabs[pos].controller.is_pinned(),
                            enabled,
                            inner.tabs[pos].controller.is_vibe(),
                        );
                    }
                    ClawEngineEvent::VibeStatusChanged(vibe)
                        if let Some(page) = inner.tab_view.selected_page()
                            && inner.tabs[pos].controller.widget() == &page.child() =>
                    {
                        inner.claw.set_pane_state(
                            &inner.tabs[pos].controller.agent_name(),
                            inner.tabs[pos].controller.is_sleep(),
                            inner.tabs[pos].controller.is_pinned(),
                            inner.tabs[pos].controller.is_web_search(),
                            vibe,
                        );
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
                        location,
                        intent,
                        request_id,
                        ..
                    } => {
                        let source_ctrl = inner.tabs[pos].controller.clone();
                        let source_pane_id = p_id.clone();
                        let cwd = source_ctrl.working_dir().unwrap_or_default();
                        let new_pane_id = match location {
                            SpawnLocation::NewTab => {
                                let new_ctrl = super::tabs::new_tab_with_intent(inner, intent);
                                new_ctrl.active_pane_id()
                            }
                            SpawnLocation::VerticalSplit => source_ctrl.split_vertical(intent),
                            SpawnLocation::HorizontalSplit => source_ctrl.split_horizontal(intent),
                        };

                        if let Some(req_id) = request_id {
                            gtk4::glib::spawn_future_local(async move {
                                let agent = boxxy_terminal::get_agent().await;
                                let alias = agent
                                    .update_workspace_pane(
                                        new_pane_id.clone(),
                                        cwd,
                                        String::new(),
                                        String::new(),
                                        "Idle".to_string(),
                                    )
                                    .await
                                    .unwrap_or_else(|_| new_pane_id.clone());
                                source_ctrl.send_claw_to_pane(
                                    &source_pane_id,
                                    ClawMessage::SpawnAgentReply {
                                        request_id: req_id,
                                        pane_id: new_pane_id,
                                        alias,
                                    },
                                );
                            });
                        }
                    }
                    ClawEngineEvent::RequestReattachShell { pid, cwd } => {
                        if pid == 0 {
                            let tx = inner.tx.clone();
                            gtk4::glib::spawn_future_local(async move {
                                let agent = boxxy_terminal::get_agent().await;
                                if let Ok(remaining) = agent.list_detached_sessions_detailed().await
                                    && !remaining.is_empty()
                                {
                                    let _ =
                                        tx.send(AppInput::ReattachDetachedBatch(remaining)).await;
                                }
                            });
                        } else {
                            let _ = inner
                                .tx
                                .send_blocking(AppInput::ReattachDetachedSession(pid, cwd));
                        }
                    }
                    ClawEngineEvent::RequestCloseAgent {
                        target_agent_name,
                        target_pane_id,
                    } => {
                        if let Some(pane_id) = target_pane_id {
                            for tab in &inner.tabs {
                                if tab.controller.close_pane_by_id(&pane_id) {
                                    break;
                                }
                            }
                        } else {
                            let inner_clone = _inner_ref.clone();
                            let target_name = target_agent_name.clone();
                            gtk4::glib::spawn_future_local(async move {
                                let workspace =
                                    boxxy_claw::registry::workspace::global_workspace().await;
                                let resolved = workspace
                                    .resolve_pane_id_by_name(&target_name)
                                    .await
                                    .unwrap_or(target_name);
                                let inner = inner_clone.borrow_mut();
                                for tab in &inner.tabs {
                                    if tab.controller.close_pane_by_id(&resolved) {
                                        break;
                                    }
                                }
                            });
                        }
                    }
                    ClawEngineEvent::InjectKeystrokes {
                        target_agent_name,
                        target_pane_id,
                        keys,
                    } => {
                        if let Some(pane_id) = target_pane_id {
                            for tab in &inner.tabs {
                                if tab.controller.inject_keystrokes_by_id(&pane_id, &keys) {
                                    break;
                                }
                            }
                        } else {
                            let inner_clone = _inner_ref.clone();
                            let target_name = target_agent_name.clone();
                            let keys = keys.clone();
                            gtk4::glib::spawn_future_local(async move {
                                let workspace =
                                    boxxy_claw::registry::workspace::global_workspace().await;
                                let resolved = workspace
                                    .resolve_pane_id_by_name(&target_name)
                                    .await
                                    .unwrap_or(target_name);
                                let inner = inner_clone.borrow();
                                for tab in &inner.tabs {
                                    if tab.controller.inject_keystrokes_by_id(&resolved, &keys) {
                                        break;
                                    }
                                }
                            });
                        }
                    }
                    ClawEngineEvent::TaskStatusChanged { .. } => {
                        let widget = inner.tabs[pos].controller.widget();
                        let page = inner.tab_view.page(widget);

                        super::tabs::sync_tab_running_icons(inner);

                        // Update sidebar only from the active pane of the selected tab so
                        // background split panes never overwrite the focused pane's tasks.
                        if let Some(selected) = inner.tab_view.selected_page()
                            && selected == page
                        {
                            inner.claw.update_tasks(inner.tabs[pos].get_pending_tasks());
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

                        let mut resolved_name = agent_name.clone();
                        let mut icon_path = "boxxyclaw-symbolic".to_string();
                        let mut char_color: Option<String> = None;
                        let mut avatar_file_path: Option<String> = None;

                        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
                        let fallback_names = [agent_name.as_str()];
                        if let Some(info) = boxxy_claw_protocol::characters::find_character_exact(
                            &registry,
                            &character_id,
                            &fallback_names,
                        ) {
                            if !info.config.display_name.trim().is_empty() {
                                resolved_name.clone_from(&info.config.display_name);
                            } else {
                                resolved_name = info.config.name.replace('-', " ");
                            }
                            char_color = Some(info.config.color.clone());
                            if info.has_avatar {
                                let mut path = boxxy_sys_utils::get_config_dir();
                                path.push("boxxyclaw");
                                path.push("characters");
                                path.push(&info.config.name);
                                path.push("AVATAR.png");
                                let path_str = path.to_string_lossy().to_string();
                                icon_path.clone_from(&path_str);
                                avatar_file_path = Some(path_str);
                            }
                        }

                        let raw_first = resolved_name
                            .split([' ', '-'])
                            .find(|s| !s.is_empty())
                            .unwrap_or("Claw");
                        let first_name = {
                            let mut c = raw_first.chars();
                            match c.next() {
                                None => "Claw".to_string(),
                                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                            }
                        };

                        let display_message = message
                            .clone()
                            .unwrap_or_else(|| format!("{} has finished a timer.", first_name));

                        let _ = inner.tx.send_blocking(AppInput::PushGlobalNotification(
                            crate::widgets::notification::Notification {
                                id: uuid::Uuid::new_v4().to_string(),
                                level: crate::widgets::notification::NotificationLevel::Info,
                                title: format!("{} reminds", first_name),
                                message: display_message,
                                icon_name: icon_path,
                                actions: vec![crate::widgets::notification::NotificationAction {
                                    label: "Dismiss".to_string(),
                                    action_name: "win.dismiss-notification".to_string(),
                                    is_primary: false,
                                }],
                                details: Vec::new(),
                                suppress_toast: false,
                                character_name: Some(first_name),
                                character_color: char_color,
                                avatar_path: avatar_file_path,
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

                        let raw_first = agent_name.split_whitespace().next().unwrap_or(&agent_name);
                        let first_name = {
                            let mut c = raw_first.chars();
                            match c.next() {
                                None => String::new(),
                                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                            }
                        };
                        let mut icon_path = "boxxyclaw-symbolic".to_string();

                        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
                        if let Some(info) = registry.iter().find(|c| c.config.id == *character_id) {
                            if info.has_avatar {
                                let mut path = boxxy_sys_utils::get_config_dir();
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
                                // Redundant because the agent's message is already in the Claw overlay
                                suppress_toast: true,
                                character_name: None,
                                character_color: None,
                                avatar_path: None,
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
                                suppress_toast: false,
                                character_name: None,
                                character_color: None,
                                avatar_path: None,
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
            TerminalEventKind::MovePaneToSplit {
                source_pane_id,
                target_pane_id,
                direction,
            } => {
                super::split::move_pane_to_split(inner, source_pane_id, target_pane_id, direction);
            }
            TerminalEventKind::RestoreDetached(_, _) => {}
        }
    }
}
