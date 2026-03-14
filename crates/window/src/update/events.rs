use std::cell::RefCell;
use std::rc::Rc;
use gtk4::prelude::*;
use libadwaita::prelude::*;

use boxxy_terminal::{TerminalEvent, TerminalEventKind};
use crate::state::AppWindowInner;

pub fn handle_terminal_event(_inner_ref: &Rc<RefCell<AppWindowInner>>, inner: &mut AppWindowInner, event: TerminalEvent) {
    if let Some(pos) = inner.tabs.iter().position(|c| c.id == event.id) {
        match event.kind {
            TerminalEventKind::TitleChanged(title) => {
                let widget = inner.tabs[pos].controller.widget();
                let page = inner.tab_view.page(widget);
                page.set_title(&title);
                if Some(&page) != inner.tab_view.selected_page().as_ref() {
                    page.set_indicator_icon(Some(&gtk4::gio::ThemedIcon::new(
                        "visual-bell-symbolic",
                    )));
                    page.set_indicator_activatable(false);
                }
                super::tabs::sync_header_title(inner);
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
                        "visual-bell-symbolic",
                    )));
                    page.set_indicator_activatable(false);
                } else {
                    inner.bell_indicator.set_visible(true);
                }
            }
            TerminalEventKind::Osc133A
            | TerminalEventKind::Osc133B
            | TerminalEventKind::Osc133C
            | TerminalEventKind::Osc133D(_, _) => {}
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
                    boxxy_claw::engine::ClawEngineEvent::DiagnosisComplete { .. } |
                    boxxy_claw::engine::ClawEngineEvent::InjectCommand { .. } |
                    boxxy_claw::engine::ClawEngineEvent::ProposeFileWrite { .. } |
                    boxxy_claw::engine::ClawEngineEvent::ProposeTerminalCommand { .. } => {
                        inner.claw.refresh_visibility();
                        inner.claw.scroll_to_bottom();
                    }
                    _ => {} // Other events like AgentThinking or FileWrite are handled strictly by the Pane UI
                }
            }
        }
    }
}