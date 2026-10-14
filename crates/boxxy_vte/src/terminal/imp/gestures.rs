use crate::engine::grid::Scroll;
use crate::engine::index::{Column, Line, Point, Side};
use crate::engine::selection::{Selection, SelectionType};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

impl super::TerminalWidget {
    pub(super) fn setup_controllers(&self) {
        let obj = self.obj();

        // ── Input Method Setup ───────────────────────────────────────────────
        let im_context = self.im_context.borrow();
        im_context.set_client_widget(Some(&*obj));

        im_context.connect_commit(glib::clone!(
            #[weak]
            obj,
            move |_, text| {
                if let Some(backend) = obj.imp().backend.borrow().as_ref() {
                    if !backend.is_alt_screen() {
                        backend.scroll_display(Scroll::Bottom);
                    }
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
            if widget.has_focus() {
                widget.start_cursor_blink();
                im.focus_in();
            } else {
                widget.stop_cursor_blink();
                imp.cursor_visible.set(true);
                im.focus_out();
                widget.queue_draw();
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

                // Let the IM context have first dibs on the key event (dead keys etc.)
                if imp
                    .im_context
                    .borrow()
                    .filter_keypress(&ctrl.current_event().unwrap())
                {
                    return glib::Propagation::Stop;
                }

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
                            backend.scroll_display(op);
                        }
                        return glib::Propagation::Stop;
                    }
                }
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    let is_app_cursor = state
                        .mode
                        .contains(crate::engine::term::TermMode::APP_CURSOR);
                    if let Some(bytes) =
                        crate::terminal::input::translate_key(key, modifier, is_app_cursor)
                    {
                        // Special key handled (Ctrl+C, Up, Tab, etc.)
                        // Reset IM context so we don't have partial dead key state.
                        imp.im_context.borrow().reset();

                        if !backend.is_alt_screen() {
                            backend.scroll_display(Scroll::Bottom);
                        }
                        backend.write_to_pty(bytes);
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            }
        ));
        key_ctrl.connect_key_released(glib::clone!(
            #[weak]
            obj,
            move |ctrl, _key, _keycode, _modifier| {
                let imp = obj.imp();
                imp.im_context
                    .borrow()
                    .filter_keypress(&ctrl.current_event().unwrap());
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
                        let char_size = imp.get_char_size(&obj);
                        let padding = imp.padding.get() as f64;
                        let cell_x = ((mx - padding) / char_size.0).floor() as usize;
                        let cell_y = ((my - padding) / char_size.1).floor() as usize;
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
                            if state
                                .mode
                                .contains(crate::engine::term::TermMode::SGR_MOUSE)
                                && let Some(rs) = format_mouse_report(
                                    button, false, false, cell_x, cell_y, modifiers, state.mode,
                                )
                            {
                                backend
                                    .notifier
                                    .0
                                    .send(crate::engine::event_loop::Msg::Input(
                                        std::borrow::Cow::Owned(rs),
                                    ))
                                    .ok();
                            }
                            return glib::Propagation::Stop;
                        }
                    }
                    let mut adj_dy = dy;
                    if imp.invert_scroll.get() {
                        adj_dy = -adj_dy;
                    }
                    backend.scroll_display(Scroll::Delta((adj_dy * 3.0) as i32));
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
                    let char_size = imp.get_char_size(&obj);
                    let padding = imp.padding.get() as f64;
                    let cell_x = (x - padding) / char_size.0;
                    let col = cell_x.floor() as usize;
                    let side = if (cell_x - cell_x.floor()) > 0.5 {
                        Side::Right
                    } else {
                        Side::Left
                    };
                    let row = ((y - padding) / char_size.1).floor() as usize;
                    let point = Point::new(Line(row as i32 - state.display_offset), Column(col));

                    let is_shift = modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK);
                    let app_owns_mouse = state
                        .mode
                        .intersects(crate::engine::term::TermMode::MOUSE_MODE)
                        && !is_shift;

                    if app_owns_mouse {
                        if backend.has_selection() {
                            backend.set_selection(None);
                            obj.queue_draw();
                        }
                        imp.mouse_pressed.set(true);
                        imp.mouse_pos.set(Some((x, y)));
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
                        let st = match n_press {
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
                imp.mouse_pressed.set(false);
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    let modifiers = gesture.current_event_state();
                    let char_size = imp.get_char_size(&obj);
                    let padding = imp.padding.get() as f64;
                    let col = ((x - padding) / char_size.0).floor() as usize;
                    let row = ((y - padding) / char_size.1).floor() as usize;
                    if state
                        .mode
                        .intersects(crate::engine::term::TermMode::MOUSE_MODE)
                        && !modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
                    {
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
                    if backend.has_selection() {
                        backend.copy_selection(crate::engine::term::ClipboardType::Selection);
                        backend.copy_selection(crate::engine::term::ClipboardType::Clipboard);
                    } else if state
                        .mode
                        .contains(crate::engine::term::TermMode::CLICK_REPORT)
                        && !modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
                        && gesture.current_button() == 1
                    {
                        let press = format!("\x1b[<0;{};{}M", col + 1, row + 1);
                        let release = format!("\x1b[<0;{};{}m", col + 1, row + 1);
                        let report = press + &release;
                        backend
                            .notifier
                            .0
                            .send(crate::engine::event_loop::Msg::Input(
                                std::borrow::Cow::Owned(report.into_bytes()),
                            ))
                            .ok();
                        backend.set_selection(None);
                    }
                }
            }
        ));
        obj.add_controller(click_gesture);

        let motion_ctrl = gtk4::EventControllerMotion::new();
        motion_ctrl.connect_motion(glib::clone!(
            #[weak]
            obj,
            move |ctrl, x, y| {
                let imp = obj.imp();
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    let modifiers = ctrl.current_event_state();
                    if state
                        .mode
                        .intersects(crate::engine::term::TermMode::MOUSE_MODE)
                        && !modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
                    {
                        imp.mouse_pos.set(Some((x, y)));
                        let char_size = imp.get_char_size(&obj);
                        let padding = imp.padding.get() as f64;
                        let col = ((x - padding) / char_size.0).floor() as usize;
                        let row = ((y - padding) / char_size.1).floor() as usize;
                        let is_drag = imp.mouse_pressed.get();
                        let button = if is_drag { 0 } else { 35 };
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
                    if let Some(backend) = imp.backend.borrow().as_ref() {
                        let state = backend.render_state.load();
                        let char_size = imp.get_char_size(&obj);
                        let padding = imp.padding.get() as f64;
                        let cell_x = (x - padding) / char_size.0;
                        let col = cell_x.floor() as usize;
                        let side = if (cell_x - cell_x.floor()) > 0.5 {
                            Side::Right
                        } else {
                            Side::Left
                        };
                        let row = ((y - padding) / char_size.1).floor() as usize;
                        let point =
                            Point::new(Line(row as i32 - state.display_offset), Column(col));
                        backend.update_selection(point, side);
                        obj.queue_draw();
                    }
                } else {
                    let state = ctrl.current_event_state();
                    let is_ctrl = state.contains(gtk4::gdk::ModifierType::CONTROL_MASK);

                    let is_osclink = obj.check_hyperlink_at(x, y).is_some();
                    let (regex_text, _, regex_bounds) = obj.check_match_at(x, y);

                    // We only highlight regex matches when Control is held (performance & UX)
                    if is_ctrl && regex_text.is_some() {
                        if obj.imp().hovered_regex_match.get() != regex_bounds {
                            obj.imp().hovered_regex_match.set(regex_bounds);
                            obj.queue_draw();
                        }
                    } else if obj.imp().hovered_regex_match.get().is_some() {
                        obj.imp().hovered_regex_match.set(None);
                        obj.queue_draw();
                    }

                    let is_link = is_osclink || (is_ctrl && regex_text.is_some());

                    if obj.imp().mouse_pos.get() != Some((x, y)) {
                        obj.imp().mouse_pos.set(Some((x, y)));
                        obj.queue_draw();
                    }

                    obj.set_cursor_from_name(Some(if is_link { "pointer" } else { "text" }));
                }
            }
        ));
        motion_ctrl.connect_leave(glib::clone!(
            #[weak]
            obj,
            move |_| {
                obj.set_cursor(None);
                obj.imp().mouse_pos.set(None);
                obj.imp().hovered_regex_match.set(None);
                obj.queue_draw();
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
        if !mode.contains(crate::engine::term::TermMode::MOUSE_MOTION)
            && !mode.contains(crate::engine::term::TermMode::MOUSE_DRAG)
        {
            return None;
        }
        cb += 32;
    }
    let (x, y) = (x + 1, y + 1);
    if mode.contains(crate::engine::term::TermMode::SGR_MOUSE) {
        let suffix = if is_press { b'M' } else { b'm' };
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
