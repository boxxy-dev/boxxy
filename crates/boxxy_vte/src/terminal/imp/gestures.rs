use crate::engine::grid::Scroll;
use crate::engine::index::{Column, Line, Point, Side};
use crate::engine::selection::{Selection, SelectionType};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

const AUTOSCROLL_INTERVAL_MS: u64 = 25;
const AUTOSCROLL_MAX_LINES_PER_TICK: i32 = 12;

impl super::TerminalWidget {
    /// Update the hover state (OSC 8 hyperlink, Ctrl+hover regex match, and pointer cursor)
    /// at widget pixel coordinates `(x, y)` with `modifiers`.
    /// Only triggers a widget redraw if the active hover underline span actually changes.
    pub(super) fn update_hover_state(
        &self,
        widget: &crate::terminal::TerminalWidget,
        x: f64,
        y: f64,
        modifiers: gtk4::gdk::ModifierType,
    ) {
        let is_ctrl = modifiers.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
        let hovered_uri = widget.check_hyperlink_at(x, y);
        let is_osclink = hovered_uri.is_some();

        // Only run regex row matching when Control is held and not already on an OSC 8 link.
        let next_regex_bounds = if is_ctrl && !is_osclink {
            let (_, _, bounds) = widget.check_match_at(x, y);
            bounds
        } else {
            None
        };

        let mut needs_redraw = false;
        if *self.hovered_hyperlink.borrow() != hovered_uri {
            self.hovered_hyperlink.replace(hovered_uri);
            needs_redraw = true;
        }
        if self.hovered_regex_match.get() != next_regex_bounds {
            self.hovered_regex_match.set(next_regex_bounds);
            needs_redraw = true;
        }
        if needs_redraw {
            widget.queue_draw();
        }

        if !self.pointer_hidden_by_typing.get() {
            let is_link = is_osclink || next_regex_bounds.is_some();
            widget.set_cursor_from_name(Some(if is_link { "pointer" } else { "text" }));
        }
    }

    #[inline]
    fn hide_pointer_on_typing(&self, widget: &crate::terminal::TerminalWidget) {
        if self.mouse_autohide.get()
            && self.mouse_pos.get().is_some()
            && !self.pointer_hidden_by_typing.replace(true)
        {
            widget.set_cursor_from_name(Some("none"));
        }
    }
    /// Convert widget pixel coordinates `(x, y)` into clamped `(col, row)` grid indices.
    #[inline]
    pub(super) fn pixel_to_cell(
        &self,
        widget: &crate::terminal::TerminalWidget,
        x: f64,
        y: f64,
        cols: usize,
        lines: usize,
    ) -> (usize, usize) {
        let (char_w, char_h) = self.get_char_size(widget);
        let padding = self.padding.get() as f64;
        let max_col = cols.saturating_sub(1);
        let max_row = lines.saturating_sub(1);
        let col = if char_w > 0.0 {
            (((x - padding) / char_w).floor() as isize).clamp(0, max_col as isize) as usize
        } else {
            0
        };
        let row = if char_h > 0.0 {
            (((y - padding) / char_h).floor() as isize).clamp(0, max_row as isize) as usize
        } else {
            0
        };
        (col, row)
    }

    /// Convert widget pixel coordinates `(x, y)` into a viewport selection endpoint
    /// `(row, Column, Side)`, snapping to the start of the top line when dragged
    /// above the viewport or to the end of the bottom line when dragged below.
    #[inline]
    pub(super) fn pixel_to_selection_point(
        &self,
        widget: &crate::terminal::TerminalWidget,
        x: f64,
        y: f64,
        cols: usize,
        lines: usize,
    ) -> (usize, Column, Side) {
        let cols = cols.max(1);
        let lines = lines.max(1);
        let (char_w, char_h) = self.get_char_size(widget);
        let padding = self.padding.get() as f64;

        if char_w <= 0.0 || char_h <= 0.0 {
            return (0, Column(0), Side::Left);
        }

        let raw_row = ((y - padding) / char_h).floor() as isize;
        if raw_row < 0 {
            return (0, Column(0), Side::Left);
        }
        if raw_row >= lines as isize {
            return (lines - 1, Column(cols - 1), Side::Right);
        }

        let row = raw_row as usize;
        let cell_x = (x - padding) / char_w;
        if cell_x < 0.0 {
            (row, Column(0), Side::Left)
        } else if cell_x >= cols as f64 {
            (row, Column(cols - 1), Side::Right)
        } else {
            let col = (cell_x.floor() as usize).min(cols - 1);
            let side = if (cell_x - cell_x.floor()) > 0.5 {
                Side::Right
            } else {
                Side::Left
            };
            (row, Column(col), side)
        }
    }

    /// Compute the vertical scroll delta (positive = up into history, negative = down)
    /// when the pointer is pushed into or beyond the top/bottom terminal edges during selection.
    pub(super) fn calculate_autoscroll_delta(
        &self,
        widget: &crate::terminal::TerminalWidget,
        y: f64,
    ) -> i32 {
        let height = widget.height() as f64;
        let padding = self.padding.get() as f64;
        let (_, char_height) = self.get_char_size(widget);
        if char_height <= 0.0 || height <= 0.0 {
            return 0;
        }

        let edge_band = (padding + char_height).max(20.0);
        let top_zone = edge_band;
        let bottom_zone = (height - edge_band).max(top_zone);
        let start_y = self.drag_start_pos.get().map(|(_, sy)| sy).unwrap_or(y);

        if y < top_zone {
            let dragged_up = start_y >= top_zone
                || (start_y - y) > char_height * 0.35
                || y <= padding.max(4.0);
            if dragged_up {
                let dist = top_zone - y;
                return ((dist / (char_height * 0.65)).ceil() as i32)
                    .clamp(1, AUTOSCROLL_MAX_LINES_PER_TICK);
            }
        } else if y > bottom_zone {
            let dragged_down = start_y <= bottom_zone
                || (y - start_y) > char_height * 0.35
                || y >= height - padding.max(4.0);
            if dragged_down {
                let dist = y - bottom_zone;
                let speed = ((dist / (char_height * 0.65)).ceil() as i32)
                    .clamp(1, AUTOSCROLL_MAX_LINES_PER_TICK);
                return -speed;
            }
        }

        0
    }

    pub(super) fn start_autoscroll(&self, widget: &crate::terminal::TerminalWidget) {
        if self.autoscroll_source_id.borrow().is_some() {
            return;
        }

        let obj_weak = widget.downgrade();
        let source_id = glib::timeout_add_local(
            std::time::Duration::from_millis(AUTOSCROLL_INTERVAL_MS),
            move || {
                let Some(obj) = obj_weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let imp = obj.imp();
                if !imp.mouse_pressed.get() {
                    imp.autoscroll_source_id.replace(None);
                    return glib::ControlFlow::Break;
                }

                let Some((mx, my)) = imp.mouse_pos.get() else {
                    imp.autoscroll_source_id.replace(None);
                    return glib::ControlFlow::Break;
                };

                let delta = imp.calculate_autoscroll_delta(&obj, my);
                if delta == 0 {
                    imp.autoscroll_source_id.replace(None);
                    return glib::ControlFlow::Break;
                }

                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    if state.is_alt_screen {
                        imp.autoscroll_source_id.replace(None);
                        return glib::ControlFlow::Break;
                    }

                    let (row, col, side) = imp.pixel_to_selection_point(
                        &obj,
                        mx,
                        my,
                        state.columns,
                        state.screen_lines,
                    );
                    imp.selection_dragged.set(true);
                    imp.last_selection_point.set(Some((row, col, side)));

                    let max_offset = state.total_lines.saturating_sub(state.screen_lines) as i32;
                    let can_scroll = (delta > 0 && state.display_offset < max_offset)
                        || (delta < 0 && state.display_offset > 0);

                    if can_scroll {
                        backend.scroll_and_update_selection(Scroll::Delta(delta), row, col, side);
                        obj.queue_draw();
                        obj.update_scroll_adjustment();
                    } else {
                        backend.update_selection_viewport(row, col, side);
                        obj.queue_draw();
                    }
                }

                glib::ControlFlow::Continue
            },
        );
        self.autoscroll_source_id.replace(Some(source_id));
    }

    pub(super) fn stop_autoscroll(&self) {
        if let Some(source_id) = self.autoscroll_source_id.borrow_mut().take() {
            source_id.remove();
        }
    }

    pub(super) fn finish_selection_drag(&self) {
        self.stop_autoscroll();
        self.drag_start_pos.set(None);
        self.last_selection_point.set(None);
        let was_dragged = self.selection_dragged.replace(false);
        if !self.mouse_pressed.replace(false) || !was_dragged {
            return;
        }
        let backend_ref = self.backend.borrow();
        let Some(backend) = backend_ref.as_ref() else {
            return;
        };
        let state = backend.render_state.load();
        if let Some(text) = state.selection_text() {
            let widget = self.obj();
            widget.display().primary_clipboard().set_text(&text);
            widget.clipboard().set_text(&text);
        } else if state.selection_range.is_some() {
            backend.copy_selection(crate::engine::term::ClipboardType::Selection);
            backend.copy_selection(crate::engine::term::ClipboardType::Clipboard);
        }
    }

    pub(super) fn setup_controllers(&self) {
        let obj = self.obj();

        // ── Input Method Setup ───────────────────────────────────────────────
        let im_context = self.im_context.borrow();
        im_context.set_client_widget(Some(&*obj));

        im_context.connect_commit(glib::clone!(
            #[weak]
            obj,
            move |_, text| {
                let imp = obj.imp();
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    if !backend.is_alt_screen() && imp.scroll_on_keystroke.get() {
                        backend.scroll_display(Scroll::Bottom);
                    }
                    imp.hide_pointer_on_typing(&obj);
                    backend.write_to_pty(text.as_bytes().to_vec());
                }
            }
        ));

        im_context.connect_preedit_changed(glib::clone!(
            #[weak]
            obj,
            move |_| {
                // For a terminal, we don't usually render the preedit string
                // inline ourselves, we let the IM handle its own popup.
                // But we must at least update the cursor location.
                let imp = obj.imp();
                if let Some(rect) = obj.get_cursor_rect() {
                    let im = imp.im_context.borrow();
                    let g_rect = gtk4::gdk::Rectangle::new(
                        rect.x() as i32,
                        rect.y() as i32,
                        rect.width() as i32,
                        rect.height() as i32,
                    );
                    im.set_cursor_location(&g_rect);
                }
            }
        ));
        drop(im_context);

        obj.connect_has_focus_notify(move |widget| {
            let imp = widget.imp();
            let im = imp.im_context.borrow();
            let has_focus = widget.has_focus();
            if has_focus {
                widget.start_cursor_blink();
                im.focus_in();
            } else {
                widget.stop_cursor_blink();
                imp.cursor_visible.set(true);
                im.focus_out();
                widget.queue_draw();
            }
            if let Some(backend) = imp.backend.borrow().as_ref() {
                let state = backend.render_state.load();
                if state
                    .mode
                    .contains(crate::engine::term::TermMode::FOCUS_IN_OUT)
                {
                    let seq: &[u8] = if has_focus { b"\x1b[I" } else { b"\x1b[O" };
                    backend
                        .notifier
                        .0
                        .send(crate::engine::event_loop::Msg::Input(
                            std::borrow::Cow::Borrowed(seq),
                        ))
                        .ok();
                }
            }
        });

        let key_ctrl = gtk4::EventControllerKey::new();
        key_ctrl.connect_key_pressed(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |ctrl, key, _keycode, modifier| {
                let imp = obj.imp();
                obj.start_cursor_blink();

                if key == gtk4::gdk::Key::V
                    && modifier.contains(
                        gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                    )
                {
                    obj.paste_clipboard();
                    return glib::Propagation::Stop;
                }
                if (key == gtk4::gdk::Key::C || key == gtk4::gdk::Key::c)
                    && modifier.contains(
                        gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
                    )
                {
                    obj.copy_clipboard();
                    return glib::Propagation::Stop;
                }
                if modifier.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                    let scroll_op = match key {
                        gtk4::gdk::Key::Page_Up | gtk4::gdk::Key::KP_Page_Up => {
                            Some(Scroll::PageUp)
                        }
                        gtk4::gdk::Key::Page_Down | gtk4::gdk::Key::KP_Page_Down => {
                            Some(Scroll::PageDown)
                        }
                        gtk4::gdk::Key::Home => Some(Scroll::Top),
                        gtk4::gdk::Key::End => Some(Scroll::Bottom),
                        _ => None,
                    };
                    if let Some(op) = scroll_op {
                        if let Some(backend) = imp.backend.borrow().as_ref() {
                            if imp.mouse_pressed.get()
                                && let Some((mx, my)) = imp.mouse_pos.get()
                            {
                                let state = backend.render_state.load();
                                let (row, col, side) = imp.pixel_to_selection_point(
                                    &obj,
                                    mx,
                                    my,
                                    state.columns,
                                    state.screen_lines,
                                );
                                imp.selection_dragged.set(true);
                                imp.last_selection_point.set(Some((row, col, side)));
                                backend.scroll_and_update_selection(op, row, col, side);
                            } else {
                                backend.scroll_display(op);
                            }
                        }
                        return glib::Propagation::Stop;
                    }
                }

                let term_mode = imp
                    .backend
                    .borrow()
                    .as_ref()
                    .map(|b| b.render_state.load().mode)
                    .unwrap_or(crate::engine::term::TermMode::NONE);

                // Let the IM context handle unmodified/Shift text composition (dead keys, IME)
                // unless Ctrl/Alt are held or the app requested REPORT_ALL_KEYS_AS_ESC.
                let bypass_im = modifier.intersects(
                    gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::ALT_MASK,
                ) || term_mode
                    .contains(crate::engine::term::TermMode::REPORT_ALL_KEYS_AS_ESC)
                    || matches!(
                        key,
                        gtk4::gdk::Key::Return
                            | gtk4::gdk::Key::KP_Enter
                            | gtk4::gdk::Key::Tab
                            | gtk4::gdk::Key::KP_Tab
                            | gtk4::gdk::Key::ISO_Left_Tab
                            | gtk4::gdk::Key::BackSpace
                            | gtk4::gdk::Key::Escape
                    );
                if !bypass_im
                    && let Some(event) = ctrl.current_event()
                    && imp.im_context.borrow().filter_keypress(&event)
                {
                    return glib::Propagation::Stop;
                }

                if let Some(backend) = imp.backend.borrow().as_ref()
                    && let Some(bytes) = crate::terminal::input::translate_key_with_mode(
                        key, modifier, term_mode,
                    )
                {
                    // Special key handled (Ctrl+C, Up, Tab, Shift+Tab, CSI u, etc.)
                    // Reset IM context so we don't have partial dead key state.
                    imp.im_context.borrow().reset();

                    if !backend.is_alt_screen() && imp.scroll_on_keystroke.get() {
                        backend.scroll_display(Scroll::Bottom);
                    }
                    imp.hide_pointer_on_typing(&obj);
                    backend.write_to_pty(bytes);
                    return glib::Propagation::Stop;
                }

                // If Control was pressed while the pointer is hovering over the terminal,
                // refresh Ctrl+hover regex link highlighting immediately.
                if matches!(
                    key,
                    gtk4::gdk::Key::Control_L | gtk4::gdk::Key::Control_R
                ) && let Some((mx, my)) = imp.mouse_pos.get()
                {
                    imp.update_hover_state(
                        &obj,
                        mx,
                        my,
                        modifier | gtk4::gdk::ModifierType::CONTROL_MASK,
                    );
                }

                glib::Propagation::Proceed
            }
        ));
        key_ctrl.connect_key_released(glib::clone!(
            #[weak]
            obj,
            move |ctrl, key, _keycode, modifier| {
                let imp = obj.imp();
                if let Some(event) = ctrl.current_event() {
                    imp.im_context.borrow().filter_keypress(&event);
                }
                if matches!(
                    key,
                    gtk4::gdk::Key::Control_L | gtk4::gdk::Key::Control_R
                ) && let Some((mx, my)) = imp.mouse_pos.get()
                {
                    imp.update_hover_state(
                        &obj,
                        mx,
                        my,
                        modifier & !gtk4::gdk::ModifierType::CONTROL_MASK,
                    );
                }
            }
        ));
        obj.add_controller(key_ctrl);

        let scroll_ctrl = gtk4::EventControllerScroll::new(
            gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::DISCRETE,
        );
        scroll_ctrl.connect_scroll(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |ctrl, _dx, dy| {
                let imp = obj.imp();
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    let modifiers = ctrl.current_event_state();
                    if state
                        .mode
                        .intersects(crate::engine::term::TermMode::MOUSE_MODE)
                        && !modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
                        && let Some((mx, my)) = imp.mouse_pos.get()
                    {
                        let (cell_x, cell_y) = imp.pixel_to_cell(
                            &obj,
                            mx,
                            my,
                            state.columns,
                            state.screen_lines,
                        );
                        let button = if dy > 0.0 { 65 } else { 64 };
                        if let Some(seq) = format_mouse_report(
                            button, true, false, cell_x, cell_y, modifiers, state.mode,
                        ) {
                            backend
                                .notifier
                                .0
                                .send(crate::engine::event_loop::Msg::Input(
                                    std::borrow::Cow::Owned(seq),
                                ))
                                .ok();
                            return glib::Propagation::Stop;
                        }
                    }
                    // Alternate Scroll Mode (DECSET 1007): on the alternate screen without
                    // mouse reporting, translate wheel ticks into Up/Down cursor keys for pagers
                    // like `less`, `man`, and `git log`.
                    if state.is_alt_screen
                        && state
                            .mode
                            .contains(crate::engine::term::TermMode::ALTERNATE_SCROLL)
                        && !modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
                        && dy != 0.0
                    {
                        let lines = ((dy.abs() * 3.0).round() as usize).clamp(1, 15);
                        let step: &[u8] = if dy > 0.0 {
                            if state
                                .mode
                                .contains(crate::engine::term::TermMode::APP_CURSOR)
                            {
                                b"\x1bOB"
                            } else {
                                b"\x1b[B"
                            }
                        } else if state
                            .mode
                            .contains(crate::engine::term::TermMode::APP_CURSOR)
                        {
                            b"\x1bOA"
                        } else {
                            b"\x1b[A"
                        };
                        let seq = step.repeat(lines);
                        backend
                            .notifier
                            .0
                            .send(crate::engine::event_loop::Msg::Input(
                                std::borrow::Cow::Owned(seq),
                            ))
                            .ok();
                        return glib::Propagation::Stop;
                    }
                    let mut adj_dy = dy;
                    if imp.invert_scroll.get() {
                        adj_dy = -adj_dy;
                    }
                    let scroll_delta = Scroll::Delta((adj_dy * 3.0) as i32);
                    if imp.mouse_pressed.get()
                        && let Some((mx, my)) = imp.mouse_pos.get()
                    {
                        let (row, col, side) = imp.pixel_to_selection_point(
                            &obj,
                            mx,
                            my,
                            state.columns,
                            state.screen_lines,
                        );
                        imp.selection_dragged.set(true);
                        imp.last_selection_point.set(Some((row, col, side)));
                        backend.scroll_and_update_selection(scroll_delta, row, col, side);
                    } else {
                        backend.scroll_display(scroll_delta);
                    }
                    obj.queue_draw();
                    obj.update_scroll_adjustment();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        obj.add_controller(scroll_ctrl);

        let click_gesture = gtk4::GestureClick::new();
        click_gesture.set_button(0);
        click_gesture.connect_pressed(glib::clone!(
            #[weak]
            obj,
            move |gesture, n_press, x, y| {
                let imp = obj.imp();
                if obj.is_focusable() {
                    obj.grab_focus();
                }
                // Claim the sequence immediately so parent widgets (e.g. OverlaySplitView)
                // don't also handle this click via their own GestureClick controllers.
                gesture.set_state(gtk4::EventSequenceState::Claimed);
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    let modifiers = gesture.current_event_state();
                    let (col, row) =
                        imp.pixel_to_cell(&obj, x, y, state.columns, state.screen_lines);
                    let (sel_row, sel_col, side) = imp.pixel_to_selection_point(
                        &obj,
                        x,
                        y,
                        state.columns,
                        state.screen_lines,
                    );
                    let point = Point::new(Line(sel_row as i32 - state.display_offset), sel_col);

                    let is_shift = modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK);
                    let app_owns_mouse = state
                        .mode
                        .intersects(crate::engine::term::TermMode::MOUSE_MODE)
                        && !is_shift;

                    if app_owns_mouse {
                        imp.stop_autoscroll();
                        if backend.has_selection() {
                            backend.set_selection(None);
                            obj.queue_draw();
                        }
                        imp.mouse_pressed.set(true);
                        imp.mouse_pos.set(Some((x, y)));
                        imp.last_mouse_cell.set(Some((col, row)));
                        let button = match gesture.current_button() {
                            1 => 0,
                            2 => 1,
                            3 => 2,
                            _ => 0,
                        };
                        if let Some(seq) = format_mouse_report(
                            button, true, false, col, row, modifiers, state.mode,
                        ) {
                            backend
                                .notifier
                                .0
                                .send(crate::engine::event_loop::Msg::Input(
                                    std::borrow::Cow::Owned(seq),
                                ))
                                .ok();
                        }
                        return;
                    }

                    // ── Terminal owns this click ─────────────────────────────
                    // Right-click: fire the context menu callback (if set).
                    if gesture.current_button() == 3 {
                        if let Some(f) = imp.context_menu_callback.borrow().as_ref() {
                            f(x, y);
                        }
                        return;
                    }

                    if gesture.current_button() == 1 {
                        imp.mouse_pos.set(Some((x, y)));
                        imp.drag_start_pos.set(Some((x, y)));
                        imp.last_selection_point.set(Some((sel_row, sel_col, side)));

                        if n_press == 1 && is_shift && backend.has_selection() {
                            imp.mouse_pressed.set(true);
                            imp.selection_dragged.set(true);
                            backend.update_selection_viewport(sel_row, sel_col, side);
                            obj.queue_draw();
                            return;
                        }

                        if n_press == 1
                            && let Some(seq) = state.calculate_navigation_sequence(point)
                        {
                            backend.set_selection(None);
                            backend
                                .notifier
                                .0
                                .send(crate::engine::event_loop::Msg::Input(
                                    std::borrow::Cow::Owned(seq.into_bytes()),
                                ))
                                .ok();
                            return;
                        }
                        imp.mouse_pressed.set(true);
                        imp.selection_dragged.set(n_press >= 2);
                        let is_alt = modifiers.contains(gtk4::gdk::ModifierType::ALT_MASK);
                        let st = match n_press {
                            1 if is_alt => SelectionType::Block,
                            1 => SelectionType::Simple,
                            2 => SelectionType::Semantic,
                            _ => SelectionType::Lines,
                        };
                        backend.set_selection(Some(Selection::new(st, point, side)));
                        obj.queue_draw();
                    } else if gesture.current_button() == 2 {
                        obj.paste_primary();
                    }
                }
            }
        ));
        click_gesture.connect_released(glib::clone!(
            #[weak]
            obj,
            move |gesture, _, x, y| {
                let imp = obj.imp();
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    let modifiers = gesture.current_event_state();
                    let (col, row) =
                        imp.pixel_to_cell(&obj, x, y, state.columns, state.screen_lines);
                    if state
                        .mode
                        .intersects(crate::engine::term::TermMode::MOUSE_MODE)
                        && !modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
                    {
                        imp.stop_autoscroll();
                        imp.drag_start_pos.set(None);
                        imp.last_selection_point.set(None);
                        imp.selection_dragged.set(false);
                        imp.mouse_pressed.set(false);
                        imp.last_mouse_cell.set(Some((col, row)));
                        let button = match gesture.current_button() {
                            1 => 0,
                            2 => 1,
                            3 => 2,
                            _ => 0,
                        };
                        let rb = if state
                            .mode
                            .contains(crate::engine::term::TermMode::SGR_MOUSE)
                        {
                            button
                        } else {
                            3
                        };
                        if let Some(seq) =
                            format_mouse_report(rb, false, false, col, row, modifiers, state.mode)
                        {
                            backend
                                .notifier
                                .0
                                .send(crate::engine::event_loop::Msg::Input(
                                    std::borrow::Cow::Owned(seq),
                                ))
                                .ok();
                        }
                        return;
                    }
                    imp.finish_selection_drag();
                } else {
                    imp.finish_selection_drag();
                }
            }
        ));
        click_gesture.connect_end(glib::clone!(
            #[weak]
            obj,
            move |_, _| {
                // Ensure drags that end or are released outside the widget
                // allocation stop auto-scrolling and finalize clipboard copy.
                obj.imp().finish_selection_drag();
            }
        ));
        obj.add_controller(click_gesture);

        let motion_ctrl = gtk4::EventControllerMotion::new();
        motion_ctrl.connect_motion(glib::clone!(
            #[weak]
            obj,
            move |ctrl, x, y| {
                let imp = obj.imp();
                let modifiers = ctrl.current_event_state();
                if imp.mouse_pressed.get() {
                    let any_button_held = modifiers.intersects(
                        gtk4::gdk::ModifierType::BUTTON1_MASK
                            | gtk4::gdk::ModifierType::BUTTON2_MASK
                            | gtk4::gdk::ModifierType::BUTTON3_MASK,
                    );
                    if !any_button_held {
                        imp.finish_selection_drag();
                    }
                }

                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    if state
                        .mode
                        .intersects(crate::engine::term::TermMode::MOUSE_MODE)
                        && !modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
                    {
                        imp.mouse_pos.set(Some((x, y)));
                        let (col, row) =
                            imp.pixel_to_cell(&obj, x, y, state.columns, state.screen_lines);
                        if imp.last_mouse_cell.replace(Some((col, row))) == Some((col, row)) {
                            return;
                        }
                        let is_drag = imp.mouse_pressed.get();
                        let button = if is_drag {
                            if modifiers.contains(gtk4::gdk::ModifierType::BUTTON2_MASK) {
                                1
                            } else if modifiers.contains(gtk4::gdk::ModifierType::BUTTON3_MASK) {
                                2
                            } else {
                                0
                            }
                        } else {
                            3
                        };
                        if let Some(seq) = format_mouse_report(
                            button, is_drag, true, col, row, modifiers, state.mode,
                        ) {
                            backend
                                .notifier
                                .0
                                .send(crate::engine::event_loop::Msg::Input(
                                    std::borrow::Cow::Owned(seq),
                                ))
                                .ok();
                        }
                        return;
                    }
                }
                if imp.mouse_pressed.get() {
                    imp.mouse_pos.set(Some((x, y)));
                    if !imp.selection_dragged.get() {
                        let moved_enough = imp
                            .drag_start_pos
                            .get()
                            .is_none_or(|(sx, sy)| (x - sx).hypot(y - sy) >= 3.0);
                        if !moved_enough {
                            return;
                        }
                        imp.selection_dragged.set(true);
                    }
                    if let Some(backend) = imp.backend.borrow().as_ref() {
                        let state = backend.render_state.load();
                        let sel_pt = imp.pixel_to_selection_point(
                            &obj,
                            x,
                            y,
                            state.columns,
                            state.screen_lines,
                        );
                        if imp.last_selection_point.get() != Some(sel_pt) {
                            imp.last_selection_point.set(Some(sel_pt));
                            backend.update_selection_viewport(sel_pt.0, sel_pt.1, sel_pt.2);
                            obj.queue_draw();
                        }
                        if !state.is_alt_screen && imp.calculate_autoscroll_delta(&obj, y) != 0 {
                            imp.start_autoscroll(&obj);
                        } else {
                            imp.stop_autoscroll();
                        }
                    }
                } else {
                    imp.mouse_pos.set(Some((x, y)));
                    imp.pointer_hidden_by_typing.set(false);
                    imp.update_hover_state(&obj, x, y, modifiers);
                }
            }
        ));
        motion_ctrl.connect_leave(glib::clone!(
            #[weak]
            obj,
            move |_| {
                obj.set_cursor(None);
                let imp = obj.imp();
                imp.pointer_hidden_by_typing.set(false);
                if !imp.mouse_pressed.get() {
                    imp.mouse_pos.set(None);
                }
                imp.last_mouse_cell.set(None);
                let had_hyperlink = imp.hovered_hyperlink.replace(None).is_some();
                let had_regex = imp.hovered_regex_match.replace(None).is_some();
                if had_hyperlink || had_regex {
                    obj.queue_draw();
                }
            }
        ));
        obj.add_controller(motion_ctrl);
    }
}

fn format_mouse_report(
    button: u8,
    is_press: bool,
    is_motion: bool,
    x: usize,
    y: usize,
    modifiers: gtk4::gdk::ModifierType,
    mode: crate::engine::term::TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(crate::engine::term::TermMode::MOUSE_MODE) {
        return None;
    }
    let mut cb = button;
    if modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
        cb += 4;
    }
    if modifiers.contains(gtk4::gdk::ModifierType::ALT_MASK) {
        cb += 8;
    }
    if modifiers.contains(gtk4::gdk::ModifierType::CONTROL_MASK) {
        cb += 16;
    }
    if is_motion {
        let report_motion = mode.contains(crate::engine::term::TermMode::MOUSE_MOTION)
            || (mode.contains(crate::engine::term::TermMode::MOUSE_DRAG) && is_press);
        if !report_motion {
            return None;
        }
        cb += 32;
    }
    let (x, y) = (x + 1, y + 1);
    if mode.contains(crate::engine::term::TermMode::SGR_MOUSE) {
        // In SGR mouse protocol (1006), lowercase 'm' is exclusively for button
        // release; button press, scroll, and all motion events use uppercase 'M'.
        let suffix = if is_press || is_motion { b'M' } else { b'm' };
        Some(format!("\x1b[<{};{};{}{}", cb, x, y, suffix as char).into_bytes())
    } else {
        if x > 223 || y > 223 {
            return None;
        }
        Some(vec![
            b'\x1b',
            b'[',
            b'M',
            32 + cb,
            (32 + x as u8).max(32),
            (32 + y as u8).max(32),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::format_mouse_report;
    use crate::engine::term::TermMode;
    use gtk4::gdk::ModifierType;

    #[test]
    fn sgr_hover_motion_uses_button_35_and_uppercase_m() {
        let mode = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        let seq = format_mouse_report(3, false, true, 9, 4, ModifierType::empty(), mode);
        assert_eq!(seq.as_deref(), Some(&b"\x1b[<35;10;5M"[..]));
    }

    #[test]
    fn sgr_drag_motion_uses_button_32_and_uppercase_m() {
        let mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let seq = format_mouse_report(0, true, true, 9, 4, ModifierType::empty(), mode);
        assert_eq!(seq.as_deref(), Some(&b"\x1b[<32;10;5M"[..]));
    }

    #[test]
    fn mouse_drag_mode_ignores_unpressed_hover_motion() {
        let mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let seq = format_mouse_report(3, false, true, 9, 4, ModifierType::empty(), mode);
        assert!(seq.is_none());
    }

    #[test]
    fn sgr_click_press_and_release() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let press = format_mouse_report(0, true, false, 2, 3, ModifierType::empty(), mode);
        let release = format_mouse_report(0, false, false, 2, 3, ModifierType::empty(), mode);
        assert_eq!(press.as_deref(), Some(&b"\x1b[<0;3;4M"[..]));
        assert_eq!(release.as_deref(), Some(&b"\x1b[<0;3;4m"[..]));
    }
}
