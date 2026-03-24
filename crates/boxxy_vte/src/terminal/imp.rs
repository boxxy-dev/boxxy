use crate::engine::event::Event;
use crate::engine::grid::Scroll;
use crate::engine::index::{Column, Line, Point, Side};
use crate::engine::selection::{Selection, SelectionType};
use crate::terminal::backend::TerminalBackend;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

// ─── Cell-filling character detection ────────────────────────────────────────
#[inline]
fn is_cell_filling_char(c: char) -> bool {
    matches!(c,
        '\u{2500}'..='\u{257F}' |  // Box Drawing
        '\u{2580}'..='\u{259F}' |  // Block Elements
        '\u{E0A0}'..='\u{E0A3}' |  // Powerline Extra (PUA)
        '\u{E0B0}'..='\u{E0D4}'    // Powerline Symbols (PUA)
    )
}

// ─── Regex match rule ────────────────────────────────────────────────────────
pub struct MatchRule {
    pub id: i32,
    pub regex: regex::Regex,
    pub cursor_name: String,
}

pub type TitleCallback = Box<dyn Fn(String) + 'static>;
pub type CwdCallback = Box<dyn Fn(String) + 'static>;
pub type ProgressCallback = Box<dyn Fn(u8, u8) + 'static>;
pub type BellCallback = Box<dyn Fn() + 'static>;
pub type ExitCallback = Box<dyn Fn(i32) + 'static>;

pub type Osc133ACallback = Box<dyn Fn() + 'static>;
pub type Osc133BCallback = Box<dyn Fn() + 'static>;
pub type Osc133CCallback = Box<dyn Fn() + 'static>;
pub type Osc133DCallback = Box<dyn Fn(Option<i32>) + 'static>;
pub type ContextMenuCallback = Box<dyn Fn(f64, f64) + 'static>;

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
    pub vadjustment: RefCell<Option<gtk4::Adjustment>>,
    pub vadjustment_handler: RefCell<Option<glib::SignalHandlerId>>,
    pub hadjustment: RefCell<Option<gtk4::Adjustment>>,
    pub hscroll_policy: Cell<gtk4::ScrollablePolicy>,
    pub vscroll_policy: Cell<gtk4::ScrollablePolicy>,
    pub match_rules: RefCell<Vec<MatchRule>>,
    pub next_match_tag: Cell<i32>,
    pub mouse_pos: Cell<Option<(f64, f64)>>,
    pub hovered_regex_match: Cell<Option<(usize, usize, usize)>>, // (row, start_col, end_col)
    pub title_callback: RefCell<Option<TitleCallback>>,
    pub cwd_callback: RefCell<Option<CwdCallback>>,
    pub progress_callback: RefCell<Option<ProgressCallback>>,
    pub bell_callback: RefCell<Option<BellCallback>>,
    pub exit_callback: RefCell<Option<ExitCallback>>,
    pub osc_133_a_callback: RefCell<Option<Osc133ACallback>>,
    pub osc_133_b_callback: RefCell<Option<Osc133BCallback>>,
    pub osc_133_c_callback: RefCell<Option<Osc133CCallback>>,
    pub osc_133_d_callback: RefCell<Option<Osc133DCallback>>,
    pub context_menu_callback: RefCell<Option<ContextMenuCallback>>,
    pub last_cwd: RefCell<Option<String>>,
    pub kitty_textures: RefCell<HashMap<u32, gtk4::gdk::Texture>>,
    pub is_dimmed: Cell<bool>,
    pub cached_search_regex: RefCell<Option<(String, bool, regex::Regex)>>,
    pub visible_search_matches: RefCell<Vec<crate::engine::selection::SelectionRange>>,
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
            hovered_regex_match: Cell::new(None),
            next_match_tag: Cell::new(1),
            title_callback: RefCell::new(None),
            cwd_callback: RefCell::new(None),
            progress_callback: RefCell::new(None),
            bell_callback: RefCell::new(None),
            exit_callback: RefCell::new(None),
            osc_133_a_callback: RefCell::new(None),
            osc_133_b_callback: RefCell::new(None),
            osc_133_c_callback: RefCell::new(None),
            osc_133_d_callback: RefCell::new(None),
            context_menu_callback: RefCell::new(None),
            last_cwd: RefCell::new(None),
            kitty_textures: RefCell::new(HashMap::new()),
            is_dimmed: Cell::new(false),
            cached_search_regex: RefCell::new(None),
            visible_search_matches: RefCell::new(Vec::new()),
        }
    }
}

const PROP_VADJUSTMENT: usize = 1;
const PROP_HADJUSTMENT: usize = 2;
const PROP_VSCROLL_POLICY: usize = 3;
const PROP_HSCROLL_POLICY: usize = 4;

#[glib::object_subclass]
impl ObjectSubclass for TerminalWidget {
    const NAME: &'static str = "BoxxyTerminalWidget";
    type Type = super::TerminalWidget;
    type ParentType = gtk4::Widget;
    type Interfaces = (gtk4::Scrollable,);
}

impl ScrollableImpl for TerminalWidget {}

impl ObjectImpl for TerminalWidget {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
        PROPS.get_or_init(|| {
            vec![
                glib::ParamSpecOverride::for_interface::<gtk4::Scrollable>("vadjustment"),
                glib::ParamSpecOverride::for_interface::<gtk4::Scrollable>("hadjustment"),
                glib::ParamSpecOverride::for_interface::<gtk4::Scrollable>("vscroll-policy"),
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

        let key_ctrl = gtk4::EventControllerKey::new();
        key_ctrl.connect_key_pressed(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _keycode, modifier| {
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
                    // App owns the mouse when it has enabled any mouse-reporting
                    // mode (1000/1002/1003) OR is running in the alt screen
                    // (TUI editors like Fresh/vim always use alt screen and
                    // mouse reporting together).  Shift overrides this so the
                    // user can always force a terminal selection or open the
                    // context menu regardless of what the app is doing.
                    let app_owns_mouse = state
                        .mode
                        .intersects(crate::engine::term::TermMode::MOUSE_MODE)
                        && !is_shift;

                    if app_owns_mouse {
                        // Clear any terminal selection the user may have made
                        // with Shift+drag — a plain click means "app takes over".
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
                            && let Some(seq) = state.calculate_navigation_sequence(point) {
                                // Clear any existing selection so there's no lingering highlight.
                                backend.set_selection(None);
                                backend
                                    .notifier
                                    .0
                                    .send(crate::engine::event_loop::Msg::Input(
                                        std::borrow::Cow::Owned(seq.into_bytes()),
                                    ))
                                    .ok();
                                return; // Don't start a selection — navigation is taking the click.
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
                        // No drag selection was made — this was a plain click.
                        // CLICK_REPORT (mode 2031): fish 4.1+ handles cursor repositioning natively.
                        // Send press+release in SGR format and let fish move its own cursor.
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
            (logical.width() as f64 / gtk4::pango::SCALE as f64) * self.cell_width_scale.get(),
            (logical.height() as f64 / gtk4::pango::SCALE as f64) * self.cell_height_scale.get(),
        )
    }

    fn setup_event_loop(&self, receiver: async_channel::Receiver<Event>) {
        let obj_weak = self.obj().downgrade();
        glib::spawn_future_local(async move {
            fn handle_title(widget: &super::TerminalWidget, title: String) {
                if let Some(f) = widget.imp().title_callback.borrow().as_ref() {
                    f(title.clone());
                }
                let new_cwd = widget.imp().backend.borrow().as_ref().and_then(|b| b.cwd());
                if let Some(cwd) = new_cwd {
                    let mut last = widget.imp().last_cwd.borrow_mut();
                    if last.as_deref() != Some(cwd.as_str()) {
                        *last = Some(cwd.clone());
                        drop(last);
                        if let Some(f) = widget.imp().cwd_callback.borrow().as_ref() {
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
                            if let Some(backend) = widget.imp().backend.borrow().as_ref() {
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
                                if let Some(f) = widget.imp().cwd_callback.borrow().as_ref() {
                                    f(cwd);
                                }
                            }
                        }
                        Event::ProgressChanged { state, progress } => {
                            if let Some(f) = widget.imp().progress_callback.borrow().as_ref() {
                                f(state, progress);
                            }
                        }
                        Event::Osc133A => {
                            if let Some(f) = widget.imp().osc_133_a_callback.borrow().as_ref() {
                                f();
                            }
                        }
                        Event::Osc133B => {
                            if let Some(f) = widget.imp().osc_133_b_callback.borrow().as_ref() {
                                f();
                            }
                        }
                        Event::Osc133C => {
                            if let Some(f) = widget.imp().osc_133_c_callback.borrow().as_ref() {
                                f();
                            }
                        }
                        Event::Osc133D(ec) => {
                            if let Some(f) = widget.imp().osc_133_d_callback.borrow().as_ref() {
                                f(ec);
                            }
                        }
                        Event::ResetTitle => {
                            handle_title(&widget, "Terminal".to_string());
                        }
                        Event::Bell => {
                            if let Some(f) = widget.imp().bell_callback.borrow().as_ref() {
                                f();
                            }
                        }
                        Event::ChildExit(code) => {
                            if let Some(f) = widget.imp().exit_callback.borrow().as_ref() {
                                f(code.code().unwrap_or(0));
                            }
                        }
                        Event::ClipboardStore(ty, text) => {
                            let cb = if ty == crate::engine::term::ClipboardType::Selection {
                                widget.display().primary_clipboard()
                            } else {
                                widget.clipboard()
                            };
                            cb.set_text(&text);
                        }
                        Event::ClipboardLoad(ty, formatter) => {
                            let cb = if ty == crate::engine::term::ClipboardType::Selection {
                                widget.display().primary_clipboard()
                            } else {
                                widget.clipboard()
                            };
                            let widget_weak = widget.downgrade();
                            glib::spawn_future_local(async move {
                                if let Ok(Some(text)) = cb.read_text_future().await
                                    && let Some(widget) = widget_weak.upgrade() {
                                        let response = formatter(&text);
                                        if let Some(backend) =
                                            widget.imp().backend.borrow().as_ref()
                                        {
                                            backend.write_to_pty(response.into_bytes());
                                        }
                                    }
                            });
                        }
                        Event::ColorRequest(index, formatter) => {
                            let imp = widget.imp();
                            let dfg = (*imp.fg_color.borrow())
                                .unwrap_or_else(|| gtk4::gdk::RGBA::new(0.8, 0.8, 0.8, 1.0));
                            let dbg = (*imp.bg_color.borrow())
                                .unwrap_or_else(|| gtk4::gdk::RGBA::new(0.05, 0.05, 0.05, 1.0));
                            let rgba = if index == 256 {
                                dfg
                            } else if index == 257 {
                                dbg
                            } else if index == 258 {
                                (*imp.cursor_color.borrow()).unwrap_or(dfg)
                            } else {
                                imp.palette.borrow().get(index).cloned().unwrap_or(dfg)
                            };
                            let rgb = crate::engine::ansi::Rgb {
                                r: (rgba.red() * 255.0).round() as u8,
                                g: (rgba.green() * 255.0).round() as u8,
                                b: (rgba.blue() * 255.0).round() as u8,
                            };
                            let resp = formatter(rgb);
                            if let Some(backend) = widget.imp().backend.borrow().as_ref() {
                                backend.write_to_pty(resp.into_bytes());
                            }
                        }
                        Event::TextAreaSizeRequest(formatter) => {
                            let cs = widget.imp().get_char_size(&widget);
                            let cols = (widget.width() as f64 / cs.0).floor() as u16;
                            let lines = (widget.height() as f64 / cs.1).floor() as u16;
                            let resp = formatter(crate::engine::event::WindowSize {
                                num_cols: cols,
                                num_lines: lines,
                                cell_width: cs.0 as u16,
                                cell_height: cs.1 as u16,
                                pixel_width: widget.width() as u16,
                                pixel_height: widget.height() as u16,
                            });
                            if let Some(backend) = widget.imp().backend.borrow().as_ref() {
                                backend.write_to_pty(resp.into_bytes());
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    pub fn set_dimmed(&self, dimmed: bool) {
        if self.is_dimmed.get() != dimmed {
            self.is_dimmed.set(dimmed);
            self.obj().queue_draw();
        }
    }

    pub(crate) fn attach_pty(&self, master_fd: zbus::zvariant::OwnedFd) {
        let (sender, receiver) = async_channel::unbounded::<Event>();
        let backend = TerminalBackend::from_fd(sender, master_fd.into());
        self.backend.replace(Some(backend));
        let (w, h) = (self.obj().width(), self.obj().height());
        if w > 0 && h > 0 {
            let cs = self.get_char_size(&self.obj());
            let pad = self.padding.get() as f64;
            let cols = ((w as f64 - 2.0 * pad) / cs.0).floor().max(1.0) as usize;
            let lines = ((h as f64 - 2.0 * pad) / cs.1).floor().max(1.0) as usize;
            if let Some(b) = self.backend.borrow().as_ref() {
                b.resize(cols, lines, cs.0, cs.1, w, h);
            }
        }
        self.setup_event_loop(receiver);
        self.obj().queue_draw();
    }

    pub(crate) fn spawn(&self, working_dir: Option<&str>, command: &[&str]) {
        let (sender, receiver) = async_channel::unbounded::<Event>();
        let mut opts = crate::engine::tty::Options::default();
        opts.env
            .insert("TERM".to_string(), "xterm-256color".to_string());
        opts.env
            .insert("COLORTERM".to_string(), "truecolor".to_string());
        if let Some(wd) = working_dir {
            opts.working_directory = Some(std::path::PathBuf::from(wd));
        }
        if !command.is_empty() {
            opts.shell = Some(crate::engine::tty::Shell::new(
                command[0].to_string(),
                command[1..].iter().map(|s| s.to_string()).collect(),
            ));
        }
        let backend = TerminalBackend::new(sender, opts);
        self.backend.replace(Some(backend));
        self.setup_event_loop(receiver);
        self.obj().queue_draw();
    }

    pub(crate) fn copy_clipboard(&self) {
        let backend_ref = self.backend.borrow();
        let Some(backend) = backend_ref.as_ref() else {
            return;
        };
        // Fast path: extract selection text synchronously from the render state.
        // This avoids an async round-trip through the event loop and works
        // reliably even in alt-screen apps like Fresh or vim.
        let state = backend.render_state.load();
        if let Some(text) = state.selection_text() {
            self.obj().clipboard().set_text(&text);
            return;
        }
        // Fallback for selections that span scrollback history (not in render state).
        drop(backend_ref);
        if let Some(backend) = self.backend.borrow().as_ref() {
            backend.copy_selection(crate::engine::term::ClipboardType::Clipboard);
        }
    }
    pub(crate) fn has_selection(&self) -> bool {
        self.backend
            .borrow()
            .as_ref()
            .map(|b| b.has_selection())
            .unwrap_or(false)
    }
    pub(crate) fn paste_clipboard(&self) {
        let cb = self.obj().clipboard();
        let ow = self.obj().downgrade();
        glib::spawn_future_local(async move {
            if let Ok(Some(text)) = cb.read_text_future().await
                && !text.is_empty()
                && let Some(w) = ow.upgrade()
                    && let Some(b) = w.imp().backend.borrow().as_ref()
                {
                    b.paste(text.to_string());
                }
        });
    }
    pub(crate) fn search(&self, dir: crate::engine::index::Direction) {
        if let Some(q) = self.search_query.borrow().clone()
            && let Some(b) = self.backend.borrow().as_ref()
        {
            b.search(q, dir, !self.search_case_sensitive.get());
        }
    }
}

impl WidgetImpl for TerminalWidget {
    fn measure(&self, _: gtk4::Orientation, _: i32) -> (i32, i32, i32, i32) {
        (100, 400, -1, -1)
    }
    fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
        self.parent_size_allocate(width, height, baseline);
        let cs = self.get_char_size(&self.obj());
        let pad = self.padding.get() as f64;
        if cs.0 > 0.0 && cs.1 > 0.0 {
            let cols = ((width as f64 - 2.0 * pad) / cs.0).floor().max(1.0) as usize;
            let lines = ((height as f64 - 2.0 * pad) / cs.1).floor().max(1.0) as usize;
            if let Some(b) = self.backend.borrow().as_ref() {
                b.resize(cols, lines, cs.0, cs.1, width, height);
            }
        }
        self.obj().update_scroll_adjustment();
    }

    fn snapshot(&self, snapshot: &gtk4::Snapshot) {
        let (width, height, padding) = (
            self.obj().width() as f32,
            self.obj().height() as f32,
            self.padding.get(),
        );
        let bg_color = (*self.bg_color.borrow())
            .unwrap_or_else(|| gtk4::gdk::RGBA::new(0.05, 0.05, 0.05, 1.0));
        let fg_color_default =
            (*self.fg_color.borrow()).unwrap_or_else(|| gtk4::gdk::RGBA::new(0.8, 0.8, 0.8, 1.0));

        let mut bg_to_paint = bg_color;
        let dim_factor = if self.is_dimmed.get() { 0.85 } else { 1.0 };

        if self.is_dimmed.get() {
            let base_alpha = bg_to_paint.alpha();
            bg_to_paint.set_red(bg_to_paint.red() * dim_factor);
            bg_to_paint.set_green(bg_to_paint.green() * dim_factor);
            bg_to_paint.set_blue(bg_to_paint.blue() * dim_factor);

            let dimmed_alpha = if base_alpha < 0.2 {
                base_alpha + 0.1
            } else {
                base_alpha
            };
            bg_to_paint.set_alpha(dimmed_alpha.min(1.0));
        }

        if bg_to_paint.alpha() > 0.0 {
            snapshot.append_color(
                &bg_to_paint,
                &gtk4::graphene::Rect::new(0.0, 0.0, width, height),
            );
        }

        if let Some(backend) = self.backend.borrow().as_ref() {
            let state = backend.render_state.load();
            snapshot.save();
            snapshot.translate(&gtk4::graphene::Point::new(padding, padding));
            let display_offset = state.display_offset;

            // ── Viewport Search Scan ───────────────────────────────────────────
            // We scan the visible viewport for ALL matches of the search query
            // so we can highlight them all (Ghostty style).
            if let Some(query) = self.search_query.borrow().as_ref() {
                let case_insensitive = !self.search_case_sensitive.get();

                // Get or compile regex
                let mut cached = self.cached_search_regex.borrow_mut();
                let re = if let Some((q, ci, re)) = cached.as_ref()
                    && q == query
                    && *ci == case_insensitive
                {
                    Some(re.clone())
                } else {
                    match regex::RegexBuilder::new(query)
                        .case_insensitive(case_insensitive)
                        .build()
                    {
                        Ok(re) => {
                            *cached = Some((query.clone(), case_insensitive, re.clone()));
                            Some(re)
                        }
                        Err(_) => None,
                    }
                };

                if let Some(re) = re {
                    let mut visible_matches = self.visible_search_matches.borrow_mut();
                    visible_matches.clear();

                    // 1. Extract visible text and map byte offsets to Points
                    let mut visible_text =
                        String::with_capacity(state.screen_lines * state.columns);
                    let mut byte_to_point = Vec::with_capacity(state.screen_lines * state.columns);

                    for row in 0..state.screen_lines {
                        for col in 0..state.columns {
                            let point = Point::new(Line(row as i32 - display_offset), Column(col));
                            let cell = state.cell(point);
                            let c = if cell.c == '\0' { ' ' } else { cell.c };

                            let start_byte = visible_text.len();
                            visible_text.push(c);
                            let end_byte = visible_text.len();

                            for _ in start_byte..end_byte {
                                byte_to_point.push(point);
                            }
                        }
                        let last_cell = state.cell(Point::new(
                            Line(row as i32 - display_offset),
                            Column(state.columns - 1),
                        ));
                        if !last_cell
                            .flags
                            .contains(crate::engine::term::cell::Flags::WRAPLINE)
                        {
                            visible_text.push('\n');
                            byte_to_point.push(Point::new(
                                Line(row as i32 - display_offset),
                                Column(state.columns - 1),
                            ));
                        }
                    }

                    // 2. Find all matches in the visible viewport
                    for m in re.find_iter(&visible_text) {
                        if let (Some(&start_point), Some(&end_point)) = (
                            byte_to_point.get(m.start()),
                            byte_to_point.get(m.end().saturating_sub(1)),
                        ) {
                            visible_matches.push(crate::engine::selection::SelectionRange {
                                start: start_point,
                                end: end_point,
                                is_block: false,
                            });
                        }
                    }
                }
            } else {
                self.visible_search_matches.borrow_mut().clear();
            }

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
            let cs = self.get_char_size(&self.obj());
            let (char_width, char_height) = (cs.0 as f32, cs.1 as f32);
            let mut offset_y = 0.0_f32;

            if (self.cell_width_scale.get() - 1.0).abs() > f64::EPSILON
                || (self.cell_height_scale.get() - 1.0).abs() > f64::EPSILON
            {
                layout.set_text("A");
                let (_, logical) = layout.extents();
                if (self.cell_width_scale.get() - 1.0).abs() > f64::EPSILON {
                    let diff =
                        (logical.width() as f64 * (self.cell_width_scale.get() - 1.0)) as i32;
                    let attr_list = gtk4::pango::AttrList::new();
                    attr_list.insert(gtk4::pango::AttrInt::new_letter_spacing(diff));
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
                        let tex = match &img.data {
                            crate::engine::kitty::KittyImageData::Dynamic(dyn_img) => {
                                let rgba = dyn_img.to_rgba8();
                                let (w, h) = (rgba.width() as i32, rgba.height() as i32);
                                gtk4::gdk::MemoryTexture::new(
                                    w,
                                    h,
                                    gtk4::gdk::MemoryFormat::R8g8b8a8,
                                    &glib::Bytes::from(&rgba.into_raw()),
                                    (w * 4) as usize,
                                )
                                .upcast::<gtk4::gdk::Texture>()
                            }
                            crate::engine::kitty::KittyImageData::RawRgb {
                                width,
                                height,
                                data,
                            } => {
                                let (w, h) = (*width as i32, *height as i32);
                                gtk4::gdk::MemoryTexture::new(
                                    w,
                                    h,
                                    gtk4::gdk::MemoryFormat::R8g8b8,
                                    &glib::Bytes::from_owned(data.clone()),
                                    (w * 3) as usize,
                                )
                                .upcast::<gtk4::gdk::Texture>()
                            }
                            crate::engine::kitty::KittyImageData::RawRgba {
                                width,
                                height,
                                data,
                            } => {
                                let (w, h) = (*width as i32, *height as i32);
                                gtk4::gdk::MemoryTexture::new(
                                    w,
                                    h,
                                    gtk4::gdk::MemoryFormat::R8g8b8a8,
                                    &glib::Bytes::from_owned(data.clone()),
                                    (w * 4) as usize,
                                )
                                .upcast::<gtk4::gdk::Texture>()
                            }
                        };
                        texture_cache.insert(placement.image_id, tex.clone());
                        Some(tex)
                    } else {
                        None
                    };
                    if let Some(tex) = texture {
                        let (row, col) = (
                            placement.point.line.0 + display_offset,
                            placement.point.column.0,
                        );
                        let scale = self.obj().scale_factor() as f32;
                        let approx_height_cells = placement
                            .height
                            .unwrap_or_else(|| {
                                if char_height > 0.0 {
                                    ((placement.visible_height as f32 / scale) / char_height).ceil()
                                        as u32
                                } else {
                                    1
                                }
                            })
                            .max(1);
                        if row >= -(approx_height_cells as i32) && row < state.screen_lines as i32 {
                            let (x, y) = (
                                col as f32 * char_width + padding,
                                row as f32 * char_height + padding,
                            );
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
                            snapshot.append_texture(
                                &tex,
                                &gtk4::graphene::Rect::new(x, y, target_width, target_height),
                            );
                        }
                    }
                }
            };
            draw_kitty_images(true);
            let hovered_uri = self.mouse_pos.get().and_then(|(mx, my)| {
                let hov_col = ((mx - padding as f64) / cs.0).floor() as usize;
                let hov_row = ((my - padding as f64) / cs.1).floor() as usize;
                if hov_row < state.screen_lines && hov_col < state.columns {
                    state
                        .cell(Point::new(
                            Line(hov_row as i32 - display_offset),
                            Column(hov_col),
                        ))
                        .hyperlink()
                        .map(|h| h.uri().to_string())
                } else {
                    None
                }
            });

            for row in 0..state.screen_lines as i32 {
                // 1. Background pass
                let mut current_bg: Option<gtk4::gdk::RGBA> = None;
                let mut start_col = 0.0_f32;
                for col in 0..state.columns {
                    let cell = state.cell(Point::new(Line(row - display_offset), Column(col)));
                    let mut bg = match cell.bg {
                        crate::engine::ansi::Color::Named(
                            crate::engine::ansi::NamedColor::Background,
                        ) => None,
                        crate::engine::ansi::Color::Named(named) if (named as usize) < 256 => {
                            palette.get(named as usize).cloned()
                        }
                        crate::engine::ansi::Color::Spec(rgb) => Some(gtk4::gdk::RGBA::new(
                            rgb.r as f32 / 255.0,
                            rgb.g as f32 / 255.0,
                            rgb.b as f32 / 255.0,
                            1.0,
                        )),
                        crate::engine::ansi::Color::Indexed(idx) => {
                            palette.get(idx as usize).cloned()
                        }
                        _ => None,
                    };

                    if let Some(ref mut bg_col) = bg {
                        bg_col.set_red(bg_col.red() * dim_factor);
                        bg_col.set_green(bg_col.green() * dim_factor);
                        bg_col.set_blue(bg_col.blue() * dim_factor);
                    }
                    if cell
                        .flags
                        .contains(crate::engine::term::cell::Flags::INVERSE)
                    {
                        let fg = match cell.fg {
                            crate::engine::ansi::Color::Named(named) if (named as usize) < 256 => {
                                let mut idx = named as usize;
                                if cell.flags.contains(crate::engine::term::cell::Flags::BOLD)
                                    && idx < 8
                                {
                                    idx += 8;
                                }
                                palette.get(idx).cloned().unwrap_or(fg_color_default)
                            }
                            crate::engine::ansi::Color::Spec(rgb) => gtk4::gdk::RGBA::new(
                                rgb.r as f32 / 255.0,
                                rgb.g as f32 / 255.0,
                                rgb.b as f32 / 255.0,
                                1.0,
                            ),
                            crate::engine::ansi::Color::Indexed(idx) => {
                                let mut i = idx as usize;
                                if cell.flags.contains(crate::engine::term::cell::Flags::BOLD)
                                    && i < 8
                                {
                                    i += 8;
                                }
                                palette.get(i).cloned().unwrap_or(fg_color_default)
                            }
                            _ => fg_color_default,
                        };
                        bg = Some(fg);
                    }
                    if bg != current_bg {
                        if let Some(ref bg_col) = current_bg {
                            snapshot.append_color(
                                bg_col,
                                &gtk4::graphene::Rect::new(
                                    start_col,
                                    row as f32 * char_height,
                                    (col as f32 * char_width) - start_col + 0.5,
                                    char_height + 0.5,
                                ),
                            );
                        }
                        current_bg = bg;
                        start_col = col as f32 * char_width;
                    }
                }
                if let Some(ref bg_col) = current_bg {
                    snapshot.append_color(
                        bg_col,
                        &gtk4::graphene::Rect::new(
                            start_col,
                            row as f32 * char_height,
                            (state.columns as f32 * char_width) - start_col + 0.5,
                            char_height + 0.5,
                        ),
                    );
                }

                // 1.5 Search Highlight background pass
                {
                    let visible_matches = self.visible_search_matches.borrow();
                    if !visible_matches.is_empty() {
                        let yellow = gtk4::gdk::RGBA::new(0.961, 0.761, 0.067, 1.0); // #F5C211
                        let orange = gtk4::gdk::RGBA::new(1.0, 0.471, 0.0, 1.0); // #FF7800

                        for m in visible_matches.iter() {
                            let grid_row = Line(row - display_offset);
                            if grid_row >= m.start.line && grid_row <= m.end.line {
                                let start_col = if grid_row == m.start.line {
                                    m.start.column.0
                                } else {
                                    0
                                };
                                let end_col = if grid_row == m.end.line {
                                    m.end.column.0
                                } else {
                                    state.columns - 1
                                };

                                let is_active = selection_range
                                    .map(|r| r.start == m.start && r.end == m.end)
                                    .unwrap_or(false);
                                let color = if is_active { &orange } else { &yellow };

                                snapshot.append_color(
                                    color,
                                    &gtk4::graphene::Rect::new(
                                        start_col as f32 * char_width,
                                        row as f32 * char_height,
                                        (end_col as i32 - start_col as i32 + 1) as f32 * char_width,
                                        char_height,
                                    ),
                                );
                            }
                        }
                    }
                }

                // 2. Text pass
                let mut current_fg = fg_color_default;
                let mut line_str = String::new();
                start_col = 0.0_f32;
                for col in 0..state.columns {
                    let point = Point::new(Line(row - display_offset), Column(col));
                    let cell = state.cell(point);
                    let mut fg = match cell.fg {
                        crate::engine::ansi::Color::Named(named) => {
                            let mut idx = named as usize;
                            if cell.flags.contains(crate::engine::term::cell::Flags::BOLD)
                                && idx < 8
                            {
                                idx += 8;
                            }
                            if idx < 256 {
                                palette.get(idx).cloned().unwrap_or(fg_color_default)
                            } else if idx == 256 {
                                fg_color_default
                            } else if (259..=266).contains(&idx) {
                                palette
                                    .get(idx - 259)
                                    .cloned()
                                    .map(|mut c| {
                                        c.set_alpha(c.alpha() * 0.5);
                                        c
                                    })
                                    .unwrap_or(fg_color_default)
                            } else {
                                fg_color_default
                            }
                        }
                        crate::engine::ansi::Color::Spec(rgb) => gtk4::gdk::RGBA::new(
                            rgb.r as f32 / 255.0,
                            rgb.g as f32 / 255.0,
                            rgb.b as f32 / 255.0,
                            1.0,
                        ),
                        crate::engine::ansi::Color::Indexed(mut idx) => {
                            if cell.flags.contains(crate::engine::term::cell::Flags::BOLD)
                                && idx < 8
                            {
                                idx += 8;
                            }
                            palette
                                .get(idx as usize)
                                .cloned()
                                .unwrap_or(fg_color_default)
                        }
                    };

                    // Search Highlight text color override
                    if !self.visible_search_matches.borrow().is_empty() {
                        let visible_matches = self.visible_search_matches.borrow();
                        if visible_matches.iter().any(|m| m.contains(point)) {
                            fg = gtk4::gdk::RGBA::new(0.118, 0.118, 0.180, 1.0);
                        }
                    }

                    if dim_factor < 1.0 {
                        fg.set_red(fg.red() * dim_factor);
                        fg.set_green(fg.green() * dim_factor);
                        fg.set_blue(fg.blue() * dim_factor);
                    }

                    if cell.flags.contains(crate::engine::term::cell::Flags::DIM) {
                        fg.set_red(fg.red() * 0.6);
                        fg.set_green(fg.green() * 0.6);
                        fg.set_blue(fg.blue() * 0.6);
                    }
                    if cell
                        .flags
                        .contains(crate::engine::term::cell::Flags::INVERSE)
                    {
                        fg = match cell.bg {
                            crate::engine::ansi::Color::Named(
                                crate::engine::ansi::NamedColor::Background,
                            ) => bg_color,
                            crate::engine::ansi::Color::Named(n) => {
                                palette.get(n as usize).cloned().unwrap_or(bg_color)
                            }
                            crate::engine::ansi::Color::Spec(rgb) => gtk4::gdk::RGBA::new(
                                rgb.r as f32 / 255.0,
                                rgb.g as f32 / 255.0,
                                rgb.b as f32 / 255.0,
                                1.0,
                            ),
                            crate::engine::ansi::Color::Indexed(idx) => {
                                palette.get(idx as usize).cloned().unwrap_or(bg_color)
                            }
                        };
                    }
                    let is_wide = cell
                        .flags
                        .contains(crate::engine::term::cell::Flags::WIDE_CHAR);
                    let is_spacer = cell.flags.intersects(
                        crate::engine::term::cell::Flags::WIDE_CHAR_SPACER
                            | crate::engine::term::cell::Flags::LEADING_WIDE_CHAR_SPACER,
                    );
                    let is_filling = is_cell_filling_char(cell.c);
                    if fg != current_fg || is_wide || is_spacer || is_filling {
                        if !line_str.is_empty() {
                            layout.set_text(&line_str);
                            snapshot.save();
                            snapshot.translate(&gtk4::graphene::Point::new(
                                start_col,
                                row as f32 * char_height + offset_y,
                            ));
                            snapshot.append_layout(&layout, &current_fg);
                            snapshot.restore();
                            line_str.clear();
                        }
                        current_fg = fg;
                    }
                    if !is_spacer {
                        if line_str.is_empty() {
                            start_col = col as f32 * char_width;
                        }
                        line_str.push(cell.c);
                        if let Some(zw) = cell.zerowidth() {
                            for &c in zw {
                                line_str.push(c);
                            }
                        }
                        if is_wide {
                            layout.set_text(&line_str);
                            let (_, logical) = layout.extents();
                            let actual_width = logical.width() as f32 / gtk4::pango::SCALE as f32;
                            let target_width = char_width * 2.0;
                            snapshot.save();
                            snapshot.translate(&gtk4::graphene::Point::new(
                                start_col,
                                row as f32 * char_height + offset_y,
                            ));
                            if (actual_width - target_width).abs() > 0.1 {
                                let s = target_width / actual_width;
                                snapshot.scale(s, s);
                                let ah = logical.height() as f32 / gtk4::pango::SCALE as f32;
                                let scaled_h = ah * s;
                                if scaled_h < char_height {
                                    snapshot.translate(&gtk4::graphene::Point::new(
                                        0.0,
                                        (char_height - scaled_h) / 2.0,
                                    ));
                                }
                            }
                            snapshot.append_layout(&layout, &current_fg);
                            snapshot.restore();
                            line_str.clear();
                        } else if is_filling {
                            // Scale box-drawing / block / powerline chars to fill the cell exactly,
                            // matching Ghostty's behaviour of cell-perfect glyph rendering.
                            layout.set_text(&line_str);
                            let (_, logical) = layout.extents();
                            let aw = (logical.width() as f32 / gtk4::pango::SCALE as f32).max(0.1);
                            let ah = (logical.height() as f32 / gtk4::pango::SCALE as f32).max(0.1);
                            let sx = char_width / aw;
                            let sy = char_height / ah;
                            snapshot.save();
                            snapshot.translate(&gtk4::graphene::Point::new(
                                start_col,
                                row as f32 * char_height,
                            ));
                            if (sx - 1.0).abs() > 0.02 || (sy - 1.0).abs() > 0.02 {
                                snapshot.scale(sx, sy);
                            }
                            snapshot.append_layout(&layout, &current_fg);
                            snapshot.restore();
                            line_str.clear();
                        }
                    }
                }
                if !line_str.is_empty() {
                    layout.set_text(&line_str);
                    snapshot.save();
                    snapshot.translate(&gtk4::graphene::Point::new(
                        start_col,
                        row as f32 * char_height + offset_y,
                    ));
                    snapshot.append_layout(&layout, &current_fg);
                    snapshot.restore();
                }
                for col in 0..state.columns {
                    let point = Point::new(Line(row - display_offset), Column(col));
                    if let Some(ref range) = selection_range
                        && range.contains(point)
                    {
                        // If we have search matches, we skip drawing the standard blue selection
                        // because we already drew solid orange/yellow backgrounds in Pass 1.5.
                        let is_search_match = !self.visible_search_matches.borrow().is_empty()
                            && self
                                .visible_search_matches
                                .borrow()
                                .iter()
                                .any(|m| m.contains(point));

                        if !is_search_match {
                            snapshot.append_color(
                                &gtk4::gdk::RGBA::new(0.2, 0.4, 0.6, 0.5),
                                &gtk4::graphene::Rect::new(
                                    col as f32 * char_width,
                                    row as f32 * char_height,
                                    char_width,
                                    char_height,
                                ),
                            );
                        }
                    }
                    if let Some(cell_uri) =
                        state.cell(point).hyperlink().map(|h| h.uri().to_string())
                        && hovered_uri.as_deref() == Some(cell_uri.as_str()) {
                            snapshot.append_color(
                                &gtk4::gdk::RGBA::new(0.35, 0.75, 1.0, 1.0),
                                &gtk4::graphene::Rect::new(
                                    col as f32 * char_width,
                                    row as f32 * char_height + char_height - 1.5,
                                    char_width,
                                    1.5,
                                ),
                            );
                        }

                    if let Some((r, start_c, end_c)) = self.hovered_regex_match.get()
                        && r == row as usize && col >= start_c && col < end_c {
                            snapshot.append_color(
                                &gtk4::gdk::RGBA::new(0.35, 0.75, 1.0, 1.0),
                                &gtk4::graphene::Rect::new(
                                    col as f32 * char_width,
                                    row as f32 * char_height + char_height - 1.5,
                                    char_width,
                                    1.5,
                                ),
                            );
                        }
                }
            }
            if self.show_grid.get() {
                let grid_color = gtk4::gdk::RGBA::new(1.0, 1.0, 1.0, 0.05);
                for r in 0..=state.screen_lines as i32 {
                    snapshot.append_color(
                        &grid_color,
                        &gtk4::graphene::Rect::new(
                            0.0,
                            r as f32 * char_height,
                            width - 2.0 * padding,
                            1.0,
                        ),
                    );
                }
                for c in 0..=state.columns {
                    snapshot.append_color(
                        &grid_color,
                        &gtk4::graphene::Rect::new(
                            c as f32 * char_width,
                            0.0,
                            1.0,
                            height - 2.0 * padding,
                        ),
                    );
                }
            }
            if self.cursor_visible.get() {
                let mut cp = state.cursor_point;
                let cr = cp.line.0 + display_offset;
                if cr >= 0 && cr < state.screen_lines as i32 {
                    let cell = state.cell(cp);
                    let mut wide = cell
                        .flags
                        .contains(crate::engine::term::cell::Flags::WIDE_CHAR);
                    if cell
                        .flags
                        .contains(crate::engine::term::cell::Flags::WIDE_CHAR_SPACER)
                        && cp.column.0 > 0
                    {
                        cp.column.0 -= 1;
                        wide = true;
                    }
                    let (cw, ch, cy) = match self.cursor_shape.get() {
                        crate::engine::ansi::CursorShape::Underline => (
                            if wide { char_width * 2.0 } else { char_width },
                            2.0_f32,
                            char_height - 2.0,
                        ),
                        crate::engine::ansi::CursorShape::Beam => (2.0_f32, char_height, 0.0_f32),
                        _ => (
                            if wide { char_width * 2.0 } else { char_width },
                            char_height,
                            0.0_f32,
                        ),
                    };
                    let cc = (*self.cursor_color.borrow()).unwrap_or(fg_color_default);
                    snapshot.append_color(
                        &cc,
                        &gtk4::graphene::Rect::new(
                            cp.column.0 as f32 * char_width,
                            cr as f32 * char_height + cy,
                            cw,
                            ch,
                        ),
                    );
                }
            }
            snapshot.restore();
            draw_kitty_images(false);
        }
    }
}
