use crate::engine::event::Event;
use crate::terminal::backend::TerminalBackend;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

mod gestures;
mod kitty;
mod painter;

// ─── Regex match rule ────────────────────────────────────────────────────────
pub struct MatchRule {
    pub id: i32,
    pub regex: regex::Regex,
    pub cursor_name: String,
}

#[derive(Default)]
pub struct SearchViewportCache {
    pub generation: u64,
    pub display_offset: i32,
    pub query: String,
    pub case_sensitive: bool,
    pub matches: Vec<crate::engine::selection::SelectionRange>,
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
    pub drag_start_pos: Cell<Option<(f64, f64)>>,
    pub last_selection_point:
        Cell<Option<(usize, crate::engine::index::Column, crate::engine::index::Side)>>,
    pub autoscroll_source_id: RefCell<Option<glib::SourceId>>,
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
    pub im_context: RefCell<gtk4::IMContext>,
    pub kitty_textures: RefCell<HashMap<u32, gtk4::gdk::Texture>>,
    pub is_dimmed: Cell<bool>,
    pub cached_search_regex: RefCell<Option<(String, bool, regex::Regex)>>,
    pub visible_search_matches: RefCell<Vec<crate::engine::selection::SelectionRange>>,
    pub search_viewport_cache: RefCell<SearchViewportCache>,
    pub cached_char_size: Cell<Option<(f64, f64)>>,
    pub cached_scale_factor: Cell<i32>,
    pub cached_layout: RefCell<Option<gtk4::pango::Layout>>,
    pub frosted_glass: Cell<bool>,
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
            drag_start_pos: Cell::new(None),
            last_selection_point: Cell::new(None),
            autoscroll_source_id: RefCell::new(None),
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
            im_context: RefCell::new(gtk4::IMMulticontext::new().upcast::<gtk4::IMContext>()),
            kitty_textures: RefCell::new(HashMap::new()),
            is_dimmed: Cell::new(false),
            cached_search_regex: RefCell::new(None),
            visible_search_matches: RefCell::new(Vec::new()),
            search_viewport_cache: RefCell::new(SearchViewportCache::default()),
            cached_char_size: Cell::new(None),
            cached_scale_factor: Cell::new(0),
            cached_layout: RefCell::new(None),
            frosted_glass: Cell::new(false),
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

        self.setup_controllers();
    }
}

impl TerminalWidget {
    pub(crate) fn invalidate_font_metrics(&self) {
        self.cached_char_size.set(None);
        self.cached_layout.replace(None);
    }

    pub(crate) fn get_or_create_layout(&self, widget: &super::TerminalWidget) -> gtk4::pango::Layout {
        let scale = widget.scale_factor();
        if self.cached_scale_factor.get() != scale {
            self.cached_scale_factor.set(scale);
            self.invalidate_font_metrics();
        }
        if let Some(layout) = self.cached_layout.borrow().as_ref() {
            return layout.clone();
        }
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
        self.cached_layout.replace(Some(layout.clone()));
        layout
    }

    pub(crate) fn get_char_size(&self, widget: &super::TerminalWidget) -> (f64, f64) {
        let scale = widget.scale_factor();
        if self.cached_scale_factor.get() != scale {
            self.cached_scale_factor.set(scale);
            self.invalidate_font_metrics();
        }
        if let Some(cs) = self.cached_char_size.get() {
            return cs;
        }
        let layout = self.get_or_create_layout(widget);
        layout.set_attributes(None);
        layout.set_text("A");
        let (_, logical) = layout.extents();
        let cs = (
            (logical.width() as f64 / gtk4::pango::SCALE as f64) * self.cell_width_scale.get(),
            (logical.height() as f64 / gtk4::pango::SCALE as f64) * self.cell_height_scale.get(),
        );
        self.cached_char_size.set(Some(cs));
        cs
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
                        Event::Exit => {
                            if let Some(f) = widget.imp().exit_callback.borrow().as_ref() {
                                f(0);
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
                                    && let Some(widget) = widget_weak.upgrade()
                                {
                                    let response = formatter(&text);
                                    if let Some(backend) = widget.imp().backend.borrow().as_ref() {
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
                } else {
                    break;
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

    pub(crate) fn detach_pty(&self) {
        self.backend.replace(None);
    }

    pub(crate) fn attach_pty(&self, master_fd: std::os::unix::io::OwnedFd) {
        self.attach_pty_with_replay(master_fd, Vec::new());
    }

    pub(crate) fn attach_pty_with_replay(
        &self,
        master_fd: std::os::unix::io::OwnedFd,
        replay_bytes: Vec<u8>,
    ) {
        self.detach_pty();
        let (sender, receiver) = async_channel::unbounded::<Event>();
        let backend = TerminalBackend::from_fd_with_replay(sender, master_fd, replay_bytes);
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
        let state = backend.render_state.load();
        if let Some(text) = state.selection_text() {
            self.obj().clipboard().set_text(&text);
        }
        backend.copy_selection(crate::engine::term::ClipboardType::Clipboard);
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.backend
            .borrow()
            .as_ref()
            .map(|b| b.has_selection())
            .unwrap_or(false)
    }

    pub(crate) fn export_ansi_snapshot(&self) -> Vec<u8> {
        let backend_ref = self.backend.borrow();
        let Some(backend) = backend_ref.as_ref() else {
            return Vec::new();
        };
        backend.render_state.load().export_ansi_snapshot()
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
        self.paint_snapshot(snapshot);
    }

    fn realize(&self) {
        self.parent_realize();
        let im = self.im_context.borrow();
        im.set_client_widget(Some(&*self.obj()));
        if self.obj().has_focus() {
            im.focus_in();
            drop(im);
            self.obj().start_cursor_blink();
        }
    }

    fn unrealize(&self) {
        self.stop_autoscroll();
        self.obj().stop_cursor_blink();
        self.im_context
            .borrow()
            .set_client_widget(None::<&gtk4::Widget>);
        self.parent_unrealize();
    }
}
