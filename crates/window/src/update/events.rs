use gtk4::prelude::*;

use crate::state::{AppInput, AppWindowInner, ToastRequest};
use crate::widgets::notification::{Notification, NotificationAction, NotificationLevel};
use boxxy_claw_protocol::characters::{
    CHARACTER_CACHE, CLAIMS_CACHE, CharacterClaim, CharacterInfo,
};
use boxxy_claw_protocol::*;
use boxxy_terminal::{TerminalEvent, TerminalEventKind};

/// Ensures global `RestoreDetached` broadcasts on `TERMINAL_EVENT_BUS` are handled
/// by exactly one live window when multiple windows are open.
fn is_primary_restore_window(win: &libadwaita::ApplicationWindow) -> bool {
    if win.is_active() {
        return true;
    }
    let Some(app) = win.application() else {
        return true;
    };
    let windows = app.windows();
    if windows.iter().any(GtkWindowExt::is_active) {
        return false;
    }
    windows
        .first()
        .is_none_or(|first| first == win.upcast_ref::<gtk4::Window>())
}

fn normalize_peer_key(raw: &str) -> String {
    raw.trim()
        .trim_start_matches(['@', ':'])
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Synchronously resolves a target pane ID from `CLAIMS_CACHE` and `CHARACTER_CACHE`
/// (matching `holder_id`, `character_id`, `petname`, `name`, or `display_name`),
/// falling back to `raw_target` if it is already a direct `pane_id`.
fn resolve_pane_id_in(
    raw_target: &str,
    claims: &[CharacterClaim],
    catalog: &[CharacterInfo],
) -> String {
    let trimmed = raw_target.trim().trim_start_matches(['@', ':']).trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let query_norm = normalize_peer_key(trimmed);

    for claim in claims {
        if claim.holder_id.eq_ignore_ascii_case(trimmed)
            || claim.character_id.eq_ignore_ascii_case(trimmed)
            || (!query_norm.is_empty() && normalize_peer_key(&claim.petname) == query_norm)
        {
            return claim.holder_id.clone();
        }
        if !query_norm.is_empty()
            && let Some(char_info) = catalog.iter().find(|c| c.config.id == claim.character_id)
            && (normalize_peer_key(&char_info.config.display_name) == query_norm
                || normalize_peer_key(&char_info.config.name) == query_norm)
        {
            return claim.holder_id.clone();
        }
    }

    trimmed.to_string()
}

fn resolve_pane_id_from_caches(raw_target: &str) -> String {
    let claims = CLAIMS_CACHE.load();
    let catalog = CHARACTER_CACHE.load();
    resolve_pane_id_in(raw_target, &claims, &catalog)
}

const BG_COMMAND_BELL_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(5);

#[inline]
fn should_ring_for_completed_command(
    elapsed: std::time::Duration,
    used_alt_screen: bool,
    tab_selected: bool,
) -> bool {
    !tab_selected && !used_alt_screen && elapsed >= BG_COMMAND_BELL_THRESHOLD
}

fn trigger_background_tab_bell(inner: &mut AppWindowInner, pos: usize) {
    if is_tab_selected(inner, pos) {
        return;
    }
    let tc = &mut inner.tabs[pos];
    if let Some(prev) = tc.bell_timer_source.borrow_mut().take() {
        prev.remove();
    }
    tc.unread_bell = true;
    tc.bell_ringing.set(true);

    let bell_ringing = tc.bell_ringing.clone();
    let bell_svg = tc.bell_svg.clone();
    let bell_timer_rc = tc.bell_timer_source.clone();

    let src = gtk4::glib::timeout_add_local_once(
        std::time::Duration::from_millis(1800),
        move || {
            let _ = bell_timer_rc.borrow_mut().take();
            // Settle the ringing bell down to static state 0 (`200ms` transition),
            // then pause the frame clock so idle unread tabs consume 0% CPU.
            bell_ringing.set(false);
            if let Some(svg) = bell_svg.borrow().clone() {
                svg.set_state(0);
                let ringing_check = bell_ringing;
                gtk4::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(240),
                    move || {
                        if !ringing_check.get() {
                            svg.pause();
                        }
                    },
                );
            }
        },
    );
    *tc.bell_timer_source.borrow_mut() = Some(src);
    super::tab_bar_sync::sync_tab_running_icons(inner);
}

fn maybe_ring_on_command_finished(inner: &mut AppWindowInner, pos: usize) {
    let used_alt = inner.tabs[pos].command_used_alt_screen;
    inner.tabs[pos].command_used_alt_screen = false;
    if let Some(started_at) = inner.tabs[pos].command_started_at.take() {
        let selected = is_tab_selected(inner, pos);
        if should_ring_for_completed_command(started_at.elapsed(), used_alt, selected) {
            trigger_background_tab_bell(inner, pos);
        }
    }
}

fn is_tab_selected(inner: &AppWindowInner, pos: usize) -> bool {
    inner
        .tab_view
        .selected_page()
        .is_some_and(|p| inner.tabs[pos].controller.widget() == &p.child())
}

fn sync_selected_claw_sidebar(inner: &mut AppWindowInner, pos: usize) {
    if !is_tab_selected(inner, pos) {
        return;
    }
    let tc = &inner.tabs[pos];
    inner.claw_active = tc.controller.is_claw_active();
    inner.claw.set_pane_state(
        &tc.controller.agent_name(),
        tc.controller.is_sleep(),
        tc.controller.is_pinned(),
        tc.controller.is_web_search(),
        tc.controller.is_vibe(),
    );
    inner
        .claw
        .set_session_usage(tc.controller.get_session_usage());
    inner.claw.update_tasks(tc.get_pending_tasks());
}

struct CharacterAttribution {
    first_name: String,
    icon_name: String,
    color: Option<String>,
    avatar_path: Option<String>,
}

fn resolve_character_attribution(character_id: &str, agent_name: &str) -> CharacterAttribution {
    let mut resolved_name = agent_name.to_string();
    let mut icon_name = "boxxyclaw-symbolic".to_string();
    let mut color = None;
    let mut avatar_path = None;

    let registry = CHARACTER_CACHE.load();
    let fallback_names = [agent_name];
    if let Some(info) = boxxy_claw_protocol::characters::find_character_exact(
        &registry,
        character_id,
        &fallback_names,
    ) {
        if !info.config.display_name.trim().is_empty() {
            resolved_name.clone_from(&info.config.display_name);
        } else {
            resolved_name = info.config.name.replace('-', " ");
        }
        color = Some(info.config.color.clone());
        if info.has_avatar {
            let mut path = boxxy_sys_utils::get_config_dir();
            path.push("boxxyclaw");
            path.push("characters");
            path.push(&info.config.name);
            path.push("AVATAR.png");
            let path_str = path.to_string_lossy().into_owned();
            icon_name.clone_from(&path_str);
            avatar_path = Some(path_str);
        }
    }

    let raw_first = resolved_name
        .split([' ', '-'])
        .find(|s| !s.is_empty())
        .unwrap_or("Claw");
    let first_name = {
        let mut c = raw_first.chars();
        c.next().map_or_else(
            || "Claw".to_string(),
            |f| f.to_uppercase().collect::<String>() + c.as_str(),
        )
    };

    CharacterAttribution {
        first_name,
        icon_name,
        color,
        avatar_path,
    }
}

pub fn handle_terminal_event(inner: &mut AppWindowInner, event: TerminalEvent) {
    // Global events (no pane id) — handle before the per-pane match.
    if event.id.is_empty() {
        match event.kind {
            TerminalEventKind::Notification(message) => {
                if is_primary_restore_window(&inner.window) {
                    let _ = inner
                        .tx
                        .send_blocking(AppInput::ShowToast(ToastRequest::ephemeral(message)));
                }
            }
            TerminalEventKind::RestoreDetached(pid, cwd) => {
                if !is_primary_restore_window(&inner.window) {
                    return;
                }
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
                    super::detached_sessions::reattach_session(inner, pid, cwd_opt);
                }
            }
            TerminalEventKind::SpawnCharacter(character_id, cwd) => {
                if !is_primary_restore_window(&inner.window) {
                    return;
                }
                let cwd_opt = (!cwd.is_empty()).then_some(cwd);
                let _ = super::tabs::new_tab_with_character(inner, character_id, cwd_opt);
            }
            TerminalEventKind::DetachedSessionsChanged(sessions) => {
                super::detached_sessions::update_badge(inner, &sessions);
            }
            _ => {}
        }
        return;
    }

    let Some(pos) = inner.tabs.iter().position(|c| c.id == event.id) else {
        return;
    };

    match event.kind {
        TerminalEventKind::TitleChanged(title) => {
            if inner.tabs[pos].raw_title.as_deref() == Some(title.as_str()) {
                return;
            }
            inner.tabs[pos].raw_title = Some(title);
            super::tab_bar_sync::sync_tab_titles(inner);
        }
        TerminalEventKind::DirectoryChanged(path) => {
            if inner.tabs[pos].cwd.as_deref() == Some(path.as_str()) {
                return;
            }
            inner.tabs[pos].cwd = Some(path);
            super::tab_bar_sync::sync_tab_titles(inner);
        }
        TerminalEventKind::Exited(_code) => {
            super::tabs::close_tab(inner, event.id);
        }
        TerminalEventKind::BellRung => {
            trigger_background_tab_bell(inner, pos);
        }
        TerminalEventKind::ForegroundProcessChanged(proc_name) => {
            let new_fg = (!proc_name.is_empty()).then_some(proc_name);
            if new_fg.is_none() {
                inner.tabs[pos].raw_title = inner.tabs[pos].controller.last_title();
            }
            inner.tabs[pos].foreground_process = new_fg;
            super::tab_bar_sync::sync_tab_titles(inner);

            let cat = inner.tabs[pos].app_state.application_category;
            if cat == crate::application_detector::ApplicationCategory::RunningCommand
                && !inner.tabs[pos].app_state.alternate_screen_active
            {
                if inner.tabs[pos].command_started_at.is_none() {
                    inner.tabs[pos].command_started_at = Some(std::time::Instant::now());
                    inner.tabs[pos].command_used_alt_screen = false;
                }
            } else if cat != crate::application_detector::ApplicationCategory::Idle {
                inner.tabs[pos].command_used_alt_screen = true;
            } else {
                maybe_ring_on_command_finished(inner, pos);
            }
        }
        TerminalEventKind::AltScreenChanged(is_alt) => {
            if is_alt {
                inner.tabs[pos].command_used_alt_screen = true;
            } else {
                inner.tabs[pos].raw_title = inner.tabs[pos].controller.last_title();
            }
            super::tab_bar_sync::sync_tab_titles(inner);
        }
        TerminalEventKind::Osc133A | TerminalEventKind::Osc133D(_, _) => {
            inner.tabs[pos].foreground_process = inner.tabs[pos].controller.foreground_process();
            inner.tabs[pos].raw_title = inner.tabs[pos].controller.last_title();
            super::tab_bar_sync::sync_tab_titles(inner);
            maybe_ring_on_command_finished(inner, pos);
        }
        TerminalEventKind::Osc133B => {}
        TerminalEventKind::Osc133C => {
            if inner.tabs[pos].command_started_at.is_none() {
                inner.tabs[pos].command_started_at = Some(std::time::Instant::now());
                inner.tabs[pos].command_used_alt_screen =
                    inner.tabs[pos].controller.is_alt_screen();
            }
        }
        TerminalEventKind::Notification(message) => {
            super::notifications::push_global_notification(inner, Notification::new_info(message));
        }
        TerminalEventKind::ClawStateChanged(_active, _sleep) => {
            super::tab_bar_sync::sync_tab_running_icons(inner);
            sync_selected_claw_sidebar(inner, pos);
        }
        TerminalEventKind::PaneFocused(_) => {
            inner.tabs[pos].cwd = inner.tabs[pos].controller.working_dir();
            inner.tabs[pos].raw_title = inner.tabs[pos].controller.last_title();
            inner.tabs[pos].foreground_process = inner.tabs[pos].controller.foreground_process();
            super::tab_bar_sync::sync_tab_titles(inner);
            sync_selected_claw_sidebar(inner, pos);
        }
        TerminalEventKind::FocusClawSidebar => {
            super::window_state::show_claw_sidebar(inner);
        }
        TerminalEventKind::OpenModelSelection(character_id) => {
            if is_tab_selected(inner, pos) {
                super::claw::open_model_selection(inner, character_id);
            }
        }
        TerminalEventKind::ClawEvent(p_id, claw_event) => {
            if is_tab_selected(inner, pos) {
                let session_usage = inner.tabs[pos].controller.get_session_usage();
                inner.claw.set_session_usage(session_usage);
            }

            match claw_event {
                ClawEngineEvent::SessionStateChanged { status, .. } => {
                    inner.tabs[pos]
                        .controller
                        .set_session_status_for_pane(&p_id, status);
                    if is_tab_selected(inner, pos) {
                        let tc = &inner.tabs[pos];
                        inner.claw.set_pane_state(
                            &tc.controller.agent_name(),
                            tc.controller.is_sleep(),
                            tc.controller.is_pinned(),
                            tc.controller.is_web_search(),
                            tc.controller.is_vibe(),
                        );
                    }
                }
                ClawEngineEvent::Identity {
                    agent_name,
                    pinned,
                    web_search_enabled,
                    vibe_enabled,
                    ..
                } if is_tab_selected(inner, pos) => {
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
                ClawEngineEvent::PinStatusChanged(pinned) if is_tab_selected(inner, pos) => {
                    let tc = &inner.tabs[pos];
                    inner.claw.set_pane_state(
                        &tc.controller.agent_name(),
                        tc.controller.is_sleep(),
                        pinned,
                        tc.controller.is_web_search(),
                        tc.controller.is_vibe(),
                    );
                }
                ClawEngineEvent::WebSearchStatusChanged(enabled) if is_tab_selected(inner, pos) => {
                    let tc = &inner.tabs[pos];
                    inner.claw.set_pane_state(
                        &tc.controller.agent_name(),
                        tc.controller.is_sleep(),
                        tc.controller.is_pinned(),
                        enabled,
                        tc.controller.is_vibe(),
                    );
                }
                ClawEngineEvent::VibeStatusChanged(vibe) if is_tab_selected(inner, pos) => {
                    let tc = &inner.tabs[pos];
                    inner.claw.set_pane_state(
                        &tc.controller.agent_name(),
                        tc.controller.is_sleep(),
                        tc.controller.is_pinned(),
                        tc.controller.is_web_search(),
                        vibe,
                    );
                }
                ClawEngineEvent::DismissDrawer | ClawEngineEvent::RestoreHistory(..) => {
                    inner.claw.refresh_visibility();
                }
                ClawEngineEvent::DiagnosisComplete { .. } => {
                    inner.claw.refresh_visibility();
                    trigger_background_tab_bell(inner, pos);
                }
                ClawEngineEvent::InjectCommand { auto_approve, .. }
                | ClawEngineEvent::ProposeFileWrite { auto_approve, .. }
                | ClawEngineEvent::ProposeTerminalCommand { auto_approve, .. } => {
                    inner.claw.refresh_visibility();
                    if !auto_approve {
                        trigger_background_tab_bell(inner, pos);
                    }
                }
                ClawEngineEvent::ProposeFileDelete { .. }
                | ClawEngineEvent::ProposeKillProcess { .. }
                | ClawEngineEvent::ProposeBackgroundCommand { .. }
                | ClawEngineEvent::ProposeGetClipboard { .. }
                | ClawEngineEvent::ProposeSetClipboard { .. } => {
                    inner.claw.refresh_visibility();
                    trigger_background_tab_bell(inner, pos);
                }
                ClawEngineEvent::FocusPane => {
                    inner.window.present();
                    let widget = inner.tabs[pos].controller.widget();
                    let page = inner.tab_view.page(widget);
                    inner.tab_view.set_selected_page(&page);
                    inner.tabs[pos].controller.focus_pane_by_id(&p_id);
                }
                ClawEngineEvent::RequestSpawnAgent {
                    location,
                    intent,
                    request_id,
                    ..
                } => {
                    let source_ctrl = inner.tabs[pos].controller.clone();
                    let source_pane_id = p_id;
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
                                let _ = tx.send(AppInput::ReattachDetachedBatch(remaining)).await;
                            }
                        });
                    } else {
                        super::detached_sessions::reattach_session(inner, pid, cwd);
                    }
                }
                ClawEngineEvent::RequestCloseAgent {
                    target_agent_name,
                    target_pane_id,
                } => {
                    let resolved = target_pane_id
                        .unwrap_or_else(|| resolve_pane_id_from_caches(&target_agent_name));
                    if !resolved.is_empty() {
                        for tab in &inner.tabs {
                            if tab.controller.close_pane_by_id(&resolved) {
                                break;
                            }
                        }
                    }
                }
                ClawEngineEvent::InjectKeystrokes {
                    target_agent_name,
                    target_pane_id,
                    keys,
                } => {
                    let resolved = target_pane_id
                        .unwrap_or_else(|| resolve_pane_id_from_caches(&target_agent_name));
                    if !resolved.is_empty() {
                        for tab in &inner.tabs {
                            if tab.controller.inject_keystrokes_by_id(&resolved, &keys) {
                                break;
                            }
                        }
                    }
                }
                ClawEngineEvent::TaskStatusChanged { .. } => {
                    super::tab_bar_sync::sync_tab_running_icons(inner);
                    if is_tab_selected(inner, pos) {
                        inner.claw.update_tasks(inner.tabs[pos].get_pending_tasks());
                    }
                }
                ClawEngineEvent::TaskCompleted {
                    agent_name,
                    character_id,
                    message,
                    ..
                } => {
                    trigger_background_tab_bell(inner, pos);
                    if boxxy_preferences::Settings::load().enable_timer_sounds {
                        crate::sound::play_timer_completion_sound();
                    }

                    let attr = resolve_character_attribution(&character_id, &agent_name);
                    let display_message = message
                        .unwrap_or_else(|| format!("{} has finished a timer.", attr.first_name));

                    super::notifications::push_global_notification(
                        inner,
                        Notification {
                            id: uuid::Uuid::new_v4().to_string(),
                            level: NotificationLevel::Info,
                            title: format!("{} reminds", attr.first_name),
                            message: display_message,
                            icon_name: attr.icon_name,
                            actions: vec![NotificationAction {
                                label: "Dismiss".to_string(),
                                action_name: "win.dismiss-notification".to_string(),
                                is_primary: false,
                            }],
                            details: Vec::new(),
                            suppress_toast: false,
                            character_name: Some(attr.first_name),
                            character_color: attr.color,
                            avatar_path: attr.avatar_path,
                        },
                    );
                }
                ClawEngineEvent::LongTaskCompleted {
                    agent_name,
                    character_id,
                    message,
                    ..
                } => {
                    trigger_background_tab_bell(inner, pos);
                    if boxxy_preferences::Settings::load().enable_task_sounds {
                        crate::sound::play_task_completion_sound();
                    }

                    let attr = resolve_character_attribution(&character_id, &agent_name);
                    super::notifications::push_global_notification(
                        inner,
                        Notification {
                            id: uuid::Uuid::new_v4().to_string(),
                            level: NotificationLevel::Info,
                            title: format!("{} completed a task", attr.first_name),
                            message,
                            icon_name: attr.icon_name,
                            actions: vec![NotificationAction {
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
                    );
                }
                ClawEngineEvent::PushGlobalNotification { title, message } => {
                    super::notifications::push_global_notification(
                        inner,
                        Notification {
                            id: uuid::Uuid::new_v4().to_string(),
                            level: NotificationLevel::Info,
                            title,
                            message,
                            icon_name: "boxxyclaw-symbolic".to_string(),
                            actions: vec![NotificationAction {
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
                    );
                }
                _ => {}
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
        TerminalEventKind::RestoreDetached(_, _)
        | TerminalEventKind::SpawnCharacter(_, _)
        | TerminalEventKind::DetachedSessionsChanged(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_claw_protocol::characters::{CharacterConfig, HolderKind};

    #[test]
    fn resolve_pane_id_matches_petname_display_name_and_holder_id() {
        let catalog = vec![CharacterInfo::new(
            CharacterConfig {
                id: "char-uuid-1".to_string(),
                name: "levi-kujo".to_string(),
                display_name: "Levi".to_string(),
                color: "#204A87".to_string(),
                duties: "Systems".to_string(),
                personality: "Direct".to_string(),
            },
            false,
        )];
        let claims = vec![CharacterClaim {
            holder_id: "pane-uuid-99".to_string(),
            holder_kind: HolderKind::Pane,
            character_id: "char-uuid-1".to_string(),
            session_id: "sess-1".to_string(),
            petname: "red-pony".to_string(),
            owner_bus_name: ":1.10".to_string(),
        }];

        assert_eq!(
            resolve_pane_id_in("red-pony", &claims, &catalog),
            "pane-uuid-99"
        );
        assert_eq!(
            resolve_pane_id_in("Red Pony", &claims, &catalog),
            "pane-uuid-99"
        );
        assert_eq!(
            resolve_pane_id_in("@Levi", &claims, &catalog),
            "pane-uuid-99"
        );
        assert_eq!(
            resolve_pane_id_in(":levi-kujo", &claims, &catalog),
            "pane-uuid-99"
        );
        assert_eq!(
            resolve_pane_id_in("pane-uuid-99", &claims, &catalog),
            "pane-uuid-99"
        );
        assert_eq!(
            resolve_pane_id_in("unclaimed-pane-id", &claims, &catalog),
            "unclaimed-pane-id"
        );
    }

    #[test]
    fn background_command_completion_bell_requires_unselected_non_tui_and_5s_threshold() {
        use std::time::Duration;

        assert!(should_ring_for_completed_command(
            Duration::from_secs(5),
            false,
            false
        ));
        assert!(should_ring_for_completed_command(
            Duration::from_secs(20),
            false,
            false
        ));
        // Active tab never rings tab bell
        assert!(!should_ring_for_completed_command(
            Duration::from_secs(20),
            false,
            true
        ));
        // Alternate-screen TUIs (`vim`, `btop`, `less`) never ring command-completion bell
        assert!(!should_ring_for_completed_command(
            Duration::from_secs(20),
            true,
            false
        ));
        // Quick commands (`< 5s`) do not spam bell
        assert!(!should_ring_for_completed_command(
            Duration::from_secs(4),
            false,
            false
        ));
    }
}
