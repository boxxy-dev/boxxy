use crate::engine::index::{Column, Line, Point};
use crate::engine::term::cell::Flags;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

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

impl super::TerminalWidget {
    pub(super) fn paint_snapshot(&self, snapshot: &gtk4::Snapshot) {
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

        let effective_alpha = bg_to_paint.alpha();
        if self.frosted_glass.get() {
            // When frosted glass is active, the window-level FrostedGlassContainer handles
            // the Wayland blur node, window corner radii, and background tint across both the
            // headerbar and the terminal. We keep terminal widget background transparent
            // to prevent double-tinting and sharp rectangular corner clipping.
        } else if effective_alpha > 0.0 {
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
                        if !last_cell.flags.contains(Flags::WRAPLINE) {
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

            const STYLE_FLAGS: Flags = Flags::from_bits_truncate(
                Flags::BOLD.bits()
                    | Flags::ITALIC.bits()
                    | Flags::UNDERLINE.bits()
                    | Flags::STRIKEOUT.bits()
                    | Flags::HIDDEN.bits()
                    | Flags::DOUBLE_UNDERLINE.bits()
                    | Flags::UNDERCURL.bits()
                    | Flags::DOTTED_UNDERLINE.bits()
                    | Flags::DASHED_UNDERLINE.bits(),
            );

            let palette = self.palette.borrow();
            let apply_style = |layout: &gtk4::pango::Layout,
                               flags: Flags,
                               ucolor: Option<crate::engine::ansi::Color>,
                               palette: &[gtk4::gdk::RGBA],
                               default_fg: gtk4::gdk::RGBA| {
                let attr_list = gtk4::pango::AttrList::new();
                if flags.contains(Flags::BOLD) {
                    attr_list.insert(gtk4::pango::AttrInt::new_weight(gtk4::pango::Weight::Bold));
                }
                if flags.contains(Flags::ITALIC) {
                    attr_list.insert(gtk4::pango::AttrInt::new_style(gtk4::pango::Style::Italic));
                }
                if flags.contains(Flags::STRIKEOUT) {
                    attr_list.insert(gtk4::pango::AttrInt::new_strikethrough(true));
                }

                let underline = if flags.contains(Flags::DOUBLE_UNDERLINE) {
                    gtk4::pango::Underline::Double
                } else if flags.contains(Flags::UNDERCURL) {
                    gtk4::pango::Underline::Error
                } else if flags.contains(Flags::DOTTED_UNDERLINE) {
                    gtk4::pango::Underline::Error
                } else if flags.contains(Flags::DASHED_UNDERLINE) {
                    gtk4::pango::Underline::Single
                } else if flags.contains(Flags::UNDERLINE) {
                    gtk4::pango::Underline::Single
                } else {
                    gtk4::pango::Underline::None
                };

                if underline != gtk4::pango::Underline::None {
                    attr_list.insert(gtk4::pango::AttrInt::new_underline(underline));
                    if let Some(color) = ucolor {
                        let rgba = match color {
                            crate::engine::ansi::Color::Spec(rgb) => gtk4::gdk::RGBA::new(
                                rgb.r as f32 / 255.0,
                                rgb.g as f32 / 255.0,
                                rgb.b as f32 / 255.0,
                                1.0,
                            ),
                            crate::engine::ansi::Color::Indexed(idx) => {
                                palette.get(idx as usize).cloned().unwrap_or(default_fg)
                            }
                            crate::engine::ansi::Color::Named(named) => {
                                palette.get(named as usize).cloned().unwrap_or(default_fg)
                            }
                        };
                        attr_list.insert(gtk4::pango::AttrColor::new_underline_color(
                            (rgba.red() * 65535.0) as u16,
                            (rgba.green() * 65535.0) as u16,
                            (rgba.blue() * 65535.0) as u16,
                        ));
                    }
                }
                layout.set_attributes(Some(&attr_list));
            };

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
            self.draw_kitty_images(
                snapshot,
                &state,
                true,
                display_offset,
                char_width,
                char_height,
                padding,
            );
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
                    if cell.flags.contains(Flags::INVERSE) {
                        let fg = match cell.fg {
                            crate::engine::ansi::Color::Named(named) if (named as usize) < 256 => {
                                let mut idx = named as usize;
                                if cell.flags.contains(Flags::BOLD) && idx < 8 {
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
                                if cell.flags.contains(Flags::BOLD) && i < 8 {
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
                let mut current_flags = Flags::empty();
                let mut current_ucolor: Option<crate::engine::ansi::Color> = None;
                let mut line_str = String::new();
                start_col = 0.0_f32;
                for col in 0..state.columns {
                    let point = Point::new(Line(row - display_offset), Column(col));
                    let cell = state.cell(point);
                    let mut fg = match cell.fg {
                        crate::engine::ansi::Color::Named(named) => {
                            let mut idx = named as usize;
                            if cell.flags.contains(Flags::BOLD) && idx < 8 {
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
                            if cell.flags.contains(Flags::BOLD) && idx < 8 {
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

                    if cell.flags.contains(Flags::DIM) {
                        fg.set_red(fg.red() * 0.6);
                        fg.set_green(fg.green() * 0.6);
                        fg.set_blue(fg.blue() * 0.6);
                    }
                    if cell.flags.contains(Flags::INVERSE) {
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
                    let is_wide = cell.flags.contains(Flags::WIDE_CHAR);
                    let is_spacer = cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
                    let is_filling = is_cell_filling_char(cell.c);

                    let style_flags = cell.flags & STYLE_FLAGS;
                    let ucolor = cell.underline_color();

                    if fg != current_fg
                        || style_flags != current_flags
                        || ucolor != current_ucolor
                        || is_wide
                        || is_spacer
                        || is_filling
                    {
                        if !line_str.is_empty() {
                            apply_style(
                                &layout,
                                current_flags,
                                current_ucolor,
                                &palette,
                                fg_color_default,
                            );
                            layout.set_text(&line_str);
                            if !current_flags.contains(Flags::HIDDEN) {
                                snapshot.save();
                                snapshot.translate(&gtk4::graphene::Point::new(
                                    start_col,
                                    row as f32 * char_height + offset_y,
                                                                ));
                                snapshot.append_layout(&layout, &current_fg);
                                snapshot.restore();
                            }
                            line_str.clear();
                        }
                        current_fg = fg;
                        current_flags = style_flags;
                        current_ucolor = ucolor;
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
                            apply_style(
                                &layout,
                                current_flags,
                                current_ucolor,
                                &palette,
                                fg_color_default,
                            );
                            layout.set_text(&line_str);
                            let (_, logical) = layout.extents();
                            let actual_width = logical.width() as f32 / gtk4::pango::SCALE as f32;
                            let target_width = char_width * 2.0;
                            if !current_flags.contains(Flags::HIDDEN) {
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
                            }
                            line_str.clear();
                        } else if is_filling {
                            apply_style(
                                &layout,
                                current_flags,
                                current_ucolor,
                                &palette,
                                fg_color_default,
                            );
                            layout.set_text(&line_str);
                            let (_, logical) = layout.extents();
                            let aw = (logical.width() as f32 / gtk4::pango::SCALE as f32).max(0.1);
                            let ah = (logical.height() as f32 / gtk4::pango::SCALE as f32).max(0.1);
                            let sx = char_width / aw;
                            let sy = char_height / ah;
                            if !current_flags.contains(Flags::HIDDEN) {
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
                            }
                            line_str.clear();
                        }
                    }
                }
                if !line_str.is_empty() {
                    apply_style(
                        &layout,
                        current_flags,
                        current_ucolor,
                        &palette,
                        fg_color_default,
                    );
                    layout.set_text(&line_str);
                    if !current_flags.contains(Flags::HIDDEN) {
                        snapshot.save();
                        snapshot.translate(&gtk4::graphene::Point::new(
                            start_col,
                            row as f32 * char_height + offset_y,
                        ));
                        snapshot.append_layout(&layout, &current_fg);
                        snapshot.restore();
                    }
                }
                for col in 0..state.columns {
                    let point = Point::new(Line(row - display_offset), Column(col));
                    if let Some(ref range) = selection_range
                        && range.contains(point)
                    {
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
                        && hovered_uri.as_deref() == Some(cell_uri.as_str())
                    {
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
                        && r == row as usize
                        && col >= start_c
                        && col < end_c
                    {
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
                    let mut wide = cell.flags.contains(Flags::WIDE_CHAR);
                    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) && cp.column.0 > 0 {
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
            self.draw_kitty_images(
                snapshot,
                &state,
                false,
                display_offset,
                char_width,
                char_height,
                padding,
            );
        }
    }
}
