pub mod backend;
pub mod input;
mod imp;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use crate::engine::index::{Column, Line, Point};



pub use crate::engine::ansi::CursorShape;

glib::wrapper! {
    pub struct TerminalWidget(ObjectSubclass<imp::TerminalWidget>)
        @extends gtk4::Widget,
        // gtk::Scrollable tells ScrolledWindow to give us the full allocation
        // and drive our vadjustment/hadjustment properties directly, instead
        // of wrapping us in an invisible Viewport.
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget,
                    gtk4::Scrollable;
}

impl TerminalWidget {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn spawn_async(&self, working_dir: Option<&str>, command: &[&str]) {
        self.imp().spawn(working_dir, command);
    }

    pub fn attach_pty(&self, master_fd: zbus::zvariant::OwnedFd) {
        self.imp().attach_pty(master_fd);
    }

    pub fn copy_clipboard(&self) {
        self.imp().copy_clipboard();
    }

    pub fn paste_clipboard(&self) {
        self.imp().paste_clipboard();
    }

    pub fn has_selection(&self) -> bool {
        self.imp().has_selection()
    }

    pub fn select_all(&self) {
        if let Some(backend) = self.imp().backend.borrow().as_ref() {
            let state = backend.render_state.load();
            let (cols, lines) = (state.columns, state.screen_lines);
            let start = crate::engine::index::Point::new(crate::engine::index::Line(0), crate::engine::index::Column(0));
            let end = crate::engine::index::Point::new(crate::engine::index::Line(lines as i32 - 1), crate::engine::index::Column(cols - 1));
            
            let mut sel = crate::engine::selection::Selection::new(
                crate::engine::selection::SelectionType::Simple,
                start,
                crate::engine::index::Side::Left
            );
            sel.update(end, crate::engine::index::Side::Right);
            backend.set_selection(Some(sel));
            self.queue_draw();
        }
    }

    pub fn column_count(&self) -> usize {
        if let Some(backend) = self.imp().backend.borrow().as_ref() {
            backend.render_state.load().columns
        } else {
            80
        }
    }

    pub fn row_count(&self) -> usize {
        if let Some(backend) = self.imp().backend.borrow().as_ref() {
            backend.render_state.load().screen_lines
        } else {
            24
        }
    }

    // Stubs for VTE compatibility
    pub fn set_cursor_blink_mode(&self, mode: bool) {
        let imp = self.imp();
        imp.cursor_blinking.set(mode);
        if mode {
            self.start_cursor_blink();
        } else {
            self.stop_cursor_blink();
            imp.cursor_visible.set(true);
            self.queue_draw();
        }
    }

    pub(crate) fn start_cursor_blink(&self) {
        let imp = self.imp();
        self.stop_cursor_blink();
        
        // Immediately make the cursor visible when starting/resetting the blink timer
        imp.cursor_visible.set(true);
        self.queue_draw();

        if imp.cursor_blinking.get() {
            let obj = self.clone();
            let source_id = glib::timeout_add_local(std::time::Duration::from_millis(600), move || {
                let imp = obj.imp();
                imp.cursor_visible.set(!imp.cursor_visible.get());
                obj.queue_draw();
                glib::ControlFlow::Continue
            });
            imp.cursor_blink_id.replace(Some(source_id));
        }
    }

    pub(crate) fn stop_cursor_blink(&self) {
        let imp = self.imp();
        if let Some(source_id) = imp.cursor_blink_id.borrow_mut().take() {
            source_id.remove();
        }
    }
    pub fn set_mouse_autohide(&self, _autohide: bool) {}
    pub fn set_scroll_on_output(&self, _scroll: bool) {}
    pub fn set_scroll_on_keystroke(&self, _scroll: bool) {}
    pub fn set_enable_sixel(&self, _enable: bool) {}
    /// Enable OSC 8 hyperlink support and register the built-in URL / path
    /// matchers so that `check_match_at` works without any extra setup.
    pub fn set_allow_hyperlink(&self, allow: bool) {
        if !allow {
            return;
        }

        // Only register defaults once (guard via next_match_tag still being 1).
        let imp = self.imp();
        if imp.next_match_tag.get() > 1 {
            return;
        }

        // ── Built-in matchers (registered in priority order) ─────────────────

        // 1. HTTP / HTTPS URLs
        // Character class: anything that isn't whitespace or common URL
        // terminators.  Written with concat! to avoid raw-string/quote issues.
        self.add_match_regex_str(
            concat!(r"https?://[^\s)>\]", r#"'""#, r"]+"),
            0,
        );

        // 2. file:// URIs
        self.add_match_regex_str(
            concat!(r"file://[^\s)>\]", r#"'""#, r"]+"),
            0,
        );

        // 3. Bare absolute / relative / home-relative Unix paths
        self.add_match_regex_str(
            concat!(r"(?:~|\.{0,2})?/[^\s:,;)>\]|*?\\", r#"'""#, r"]+"),
            0,
        );
    }
    pub fn set_font(&self, font: Option<&gtk4::pango::FontDescription>) {
        let imp = self.imp();
        imp.font_desc.replace(font.cloned());
        self.queue_resize();
    }
    pub fn set_cell_height_scale(&self, scale: f64) {
        let imp = self.imp();
        if (imp.cell_height_scale.get() - scale).abs() > f64::EPSILON {
            imp.cell_height_scale.set(scale);
            self.queue_resize();
        }
    }
    
    pub fn set_cell_width_scale(&self, scale: f64) {
        let imp = self.imp();
        if (imp.cell_width_scale.get() - scale).abs() > f64::EPSILON {
            imp.cell_width_scale.set(scale);
            self.queue_resize();
        }
    }
    pub fn set_cursor_shape(&self, shape: crate::engine::ansi::CursorShape) {
        let imp = self.imp();
        imp.cursor_shape.set(shape);
        self.queue_draw();
    }
    pub fn set_padding(&self, padding: f32) {
        let imp = self.imp();
        if (imp.padding.get() - padding).abs() > f32::EPSILON {
            imp.padding.set(padding);
            self.queue_resize();
        }
    }

    pub fn set_show_grid(&self, show: bool) {
        let imp = self.imp();
        imp.show_grid.set(show);
        self.queue_draw();
    }

    pub fn set_invert_scroll(&self, invert: bool) {
        self.imp().invert_scroll.set(invert);
    }

    pub fn set_colors(&self, fg: Option<&gtk4::gdk::RGBA>, bg: Option<&gtk4::gdk::RGBA>, palette: &[&gtk4::gdk::RGBA]) {
        let imp = self.imp();
        imp.fg_color.replace(fg.copied());
        imp.bg_color.replace(bg.copied());
        
        // ── Pre-compute the full 256-color palette ───────────────────────────
        // We generate the entire XTerm-compatible color space ahead of time.
        // This allows the render loop to perform O(1) array lookups instead of
        // recalculating RGB values for every character cell in every frame.
        let mut full_palette: Vec<gtk4::gdk::RGBA> = Vec::with_capacity(256);
        
        // 0-15: Standard and high-intensity theme colors (ANSI 16)
        for i in 0..16 {
            if i < palette.len() {
                full_palette.push(*palette[i]);
            } else {
                full_palette.push(gtk4::gdk::RGBA::BLACK); // Fallback if theme is incomplete
            }
        }
        
        // 16-231: 6x6x6 RGB color cube (Extended ANSI)
        // These are mathematically derived colors used by many CLI tools.
        for i in 0..216 {
            let r = (i / 36) as f32 / 5.0;
            let g = ((i / 6) % 6) as f32 / 5.0;
            let b = (i % 6) as f32 / 5.0;
            full_palette.push(gtk4::gdk::RGBA::new(r, g, b, 1.0));
        }
        
        // 232-255: 24-step grayscale ramp
        // Fine-grained gray shades from near-black to near-white.
        for i in 0..24 {
            let v = (i as f32 * 10.0 + 8.0) / 255.0;
            full_palette.push(gtk4::gdk::RGBA::new(v, v, v, 1.0));
        }

        imp.palette.replace(full_palette);
        self.queue_draw();
    }
    pub fn set_color_cursor(&self, color: Option<&gtk4::gdk::RGBA>) {
        let imp = self.imp();
        imp.cursor_color.replace(color.copied());
        self.queue_draw();
    }
    // ── OSC 8 hyperlink detection ─────────────────────────────────────────────

    /// Return the OSC 8 URI attached to the terminal cell at pixel position
    /// `(x, y)`, or `None` if the cell carries no hyperlink.
    pub fn check_hyperlink_at(&self, x: f64, y: f64) -> Option<String> {
        let imp = self.imp();
        let backend_ref = imp.backend.borrow();
        let backend = backend_ref.as_ref()?;
        let state = backend.render_state.load();

        let char_size = imp.get_char_size(self);
        let padding = imp.padding.get() as f64;
        let col = ((x - padding) / char_size.0).floor() as usize;
        let row = ((y - padding) / char_size.1).floor() as usize;

        if row >= state.screen_lines || col >= state.columns {
            return None;
        }

        let display_offset = state.display_offset;
        let point = Point::new(Line(row as i32 - display_offset), Column(col));
        state.cell(point)
            .hyperlink()
            .map(|h| h.uri().to_string())
    }

    // ── Regex match detection ─────────────────────────────────────────────────

    /// Return the regex-matched text at pixel position `(x, y)` together with
    /// the tag ID of the rule that matched, or `(None, 0)` if no rule matched.
    ///
    /// Matching is performed against the full text of the visible grid row.
    /// For ASCII-dominant content (URLs, file paths) the column-to-byte
    /// mapping is 1-to-1.
    pub fn check_match_at(&self, x: f64, y: f64) -> (Option<String>, i32) {
        let imp = self.imp();
        let backend_ref = imp.backend.borrow();
        let Some(backend) = backend_ref.as_ref() else { return (None, 0); };
        let state = backend.render_state.load();

        let char_size = imp.get_char_size(self);
        let padding = imp.padding.get() as f64;
        let col = ((x - padding) / char_size.0).floor() as usize;
        let row = ((y - padding) / char_size.1).floor() as usize;

        if row >= state.screen_lines || col >= state.columns {
            return (None, 0);
        }

        let display_offset = state.display_offset;
        let grid_line = Line(row as i32 - display_offset);

        // Build a String from the grid row while tracking which byte offset
        // corresponds to each column.  Wide-char spacer cells emit a space so
        // that the byte→column mapping stays 1-to-1 for ASCII content.
        let mut line_text = String::with_capacity(state.columns);
        let mut col_byte_offsets: Vec<usize> = Vec::with_capacity(state.columns + 1);

        for c in 0..state.columns {
            col_byte_offsets.push(line_text.len());
            let ch = state.cell(Point::new(grid_line, Column(c))).c;
            // Null / wide-char spacer cells → space, keeps offset table aligned
            line_text.push(if ch == '\0' { ' ' } else { ch });
        }
        col_byte_offsets.push(line_text.len()); // sentinel

        let cursor_byte = col_byte_offsets.get(col).copied().unwrap_or(line_text.len());

        let rules = imp.match_rules.borrow();
        for rule in rules.iter() {
            for m in rule.regex.find_iter(&line_text) {
                if cursor_byte >= m.start() && cursor_byte < m.end() {
                    // Trim trailing whitespace that crept in from spacer cells.
                    return (Some(m.as_str().trim_end().to_string()), rule.id);
                }
            }
        }

        (None, 0)
    }

    // ── Regex rule registration ───────────────────────────────────────────────

    /// Register a new regex pattern and return its tag ID (≥ 1).
    ///
    /// `flags` is a bitmask compatible with PCRE2 flags used by the VTE shim:
    ///   - bit 3 (`0x08`) → case-insensitive (`(?i)` prefix).
    ///
    /// Returns `0` if the pattern fails to compile.
    pub fn add_match_regex_str(&self, pattern: &str, flags: u32) -> i32 {
        let imp = self.imp();

        let case_insensitive = (flags & 0x08) != 0;
        let full_pattern = if case_insensitive {
            format!("(?i){}", pattern)
        } else {
            pattern.to_string()
        };

        match regex::Regex::new(&full_pattern) {
            Ok(re) => {
                let tag = imp.next_match_tag.get();
                imp.next_match_tag.set(tag + 1);
                imp.match_rules.borrow_mut().push(imp::MatchRule {
                    id: tag,
                    regex: re,
                    cursor_name: String::from("pointer"),
                });
                tag
            }
            Err(e) => {
                log::warn!("add_match_regex_str: failed to compile {:?}: {}", pattern, e);
                0
            }
        }
    }

    /// Update the cursor name shown for an existing match tag.
    pub fn match_set_cursor_name(&self, tag: i32, name: &str) {
        let imp = self.imp();
        let mut rules = imp.match_rules.borrow_mut();
        if let Some(rule) = rules.iter_mut().find(|r| r.id == tag) {
            rule.cursor_name = name.to_string();
        }
    }

    /// VTE-compat stub — VTE `Regex` objects cannot be used without `vte4`.
    /// Use `add_match_regex_str` instead.
    pub fn match_add_regex(&self, _regex: &glib::Object, _flags: u32) -> i32 { 0 }

    // ── Terminal event callbacks ──────────────────────────────────────────────

    /// Register a callback that is invoked on the GTK main loop whenever the
    /// shell sets a new window title (OSC 0 / OSC 2).
    /// Only one callback is stored; calling this again replaces the previous one.
    pub fn on_title_changed<F: Fn(String) + 'static>(&self, f: F) {
        self.imp().title_callback.replace(Some(Box::new(f)));
    }

    /// Register a callback that is invoked when the child process's current
    /// working directory changes.
    ///
    /// Detection works by reading `/proc/{child_pid}/cwd` each time the shell
    /// emits a title event.  This is accurate on native Linux; on Flatpak the
    /// child is `host-spawn`, whose CWD does not follow `cd` commands inside
    /// the host shell, so callers should skip this on Flatpak builds.
    pub fn on_cwd_changed<F: Fn(String) + 'static>(&self, f: F) {
        self.imp().cwd_callback.replace(Some(Box::new(f)));
    }

    /// Register a callback that is invoked whenever the terminal bell fires.
    pub fn on_bell<F: Fn() + 'static>(&self, f: F) {
        self.imp().bell_callback.replace(Some(Box::new(f)));
    }

    /// Register a callback that is invoked when the child shell process exits.
    /// The argument is the exit status code reported by the process.
    pub fn on_exit<F: Fn(i32) + 'static>(&self, f: F) {
        self.imp().exit_callback.replace(Some(Box::new(f)));
    }

    pub fn on_osc_133_a<F: Fn() + 'static>(&self, f: F) {
        self.imp().osc_133_a_callback.replace(Some(Box::new(f)));
    }

    pub fn on_osc_133_b<F: Fn() + 'static>(&self, f: F) {
        self.imp().osc_133_b_callback.replace(Some(Box::new(f)));
    }

    pub fn on_osc_133_c<F: Fn() + 'static>(&self, f: F) {
        self.imp().osc_133_c_callback.replace(Some(Box::new(f)));
    }

    pub fn on_osc_133_d<F: Fn(Option<i32>) + 'static>(&self, f: F) {
        self.imp().osc_133_d_callback.replace(Some(Box::new(f)));
    }

    pub fn on_claw_query<F: Fn(String) + 'static>(&self, f: F) {
        self.imp().claw_query_callback.replace(Some(Box::new(f)));
    }

    /// Return the current working directory of the child process by reading
    /// `/proc/{pid}/cwd`, or `None` if unavailable (Flatpak, process already
    /// exited, or `/proc` not mounted).
    pub fn cwd(&self) -> Option<String> {
        self.imp().backend.borrow().as_ref().and_then(|b| b.cwd())
    }

    pub fn paste_primary(&self) {
        let clipboard = self.display().primary_clipboard();
        let obj_weak = self.downgrade();
        glib::spawn_future_local(async move {
            match clipboard.read_text_future().await {
                Ok(Some(text)) if !text.is_empty() => {
                    log::info!("Terminal: Pasting text from PRIMARY, len={}", text.len());
                    if let Some(widget) = obj_weak.upgrade()
                        && let Some(backend) = widget.imp().backend.borrow().as_ref() {
                            backend.write_to_pty(text.as_str().as_bytes().to_vec());
                        }
                }
                _ => {
                    log::warn!("Terminal: PRIMARY paste failed or empty");
                }
            }
        });
    }

    /// Write raw bytes directly into the PTY (injects input into the terminal).
    pub fn write_all(&self, bytes: Vec<u8>) {
        if let Some(backend) = self.imp().backend.borrow().as_ref() {
            backend.write_to_pty(bytes);
        }
    }

    pub fn search_set_regex(&self, regex: Option<&str>, flags: u32) {
        let imp = self.imp();
        if let Some(r) = regex {
            let query = r.to_string();
            let case_insensitive = (flags & 0x00000008) != 0; // PCRE2_CASELESS
            imp.search_query.replace(Some(query));
            imp.search_case_sensitive.set(!case_insensitive);
        } else {
            imp.search_query.replace(None);
            if let Some(backend) = imp.backend.borrow().as_ref() {
                backend.clear_selection();
            }
            self.queue_draw();
        }
    }

    pub fn search_set_wrap_around(&self, wrap: bool) {
        self.imp().search_wrap_around.set(wrap);
    }

    pub fn search_find_next(&self) {
        self.imp().search(crate::engine::index::Direction::Right);
    }

    pub fn search_find_previous(&self) {
        self.imp().search(crate::engine::index::Direction::Left);
    }

    pub async fn get_text_snapshot(&self, max_lines: usize) -> Option<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let sent = if let Some(backend) = self.imp().backend.borrow().as_ref() {
            backend.notifier.0.send(crate::engine::event_loop::Msg::GetTextSnapshot(max_lines, tx)).is_ok()
        } else {
            false
        };
        
        if sent {
            rx.await.ok()
        } else {
            None
        }
    }

    pub fn set_vadjustment(&self, adjustment: Option<&gtk4::Adjustment>) {
        let imp = self.imp();

        // Disconnect the old handler before swapping the adjustment out.
        if let Some(old_adj) = imp.vadjustment.borrow().as_ref()
            && let Some(handler_id) = imp.vadjustment_handler.borrow_mut().take() {
                old_adj.disconnect(handler_id);
            }

        imp.vadjustment.replace(adjustment.cloned());

        if let Some(adj) = adjustment {
            let obj = self.clone();
            let handler_id = adj.connect_value_changed(move |adj| {
                let imp = obj.imp();
                if let Some(backend) = imp.backend.borrow().as_ref() {
                    let state = backend.render_state.load();

                    // Display offset: 0 = bottom (newest), increases = older.
                    // GTK adjustment value: 0 = top of range, upper-page_size = bottom.
                    // We map: GTK value → display_offset = max_value - gtk_value
                    let total_lines = state.total_lines as f64;
                    let screen_lines = state.screen_lines as f64;
                    let max_value = (total_lines - screen_lines).max(0.0);
                    let target_offset = (max_value - adj.value()).round() as usize;

                    let current_offset = state.display_offset as usize;
                    if current_offset != target_offset {
                        let delta = target_offset as i32 - current_offset as i32;
                        backend.scroll_display(crate::engine::grid::Scroll::Delta(delta));
                    }
                }
            });
            imp.vadjustment_handler.replace(Some(handler_id));
        }

        // Notify GObject listeners (e.g. ScrolledWindow) that the property changed.
        self.notify("vadjustment");
        self.update_scroll_adjustment();
    }

    pub(crate) fn update_scroll_adjustment(&self) {
        let imp = self.imp();
        if let Some(adj) = imp.vadjustment.borrow().as_ref()
            && let Some(backend) = imp.backend.borrow().as_ref() {
                let state = backend.render_state.load();
                
                let total_lines = state.total_lines as f64;
                let screen_lines = state.screen_lines as f64;
                let display_offset = state.display_offset as f64;
                
                let value = (total_lines - screen_lines - display_offset).max(0.0);
                
                // Block signal handler while updating to prevent feedback loop
                if let Some(handler_id) = imp.vadjustment_handler.borrow().as_ref() {
                    adj.block_signal(handler_id);
                    adj.configure(
                        value,
                        0.0,
                        total_lines.max(screen_lines),
                        1.0,
                        screen_lines,
                        screen_lines,
                    );
                    adj.unblock_signal(handler_id);
                }
            }
    }
}

impl Default for TerminalWidget {
    fn default() -> Self {
        Self::new()
    }
}