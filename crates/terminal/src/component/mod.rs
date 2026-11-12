mod split_tree;

use boxxy_claw_protocol::{AgentStatus, SessionUsage};
use boxxy_preferences::Settings;
use boxxy_themes::Palette;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::TERMINAL_EVENT_BUS;
use crate::events::*;
use crate::pane::TerminalPaneComponent;

#[derive(Debug)]
pub struct PaneData {
    pub controller: TerminalPaneComponent,
    pub wrapper: gtk::Box,
    pub title: Option<String>,
}

#[derive(Debug)]
pub struct ExtractedPane {
    pub id: String,
    pub controller: TerminalPaneComponent,
    pub wrapper: gtk::Box,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone)]
pub struct TerminalComponent {
    pub(super) widget: gtk::Overlay,
    pub(super) inner: Rc<RefCell<TerminalInner>>,
}

pub struct TerminalInner {
    pub id: String, // Tab ID
    pub panes: HashMap<String, PaneData>,
    pub active_pane_id: String,
    pub is_maximized: bool,
    pub stack: gtk::Stack,
    pub background_picture: gtk::Picture,
    pub maximized_container: gtk::Box,
    pub split_container: gtk::Box,
    pub current_settings: Option<Settings>,
    pub current_palette: Option<Palette>,
    pub working_dir: Option<String>,
    pub(super) output_tx: tokio::sync::mpsc::UnboundedSender<PaneOutput>,
    bg_load_gen: u64,
}

impl std::fmt::Debug for TerminalComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalComponent").finish()
    }
}

impl TerminalComponent {
    pub fn new(init: TerminalInit) -> Self {
        let overlay = gtk::Overlay::new();
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        let background_picture = gtk::Picture::new();
        background_picture.add_css_class("terminal-background-image");
        background_picture.set_content_fit(gtk::ContentFit::Cover);
        background_picture.set_can_shrink(true);
        background_picture.set_hexpand(true);
        background_picture.set_vexpand(true);
        background_picture.set_visible(false);
        overlay.set_child(Some(&background_picture));

        let stack = gtk::Stack::new();
        stack.add_css_class("terminal-stack");
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        overlay.add_overlay(&stack);

        let maximized_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        maximized_container.set_widget_name("maximized-container");
        maximized_container.set_hexpand(true);
        maximized_container.set_vexpand(true);

        let split_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        split_container.add_css_class("terminal-split-container");
        split_container.set_hexpand(true);
        split_container.set_vexpand(true);

        let mut panes = HashMap::new();
        let initial_pane_id = uuid::Uuid::new_v4().to_string();

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        split_container.append(&wrapper);

        let (output_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PaneOutput>();
        let initial_tx = output_tx.clone();

        let pane_controller = TerminalPaneComponent::new(
            PaneInit {
                id: initial_pane_id.clone(),
                working_dir: init.working_dir.clone(),
                spawn_intent: init.spawn_intent,
                reattach_pid: init.reattach_pid,
                layout_tag: Some(format!("{}::0::root", init.id)),
                command: init.command,
                hold: init.hold,
            },
            move |output: PaneOutput| {
                let _ = initial_tx.send(output);
            },
        );

        wrapper.append(pane_controller.widget());

        panes.insert(
            initial_pane_id.clone(),
            PaneData {
                controller: pane_controller,
                wrapper,
                title: None,
            },
        );

        stack.add_named(&split_container, Some("split"));
        stack.add_named(&maximized_container, Some("max"));
        stack.set_visible_child_name("split");

        let inner = Rc::new(RefCell::new(TerminalInner {
            id: init.id,
            panes,
            active_pane_id: initial_pane_id,
            is_maximized: false,
            stack,
            background_picture,
            maximized_container,
            split_container,
            current_settings: None,
            current_palette: None,
            working_dir: init.working_dir,
            output_tx,
            bg_load_gen: 0,
        }));

        // Hold only weak references in the tab event pump so closing a tab
        // drops `TerminalInner`, its panes, and all `output_tx` senders immediately.
        let inner_weak = Rc::downgrade(&inner);
        let widget_weak = overlay.downgrade();
        gtk::glib::spawn_future_local(async move {
            while let Some(msg) = rx.recv().await {
                let (Some(inner_rc), Some(widget)) = (inner_weak.upgrade(), widget_weak.upgrade())
                else {
                    break;
                };
                let comp = Self {
                    widget,
                    inner: inner_rc,
                };
                comp.handle_pane_output(msg);
            }
        });

        let comp = Self {
            widget: overlay,
            inner,
        };
        comp.spawn();
        comp
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.widget
    }

    pub fn id(&self) -> String {
        self.inner.borrow().id.clone()
    }

    pub fn get_pids(&self) -> Vec<u32> {
        self.inner
            .borrow()
            .panes
            .values()
            .filter_map(|p| p.controller.get_pid())
            .collect()
    }

    pub fn get_pane_ids(&self) -> Vec<String> {
        self.inner.borrow().panes.keys().cloned().collect()
    }

    pub fn has_pane(&self, pane_id: &str) -> bool {
        self.inner.borrow().panes.contains_key(pane_id)
    }

    pub fn pane_count(&self) -> usize {
        self.inner.borrow().panes.len()
    }

    pub fn send_text(&self, text: &str) {
        let inner = self.inner.borrow();
        if let Some(pane) = inner.panes.get(&inner.active_pane_id) {
            pane.controller.send_text(text);
        }
    }

    pub fn has_selection(&self) -> bool {
        self.inner
            .borrow()
            .panes
            .values()
            .any(|p| p.controller.has_selection())
    }

    pub fn show_bookmark_proposal(
        &self,
        name: &str,
        filename: &str,
        script: &str,
        placeholders: Vec<String>,
    ) {
        let inner = self.inner.borrow();
        if let Some(pane) = inner.panes.get(&inner.active_pane_id) {
            let proposal = crate::TerminalProposal::Bookmark(
                filename.to_string(),
                script.to_string(),
                placeholders,
            );
            pane.controller.show_bookmark_proposal(
                &format!("Bookmark: {name}"),
                "Execute the following script?",
                proposal,
            );
        }
    }

    pub async fn get_text_snapshot(&self, max_lines: usize, offset_lines: usize) -> Option<String> {
        let pane = {
            let inner = self.inner.borrow();
            inner
                .panes
                .get(&inner.active_pane_id)
                .map(|p| p.controller.clone())
        }?;
        pane.get_text_snapshot(max_lines, offset_lines).await
    }

    fn handle_pane_output(&self, msg: PaneOutput) {
        match msg {
            PaneOutput::Focused(id) => {
                let mut inner = self.inner.borrow_mut();
                if inner.active_pane_id != id {
                    inner.active_pane_id = id.clone();
                    inner.working_dir = inner
                        .panes
                        .get(&id)
                        .and_then(|p| p.controller.working_dir());
                    let term_id = inner.id.clone();
                    drop(inner);
                    self.update_dimming();

                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::PaneFocused(id),
                    });
                }
            }
            PaneOutput::Exited(id, code) => {
                let mut inner = self.inner.borrow_mut();
                if !inner.panes.contains_key(&id) {
                    return;
                }
                if inner.panes.len() == 1 {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::Exited(code),
                    });
                } else {
                    let was_active = inner.active_pane_id.clone();
                    inner.active_pane_id = id.clone();
                    drop(inner);
                    self.close_split();

                    let mut inner = self.inner.borrow_mut();
                    if was_active != id && inner.panes.contains_key(&was_active) {
                        inner.active_pane_id = was_active.clone();
                        if let Some(pane) = inner.panes.get(&was_active) {
                            pane.controller.grab_focus();
                        }
                    }
                }
            }
            PaneOutput::TitleChanged(id, title) => {
                let mut inner = self.inner.borrow_mut();
                if let Some(pane) = inner.panes.get_mut(&id) {
                    pane.title = Some(title.clone());
                }

                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::TitleChanged(title),
                    });
                }
            }
            PaneOutput::BellRung(_id) => {
                let term_id = self.inner.borrow().id.clone();
                let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                    id: term_id,
                    kind: TerminalEventKind::BellRung,
                });
            }
            PaneOutput::DirectoryChanged(id, dir) => {
                let mut inner = self.inner.borrow_mut();
                if inner.active_pane_id == id {
                    inner.working_dir = Some(dir.clone());
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::DirectoryChanged(dir),
                    });
                }
            }
            PaneOutput::Osc133A(id) => {
                let mut inner = self.inner.borrow_mut();
                if let Some(pane) = inner.panes.get_mut(&id) {
                    pane.title = pane.controller.last_title();
                }
                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::Osc133A,
                    });
                }
            }
            PaneOutput::Osc133B(id) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: inner.id.clone(),
                        kind: TerminalEventKind::Osc133B,
                    });
                }
            }
            PaneOutput::Osc133C(id) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: inner.id.clone(),
                        kind: TerminalEventKind::Osc133C,
                    });
                }
            }
            PaneOutput::Osc133D(id, exit_code) => {
                let mut inner = self.inner.borrow_mut();
                if let Some(pane) = inner.panes.get_mut(&id) {
                    pane.title = pane.controller.last_title();
                }
                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::Osc133D(id, exit_code),
                    });
                }
            }
            PaneOutput::ClawEvent(id, event) => {
                let term_id = self.inner.borrow().id.clone();
                let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                    id: term_id,
                    kind: TerminalEventKind::ClawEvent(id, event),
                });
            }
            PaneOutput::FocusClawSidebar(id) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: inner.id.clone(),
                        kind: TerminalEventKind::FocusClawSidebar,
                    });
                }
            }
            PaneOutput::RequestFocus(id) => {
                self.focus_pane_by_id(&id);
            }
            PaneOutput::ForegroundProcessChanged(id, process_name) => {
                let mut inner = self.inner.borrow_mut();
                if process_name.is_empty()
                    && let Some(pane) = inner.panes.get_mut(&id)
                {
                    pane.title = pane.controller.last_title();
                }
                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::ForegroundProcessChanged(process_name),
                    });
                }
            }
            PaneOutput::AltScreenChanged(id, is_alt) => {
                let mut inner = self.inner.borrow_mut();
                if !is_alt
                    && let Some(pane) = inner.panes.get_mut(&id)
                {
                    pane.title = pane.controller.last_title();
                }
                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::AltScreenChanged(is_alt),
                    });
                }
            }
            PaneOutput::Notification(id, message) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: inner.id.clone(),
                        kind: TerminalEventKind::Notification(message),
                    });
                }
            }
            PaneOutput::ClawStateChanged(id, active, sleep) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: inner.id.clone(),
                        kind: TerminalEventKind::ClawStateChanged(active, sleep),
                    });
                }
            }
            PaneOutput::MovePaneToSplit {
                source_pane_id,
                target_pane_id,
                direction,
            } => {
                let term_id = self.inner.borrow().id.clone();
                let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                    id: term_id,
                    kind: TerminalEventKind::MovePaneToSplit {
                        source_pane_id,
                        target_pane_id,
                        direction,
                    },
                });
            }
            PaneOutput::ZoomIn => {
                let term_id = self.inner.borrow().id.clone();
                let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                    id: term_id,
                    kind: TerminalEventKind::ZoomIn,
                });
            }
            PaneOutput::ZoomOut => {
                let term_id = self.inner.borrow().id.clone();
                let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                    id: term_id,
                    kind: TerminalEventKind::ZoomOut,
                });
            }
            PaneOutput::ResetZoom => {
                let term_id = self.inner.borrow().id.clone();
                let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                    id: term_id,
                    kind: TerminalEventKind::ResetZoom,
                });
            }
        }
    }

    pub(super) fn update_dimming(&self) {
        let inner = self.inner.borrow();
        let dimming_enabled = inner
            .current_settings
            .as_ref()
            .is_some_and(|s| s.dim_inactive_panes);
        let has_multiple = inner.panes.len() > 1 && !inner.is_maximized;

        for (id, data) in &inner.panes {
            let should_dim = dimming_enabled && has_multiple && *id != inner.active_pane_id;
            data.controller.set_dimmed(should_dim);
        }
    }

    pub fn spawn(&self) {
        let inner = self.inner.borrow();
        for pane_data in inner.panes.values() {
            pane_data.controller.spawn();
        }
    }

    pub fn copy(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.copy();
        }
    }

    pub fn paste(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.paste();
        }
    }

    pub fn select_all(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.select_all();
        }
    }

    pub fn toggle_search(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.toggle_search();
        }
    }

    pub fn copy_last_command_output(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.copy_last_command_output();
        }
    }

    pub fn open_scrollback_in_editor(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.open_scrollback_in_editor();
        }
    }

    pub fn clear_scrollback(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.clear_scrollback();
        }
    }

    pub fn reset_terminal(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.reset_terminal();
        }
    }

    pub fn grab_focus(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.grab_focus();
        }
    }

    pub fn inject_text(&self, text: &str) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.inject_text(text);
        }
    }

    pub fn show_claw_popover(&self, title: &str, diagnosis: &str, proposal: TerminalProposal) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data
                .controller
                .show_claw_popover(title, diagnosis, proposal);
        }
    }

    pub fn show_claw_popover_for_pane(
        &self,
        pane_id: &str,
        title: &str,
        diagnosis: &str,
        proposal: TerminalProposal,
    ) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data
                .controller
                .show_claw_popover(title, diagnosis, proposal);
        }
    }

    pub fn hide_claw_popover(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.hide_claw_popover();
        }
    }

    pub fn hide_claw_popover_for_pane(&self, pane_id: &str) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.hide_claw_popover();
        }
    }

    pub fn show_lazy_error(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.show_lazy_error();
        }
    }

    pub fn show_lazy_error_for_pane(&self, pane_id: &str) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.show_lazy_error();
        }
    }

    pub fn show_diagnosis_ready(&self, diagnosis: String, proposal: TerminalProposal) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data
                .controller
                .show_diagnosis_ready(diagnosis, proposal);
        }
    }

    pub fn show_diagnosis_ready_for_pane(
        &self,
        pane_id: &str,
        diagnosis: String,
        proposal: TerminalProposal,
    ) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data
                .controller
                .show_diagnosis_ready(diagnosis, proposal);
        }
    }

    pub fn reload_claw(&self) {
        let inner = self.inner.borrow();
        for pane_data in inner.panes.values() {
            pane_data.controller.reload_claw();
        }
    }

    pub fn soft_clear_claw_history(&self, pane_id: &str) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.soft_clear_claw_history();
            true
        } else {
            false
        }
    }

    pub fn clear_claw_history(&self, pane_id: &str) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.clear_claw_history();
            true
        } else {
            false
        }
    }

    pub fn cancel_task_by_id(&self, pane_id: &str, task_id: uuid::Uuid) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.cancel_task(task_id);
            true
        } else {
            false
        }
    }

    pub fn toggle_pause_task_by_id(&self, pane_id: &str, task_id: uuid::Uuid) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.toggle_pause_task(task_id);
            true
        } else {
            false
        }
    }

    pub fn cancel_all_tasks_by_id(&self, pane_id: &str) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.cancel_all_tasks();
            true
        } else {
            false
        }
    }

    pub fn active_pane_id(&self) -> String {
        self.inner.borrow().active_pane_id.clone()
    }

    pub fn set_claw_active_for_pane(&self, pane_id: &str, active: bool) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.set_claw_active(active);
            true
        } else {
            false
        }
    }

    pub fn set_session_status_for_pane(&self, pane_id: &str, status: AgentStatus) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.set_session_status(status);
            true
        } else {
            false
        }
    }

    pub fn set_claw_active(&self, active: bool) {
        let inner = self.inner.borrow();
        for pane_data in inner.panes.values() {
            pane_data.controller.set_claw_active(active);
        }
    }

    pub fn is_claw_active(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .is_some_and(|p| p.controller.is_claw_active())
    }

    pub fn is_sleep(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .is_some_and(|p| p.controller.is_sleep())
    }

    pub fn is_pinned(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .is_some_and(|p| p.controller.is_pinned())
    }

    pub fn is_web_search(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .is_some_and(|p| p.controller.is_web_search())
    }

    pub fn is_vibe(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .is_some_and(|p| p.controller.is_vibe())
    }

    pub fn agent_name(&self) -> String {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .map_or_else(String::new, |p| p.controller.agent_name())
    }

    pub fn get_session_usage(&self) -> SessionUsage {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .map_or_else(SessionUsage::default, |p| p.controller.get_session_usage())
    }

    pub fn get_pending_tasks(&self) -> Vec<boxxy_claw_protocol::ScheduledTask> {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .map_or_else(Vec::new, |p| p.controller.get_pending_tasks())
    }

    pub fn has_pending_tasks(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .values()
            .any(|p| p.controller.has_pending_tasks())
    }

    pub fn resume_session(&self, session_id: &str, override_character_id: Option<String>) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data
                .controller
                .resume_session(session_id, override_character_id);
        }
    }

    pub fn start_claw_query_with_default_character(&self, query: String) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .is_some_and(|p| p.controller.start_claw_query_with_default_character(query))
    }

    /// Returns `(pane_id, formatted_agent_name)` for every pane in this tab that
    /// currently has an active Claw agent session.
    pub fn get_active_claw_panes(&self) -> Vec<(String, String)> {
        let inner = self.inner.borrow();
        let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
        let catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let mut out = Vec::new();
        for (pane_id, data) in &inner.panes {
            if data.controller.get_pid().is_none() {
                continue;
            }
            let claim_opt = claims.iter().find(|c| &c.holder_id == pane_id);
            if claim_opt.is_some_and(|c| {
                boxxy_claw_protocol::characters::is_incognito_character(&c.character_id)
            }) || boxxy_claw_protocol::characters::is_incognito_character(
                &data.controller.agent_name(),
            ) {
                continue;
            }
            if data.controller.is_claw_active() || claim_opt.is_some() {
                let mut name = data.controller.agent_name();
                if name.trim().is_empty()
                    && let Some(claim) = claim_opt
                    && let Some(info) = catalog.iter().find(|c| c.config.id == claim.character_id)
                {
                    name = info.config.display_name.clone();
                }
                let formatted = name
                    .split_whitespace()
                    .map(|word| {
                        let mut chars = word.chars();
                        chars.next().map_or_else(String::new, |first| {
                            first.to_uppercase().collect::<String>() + chars.as_str()
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let display = if formatted.is_empty() {
                    "AI Agent".to_string()
                } else {
                    formatted
                };
                out.push((pane_id.clone(), display));
            }
        }
        out
    }

    pub fn get_total_tokens(&self) -> u64 {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .map_or(0, |p| p.controller.get_total_tokens())
    }

    pub fn working_dir(&self) -> Option<String> {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .and_then(|p| p.controller.working_dir())
    }

    pub fn last_title(&self) -> Option<String> {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .and_then(|p| p.controller.last_title())
    }

    pub fn foreground_process(&self) -> Option<String> {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .and_then(|p| p.controller.foreground_process())
    }

    pub fn is_alt_screen(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .is_some_and(|p| p.controller.is_alt_screen())
    }

    pub fn open_in_files(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.open_in_files();
        }
    }

    pub fn notify_settings_invalidated(&self) {
        let inner = self.inner.borrow();
        for pane in inner.panes.values() {
            pane.controller.notify_settings_invalidated();
        }
    }

    /// Called when the enclosing tab is being permanently closed.
    pub fn on_close(&self) {
        let inner = self.inner.borrow();
        for pane in inner.panes.values() {
            pane.controller.end_session();
        }
    }

    /// Marks all panes in this tab to terminate their PTY process group on close
    /// even if `pty_persistence` is enabled (used when explicitly closing an idle tab).
    pub fn terminate_on_close(&self) {
        let inner = self.inner.borrow();
        for pane in inner.panes.values() {
            pane.controller.set_force_terminate_on_close(true);
            pane.controller.end_session();
        }
    }

    pub async fn on_close_sync(&self) {
        let panes: Vec<_> = {
            let inner = self.inner.borrow();
            inner.panes.values().map(|p| p.controller.clone()).collect()
        };
        for controller in panes {
            controller.end_session_sync().await;
        }
    }

    pub async fn detach_on_close_sync(&self) {
        let panes: Vec<_> = {
            let inner = self.inner.borrow();
            inner.panes.values().map(|p| p.controller.clone()).collect()
        };
        for controller in panes {
            controller.detach_on_close_sync().await;
        }
    }

    pub async fn terminate_on_close_sync(&self) {
        let panes: Vec<_> = {
            let inner = self.inner.borrow();
            inner.panes.values().map(|p| p.controller.clone()).collect()
        };
        for controller in panes {
            controller.terminate_on_close_sync().await;
        }
    }

    pub fn update_settings(&self, settings: Settings, palette: Option<Palette>) {
        let mut inner = self.inner.borrow_mut();

        let needs_bg_update = match &inner.current_settings {
            Some(curr) => curr.background_image_path != settings.background_image_path,
            None => settings.background_image_path.is_some(),
        };

        if needs_bg_update {
            inner.bg_load_gen = inner.bg_load_gen.wrapping_add(1);
            let expected_gen = inner.bg_load_gen;
            if let Some(path) = settings.background_image_path.clone() {
                log::info!("Updating background image to: {path}");
                let inner_weak = Rc::downgrade(&self.inner);
                gtk::glib::spawn_future_local(async move {
                    let path_for_load = path.clone();
                    let tex_opt = boxxy_sys_utils::runtime()
                        .spawn_blocking(move || boxxy_themes::get_texture_from_path(&path_for_load))
                        .await
                        .ok()
                        .flatten();

                    let Some(inner_rc) = inner_weak.upgrade() else {
                        return;
                    };
                    let inner_b = inner_rc.borrow();
                    if inner_b.bg_load_gen != expected_gen {
                        return;
                    }
                    if let Some(texture) = tex_opt {
                        inner_b.background_picture.set_paintable(Some(&texture));
                        inner_b.background_picture.set_visible(true);
                    } else {
                        log::warn!("Failed to load texture from path: {path}");
                        inner_b
                            .background_picture
                            .set_paintable(None::<&gdk::Texture>);
                        inner_b.background_picture.set_visible(false);
                    }
                });
            } else {
                log::info!("Clearing background image");
                inner
                    .background_picture
                    .set_paintable(None::<&gdk::Texture>);
                inner.background_picture.set_visible(false);
            }
        }

        inner.current_settings = Some(settings.clone());
        inner.current_palette = palette;

        for pane in inner.panes.values() {
            pane.controller.update_settings(settings.clone(), palette);
        }
    }

    pub fn inject_keystrokes_by_id(&self, id: &str, keys: &str) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(id) {
            pane_data.controller.inject_keystrokes(keys);
            true
        } else {
            false
        }
    }

    pub fn send_claw_to_pane(&self, pane_id: &str, msg: boxxy_claw_protocol::ClawMessage) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.send_claw_message(msg);
            true
        } else {
            false
        }
    }

    pub fn is_active_claw_drawer_visible(&self) -> bool {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .is_some_and(|p| p.controller.is_claw_drawer_visible())
    }

    pub fn zoom_in_claw_overlay() {
        boxxy_claw_widget::zoom_in_overlay();
    }

    pub fn zoom_out_claw_overlay() {
        boxxy_claw_widget::zoom_out_overlay();
    }

    pub fn reset_claw_overlay_zoom() {
        boxxy_claw_widget::reset_overlay_zoom();
    }

    pub fn set_claw_overlay_zoom_pct(pct: u32) {
        boxxy_claw_widget::set_overlay_zoom_pct(pct);
    }
}
