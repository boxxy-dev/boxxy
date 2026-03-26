use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::state::AppWindowInner;
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
            TerminalEventKind::PaneFocused(_) => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);
                if Some(&page) == inner.tab_view.selected_page().as_ref() {
                    inner
                        .claw
                        .set_history_widget(&inner.tabs[pos].controller.claw_history_widget());
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
