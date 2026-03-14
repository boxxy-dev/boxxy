use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use crate::engine::event::Event;
use crate::engine::grid::Scroll;
use crate::engine::index::{Point, Column, Line, Side};
use crate::engine::selection::{Selection, SelectionType};
use crate::terminal::backend::TerminalBackend;

// ─── Regex match rule ────────────────────────────────────────────────────────
/// A single registered regex matcher (registered via `add_match_regex_str`).
/// Tag IDs start at 1 and increment; 0 is reserved for "no match".
pub struct MatchRule {
    pub id: i32,
    pub regex: regex::Regex,
    pub cursor_name: String,
}

pub type TitleCallback = Box<dyn Fn(String) + 'static>;
pub type CwdCallback = Box<dyn Fn(String) + 'static>;
pub type BellCallback = Box<dyn Fn() + 'static>;
pub type ExitCallback = Box<dyn Fn(i32) + 'static>;

pub type Osc133ACallback = Box<dyn Fn() + 'static>;
pub type Osc133BCallback = Box<dyn Fn() + 'static>;
pub type Osc133CCallback = Box<dyn Fn() + 'static>;
pub type Osc133DCallback = Box<dyn Fn(Option<i32>) + 'static>;
pub type ClawQueryCallback = Box<dyn Fn(String) + 'static>;

pub struct TerminalWidget {
    pub backend: RefCell<Option<TerminalBackend>>,
    pub mouse_pressed: Cell<bool>,
    pub fg_color: RefCell<Option<gtk4::gdk::RGBA>>,
    pub bg_color: RefCell<Option<gtk4::gdk::RGBA>>,
    pub palette: RefCell<Vec<gtk4::gdk::RGBA>>,
    pub show_grid: Cell<bool>,
    pub cell_width_scale: Cell<f64>,
    pub cell_height_scale: Cell<f64>,
    pub font_desc: RefCell<Option<gtk4::pango::FontDescription>>,
    pub padding: Cell<f32>,
    pub padding_handler_id: RefCell<Option<glib::SignalHandlerId>>,
    pub cursor_color: RefCell<Option<gtk4::gdk::RGBA>>,
    pub cursor_blinking: Cell<bool>,
    pub cursor_visible: Cell<bool>,
    pub cursor_blink_id: RefCell<Option<glib::SourceId>>,
    pub cursor_shape: Cell<crate::engine::ansi::CursorShape>,
    pub search_query: RefCell<Option<String>>,
    pub search_case_sensitive: Cell<bool>,
    pub search_wrap_around: Cell<bool>,
    pub invert_scroll: Cell<bool>,
    // gtk::Scrollable vertical adjustment + its signal handler ID
    pub vadjustment: RefCell<Option<gtk4::Adjustment>>,
    pub vadjustment_handler: RefCell<Option<glib::SignalHandlerId>>,
    // gtk::Scrollable horizontal adjustment (unused for scrolling, required by interface)
    pub hadjustment: RefCell<Option<gtk4::Adjustment>>,
    // gtk::Scrollable scroll policies
    pub hscroll_policy: Cell<gtk4::ScrollablePolicy>,
    pub vscroll_policy: Cell<gtk4::ScrollablePolicy>,
    // ── Link / regex matching ────────────────────────────────────────────────
    /// Registered regex rules (see `add_match_regex_str`).
    pub match_rules: RefCell<Vec<MatchRule>>,
    /// Next tag ID to assign when registering a new rule.
    pub next_match_tag: Cell<i32>,
    // ── Hover tracking ───────────────────────────────────────────────────────
    /// Current pointer position in widget-local pixels, or `None` when the
    /// pointer is outside the widget. Used to restrict OSC 8 underlines to
    /// the hovered hyperlink only.
    pub mouse_pos: Cell<Option<(f64, f64)>>,

    // ── Terminal event callbacks ─────────────────────────────────────────────
    /// Called on the GTK main loop whenever the shell sets a new window title
    /// (OSC 0 / OSC 2).  Wired by `pane.rs` to emit `PaneOutput::TitleChanged`.
    pub title_callback: RefCell<Option<TitleCallback>>,
    /// Called when the CWD changes, detected by piggybacking on title events
    /// and reading `/proc/{child_pid}/cwd`.  Only useful on native Linux builds;
    /// on Flatpak the direct child PID belongs to `host-spawn` whose CWD does
    /// not follow `cd` commands issued inside the host shell.
    pub cwd_callback: RefCell<Option<CwdCallback>>,
    /// Called when the terminal bell fires (BEL / `\x07`).
    pub bell_callback: RefCell<Option<BellCallback>>,
    /// Called when the child shell process exits with an exit code.
    pub exit_callback: RefCell<Option<ExitCallback>>,
    
    // OSC 133 callbacks
    pub osc_133_a_callback: RefCell<Option<Osc133ACallback>>,
    pub osc_133_b_callback: RefCell<Option<Osc133BCallback>>,
    pub osc_133_c_callback: RefCell<Option<Osc133CCallback>>,
    pub osc_133_d_callback: RefCell<Option<Osc133DCallback>>,
    pub claw_query_callback: RefCell<Option<ClawQueryCallback>>,
    
    /// Last CWD emitted via `cwd_callback`; used to suppress duplicate events.
    pub last_cwd: RefCell<Option<String>>,
    // ── Kitty Graphics Cache ────────────────────────────────────────────────
    pub kitty_textures: RefCell<HashMap<u32, gtk4::gdk::Texture>>,
}

impl Default for TerminalWidget {
    fn default() -> Self {
        Self {
            backend: RefCell::new(None),
            mouse_pressed: Cell::new(false),
            fg_color: RefCell::new(None),
            bg_color: RefCell::new(None),
            palette: RefCell::new(Vec::new()),
            show_grid: Cell::new(false),
            cell_width_scale: Cell::new(1.0),
            cell_height_scale: Cell::new(1.0),
            font_desc: RefCell::new(None),
            padding: Cell::new(0.0),
            padding_handler_id: RefCell::new(None),
            cursor_color: RefCell::new(None),
            cursor_blinking: Cell::new(true),
            cursor_visible: Cell::new(true),
            cursor_blink_id: RefCell::new(None),
            cursor_shape: Cell::new(crate::engine::ansi::CursorShape::Block),
            search_query: RefCell::new(None),
            search_case_sensitive: Cell::new(false),
            search_wrap_around: Cell::new(true),
            invert_scroll: Cell::new(false),
            vadjustment: RefCell::new(None),
            vadjustment_handler: RefCell::new(None),
            hadjustment: RefCell::new(None),
            hscroll_policy: Cell::new(gtk4::ScrollablePolicy::Minimum),
            vscroll_policy: Cell::new(gtk4::ScrollablePolicy::Minimum),
            match_rules: RefCell::new(Vec::new()),
            mouse_pos: Cell::new(None),
            next_match_tag: Cell::new(1),
            title_callback: RefCell::new(None),
            cwd_callback: RefCell::new(None),
            bell_callback: RefCell::new(None),
            exit_callback: RefCell::new(None),
            osc_133_a_callback: RefCell::new(None),
            osc_133_b_callback: RefCell::new(None),
            osc_133_c_callback: RefCell::new(None),
            osc_133_d_callback: RefCell::new(None),
            claw_query_callback: RefCell::new(None),
            last_cwd: RefCell::new(None),
            kitty_textures: RefCell::new(HashMap::new()),
        }
    }
}

// ─── Property IDs ────────────────────────────────────────────────────────────
// Must start at 1. These correspond to the four properties required by the
// gtk::Scrollable interface; we install them as override properties so that
// gtk::ScrolledWindow finds and drives them automatically.
const PROP_VADJUSTMENT: usize = 1;
const PROP_HADJUSTMENT: usize = 2;
const PROP_VSCROLL_POLICY: usize = 3;
const PROP_HSCROLL_POLICY: usize = 4;

#[glib::object_subclass]
impl ObjectSubclass for TerminalWidget {
    const NAME: &'static str = "BoxxyTerminalWidget";
    type Type = super::TerminalWidget;
    type ParentType = gtk4::Widget;
    // Declaring this causes GTK to allocate our widget the full space inside a
    // ScrolledWindow instead of wrapping us in an invisible Viewport.
    type Interfaces = (gtk4::Scrollable,);
}

// ─── gtk::Scrollable interface implementation ─────────────────────────────────
// ScrollableImpl is a marker trait; the adjustment logic is driven by our
// GObject properties below.
impl ScrollableImpl for TerminalWidget {}

// ─── GObject property plumbing ────────────────────────────────────────────────
impl ObjectImpl for TerminalWidget {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
        PROPS.get_or_init(|| {
            vec![
                // ID 1
                glib::ParamSpecOverride::for_interface::<gtk4::Scrollable>("vadjustment"),
                // ID 2
                glib::ParamSpecOverride::for_interface::<gtk4::Scrollable>("hadjustment"),
                // ID 3
                glib::ParamSpecOverride::for_interface::<gtk4::Scrollable>("vscroll-policy"),
                // ID 4
                glib::ParamSpecOverride::for_interface::<gtk4::Scrollable>("hscroll-policy"),
            ]
        })
    }

    fn set_property(&self, id: usize, value: &glib::Value, _pspec: &glib::ParamSpec) {
        match id {
            PROP_VADJUSTMENT => {
                let adj: Option<gtk4::Adjustment> = value.get().unwrap();
                self.obj().set_vadjustment(adj.as_ref());
            }
            PROP_HADJUSTMENT => {
                let adj: Option<gtk4::Adjustment> = value.get().unwrap();
                self.hadjustment.replace(adj);
                self.obj().notify("hadjustment");
            }
            PROP_VSCROLL_POLICY => {
                let policy: gtk4::ScrollablePolicy = value.get().unwrap();
                self.vscroll_policy.set(policy);
                self.obj().notify("vscroll-policy");
            }
            PROP_HSCROLL_POLICY => {
                let policy: gtk4::ScrollablePolicy = value.get().unwrap();
                self.hscroll_policy.set(policy);
                self.obj().notify("hscroll-policy");
            }
            _ => unimplemented!("Unknown property id {id}"),
        }
    }

    fn property(&self, id: usize, _pspec: &glib::ParamSpec) -> glib::Value {
        match id {
            PROP_VADJUSTMENT => self.vadjustment.borrow().to_value(),
            PROP_HADJUSTMENT => self.hadjustment.borrow().to_value(),
            PROP_VSCROLL_POLICY => self.vscroll_policy.get().to_value(),
            PROP_HSCROLL_POLICY => self.hscroll_policy.get().to_value(),
            _ => unimplemented!("Unknown property id {id}"),
        }
    }

    fn constructed(&self) {
        self.parent_constructed();
        let obj = self.obj();
        obj.set_hexpand(true);
        obj.set_vexpand(true);
        obj.set_focusable(true);
        obj.set_can_focus(true);

        obj.connect_has_focus_notify(move |widget| {
            if widget.has_focus() {
                widget.start_cursor_blink();
            } else {
                widget.stop_cursor_blink();
                let imp = widget.imp();
                imp.cursor_visible.set(true);
                widget.queue_draw();
            }
        });

        // ── Keyboard input ────────────────────────────────────────────────────
        let key_ctrl = gtk4::EventControllerKey::new();
        key_ctrl.connect_key_pressed(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _keycode, modifier| {
                let imp = obj.imp();
                
                // Keep cursor solidly visible while typing/navigating
                obj.start_cursor_blink();

                // Ctrl+Shift+V → paste
                if key == gtk4::gdk::Key::V
                    && modifier.contains(
                        gtk4::gdk::ModifierType::CONTROL_MASK
                            | gtk4::gdk::ModifierType::SHIFT_MASK,
                    )
                {
                    log::info!("Terminal: Ctrl+Shift+V detected, pasting...");
                    obj.paste_clipboard();
                    return glib::Propagation::Stop;
                }

                // Ctrl+Shift+C → copy
                if key == gtk4::gdk::Key::C
                    && modifier.contains(
                        gtk4::gdk::ModifierType::CONTROL_MASK
                            | gtk4::gdk::ModifierType::SHIFT_MASK,
                    )
                {
                    log::info!("Terminal: Ctrl+Shift+C detected, copying...");
                    obj.copy_clipboard();
                    return glib::Propagation::Stop;
                }

                // ── Shift + navigation keys → scroll the scrollback buffer ───
                //
                // Shift+PageUp/Down scrolls the terminal's scrollback by one
                // full screen page.  Shift+Home/End jump to the top / bottom of
                // the buffer.  These are intercepted here *before* translate_key
                // so the raw \x1b[5~ sequences are never forwarded to the PTY.
                if modifier.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                    let scroll_op = match key {
                        gtk4::gdk::Key::Page_Up | gtk4::gdk::Key::KP_Page_Up => {
                            Some(Scroll::PageUp)
                        }
                        gtk4::gdk::Key::Page_Down | gtk4::gdk::Key::KP_Page_Down => {
                            Some(Scroll::PageDown)
                        }
                        gtk4::gdk::Key::Home => Some(Scroll::Top),
                        gtk4::gdk::Key::End  => Some(Scroll::Bottom),
                        _ => None,
                    };
                    if let Some(op) = scroll_op {
                        let backend_ref = imp.backend.borrow();
                        if let Some(backend) = backend_ref.as_ref() {
                            backend.scroll_display(op);
                        }
                        return glib::Propagation::Stop;
                    }
                }

                if let Some(bytes) = crate::terminal::input::translate_key(key, modifier) {
                    if let Some(backend) = imp.backend.borrow().as_ref() {
                        if !backend.is_alt_screen() {
                            backend.scroll_display(Scroll::Bottom);
                        }
                        backend.write_to_pty(bytes);
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        obj.add_controller(key_ctrl);

        // ── Mouse wheel / touchpad scrolling ──────────────────────────────────
        let scroll_ctrl = gtk4::EventControllerScroll::new(
            gtk4::EventControllerScrollFlags::VERTICAL
                | gtk4::EventControllerScrollFlags::DISCRETE,
        );
        scroll_ctrl.connect_scroll(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |ctrl, _dx, dy| {
                let imp = obj.imp();

                let did_scroll = {
                    let backend_ref = imp.backend.borrow();
                    if let Some(backend) = backend_ref.as_ref() {
                        let state = backend.render_state.load();
                        let modifiers = ctrl.current_event_state();
                        
                        if state.mode.intersects(crate::engine::term::TermMode::MOUSE_MODE) 
                            && !modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK) 
                            && let Some((mx, my)) = imp.mouse_pos.get() 
                        {
                            let char_size = imp.get_char_size(&obj);
                            let padding = imp.padding.get() as f64;
                            let cell_x = ((mx - padding) / char_size.0).floor() as usize;
                            let cell_y = ((my - padding) / char_size.1).floor() as usize;
                            
                            let button = if dy > 0.0 { 65 } else { 64 };
                            
                            if let Some(seq) = format_mouse_report(button, true, false, cell_x, cell_y, modifiers, state.mode) {
                                backend.notifier.0.send(crate::engine::event_loop::Msg::Input(std::borrow::Cow::Owned(seq))).ok();
                                if state.mode.contains(crate::engine::term::TermMode::SGR_MOUSE)
                                    && let Some(release_seq) = format_mouse_report(button, false, false, cell_x, cell_y, modifiers, state.mode) {
                                        backend.notifier.0.send(crate::engine::event_loop::Msg::Input(std::borrow::Cow::Owned(release_seq))).ok();
                                }
                                return glib::Propagation::Stop;
                            }
                        }

                        let mut adjusted_dy = dy;
                        if imp.invert_scroll.get() {
                            adjusted_dy = -adjusted_dy;
                        }
                        let lines = (adjusted_dy * 3.0) as i32;
                        backend.scroll_display(Scroll::Delta(lines));
                        true
                    } else {
                        false
                    }
                };

                if did_scroll {
                    obj.queue_draw();
                    obj.update_scroll_adjustment();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        obj.add_controller(scroll_ctrl);

        // ── Mouse selection ───────────────────────────────────────────────────
        let click_gesture = gtk4::GestureClick::new();
        click_gesture.set_button(0);
        click_gesture.connect_pressed(glib::clone!(
            #[weak]
            obj,
            move |gesture, n_press, x, y| {
                let imp = obj.imp();
                obj.grab_focus();
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();
                    let modifiers = gesture.current_event_state();
                    
                    let char_size = imp.get_char_size(&obj);
                    let padding = imp.padding.get() as f64;
                    let cell_x = (x - padding) / char_size.0;
                    let col = cell_x.floor() as usize;
                    let side = if (cell_x - cell_x.floor()) > 0.5 { Side::Right } else { Side::Left };
                    let row = ((y - padding) / char_size.1).floor() as usize;
                    let display_offset = state.display_offset;
                    let point = Point::new(
                        Line(row as i32 - display_offset),
                        Column(col),
                    );

                    let is_mouse_mode = state.mode.intersects(crate::engine::term::TermMode::MOUSE_MODE);
                    let bypass_mouse_mode = modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK);

                    if is_mouse_mode && !bypass_mouse_mode {
                        imp.mouse_pressed.set(true);
                        imp.mouse_pos.set(Some((x, y)));
                        let button = match gesture.current_button() {
                            1 => 0,
                            2 => 1,
                            3 => 2,
                            _ => 0,
                        };
                        if let Some(seq) = format_mouse_report(button, true, false, col, row, modifiers, state.mode) {
                            backend.notifier.0.send(crate::engine::event_loop::Msg::Input(std::borrow::Cow::Owned(seq))).ok();
                        }
                        return;
                    }

                    if gesture.current_button() == 1 {
                        imp.mouse_pressed.set(true);
                        let selection_type = match n_press {
                            1 => SelectionType::Simple,
                            2 => SelectionType::Semantic,
                            _ => SelectionType::Lines,
                        };
                        backend.set_selection(Some(Selection::new(
                            selection_type,
                            point,
                            side,
                        )));
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
                    
                    let is_mouse_mode = state.mode.intersects(crate::engine::term::TermMode::MOUSE_MODE);
                    let bypass_mouse_mode = modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK);

                    if is_mouse_mode && !bypass_mouse_mode {
                        let char_size = imp.get_char_size(&obj);
                        let padding = imp.padding.get() as f64;
                        let col = ((x - padding) / char_size.0).floor() as usize;
                        let row = ((y - padding) / char_size.1).floor() as usize;

                        let button = match gesture.current_button() {
                            1 => 0,
                            2 => 1,
                            3 => 2,
                            _ => 0,
                        };
                        // For X10/Normal, release is always button 3
                        let release_button = if state.mode.contains(crate::engine::term::TermMode::SGR_MOUSE) { button } else { 3 };
                        if let Some(seq) = format_mouse_report(release_button, false, false, col, row, modifiers, state.mode) {
                            backend.notifier.0.send(crate::engine::event_loop::Msg::Input(std::borrow::Cow::Owned(seq))).ok();
                        }
                        return;
                    }

                    backend.copy_selection(crate::engine::term::ClipboardType::Selection);
                    backend.copy_selection(crate::engine::term::ClipboardType::Clipboard);
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
                    let is_mouse_mode = state.mode.intersects(crate::engine::term::TermMode::MOUSE_MODE);
                    let bypass_mouse_mode = modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK);
                    
                    if is_mouse_mode && !bypass_mouse_mode {
                        imp.mouse_pos.set(Some((x, y)));
                        
                        let char_size = imp.get_char_size(&obj);
                        let padding = imp.padding.get() as f64;
                        let col = ((x - padding) / char_size.0).floor() as usize;
                        let row = ((y - padding) / char_size.1).floor() as usize;
                        
                        let is_drag = imp.mouse_pressed.get();
                        
                        // Default to left button (0) for drag, or 35 (no button) for motion
                        let button = if is_drag { 0 } else { 35 };

                        if let Some(seq) = format_mouse_report(button, is_drag, true, col, row, modifiers, state.mode) {
                            backend.notifier.0.send(crate::engine::event_loop::Msg::Input(std::borrow::Cow::Owned(seq))).ok();
                        }
                        return;
                    }
                }

                if imp.mouse_pressed.get() {
                    // ── Drag selection ────────────────────────────────────────
                    imp.mouse_pos.set(Some((x, y)));
                    if let Some(backend) = imp.backend.borrow().as_ref() {
                        let state = backend.render_state.load();
                        let char_size = imp.get_char_size(&obj);
                        let padding = imp.padding.get() as f64;
                        let cell_x = (x - padding) / char_size.0;
                        let col = cell_x.floor() as usize;
                        let side = if (cell_x - cell_x.floor()) > 0.5 { Side::Right } else { Side::Left };
                        let row = ((y - padding) / char_size.1).floor() as usize;
                        let display_offset = state.display_offset;
                        let point = Point::new(
                            Line(row as i32 - display_offset),
                            Column(col),
                        );
                        backend.update_selection(point, side);
                        obj.queue_draw();
                    }
                } else {
                    let is_link = obj.check_hyperlink_at(x, y).is_some()
                        || obj.check_match_at(x, y).0.is_some();

                    obj.imp().mouse_pos.set(Some((x, y)));
                    obj.queue_draw();

                    if is_link {
                        obj.set_cursor_from_name(Some("pointer"));
                    } else {
                        obj.set_cursor_from_name(Some("text"));
                    }
                }
            }
        ));
        motion_ctrl.connect_leave(glib::clone!(
            #[weak]
            obj,
            move |_| {
                obj.set_cursor(None);
                obj.imp().mouse_pos.set(None);
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

    if modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK) { cb += 4; }
    if modifiers.contains(gtk4::gdk::ModifierType::ALT_MASK) { cb += 8; }
    if modifiers.contains(gtk4::gdk::ModifierType::CONTROL_MASK) { cb += 16; }

    if is_motion {
        if !mode.contains(crate::engine::term::TermMode::MOUSE_MOTION) && !mode.contains(crate::engine::term::TermMode::MOUSE_DRAG) {
            return None;
        }
        cb += 32;
    }

    let x = x + 1;
    let y = y + 1;

    if mode.contains(crate::engine::term::TermMode::SGR_MOUSE) {
        let suffix = if is_press { b'M' } else { b'm' };
        let mut buf = Vec::new();
        buf.extend_from_slice(b"\x1b[<");
        buf.extend_from_slice(cb.to_string().as_bytes());
        buf.push(b';');
        buf.extend_from_slice(x.to_string().as_bytes());
        buf.push(b';');
        buf.extend_from_slice(y.to_string().as_bytes());
        buf.push(suffix);
        Some(buf)
    } else {
        if x > 223 || y > 223 {
            return None;
        }
        let cb_byte = 32 + cb;
        let cx_byte = (32 + x as u8).max(32);
        let cy_byte = (32 + y as u8).max(32);
        Some(vec![b'\x1b', b'[', b'M', cb_byte, cx_byte, cy_byte])
    }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────
impl TerminalWidget {
    pub(crate) fn get_char_size(&self, widget: &super::TerminalWidget) -> (f64, f64) {
        let pango_ctx = widget.pango_context();
        let layout = gtk4::pango::Layout::new(&pango_ctx);
        if let Some(ref fd) = *self.font_desc.borrow() {
            layout.set_font_description(Some(fd));
        } else {
            let mut font_desc = gtk4::pango::FontDescription::new();
            font_desc.set_family("Monospace");
            font_desc.set_size(12 * gtk4::pango::SCALE);
            layout.set_font_description(Some(&font_desc));
        }
        layout.set_text("A");
        let (_, logical) = layout.extents();
        (
            (logical.width() as f64 / gtk4::pango::SCALE as f64)
                * self.cell_width_scale.get(),
            (logical.height() as f64 / gtk4::pango::SCALE as f64)
                * self.cell_height_scale.get(),
        )
    }

    fn setup_event_loop(&self, receiver: async_channel::Receiver<Event>) {
        let obj_weak = self.obj().downgrade();
        glib::spawn_future_local(async move {
            fn handle_title(widget: &super::TerminalWidget, title: String) {
                {
                    let cb = widget.imp().title_callback.borrow();
                    if let Some(f) = cb.as_ref() {
                        f(title.clone());
                    }
                }

                let new_cwd = {
                    let backend_ref = widget.imp().backend.borrow();
                    backend_ref.as_ref().and_then(|b| b.cwd())
                };
                if let Some(cwd) = new_cwd {
                    let mut last = widget.imp().last_cwd.borrow_mut();
                    if last.as_deref() != Some(cwd.as_str()) {
                        *last = Some(cwd.clone());
                        drop(last); 
                        let cb = widget.imp().cwd_callback.borrow();
                        if let Some(f) = cb.as_ref() {
                            f(cwd);
                        }
                    }
                }
            }

            while let Ok(event) = receiver.recv().await {
                if let Some(widget) = obj_weak.upgrade() {
                    match event {
                        Event::Wakeup => {
                            if let Some(backend) = widget.imp().backend.borrow().as_ref() {
                                backend.clear_pending_wakeups();
                            }
                            widget.queue_draw();
                            widget.update_scroll_adjustment();
                        }
                        Event::PtyWrite(text) => {
                            log::trace!("TerminalWidget: received PtyWrite event, len={}", text.len());
                            if let Some(backend) =
                                widget.imp().backend.borrow().as_ref()
                            {
                                backend.write_to_pty(text.into_bytes());
                            }
                        }
                        Event::Title(title) => {
                            handle_title(&widget, title);
                        }
                        Event::CwdChanged(cwd) => {
                            let mut last = widget.imp().last_cwd.borrow_mut();
                            if last.as_deref() != Some(cwd.as_str()) {
                                *last = Some(cwd.clone());
                                drop(last);
                                let cb = widget.imp().cwd_callback.borrow();
                                if let Some(f) = cb.as_ref() {
                                    f(cwd);
                                }
                            }
                        }
                        Event::Osc133A => {
                            let cb = widget.imp().osc_133_a_callback.borrow();
                            if let Some(f) = cb.as_ref() {
                                f();
                            }
                        }
                        Event::Osc133B => {
                            let cb = widget.imp().osc_133_b_callback.borrow();
                            if let Some(f) = cb.as_ref() {
                                f();
                            }
                        }
                        Event::Osc133C => {
                            let cb = widget.imp().osc_133_c_callback.borrow();
                            if let Some(f) = cb.as_ref() {
                                f();
                            }
                        }
                        Event::Osc133D(exit_code) => {
                            let cb = widget.imp().osc_133_d_callback.borrow();
                            if let Some(f) = cb.as_ref() {
                                f(exit_code);
                            }
                        }
                        Event::ClawQuery(query) => {
                            let cb = widget.imp().claw_query_callback.borrow();
                            if let Some(f) = cb.as_ref() {
                                f(query);
                            }
                        }
                        Event::ResetTitle => {
                            handle_title(&widget, "Terminal".to_string());
                        }
                        Event::Bell => {
                            let cb = widget.imp().bell_callback.borrow();
                            if let Some(f) = cb.as_ref() {
                                f();
                            }
                        }
                        Event::ChildExit(code) => {
                            let cb = widget.imp().exit_callback.borrow();
                            if let Some(f) = cb.as_ref() {
                                f(code.code().unwrap_or(0));
                            }
                        }
                        // ── Clipboard Operations (OSC 52 and Selection) ────────
                        Event::ClipboardStore(ty, text) => {
                            log::info!("Terminal: Event::ClipboardStore received, type={:?}, len={}", ty, text.len());
                            let clipboard = if ty == crate::engine::term::ClipboardType::Selection {
                                widget.display().primary_clipboard()
                            } else {
                                widget.clipboard()
                            };
                            clipboard.set_text(&text);
                        }
                        Event::ClipboardLoad(ty, formatter) => {
                            log::info!("Terminal: Event::ClipboardLoad received, type={:?}", ty);
                            let clipboard = if ty == crate::engine::term::ClipboardType::Selection {
                                widget.display().primary_clipboard()
                            } else {
                                widget.clipboard()
                            };
                            let widget_weak = widget.downgrade();
                            glib::spawn_future_local(async move {
                                match clipboard.read_text_future().await {
                                    Ok(Some(text)) => {
                                        log::info!("Terminal: Clipboard content loaded, len={}", text.len());
                                        if let Some(widget) = widget_weak.upgrade() {
                                            let response = formatter(&text);
                                            if let Some(backend) = widget.imp().backend.borrow().as_ref() {
                                                backend.write_to_pty(response.into_bytes());
                                            }
                                        }
                                    }
                                    _ => {
                                        log::warn!("Terminal: Failed to load clipboard content for ClipboardLoad");
                                    }
                                }
                            });
                        }

                        // ── Color and Size Requests ────────────────────────────
                        // Handle OSC 10, 11, 12 queries from the shell (e.g. "What is your background color?").
                        // Modern CLI tools like Gemini CLI use these to adapt their UI themes to the terminal.
                        Event::ColorRequest(index, formatter) => {
                            let imp = widget.imp();
                            let default_fg = (*imp.fg_color.borrow()).unwrap_or_else(|| gtk4::gdk::RGBA::new(0.8, 0.8, 0.8, 1.0));
                            let default_bg = (*imp.bg_color.borrow()).unwrap_or_else(|| gtk4::gdk::RGBA::new(0.05, 0.05, 0.05, 1.0));
                            let default_cursor = (*imp.cursor_color.borrow()).unwrap_or(default_fg);

                            // Resolve the requested index to an actual color.
                            // 256: Foreground, 257: Background, 258: Cursor, 0-255: Palette
                            let rgba = if index == 256 {
                                default_fg
                            } else if index == 257 {
                                default_bg
                            } else if index == 258 {
                                default_cursor
                            } else {
                                let palette = imp.palette.borrow();
                                palette.get(index).cloned().unwrap_or(default_fg)
                            };

                            // Convert GDK's 0.0-1.0 floats back to standard 0-255 RGB for the shell response.
                            let rgb = crate::engine::ansi::Rgb {
                                r: (rgba.red() * 255.0).round() as u8,
                                g: (rgba.green() * 255.0).round() as u8,
                                b: (rgba.blue() * 255.0).round() as u8,
                            };

                            let response = formatter(rgb);
                            if let Some(backend) = widget.imp().backend.borrow().as_ref() {
                                backend.write_to_pty(response.into_bytes());
                            }
                        }
                        Event::TextAreaSizeRequest(formatter) => {
                            let char_size = widget.imp().get_char_size(&widget);
                            
                            // `width()` and `height()` might return widget size instead of character size?
                            // Wait, the formatter expects WindowSize where:
                            // num_cols is number of columns, num_lines is number of lines
                            // cell_width is pixel width of a cell
                            // cell_height is pixel height of a cell
                            // And `formatter` multiplies them to get total pixels.
                            // BUT widget.width() / height() returns the PIXEL width/height of the widget, NOT columns!
                            // So `widget.width()` as `num_cols` is totally wrong.
                            let cols = (widget.width() as f64 / char_size.0).floor() as u16;
                            let lines = (widget.height() as f64 / char_size.1).floor() as u16;

                            let size = crate::engine::event::WindowSize {
                                num_cols: cols,
                                num_lines: lines,
                                cell_width: char_size.0 as u16,
                                cell_height: char_size.1 as u16,
                                pixel_width: widget.width() as u16,
                                pixel_height: widget.height() as u16,
                            };
                            let response = formatter(size);
                            if let Some(backend) = widget.imp().backend.borrow().as_ref() {
                                backend.write_to_pty(response.into_bytes());
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    pub(crate) fn attach_pty(&self, master_fd: zbus::zvariant::OwnedFd) {
        let (sender, receiver) = async_channel::unbounded::<Event>();
        let fd: std::os::fd::OwnedFd = master_fd.into();
        let backend = TerminalBackend::from_fd(sender, fd);
        self.backend.replace(Some(backend));

        let width = self.obj().width();
        let height = self.obj().height();
        if width > 0 && height > 0 {
            let cell_size = self.get_char_size(&self.obj());
            let padding = self.padding.get() as f64;
            let cols = ((width as f64 - 2.0 * padding) / cell_size.0).floor().max(1.0) as usize;
            let lines = ((height as f64 - 2.0 * padding) / cell_size.1).floor().max(1.0) as usize;
            if let Some(b) = self.backend.borrow().as_ref() {
                b.resize(cols, lines, cell_size.0, cell_size.1, width, height);
            }
        }

        self.setup_event_loop(receiver);
        self.obj().queue_draw();
    }

    pub(crate) fn spawn(&self, working_dir: Option<&str>, command: &[&str]) {
        let (sender, receiver) = async_channel::unbounded::<Event>();
        let mut pty_options = crate::engine::tty::Options::default();
        pty_options.env.insert("TERM".to_string(), "xterm-256color".to_string());
        pty_options.env.insert("COLORTERM".to_string(), "truecolor".to_string());

        if let Some(wd) = working_dir {
            pty_options.working_directory = Some(std::path::PathBuf::from(wd));
        }
        if !command.is_empty() {
            let prog = command[0].to_string();
            let args = command[1..].iter().map(|s| s.to_string()).collect();
            pty_options.shell = Some(crate::engine::tty::Shell::new(prog, args));
        }

        let backend = TerminalBackend::new(sender, pty_options);
        self.backend.replace(Some(backend));

        self.setup_event_loop(receiver);
        self.obj().queue_draw();
    }

    pub(crate) fn copy_clipboard(&self) {
        if let Some(backend) = self.backend.borrow().as_ref() {
            backend.copy_selection(crate::engine::term::ClipboardType::Clipboard);
        }
    }

    pub(crate) fn has_selection(&self) -> bool {
        if let Some(backend) = self.backend.borrow().as_ref() {
            backend.has_selection()
        } else {
            false
        }
    }

    pub(crate) fn paste_clipboard(&self) {
        let clipboard = self.obj().clipboard();
        let obj_weak = self.obj().downgrade();
        glib::spawn_future_local(async move {
            match clipboard.read_text_future().await {
                Ok(Some(text)) => {
                    log::info!("Terminal: Pasting text from CLIPBOARD, len={}", text.len());
                    if let Some(widget) = obj_weak.upgrade()
                        && let Some(backend) = widget.imp().backend.borrow().as_ref() {
                            backend.write_to_pty(text.as_str().as_bytes().to_vec());
                        }
                }
                _ => {
                    log::warn!("Terminal: CLIPBOARD paste failed or empty");
                }
            }
        });
    }

    pub(crate) fn search(&self, direction: crate::engine::index::Direction) {
        let query = self.search_query.borrow().clone();
        if let Some(q) = query
            && let Some(backend) = self.backend.borrow().as_ref() {
                backend.search(q, direction, !self.search_case_sensitive.get());
            }
    }
}

// ─── Widget implementation ────────────────────────────────────────────────────
impl WidgetImpl for TerminalWidget {
    fn measure(
        &self,
        _orientation: gtk4::Orientation,
        _for_size: i32,
    ) -> (i32, i32, i32, i32) {
        (100, 400, -1, -1)
    }

    fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
        self.parent_size_allocate(width, height, baseline);
        let char_size = self.get_char_size(&self.obj());
        let padding = self.padding.get() as f64;
        if char_size.0 > 0.0 && char_size.1 > 0.0 {
            let cols = ((width as f64 - 2.0 * padding) / char_size.0).floor().max(1.0) as usize;
            let lines = ((height as f64 - 2.0 * padding) / char_size.1).floor().max(1.0) as usize;
            if let Some(backend) = self.backend.borrow().as_ref() {
                backend.resize(cols, lines, char_size.0, char_size.1, width, height);
            }
        }
        self.obj().update_scroll_adjustment();
    }

    fn snapshot(&self, snapshot: &gtk4::Snapshot) {
        let width = self.obj().width() as f32;
        let height = self.obj().height() as f32;
        let padding = self.padding.get();
        let bg_color = (*self.bg_color.borrow()).unwrap_or_else(|| gtk4::gdk::RGBA::new(0.05, 0.05, 0.05, 1.0));
        let fg_color_default = (*self.fg_color.borrow()).unwrap_or_else(|| gtk4::gdk::RGBA::new(0.8, 0.8, 0.8, 1.0));
        let rect = gtk4::graphene::Rect::new(0.0, 0.0, width, height);
        snapshot.append_color(&bg_color, &rect);

        if let Some(backend) = self.backend.borrow().as_ref() {
            let state = backend.render_state.load();
            snapshot.save();
            snapshot.translate(&gtk4::graphene::Point::new(padding, padding));
            let display_offset = state.display_offset;
            let pango_ctx = self.obj().pango_context();
            let layout = gtk4::pango::Layout::new(&pango_ctx);
            if let Some(ref fd) = *self.font_desc.borrow() {
                layout.set_font_description(Some(fd));
            } else {
                let mut font_desc = gtk4::pango::FontDescription::new();
                font_desc.set_family("Monospace");
                font_desc.set_size(12 * gtk4::pango::SCALE);
                layout.set_font_description(Some(&font_desc));
            }

            let char_size = self.get_char_size(&self.obj());
            let char_width = char_size.0 as f32;
            let char_height = char_size.1 as f32;
            let mut offset_y = 0.0_f32;

            if (self.cell_width_scale.get() - 1.0).abs() > f64::EPSILON || (self.cell_height_scale.get() - 1.0).abs() > f64::EPSILON {
                layout.set_text("A");
                let (_, logical) = layout.extents();
                if (self.cell_width_scale.get() - 1.0).abs() > f64::EPSILON {
                    let diff = (logical.width() as f64 * (self.cell_width_scale.get() - 1.0)) as i32;
                    let attr_list = gtk4::pango::AttrList::new();
                    let attr = gtk4::pango::AttrInt::new_letter_spacing(diff);
                    attr_list.insert(attr);
                    layout.set_attributes(Some(&attr_list));
                }
                if (self.cell_height_scale.get() - 1.0).abs() > f64::EPSILON {
                    let logical_h = logical.height() as f64 / gtk4::pango::SCALE as f64;
                    offset_y = ((char_height as f64 - logical_h) / 2.0).max(0.0) as f32;
                }
            }

            let selection_range = state.selection_range;
            let palette = self.palette.borrow();

            let draw_kitty_images = |is_background: bool| {
                let mut texture_cache = self.kitty_textures.borrow_mut();
                for placement in &state.kitty_placements {
                    if (placement.z_index < 0) != is_background {
                        continue;
                    }
                    let texture = if let Some(tex) = texture_cache.get(&placement.image_id) {
                        Some(tex.clone())
                    } else if let Some(img) = state.kitty_images.get(&placement.image_id) {
                        log::info!("Creating texture for image ID {}", placement.image_id);
                        let tex = match &img.data {
                            crate::engine::kitty::KittyImageData::Dynamic(dyn_img) => {
                                let rgba = dyn_img.to_rgba8();
                                let width = rgba.width() as i32;
                                let height = rgba.height() as i32;
                                let bytes = glib::Bytes::from(&rgba.into_raw());
                                gtk4::gdk::MemoryTexture::new(
                                    width,
                                    height,
                                    gtk4::gdk::MemoryFormat::R8g8b8a8,
                                    &bytes,
                                    (width * 4) as usize,
                                ).upcast::<gtk4::gdk::Texture>()
                            }
                            crate::engine::kitty::KittyImageData::RawRgb { width, height, data } => {
                                let w = *width as i32;
                                let h = *height as i32;
                                let bytes = glib::Bytes::from_owned(data.clone());
                                gtk4::gdk::MemoryTexture::new(
                                    w,
                                    h,
                                    gtk4::gdk::MemoryFormat::R8g8b8,
                                    &bytes,
                                    (w * 3) as usize,
                                ).upcast::<gtk4::gdk::Texture>()
                            }
                            crate::engine::kitty::KittyImageData::RawRgba { width, height, data } => {
                                let w = *width as i32;
                                let h = *height as i32;
                                let bytes = glib::Bytes::from_owned(data.clone());
                                gtk4::gdk::MemoryTexture::new(
                                    w,
                                    h,
                                    gtk4::gdk::MemoryFormat::R8g8b8a8,
                                    &bytes,
                                    (w * 4) as usize,
                                ).upcast::<gtk4::gdk::Texture>()
                            }
                        };
                        texture_cache.insert(placement.image_id, tex.clone());
                        Some(tex)
                    } else {
                        None
                    };

                    if let Some(tex) = texture {
                        let row = placement.point.line.0 + display_offset;
                        let col = placement.point.column.0;
                        
                        let scale = self.obj().scale_factor() as f32;

                        // Only draw if visible
                        let approx_height_cells = placement.height.unwrap_or_else(|| {
                             if char_height > 0.0 {
                                 ((placement.visible_height as f32 / scale) / char_height).ceil() as u32
                             } else {
                                 1
                             }
                        }).max(1);
                        
                        if row >= -(approx_height_cells as i32) && row < state.screen_lines as i32 {
                            let x = col as f32 * char_width + padding;
                            let y = row as f32 * char_height + padding;
                            
                            // Scale to cell dimensions or use pixel dimensions
                            let target_width = if let Some(c) = placement.width {
                                c as f32 * char_width
                            } else {
                                placement.visible_width as f32 / scale
                            };

                            let target_height = if let Some(r) = placement.height {
                                r as f32 * char_height
                            } else {
                                placement.visible_height as f32 / scale
                            };
                            
                            let rect = gtk4::graphene::Rect::new(x, y, target_width, target_height);
                            snapshot.append_texture(&tex, &rect);
                        }
                    }
                }
            };

            // Draw background images
            draw_kitty_images(true);

            let hovered_uri = self.mouse_pos.get().and_then(|(mx, my)| {
                let padding = self.padding.get() as f64;
                let hov_col = ((mx - padding) / char_size.0).floor() as usize;
                let hov_row = ((my - padding) / char_size.1).floor() as usize;
                if hov_row < state.screen_lines && hov_col < state.columns {
                    let hov_point = Point::new(Line(hov_row as i32 - display_offset), Column(hov_col));
                    state.cell(hov_point).hyperlink().map(|h| h.uri().to_string())
                } else { None }
            });

            let mut line_str = String::with_capacity(state.columns);

            for row in 0..state.screen_lines {
                let mut current_fg = fg_color_default;
                let mut current_bg: Option<gtk4::gdk::RGBA> = None;
                let mut start_col = 0.0_f32;
                line_str.clear();

                for col in 0..state.columns {
                    let point = Point::new(Line(row as i32 - display_offset), Column(col));
                    let cell = state.cell(point);

                    let mut bg = match cell.bg {
                        crate::engine::ansi::Color::Named(named) => {
                            let idx = named as usize;
                            if idx < 256 {
                                palette.get(idx).cloned()
                            } else if idx == 257 { // NamedColor::Background
                                None // Use default background
                            } else if (259..=266).contains(&idx) { // Dim colors
                                palette.get(idx - 259).cloned().map(|mut c| {
                                    c.set_alpha(c.alpha() * 0.5);
                                    c
                                })
                            } else {
                                None
                            }
                        }
                        crate::engine::ansi::Color::Spec(rgb) => Some(gtk4::gdk::RGBA::new(rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0, 1.0)),
                        crate::engine::ansi::Color::Indexed(idx) => {
                            palette.get(idx as usize).cloned()
                        }
                    };

                    let mut fg = match cell.fg {
                        crate::engine::ansi::Color::Named(named) => {
                            let mut idx = named as usize;
                            if cell.flags.contains(crate::engine::term::cell::Flags::BOLD) && idx < 8 {
                                idx += 8;
                            }
                            if idx < 256 {
                                palette.get(idx).cloned().unwrap_or(fg_color_default)
                            } else if idx == 256 { // NamedColor::Foreground
                                fg_color_default
                            } else if (259..=266).contains(&idx) { // Dim colors
                                palette.get(idx - 259).cloned().map(|mut c| {
                                    c.set_alpha(c.alpha() * 0.5);
                                    c
                                }).unwrap_or(fg_color_default)
                            } else {
                                fg_color_default
                            }
                        }
                        crate::engine::ansi::Color::Spec(rgb) => gtk4::gdk::RGBA::new(rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0, 1.0),
                        crate::engine::ansi::Color::Indexed(mut idx) => {
                            if cell.flags.contains(crate::engine::term::cell::Flags::BOLD) && idx < 8 {
                                idx += 8;
                            }
                            palette.get(idx as usize).cloned().unwrap_or(fg_color_default)
                        }
                    };

                    if cell.flags.contains(crate::engine::term::cell::Flags::DIM) {
                        fg.set_red(fg.red() * 0.6);
                        fg.set_green(fg.green() * 0.6);
                        fg.set_blue(fg.blue() * 0.6);
                    }

                    if cell.flags.contains(crate::engine::term::cell::Flags::INVERSE) {
                        let actual_bg = bg.unwrap_or(bg_color);
                        bg = Some(fg);
                        fg = actual_bg;
                    }

                    if bg != current_bg || fg != current_fg {
                        if let Some(ref bg_col) = current_bg {
                            let width = (col as f32 * char_width) - start_col;
                            snapshot.append_color(bg_col, &gtk4::graphene::Rect::new(start_col, row as f32 * char_height, width + 0.5, char_height + 0.5));
                        }

                        if !line_str.is_empty() {
                            layout.set_text(&line_str);
                            snapshot.save();
                            snapshot.translate(&gtk4::graphene::Point::new(start_col, row as f32 * char_height + offset_y));
                            snapshot.append_layout(&layout, &current_fg);
                            snapshot.restore();
                        }

                        current_bg = bg;
                        current_fg = fg;
                        start_col = col as f32 * char_width;
                        line_str.clear();
                    }
                    line_str.push(cell.c);
                }

                // Final flush for the row
                if let Some(ref bg_col) = current_bg {
                    let width = (state.columns as f32 * char_width) - start_col;
                    snapshot.append_color(bg_col, &gtk4::graphene::Rect::new(start_col, row as f32 * char_height, width + 0.5, char_height + 0.5));
                }

                if !line_str.is_empty() {
                    layout.set_text(&line_str);
                    snapshot.save();
                    snapshot.translate(&gtk4::graphene::Point::new(start_col, row as f32 * char_height + offset_y));
                    snapshot.append_layout(&layout, &current_fg);
                    snapshot.restore();
                }

                // Draw selections and hyperlinks on top of the rendered text
                for col in 0..state.columns {
                    let point = Point::new(Line(row as i32 - display_offset), Column(col));
                    let cell = state.cell(point);

                    if let Some(ref range) = selection_range
                        && range.contains(point) {
                            snapshot.append_color(&gtk4::gdk::RGBA::new(0.2, 0.4, 0.6, 0.5), &gtk4::graphene::Rect::new(col as f32 * char_width, row as f32 * char_height, char_width, char_height));
                        }

                    if let Some(cell_uri) = cell.hyperlink().map(|h| h.uri().to_string())
                        && hovered_uri.as_deref() == Some(cell_uri.as_str()) {
                            snapshot.append_color(&gtk4::gdk::RGBA::new(0.35, 0.75, 1.0, 1.0), &gtk4::graphene::Rect::new(col as f32 * char_width, row as f32 * char_height + char_height - 1.5, char_width, 1.5));
                        }
                }
            }

            if self.show_grid.get() {
                let grid_color = gtk4::gdk::RGBA::new(1.0, 1.0, 1.0, 0.05);
                for row in 0..=state.screen_lines {
                    let y = row as f32 * char_height;
                    snapshot.append_color(&grid_color, &gtk4::graphene::Rect::new(0.0, y, width - 2.0 * padding, 1.0));
                }
                for col in 0..=state.columns {
                    let x = col as f32 * char_width;
                    snapshot.append_color(&grid_color, &gtk4::graphene::Rect::new(x, 0.0, 1.0, height - 2.0 * padding));
                }
            }

            if self.cursor_visible.get() {
                let cursor_point = state.cursor_point;
                let cursor_row = cursor_point.line.0 + display_offset;
                if cursor_row >= 0 && cursor_row < state.screen_lines as i32 {
                    let shape = self.cursor_shape.get();
                    let (c_width, c_height, c_y_offset) = match shape {
                        crate::engine::ansi::CursorShape::Underline => (char_width, 2.0_f32, char_height - 2.0),
                        crate::engine::ansi::CursorShape::Beam => (2.0_f32, char_height, 0.0_f32),
                        _ => (char_width, char_height, 0.0_f32),
                    };
                    let cursor_color = (*self.cursor_color.borrow()).unwrap_or_else(|| gtk4::gdk::RGBA::new(1.0, 1.0, 1.0, 0.7));
                    snapshot.append_color(&cursor_color, &gtk4::graphene::Rect::new(cursor_point.column.0 as f32 * char_width, cursor_row as f32 * char_height + c_y_offset, c_width, c_height));
                }
            }
            snapshot.restore();

            // ── Draw Foreground Kitty Graphics ───────────────────────────────
            draw_kitty_images(false);
        }
    }
}
