use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::state::{AppInput, AppWindowInner};
use boxxy_terminal::{TerminalEvent, TerminalEventKind};

pub fn handle_terminal_event(
    _inner_ref: &Rc<RefCell<AppWindowInner>>,
    inner: &mut AppWindowInner,
    event: TerminalEvent,
) {
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
                let _ = inner.tx.send_blocking(AppInput::PushNotification(
                    crate::widgets::notification::Notification::new_info(message),
                ));
            }
            TerminalEventKind::ClawStateChanged(active, proactive) => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);

                // Update tab icon
                if active {
                    page.set_icon(Some(&gtk4::gio::ThemedIcon::new("boxxyclaw")));
                } else {
                    page.set_icon(None::<&gtk4::gio::Icon>);
                }

                if Some(&page) == inner.tab_view.selected_page().as_ref() {
                    inner.claw_active = active;
                    inner.claw_proactive = proactive;
                    inner
                        .claw
                        .update_ui(inner.claw_active, inner.claw_proactive);
                }
            }
            TerminalEventKind::PaneFocused(_) => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);

                // Update tab icon
                let is_claw_active = inner.tabs[pos].controller.is_claw_active();
                if is_claw_active {
                    page.set_icon(Some(&gtk4::gio::ThemedIcon::new("boxxyclaw")));
                } else {
                    page.set_icon(None::<&gtk4::gio::Icon>);
                }

                if Some(&page) == inner.tab_view.selected_page().as_ref() {
                    inner.claw_active = is_claw_active;
                    let is_proactive = inner.tabs[pos].controller.is_proactive();
                    inner.claw_proactive = is_proactive;

                    inner
                        .claw
                        .update_ui(inner.claw_active, inner.claw_proactive);

                    inner
                        .claw
                        .set_history_widget(&inner.tabs[pos].controller.claw_history_widget());
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
            TerminalEventKind::ClawEvent(_p_id, claw_event) => {
                // Update token usage if this is the active tab/pane
                let total_tokens = inner.tabs[pos].controller.get_total_tokens();
                if let Some(page) = inner.tab_view.selected_page() {
                    let child = page.child();
                    if inner.tabs[pos].controller.widget() == &child {
                        inner.claw.set_token_usage(total_tokens);
                    }
                }

                match claw_event {
                    boxxy_claw::engine::ClawEngineEvent::DiagnosisComplete { .. }
                    | boxxy_claw::engine::ClawEngineEvent::InjectCommand { .. }
                    | boxxy_claw::engine::ClawEngineEvent::ProposeFileWrite { .. }
                    | boxxy_claw::engine::ClawEngineEvent::ProposeTerminalCommand { .. } => {
                        inner.claw.refresh_visibility();
                        inner.claw.scroll_to_bottom();
                    }
                    boxxy_claw::engine::ClawEngineEvent::RequestSpawnAgent {
                        location,
                        intent,
                        ..
                    } => match location {
                        boxxy_claw::engine::SpawnLocation::NewTab => {
                            super::tabs::new_tab_with_intent(inner, intent);
                        }
                        boxxy_claw::engine::SpawnLocation::VerticalSplit => {
                            inner.tabs[pos].controller.split_vertical(intent);
                        }
                        boxxy_claw::engine::SpawnLocation::HorizontalSplit => {
                            inner.tabs[pos].controller.split_horizontal(intent);
                        }
                    },
                    boxxy_claw::engine::ClawEngineEvent::RequestCloseAgent {
                        target_agent_name,
                    } => {
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
                    boxxy_claw::engine::ClawEngineEvent::InjectKeystrokes {
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
                    _ => {} // Other events like AgentThinking or FileWrite are handled strictly by the Pane UI
                }
            }
        }
    }
}
