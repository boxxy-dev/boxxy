use boxxy_preferences::Settings;
use boxxy_themes::Palette;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use std::collections::HashMap;

use crate::TERMINAL_EVENT_BUS;
use crate::events::*;
use crate::pane::TerminalPaneComponent;

#[derive(Debug)]
pub struct PaneData {
    pub controller: TerminalPaneComponent,
    pub wrapper: gtk::Box,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone)]
pub struct TerminalComponent {
    widget: gtk::Overlay,
    inner: std::rc::Rc<std::cell::RefCell<TerminalInner>>,
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

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PaneOutput>();

        let pane_controller = TerminalPaneComponent::new(
            PaneInit {
                id: initial_pane_id.clone(),
                working_dir: init.working_dir.clone(),
                spawn_intent: init.spawn_intent.clone(),
            },
            move |output: PaneOutput| {
                let _ = tx.send(output);
            },
        );

        wrapper.append(pane_controller.widget());

        panes.insert(
            initial_pane_id.clone(),
            PaneData {
                controller: pane_controller.clone(),
                wrapper,
                title: None,
            },
        );

        stack.add_named(&split_container, Some("split"));
        stack.add_named(&maximized_container, Some("max"));
        stack.set_visible_child_name("split");

        let inner = std::rc::Rc::new(std::cell::RefCell::new(TerminalInner {
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
        }));

        let comp = Self {
            widget: overlay,
            inner,
        };

        let c = comp.clone();
        gtk::glib::spawn_future_local(async move {
            while let Some(msg) = rx.recv().await {
                c.handle_pane_output(msg);
            }
        });

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

    pub fn send_text(&self, text: &str) {
        let active_id = self.inner.borrow().active_pane_id.clone();
        if let Some(pane) = self.inner.borrow().panes.get(&active_id) {
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
        let active_id = self.inner.borrow().active_pane_id.clone();
        if let Some(pane) = self.inner.borrow().panes.get(&active_id) {
            let proposal = crate::TerminalProposal::Bookmark(
                filename.to_string(),
                script.to_string(),
                placeholders,
            );
            pane.controller.show_bookmark_proposal(
                &format!("Bookmark: {}", name),
                "Execute the following script?",
                proposal,
            );
        }
    }

    pub async fn get_text_snapshot(&self, max_lines: usize, offset_lines: usize) -> Option<String> {
        let active_id = self.inner.borrow().active_pane_id.clone();
        let pane = {
            let inner = self.inner.borrow();
            inner.panes.get(&active_id).map(|p| p.controller.clone())
        };

        if let Some(pane) = pane {
            pane.get_text_snapshot(max_lines, offset_lines).await
        } else {
            None
        }
    }

    fn handle_pane_output(&self, msg: PaneOutput) {
        match msg {
            PaneOutput::Focused(id) => {
                let mut inner = self.inner.borrow_mut();
                if inner.active_pane_id != id {
                    inner.active_pane_id = id.clone();
                    let title_opt = inner.panes.get(&id).and_then(|p| p.title.clone());
                    let term_id = inner.id.clone();
                    drop(inner);
                    self.update_dimming();

                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id.clone(),
                        kind: TerminalEventKind::PaneFocused(id.clone()),
                    });

                    if let Some(title) = title_opt {
                        let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                            id: term_id,
                            kind: TerminalEventKind::TitleChanged(title),
                        });
                    } else {
                        let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                            id: term_id,
                            kind: TerminalEventKind::TitleChanged("Terminal".to_string()),
                        });
                    }
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
                        inner
                            .panes
                            .get(&was_active)
                            .unwrap()
                            .controller
                            .grab_focus();
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
                let inner = self.inner.borrow();
                let term_id = inner.id.clone();
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
                let inner = self.inner.borrow();
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
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::Osc133B,
                    });
                }
            }
            PaneOutput::Osc133C(id) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::Osc133C,
                    });
                }
            }
            PaneOutput::Osc133D(id, exit_code) => {
                let inner = self.inner.borrow();
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
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::FocusClawSidebar,
                    });
                }
            }
            PaneOutput::ForegroundProcessChanged(id, process_name) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::ForegroundProcessChanged(process_name),
                    });
                }
            }
            PaneOutput::Notification(id, message) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id,
                        kind: TerminalEventKind::Notification(message),
                    });
                }
            }
            PaneOutput::ClawStateChanged(id, active, proactive) => {
                let inner = self.inner.borrow();
                if inner.active_pane_id == id {
                    let term_id = inner.id.clone();
                    let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                        id: term_id.clone(),
                        kind: TerminalEventKind::ClawStateChanged(active, proactive),
                    });
                }
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


    fn update_dimming(&self) {
        let inner = self.inner.borrow();
        let dimming_enabled = inner
            .current_settings
            .as_ref()
            .map(|s| s.dim_inactive_panes)
            .unwrap_or(false);
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

    pub fn set_agent_thinking(&self, thinking: bool) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.set_agent_thinking(thinking);
        }
    }

    pub fn set_agent_thinking_for_pane(&self, pane_id: &str, thinking: bool) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.set_agent_thinking(thinking);
        }
    }

    pub fn reload_claw(&self) {
        let inner = self.inner.borrow();
        for pane_data in inner.panes.values() {
            pane_data.controller.reload_claw();
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

    pub fn update_diagnosis_mode_for_pane(
        &self,
        pane_id: &str,
        mode: &boxxy_preferences::config::ClawAutoDiagnosisMode,
    ) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(pane_id) {
            pane_data.controller.update_diagnosis_mode(mode);
            true
        } else {
            false
        }
    }

    pub fn set_claw_active(&self, active: bool) {
        let inner = self.inner.borrow_mut();
        for pane_data in inner.panes.values() {
            pane_data.controller.set_claw_active(active);
        }
    }

    pub fn is_claw_active(&self) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.is_claw_active()
        } else {
            false
        }
    }

    pub fn is_proactive(&self) -> bool {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.is_proactive()
        } else {
            false
        }
    }

    pub fn update_diagnosis_mode(&self, mode: &boxxy_preferences::config::ClawAutoDiagnosisMode) {
        let inner = self.inner.borrow();
        for pane_data in inner.panes.values() {
            pane_data.controller.update_diagnosis_mode(mode);
        }
    }

    pub fn claw_history_widget(&self) -> gtk::ListBox {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .unwrap()
            .controller
            .claw_history_widget()
    }

    pub fn get_total_tokens(&self) -> u64 {
        let inner = self.inner.borrow();
        inner
            .panes
            .get(&inner.active_pane_id)
            .map(|p| p.controller.get_total_tokens())
            .unwrap_or(0)
    }

    pub fn working_dir(&self) -> Option<String> {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.working_dir()
        } else {
            None
        }
    }

    pub fn open_in_files(&self) {
        let inner = self.inner.borrow();
        if let Some(pane_data) = inner.panes.get(&inner.active_pane_id) {
            pane_data.controller.open_in_files();
        }
    }

    pub fn update_settings(&self, settings: Settings, palette: Option<Palette>) {
        let mut inner = self.inner.borrow_mut();

        // 1. Background Image
        let needs_bg_update = match &inner.current_settings {
            Some(curr) => curr.background_image_path != settings.background_image_path,
            None => true,
        };

        if needs_bg_update {
            if let Some(path) = &settings.background_image_path {
                log::info!("Updating background image to: {}", path);
                if let Some(texture) = boxxy_themes::get_texture_from_path(path) {
                    log::info!("Texture loaded successfully");
                    inner.background_picture.set_paintable(Some(&texture));
                    inner.background_picture.set_visible(true);
                } else {
                    log::warn!("Failed to load texture from path: {}", path);
                    inner
                        .background_picture
                        .set_paintable(None::<&gdk::Texture>);
                    inner.background_picture.set_visible(false);
                }
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

    pub fn close_pane_by_id(&self, id: &str) -> bool {
        let mut inner = self.inner.borrow_mut();
        if !inner.panes.contains_key(id) {
            return false;
        }

        let was_active = inner.active_pane_id.clone();
        inner.active_pane_id = id.to_string();
        drop(inner);
        self.close_split();

        let mut inner = self.inner.borrow_mut();
        if was_active != id && inner.panes.contains_key(&was_active) {
            inner.active_pane_id = was_active.clone();
            inner
                .panes
                .get(&was_active)
                .unwrap()
                .controller
                .grab_focus();
        }

        drop(inner);
        self.update_dimming();
        true
    }

    pub fn split_vertical(&self, intent: Option<String>) {
        self.split(true, intent);
    }

    pub fn split_horizontal(&self, intent: Option<String>) {
        self.split(false, intent);
    }

    fn split(&self, is_vertical: bool, intent: Option<String>) {
        if self.inner.borrow().is_maximized {
            self.toggle_maximize();
        }

        let mut inner = self.inner.borrow_mut();

        let orientation = if is_vertical {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        };

        let new_id = uuid::Uuid::new_v4().to_string();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PaneOutput>();
        let c = self.clone();
        gtk::glib::spawn_future_local(async move {
            while let Some(msg) = rx.recv().await {
                c.handle_pane_output(msg);
            }
        });

        let new_controller = TerminalPaneComponent::new(
            PaneInit {
                id: new_id.clone(),
                working_dir: inner.working_dir.clone(),
                spawn_intent: intent,
            },
            move |msg: PaneOutput| {
                let _ = tx.send(msg);
            },
        );

        if let Some(ref settings) = inner.current_settings {
            new_controller.update_settings(settings.clone(), inner.current_palette);
            new_controller.set_claw_active(settings.claw_on_by_default);
            let mode = if settings.claw_auto_diagnosis_mode
                == boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
            {
                boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
            } else {
                boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
            };
            new_controller.update_diagnosis_mode(&mode);
        }
        new_controller.spawn();

        let new_wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
        new_wrapper.set_hexpand(true);
        new_wrapper.set_vexpand(true);
        new_wrapper.append(new_controller.widget());

        let active_wrapper = inner
            .panes
            .get(&inner.active_pane_id)
            .unwrap()
            .wrapper
            .clone();

        let paned = gtk::Paned::new(orientation);
        paned.set_hexpand(true);
        paned.set_vexpand(true);
        paned.set_position(if is_vertical {
            active_wrapper.width() / 2
        } else {
            active_wrapper.height() / 2
        });

        if let Some(parent) = active_wrapper.parent() {
            if let Some(p_paned) = parent.downcast_ref::<gtk::Paned>() {
                let p_paned: &gtk::Paned = p_paned;
                if p_paned.start_child().as_ref() == Some(active_wrapper.upcast_ref()) {
                    p_paned.set_start_child(Some(&paned));
                } else {
                    p_paned.set_end_child(Some(&paned));
                }
            } else if let Some(p_box) = parent.downcast_ref::<gtk::Box>() {
                let p_box: &gtk::Box = p_box;
                p_box.remove(&active_wrapper);
                p_box.append(&paned);
            }
        }

        paned.set_start_child(Some(&active_wrapper));
        paned.set_end_child(Some(&new_wrapper));

        inner.panes.insert(
            new_id.clone(),
            PaneData {
                controller: new_controller,
                wrapper: new_wrapper,
                title: None,
            },
        );

        inner.active_pane_id = new_id.clone();
        inner.panes.get(&new_id).unwrap().controller.grab_focus();
        drop(inner);
        self.update_dimming();
    }

    pub fn close_split(&self) {
        let mut inner = self.inner.borrow_mut();

        if inner.panes.len() == 1 {
            let term_id = inner.id.clone();
            let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                id: term_id,
                kind: TerminalEventKind::Exited(0),
            });
            return;
        }

        if inner.is_maximized {
            drop(inner);
            self.toggle_maximize();
            inner = self.inner.borrow_mut();
        }

        let active_id = inner.active_pane_id.clone();
        if let Some(active_data) = inner.panes.remove(&active_id) {
            let active_wrapper = &active_data.wrapper;

            if let Some(parent) = active_wrapper.parent()
                && let Some(parent_paned) = parent.downcast_ref::<gtk::Paned>()
            {
                let parent_paned: &gtk::Paned = parent_paned;
                let sibling =
                    if parent_paned.start_child().as_ref() == Some(active_wrapper.upcast_ref()) {
                        parent_paned.end_child()
                    } else {
                        parent_paned.start_child()
                    };

                if let Some(sibling) = sibling
                    && let Some(grandparent) = parent_paned.parent()
                {
                    // Detach both children from parent_paned so sibling can be freely moved
                    parent_paned.set_start_child(None::<&gtk::Widget>);
                    parent_paned.set_end_child(None::<&gtk::Widget>);

                    if let Some(gp_paned) = grandparent.downcast_ref::<gtk::Paned>() {
                        let gp_paned: &gtk::Paned = gp_paned;
                        if gp_paned.start_child().as_ref() == Some(parent_paned.upcast_ref()) {
                            gp_paned.set_start_child(Some(&sibling));
                        } else {
                            gp_paned.set_end_child(Some(&sibling));
                        }
                    } else if let Some(gp_box) = grandparent.downcast_ref::<gtk::Box>() {
                        let gp_box: &gtk::Box = gp_box;
                        gp_box.remove(parent_paned);
                        gp_box.append(&sibling);
                    }
                }
            }

            let next_id_opt = inner.panes.iter().next().map(|(id, _)| id.clone());
            if let Some(id) = next_id_opt {
                inner.active_pane_id = id.clone();
                inner.panes.get(&id).unwrap().controller.grab_focus();
            }
        }
        drop(inner);
        self.update_dimming();
    }

    pub fn toggle_maximize(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.panes.len() <= 1 {
            return;
        }

        let active_id = inner.active_pane_id.clone();
        let is_max = inner.is_maximized;

        if let Some(pane_data) = inner.panes.get(&active_id) {
            let leaf_widget = pane_data.controller.widget();

            if is_max {
                inner.maximized_container.remove(leaf_widget);
                pane_data.wrapper.append(leaf_widget);
            } else {
                pane_data.wrapper.remove(leaf_widget);
                inner.maximized_container.append(leaf_widget);
            }
        }

        inner.is_maximized = !is_max;
        if is_max {
            inner.stack.set_visible_child_name("split");
        } else {
            inner.stack.set_visible_child_name("max");
        }

        if let Some(pane_data) = inner.panes.get(&active_id) {
            pane_data.controller.grab_focus();
        }
        drop(inner);
        self.update_dimming();
    }

    pub fn focus(&self, dir: Direction) {
        let mut inner = self.inner.borrow_mut();

        if inner.panes.len() <= 1 || inner.is_maximized {
            return;
        }

        let active_wrapper = match inner.panes.get(&inner.active_pane_id) {
            Some(data) => data.wrapper.clone(),
            None => return,
        };

        let split_container = inner.split_container.clone();

        #[allow(deprecated)]
        let (ax, ay) = active_wrapper
            .translate_coordinates(&split_container, 0.0, 0.0)
            .unwrap_or((0.0, 0.0));
        let aw = active_wrapper.width() as f64;
        let ah = active_wrapper.height() as f64;
        let acx = ax + aw / 2.0;
        let acy = ay + ah / 2.0;

        let mut best_id = None;
        let mut min_score = f64::MAX;

        for (id, data) in &inner.panes {
            if *id == inner.active_pane_id {
                continue;
            }

            #[allow(deprecated)]
            let (cx, cy) = data
                .wrapper
                .translate_coordinates(&split_container, 0.0, 0.0)
                .unwrap_or((0.0, 0.0));
            let cw = data.wrapper.width() as f64;
            let ch = data.wrapper.height() as f64;
            let ccx = cx + cw / 2.0;
            let ccy = cy + ch / 2.0;

            let valid = match dir {
                Direction::Left => ccx < acx - 1.0,
                Direction::Right => ccx > acx + 1.0,
                Direction::Up => ccy < acy - 1.0,
                Direction::Down => ccy > acy + 1.0,
            };

            if valid {
                let dx = ccx - acx;
                let dy = ccy - acy;
                let score = match dir {
                    Direction::Left | Direction::Right => dx.abs() + dy.abs() * 2.0,
                    Direction::Up | Direction::Down => dy.abs() + dx.abs() * 2.0,
                };

                if score < min_score {
                    min_score = score;
                    best_id = Some(id.clone());
                }
            }
        }

        if let Some(id) = best_id {
            inner.active_pane_id = id.clone();
            if let Some(data) = inner.panes.get(&id) {
                data.controller.grab_focus();
            }
            drop(inner);
            self.update_dimming();
        }
    }

    pub fn swap(&self, dir: Direction) {
        let mut inner = self.inner.borrow_mut();

        if inner.panes.len() <= 1 || inner.is_maximized {
            return;
        }

        let active_wrapper = match inner.panes.get(&inner.active_pane_id) {
            Some(data) => data.wrapper.clone(),
            None => return,
        };

        let split_container = inner.split_container.clone();

        #[allow(deprecated)]
        let (ax, ay) = active_wrapper
            .translate_coordinates(&split_container, 0.0, 0.0)
            .unwrap_or((0.0, 0.0));
        let aw = active_wrapper.width() as f64;
        let ah = active_wrapper.height() as f64;
        let acx = ax + aw / 2.0;
        let acy = ay + ah / 2.0;

        let mut best_id = None;
        let mut min_score = f64::MAX;

        for (id, data) in &inner.panes {
            if *id == inner.active_pane_id {
                continue;
            }

            #[allow(deprecated)]
            let (cx, cy) = data
                .wrapper
                .translate_coordinates(&split_container, 0.0, 0.0)
                .unwrap_or((0.0, 0.0));
            let cw = data.wrapper.width() as f64;
            let ch = data.wrapper.height() as f64;
            let ccx = cx + cw / 2.0;
            let ccy = cy + ch / 2.0;

            let valid = match dir {
                Direction::Left => ccx < acx - 1.0,
                Direction::Right => ccx > acx + 1.0,
                Direction::Up => ccy < acy - 1.0,
                Direction::Down => ccy > acy + 1.0,
            };

            if valid {
                let dx = ccx - acx;
                let dy = ccy - acy;
                let score = match dir {
                    Direction::Left | Direction::Right => dx.abs() + dy.abs() * 2.0,
                    Direction::Up | Direction::Down => dy.abs() + dx.abs() * 2.0,
                };

                if score < min_score {
                    min_score = score;
                    best_id = Some(id.clone());
                }
            }
        }

        if let Some(target_id) = best_id {
            let active_id = inner.active_pane_id.clone();

            let active_widget = inner
                .panes
                .get(&active_id)
                .unwrap()
                .controller
                .widget()
                .clone();
            let target_widget = inner
                .panes
                .get(&target_id)
                .unwrap()
                .controller
                .widget()
                .clone();

            let active_wrapper = inner.panes.get(&active_id).unwrap().wrapper.clone();
            let target_wrapper = inner.panes.get(&target_id).unwrap().wrapper.clone();

            // Lock size requests temporarily to prevent VTE resize signals when unparented
            active_widget.set_size_request(active_widget.width(), active_widget.height());
            target_widget.set_size_request(target_widget.width(), target_widget.height());

            active_wrapper.remove(&active_widget);
            target_wrapper.remove(&target_widget);

            active_wrapper.append(&target_widget);
            target_wrapper.append(&active_widget);

            gtk::glib::idle_add_local_once({
                let active_widget = active_widget.clone();
                let target_widget = target_widget.clone();
                move || {
                    active_widget.set_size_request(-1, -1);
                    target_widget.set_size_request(-1, -1);
                }
            });

            let mut pane1_data = inner.panes.remove(&active_id).unwrap();
            let mut pane2_data = inner.panes.remove(&target_id).unwrap();

            std::mem::swap(&mut pane1_data.wrapper, &mut pane2_data.wrapper);

            inner.panes.insert(active_id.clone(), pane1_data);
            inner.panes.insert(target_id, pane2_data);

            if let Some(data) = inner.panes.get(&active_id) {
                data.controller.grab_focus();
            }
        }
    }
}
