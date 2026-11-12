use super::{Direction, ExtractedPane, PaneData, TerminalComponent, TerminalInner};
use crate::TERMINAL_EVENT_BUS;
use crate::events::{PaneInit, PaneOutput, TerminalEvent, TerminalEventKind};
use crate::pane::TerminalPaneComponent;
use gtk4 as gtk;
use gtk4::prelude::*;

fn widget_center_in_container(widget: &gtk::Box, container: &gtk::Box) -> (f64, f64) {
    let origin = widget
        .compute_point(container, &gtk::graphene::Point::zero())
        .map_or((0.0, 0.0), |pt| (f64::from(pt.x()), f64::from(pt.y())));
    let cx = origin.0 + f64::from(widget.width()) / 2.0;
    let cy = origin.1 + f64::from(widget.height()) / 2.0;
    (cx, cy)
}

fn find_neighbor_pane_id(inner: &TerminalInner, dir: Direction) -> Option<String> {
    if inner.panes.len() <= 1 || inner.is_maximized {
        return None;
    }

    let active_wrapper = &inner.panes.get(&inner.active_pane_id)?.wrapper;
    let (acx, acy) = widget_center_in_container(active_wrapper, &inner.split_container);

    let mut best_id = None;
    let mut min_score = f64::MAX;

    for (id, data) in &inner.panes {
        if *id == inner.active_pane_id {
            continue;
        }

        let (ccx, ccy) = widget_center_in_container(&data.wrapper, &inner.split_container);
        let valid = match dir {
            Direction::Left => ccx < acx - 1.0,
            Direction::Right => ccx > acx + 1.0,
            Direction::Up => ccy < acy - 1.0,
            Direction::Down => ccy > acy + 1.0,
        };

        if valid {
            let dx = (ccx - acx).abs();
            let dy = (ccy - acy).abs();
            let score = match dir {
                Direction::Left | Direction::Right => dx + dy * 2.0,
                Direction::Up | Direction::Down => dy + dx * 2.0,
            };

            if score < min_score {
                min_score = score;
                best_id = Some(id.clone());
            }
        }
    }

    best_id
}

impl TerminalComponent {
    pub fn focus_pane_by_id(&self, id: &str) -> bool {
        let mut inner = self.inner.borrow_mut();
        if !inner.panes.contains_key(id) {
            return false;
        }

        if inner.active_pane_id != id {
            inner.active_pane_id = id.to_string();
            let term_id = inner.id.clone();
            let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                id: term_id,
                kind: TerminalEventKind::PaneFocused(id.to_string()),
            });
        }

        let Some(pane_data) = inner.panes.get(id) else {
            return false;
        };
        let controller = pane_data.controller.clone();
        let is_maximized = inner.is_maximized;
        let maximized_container = inner.maximized_container.clone();

        if is_maximized {
            let leaf_widget = controller.widget().clone();
            while let Some(child) = maximized_container.first_child() {
                maximized_container.remove(&child);
            }
            maximized_container.append(&leaf_widget);
        }

        controller.grab_focus();
        drop(inner);
        self.update_dimming();
        true
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
            if let Some(pane) = inner.panes.get(&was_active) {
                pane.controller.grab_focus();
            }
        }

        drop(inner);
        self.update_dimming();
        true
    }

    pub fn split_vertical(&self, intent: Option<String>) -> String {
        self.split_internal(true, intent, None, None)
    }

    pub fn split_horizontal(&self, intent: Option<String>) -> String {
        self.split_internal(false, intent, None, None)
    }

    pub fn split_reattach(
        &self,
        is_vertical: bool,
        reattach_pid: u32,
        working_dir: Option<String>,
    ) -> String {
        self.split_internal(is_vertical, None, Some(reattach_pid), working_dir)
    }

    fn split_internal(
        &self,
        is_vertical: bool,
        intent: Option<String>,
        reattach_pid: Option<u32>,
        working_dir_override: Option<String>,
    ) -> String {
        if self.inner.borrow().is_maximized {
            self.toggle_maximize();
        }

        let mut inner = self.inner.borrow_mut();
        let new_id = uuid::Uuid::new_v4().to_string();
        let split_idx = inner.panes.len();
        let split_dir_code = if is_vertical { "v" } else { "h" };
        let layout_tag = format!("{}::{split_idx}::{split_dir_code}", inner.id);

        let tx = inner.output_tx.clone();
        let new_controller = TerminalPaneComponent::new(
            PaneInit {
                id: new_id.clone(),
                working_dir: working_dir_override.or_else(|| inner.working_dir.clone()),
                spawn_intent: intent,
                reattach_pid,
                layout_tag: Some(layout_tag),
                command: None,
                hold: false,
            },
            move |msg: PaneOutput| {
                let _ = tx.send(msg);
            },
        );

        if let Some(ref settings) = inner.current_settings {
            new_controller.update_settings(settings.clone(), inner.current_palette);
        }
        new_controller.spawn();

        let new_wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
        new_wrapper.set_hexpand(true);
        new_wrapper.set_vexpand(true);
        new_wrapper.append(new_controller.widget());

        let Some(active_data) = inner.panes.get(&inner.active_pane_id) else {
            return new_id;
        };
        let active_wrapper = active_data.wrapper.clone();
        let expected_w = active_wrapper.width();
        let expected_h = active_wrapper.height();
        let direction = if is_vertical {
            Direction::Right
        } else {
            Direction::Down
        };

        Self::attach_wrapper_split(
            &active_wrapper,
            &new_wrapper,
            direction,
            expected_w,
            expected_h,
        );

        let prev_active_id = inner.active_pane_id.clone();
        let keep_source_focus = inner
            .panes
            .get(&prev_active_id)
            .is_some_and(|p| p.controller.is_claw_drawer_visible());

        inner.panes.insert(
            new_id.clone(),
            PaneData {
                controller: new_controller,
                wrapper: new_wrapper,
                title: None,
            },
        );

        if keep_source_focus {
            if let Some(prev) = inner.panes.get(&prev_active_id) {
                prev.controller.grab_focus();
            }
        } else {
            inner.active_pane_id = new_id.clone();
            if let Some(next) = inner.panes.get(&new_id) {
                next.controller.grab_focus();
            }
        }
        drop(inner);
        self.update_dimming();
        new_id
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
            active_data.controller.set_force_terminate_on_close(true);
            active_data.controller.terminal().detach_pty();
            let ctrl_for_close = active_data.controller.clone();
            gtk::glib::spawn_future_local(async move {
                ctrl_for_close.terminate_on_close_sync().await;
            });

            Self::detach_wrapper_from_tree(&active_data.wrapper);

            let next_id_opt = inner.panes.keys().next().cloned();
            let term_id = inner.id.clone();
            if let Some(id) = next_id_opt {
                inner.active_pane_id = id.clone();
                inner.working_dir = inner
                    .panes
                    .get(&id)
                    .and_then(|p| p.controller.working_dir());
                if let Some(next_pane) = inner.panes.get(&id) {
                    next_pane.controller.grab_focus();
                }
                let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                    id: term_id,
                    kind: TerminalEventKind::PaneFocused(id),
                });
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
        let Some(id) = find_neighbor_pane_id(&inner, dir) else {
            return;
        };

        inner.active_pane_id = id.clone();
        inner.working_dir = inner
            .panes
            .get(&id)
            .and_then(|p| p.controller.working_dir());
        let term_id = inner.id.clone();
        if let Some(data) = inner.panes.get(&id) {
            data.controller.grab_focus();
        }
        drop(inner);
        self.update_dimming();
        let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
            id: term_id,
            kind: TerminalEventKind::PaneFocused(id),
        });
    }

    pub fn swap(&self, dir: Direction) {
        let mut inner = self.inner.borrow_mut();
        let Some(target_id) = find_neighbor_pane_id(&inner, dir) else {
            return;
        };

        let active_id = inner.active_pane_id.clone();
        let (Some(active_data_ref), Some(target_data_ref)) =
            (inner.panes.get(&active_id), inner.panes.get(&target_id))
        else {
            return;
        };

        let active_widget = active_data_ref.controller.widget().clone();
        let target_widget = target_data_ref.controller.widget().clone();
        let active_wrapper = active_data_ref.wrapper.clone();
        let target_wrapper = target_data_ref.wrapper.clone();

        // Lock size requests temporarily to prevent VTE resize signals when unparented.
        active_widget.set_size_request(active_widget.width(), active_widget.height());
        target_widget.set_size_request(target_widget.width(), target_widget.height());

        active_wrapper.remove(&active_widget);
        target_wrapper.remove(&target_widget);

        active_wrapper.append(&target_widget);
        target_wrapper.append(&active_widget);

        gtk::glib::idle_add_local_once(move || {
            active_widget.set_size_request(-1, -1);
            target_widget.set_size_request(-1, -1);
        });

        let Some(mut pane1_data) = inner.panes.remove(&active_id) else {
            return;
        };
        let Some(mut pane2_data) = inner.panes.remove(&target_id) else {
            inner.panes.insert(active_id, pane1_data);
            return;
        };

        std::mem::swap(&mut pane1_data.wrapper, &mut pane2_data.wrapper);

        inner.panes.insert(active_id.clone(), pane1_data);
        inner.panes.insert(target_id, pane2_data);

        if let Some(data) = inner.panes.get(&active_id) {
            data.controller.grab_focus();
        }
    }

    fn detach_wrapper_from_tree(wrapper: &gtk::Box) {
        let Some(parent) = wrapper.parent() else {
            return;
        };
        if let Some(parent_paned) = parent.downcast_ref::<gtk::Paned>() {
            let sibling = if parent_paned.start_child().as_ref() == Some(wrapper.upcast_ref()) {
                parent_paned.end_child()
            } else {
                parent_paned.start_child()
            };

            let grandparent = parent_paned.parent();
            parent_paned.set_start_child(None::<&gtk::Widget>);
            parent_paned.set_end_child(None::<&gtk::Widget>);

            if let (Some(sibling), Some(grandparent)) = (sibling, grandparent) {
                if let Some(gp_paned) = grandparent.downcast_ref::<gtk::Paned>() {
                    if gp_paned.start_child().as_ref() == Some(parent_paned.upcast_ref()) {
                        gp_paned.set_start_child(Some(&sibling));
                    } else {
                        gp_paned.set_end_child(Some(&sibling));
                    }
                } else if let Some(gp_box) = grandparent.downcast_ref::<gtk::Box>() {
                    gp_box.remove(parent_paned);
                    gp_box.append(&sibling);
                }
            }
        } else if let Some(parent_box) = parent.downcast_ref::<gtk::Box>() {
            parent_box.remove(wrapper);
        }
    }

    fn attach_wrapper_split(
        target_wrapper: &gtk::Box,
        new_wrapper: &gtk::Box,
        direction: Direction,
        expected_w: i32,
        expected_h: i32,
    ) {
        let is_horizontal_paned = matches!(direction, Direction::Left | Direction::Right);
        let orientation = if is_horizontal_paned {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        };

        let paned = gtk::Paned::new(orientation);
        paned.set_hexpand(true);
        paned.set_vexpand(true);

        if let Some(parent) = target_wrapper.parent() {
            if let Some(p_paned) = parent.downcast_ref::<gtk::Paned>() {
                if p_paned.start_child().as_ref() == Some(target_wrapper.upcast_ref()) {
                    p_paned.set_start_child(Some(&paned));
                } else {
                    p_paned.set_end_child(Some(&paned));
                }
            } else if let Some(p_box) = parent.downcast_ref::<gtk::Box>() {
                p_box.remove(target_wrapper);
                p_box.append(&paned);
            }
        }

        match direction {
            Direction::Left | Direction::Up => {
                paned.set_start_child(Some(new_wrapper));
                paned.set_end_child(Some(target_wrapper));
            }
            Direction::Right | Direction::Down => {
                paned.set_start_child(Some(target_wrapper));
                paned.set_end_child(Some(new_wrapper));
            }
        }

        let initial_pos = if is_horizontal_paned {
            expected_w / 2
        } else {
            expected_h / 2
        };
        if initial_pos > 0 {
            paned.set_position(initial_pos);
        }

        let paned_weak = paned.downgrade();
        gtk::glib::idle_add_local_once(move || {
            if let Some(paned) = paned_weak.upgrade() {
                let size = if is_horizontal_paned {
                    paned.width()
                } else {
                    paned.height()
                };
                if size > 0 {
                    paned.set_position(size / 2);
                }
            }
        });
    }

    pub fn move_pane_to_split(
        &self,
        source_pane_id: &str,
        target_pane_id: &str,
        direction: Direction,
    ) -> bool {
        if source_pane_id == target_pane_id {
            return false;
        }
        if self.inner.borrow().is_maximized {
            self.toggle_maximize();
        }

        let mut inner = self.inner.borrow_mut();
        let (Some(source_data), Some(target_data)) = (
            inner.panes.get(source_pane_id),
            inner.panes.get(target_pane_id),
        ) else {
            return false;
        };
        let source_wrapper = source_data.wrapper.clone();
        let target_wrapper = target_data.wrapper.clone();
        let source_controller = source_data.controller.clone();
        let target_controller = target_data.controller.clone();

        let (expected_w, expected_h) = if let Some(parent) = source_wrapper.parent()
            && Some(&parent) == target_wrapper.parent().as_ref()
        {
            (parent.width(), parent.height())
        } else {
            (target_wrapper.width(), target_wrapper.height())
        };

        let source_widget = source_controller.widget().clone();
        let target_widget = target_controller.widget().clone();
        source_widget.set_size_request(source_widget.width().max(1), source_widget.height().max(1));
        target_widget.set_size_request(target_widget.width().max(1), target_widget.height().max(1));

        Self::detach_wrapper_from_tree(&source_wrapper);
        Self::attach_wrapper_split(
            &target_wrapper,
            &source_wrapper,
            direction,
            expected_w,
            expected_h,
        );

        gtk::glib::idle_add_local_once(move || {
            source_widget.set_size_request(-1, -1);
            target_widget.set_size_request(-1, -1);
        });

        inner.active_pane_id = source_pane_id.to_string();
        inner.working_dir = source_controller.working_dir();
        source_controller.grab_focus();
        drop(inner);
        self.update_dimming();
        true
    }

    pub fn take_pane(&self, pane_id: &str) -> Option<ExtractedPane> {
        if self.inner.borrow().is_maximized {
            self.toggle_maximize();
        }

        let mut inner = self.inner.borrow_mut();
        let pane_data = inner.panes.remove(pane_id)?;

        let leaf_widget = pane_data.controller.widget().clone();
        leaf_widget.set_size_request(leaf_widget.width().max(1), leaf_widget.height().max(1));
        Self::detach_wrapper_from_tree(&pane_data.wrapper);
        gtk::glib::idle_add_local_once(move || {
            leaf_widget.set_size_request(-1, -1);
        });

        let term_id = inner.id.clone();
        let mut next_focus_events = None;
        if let Some((next_id, next_data)) = inner.panes.iter().next() {
            let next_id = next_id.clone();
            let next_ctrl = next_data.controller.clone();
            let next_title = next_data
                .title
                .clone()
                .or_else(|| next_ctrl.last_title())
                .unwrap_or_else(|| "Terminal".to_string());
            let next_cwd = next_ctrl.working_dir();
            inner.active_pane_id = next_id.clone();
            inner.working_dir = next_cwd.clone();
            next_focus_events = Some((next_id, next_ctrl, next_title, next_cwd));
        }
        drop(inner);
        self.update_dimming();

        if let Some((next_id, next_ctrl, next_title, next_cwd)) = next_focus_events {
            next_ctrl.grab_focus();
            let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                id: term_id.clone(),
                kind: TerminalEventKind::PaneFocused(next_id),
            });
            let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                id: term_id.clone(),
                kind: TerminalEventKind::TitleChanged(next_title),
            });
            if let Some(dir) = next_cwd {
                let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                    id: term_id,
                    kind: TerminalEventKind::DirectoryChanged(dir),
                });
            }
        }

        let title = pane_data
            .title
            .or_else(|| pane_data.controller.last_title());
        Some(ExtractedPane {
            id: pane_id.to_string(),
            controller: pane_data.controller,
            wrapper: pane_data.wrapper,
            title,
        })
    }

    pub fn insert_pane_split(
        &self,
        extracted: ExtractedPane,
        target_pane_id: &str,
        direction: Direction,
    ) -> bool {
        if self.inner.borrow().is_maximized {
            self.toggle_maximize();
        }

        let mut inner = self.inner.borrow_mut();
        let resolved_target_id = if inner.panes.contains_key(target_pane_id) {
            target_pane_id.to_string()
        } else {
            inner.active_pane_id.clone()
        };
        let Some(target_data) = inner.panes.get(&resolved_target_id) else {
            return false;
        };
        let target_wrapper = target_data.wrapper.clone();
        let expected_w = target_wrapper.width();
        let expected_h = target_wrapper.height();

        let tx = inner.output_tx.clone();
        extracted.controller.set_callback(move |msg: PaneOutput| {
            let _ = tx.send(msg);
        });

        let split_idx = inner.panes.len();
        let split_dir_code = if matches!(direction, Direction::Left | Direction::Right) {
            "v"
        } else {
            "h"
        };
        let layout_tag = format!("{}::{split_idx}::{split_dir_code}", inner.id);
        extracted.controller.set_layout_tag(layout_tag);

        if let Some(ref settings) = inner.current_settings {
            extracted
                .controller
                .update_settings(settings.clone(), inner.current_palette);
        }

        Self::attach_wrapper_split(
            &target_wrapper,
            &extracted.wrapper,
            direction,
            expected_w,
            expected_h,
        );

        let new_id = extracted.id;
        let new_ctrl = extracted.controller.clone();
        let new_title = extracted
            .title
            .clone()
            .or_else(|| new_ctrl.last_title())
            .unwrap_or_else(|| "Terminal".to_string());
        let new_cwd = new_ctrl.working_dir();
        let term_id = inner.id.clone();

        inner.panes.insert(
            new_id.clone(),
            PaneData {
                controller: extracted.controller,
                wrapper: extracted.wrapper,
                title: extracted.title,
            },
        );
        inner.active_pane_id = new_id.clone();
        inner.working_dir = new_cwd.clone();
        drop(inner);

        self.update_dimming();
        new_ctrl.grab_focus();

        let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
            id: term_id.clone(),
            kind: TerminalEventKind::PaneFocused(new_id),
        });
        let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
            id: term_id.clone(),
            kind: TerminalEventKind::TitleChanged(new_title),
        });
        if let Some(dir) = new_cwd {
            let _ = TERMINAL_EVENT_BUS.send(TerminalEvent {
                id: term_id,
                kind: TerminalEventKind::DirectoryChanged(dir),
            });
        }

        true
    }
}
