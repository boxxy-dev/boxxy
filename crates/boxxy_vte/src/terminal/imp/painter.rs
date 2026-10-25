use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::engine::grid::GridCell;
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

#[allow(clippy::too_many_arguments)]
fn paint_underline_span(
    snapshot: &gtk4::Snapshot,
    flags: Flags,
    ucolor: Option<crate::engine::ansi::Color>,
    fg_color: gtk4::gdk::RGBA,
    palette: &[gtk4::gdk::RGBA],
    default_fg: gtk4::gdk::RGBA,
    dim_factor: f32,
    start_x: f32,
    end_x: f32,
    char_width: f32,
    char_height: f32,
) {
    let width = end_x - start_x;
    if width <= 0.0 || !flags.intersects(Flags::ALL_UNDERLINES) {
        return;
    }

    let mut ul_rgba = if let Some(color) = ucolor {
        match color {
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
        }
    } else {
        fg_color
    };

    if ucolor.is_some() && dim_factor < 1.0 {
        ul_rgba.set_red(ul_rgba.red() * dim_factor);
        ul_rgba.set_green(ul_rgba.green() * dim_factor);
        ul_rgba.set_blue(ul_rgba.blue() * dim_factor);
    }

    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        let y_top = (char_height - 3.2).max(0.0);
        let y_bot = (char_height - 1.2).max(0.0);
        snapshot.append_color(
            &ul_rgba,
            &gtk4::graphene::Rect::new(start_x, y_top, width, 1.0),
        );
        snapshot.append_color(
            &ul_rgba,
            &gtk4::graphene::Rect::new(start_x, y_bot, width, 1.0),
        );
    } else if flags.contains(Flags::UNDERCURL) {
        let pb = gtk4::gsk::PathBuilder::new();
        let y_mid = (char_height - 2.0).max(1.5);
        let amp = 1.35_f32;
        let half_wave = (char_width * 0.5).max(2.5);
        pb.move_to(start_x, y_mid);
        let mut x = start_x;
        let mut dir = 1.0_f32;
        while x < end_x - 0.1 {
            let next_x = (x + half_wave).min(end_x);
            let mid_x = (x + next_x) * 0.5;
            pb.quad_to(mid_x, y_mid + dir * amp, next_x, y_mid);
            x = next_x;
            dir = -dir;
        }
        let stroke = gtk4::gsk::Stroke::new(1.15);
        snapshot.append_stroke(&pb.to_path(), &stroke, &ul_rgba);
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        let pb = gtk4::gsk::PathBuilder::new();
        let y = (char_height - 1.5).max(0.5);
        pb.move_to(start_x, y);
        pb.line_to(end_x, y);
        let stroke = gtk4::gsk::Stroke::new(1.3);
        stroke.set_dash(&[1.5, 2.0]);
        snapshot.append_stroke(&pb.to_path(), &stroke, &ul_rgba);
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        let pb = gtk4::gsk::PathBuilder::new();
        let y = (char_height - 1.5).max(0.5);
        pb.move_to(start_x, y);
        pb.line_to(end_x, y);
        let stroke = gtk4::gsk::Stroke::new(1.3);
        stroke.set_dash(&[4.0, 2.5]);
        snapshot.append_stroke(&pb.to_path(), &stroke, &ul_rgba);
    } else if flags.contains(Flags::UNDERLINE) {
        let y = (char_height - 1.5).max(0.0);
        snapshot.append_color(
            &ul_rgba,
            &gtk4::graphene::Rect::new(start_x, y, width, 1.25),
        );
    }
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

            // ── Viewport Search Scan (Cached by Generation & Offset) ───────────
            if let Some(query) = self.search_query.borrow().as_ref() {
                let case_sensitive = self.search_case_sensitive.get();
                let case_insensitive = !case_sensitive;

                let needs_rescan = {
                    let cache = self.search_viewport_cache.borrow();
                    cache.generation != state.generation
                        || cache.display_offset != display_offset
                        || cache.query != *query
                        || cache.case_sensitive != case_sensitive
                };

                if needs_rescan {
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
                    drop(cached);

                    let mut visible_matches = self.visible_search_matches.borrow_mut();
                    visible_matches.clear();

                    if let Some(re) = re {
                        let mut visible_text =
                            String::with_capacity(state.screen_lines * state.columns);
                        let mut byte_to_point =
                            Vec::with_capacity(state.screen_lines * state.columns);

                        for row in 0..state.screen_lines {
                            for col in 0..state.columns {
                                let point =
                                    Point::new(Line(row as i32 - display_offset), Column(col));
                                let cell = state.cell(point);
                                if cell.flags.intersects(
                                    Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER,
                                ) {
                                    continue;
                                }
                                let c = if cell.c == '\0' { ' ' } else { cell.c };

                                let start_byte = visible_text.len();
                                visible_text.push(c);
                                if let Some(zw) = cell.zerowidth() {
                                    for &zw_c in zw {
                                        visible_text.push(zw_c);
                                    }
                                }
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

                        for m in re.find_iter(&visible_text) {
                            if let (Some(&start_point), Some(&(mut end_point))) = (
                                byte_to_point.get(m.start()),
                                byte_to_point.get(m.end().saturating_sub(1)),
                            ) {
                                // Expand end_point to include the trailing spacer column when a
                                // match ends on a 2-column wide character or emoji/flag cluster.
                                if state.cell(end_point).flags.contains(Flags::WIDE_CHAR)
                                    && end_point.column.0 + 1 < state.columns
                                {
                                    end_point.column.0 += 1;
                                }
                                visible_matches.push(crate::engine::selection::SelectionRange {
                                    start: start_point,
                                    end: end_point,
                                    is_block: false,
                                });
                            }
                        }
                    }

                    let mut cache = self.search_viewport_cache.borrow_mut();
                    cache.generation = state.generation;
                    cache.display_offset = display_offset;
                    cache.query.clone_from(query);
                    cache.case_sensitive = case_sensitive;
                    cache.matches.clone_from(&visible_matches);
                }
            } else {
                self.visible_search_matches.borrow_mut().clear();
                self.search_viewport_cache.borrow_mut().query.clear();
            }

            let cs = self.get_char_size(&self.obj());
            let layout = self.get_or_create_layout(&self.obj());
            layout.set_attributes(None);
            let (char_width, char_height) = (cs.0 as f32, cs.1 as f32);
            let mut offset_y = 0.0_f32;
            let mut letter_spacing_diff: Option<i32> = None;

            if (self.cell_width_scale.get() - 1.0).abs() > f64::EPSILON
                || (self.cell_height_scale.get() - 1.0).abs() > f64::EPSILON
            {
                layout.set_text("A");
                let (_, logical) = layout.extents();
                if (self.cell_width_scale.get() - 1.0).abs() > f64::EPSILON {
                    let diff =
                        (logical.width() as f64 * (self.cell_width_scale.get() - 1.0)) as i32;
                    letter_spacing_diff = Some(diff);
                }
                if (self.cell_height_scale.get() - 1.0).abs() > f64::EPSILON {
                    let logical_h = logical.height() as f64 / gtk4::pango::SCALE as f64;
                    offset_y = ((char_height as f64 - logical_h) / 2.0).max(0.0) as f32;
                }
            }

            // Validate per-row GSK RenderNode cache epoch (invalidates on font, scale,
            // palette, or dimming changes).
            let epoch = super::RowCacheEpochKey {
                columns: state.columns,
                char_width_bits: cs.0.to_bits(),
                char_height_bits: cs.1.to_bits(),
                offset_y_bits: offset_y.to_bits(),
                scale_factor: self.obj().scale_factor(),
                is_dimmed: self.is_dimmed.get(),
                palette_generation: self.palette_generation.get(),
                font_generation: self.font_generation.get(),
            };
            if self.row_cache_epoch.get() != Some(epoch) {
                self.row_cache_epoch.set(Some(epoch));
                self.row_node_cache.borrow_mut().clear();
            } else if self.row_node_cache.borrow().len() > 512 {
                self.row_node_cache.borrow_mut().clear();
            }

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
            let apply_pango_style = |layout: &gtk4::pango::Layout, flags: Flags| {
                let needs_bold = flags.contains(Flags::BOLD);
                let needs_italic = flags.contains(Flags::ITALIC);
                let needs_strike = flags.contains(Flags::STRIKEOUT);
                if !needs_bold && !needs_italic && !needs_strike && letter_spacing_diff.is_none() {
                    layout.set_attributes(None);
                    return;
                }
                let attr_list = gtk4::pango::AttrList::new();
                if let Some(diff) = letter_spacing_diff {
                    attr_list.insert(gtk4::pango::AttrInt::new_letter_spacing(diff));
                }
                if needs_bold {
                    attr_list.insert(gtk4::pango::AttrInt::new_weight(gtk4::pango::Weight::Bold));
                }
                if needs_italic {
                    attr_list.insert(gtk4::pango::AttrInt::new_style(gtk4::pango::Style::Italic));
                }
                if needs_strike {
                    attr_list.insert(gtk4::pango::AttrInt::new_strikethrough(true));
                }
                layout.set_attributes(Some(&attr_list));
            };

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
            let hovered_uri = self.hovered_hyperlink.borrow().clone();

            let visible_matches = self.visible_search_matches.borrow();
            let selection_color = gtk4::gdk::RGBA::new(0.2, 0.4, 0.6, 0.5);
            let link_underline_color = gtk4::gdk::RGBA::new(0.35, 0.75, 1.0, 1.0);
            let yellow = gtk4::gdk::RGBA::new(0.961, 0.761, 0.067, 1.0); // #F5C211
            let orange = gtk4::gdk::RGBA::new(1.0, 0.471, 0.0, 1.0); // #FF7800

            for row in 0..state.screen_lines as i32 {
                let grid_line = Line(row - display_offset);
                let row_idx = row as usize;
                let row_cells = &state.cells[row_idx * state.columns..(row_idx + 1) * state.columns];

                // Compute 64-bit content hash for this row (cells + active search highlights on this row)
                let mut hasher = DefaultHasher::new();
                row_cells.hash(&mut hasher);
                let mut row_has_search_match = false;
                if !visible_matches.is_empty() {
                    for m in visible_matches.iter() {
                        if grid_line >= m.start.line && grid_line <= m.end.line {
                            row_has_search_match = true;
                            let sc = if grid_line == m.start.line {
                                m.start.column.0
                            } else {
                                0
                            };
                            let ec = if grid_line == m.end.line {
                                m.end.column.0
                            } else {
                                state.columns.saturating_sub(1)
                            };
                            let is_active = selection_range
                                .map(|r| r.start == m.start && r.end == m.end)
                                .unwrap_or(false);
                            (sc, ec, is_active).hash(&mut hasher);
                        }
                    }
                }
                let row_hash = hasher.finish();

                let cached_hit = self.row_node_cache.borrow().get(&row_hash).cloned();
                let row_node_opt = if let Some(cached_opt) = cached_hit {
                    cached_opt
                } else if !row_has_search_match && row_cells.iter().all(|c| c.is_empty()) {
                    self.row_node_cache.borrow_mut().insert(row_hash, None);
                    None
                } else {
                    // Build row-local GSK node at y = 0.0..char_height
                    let row_snap = gtk4::Snapshot::new();

                    // 1. Background pass (row-local y = 0.0)
                    let mut current_bg: Option<gtk4::gdk::RGBA> = None;
                    let mut start_col = 0.0_f32;
                    for (col, cell) in row_cells.iter().enumerate() {
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
                                crate::engine::ansi::Color::Named(named)
                                    if (named as usize) < 256 =>
                                {
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
                                row_snap.append_color(
                                    bg_col,
                                    &gtk4::graphene::Rect::new(
                                        start_col,
                                        0.0,
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
                        row_snap.append_color(
                            bg_col,
                            &gtk4::graphene::Rect::new(
                                start_col,
                                0.0,
                                (state.columns as f32 * char_width) - start_col + 0.5,
                                char_height + 0.5,
                            ),
                        );
                    }

                    // 1.5 Search Highlight background pass (row-local y = 0.0)
                    if row_has_search_match {
                        for m in visible_matches.iter() {
                            if grid_line >= m.start.line && grid_line <= m.end.line {
                                let start_col = if grid_line == m.start.line {
                                    m.start.column.0
                                } else {
                                    0
                                };
                                let end_col = if grid_line == m.end.line {
                                    m.end.column.0
                                } else {
                                    state.columns - 1
                                };

                                let is_active = selection_range
                                    .map(|r| r.start == m.start && r.end == m.end)
                                    .unwrap_or(false);
                                let color = if is_active { &orange } else { &yellow };

                                row_snap.append_color(
                                    color,
                                    &gtk4::graphene::Rect::new(
                                        start_col as f32 * char_width,
                                        0.0,
                                        (end_col as i32 - start_col as i32 + 1) as f32 * char_width,
                                        char_height,
                                    ),
                                );
                            }
                        }
                    }

                    // 2. Text & custom GSK underline pass (row-local y = offset_y)
                    let mut current_fg = fg_color_default;
                    let mut current_flags = Flags::empty();
                    let mut current_ucolor: Option<crate::engine::ansi::Color> = None;
                    let mut line_str = String::new();
                    let mut run_cells = 0usize;
                    start_col = 0.0_f32;

                    for (col, cell) in row_cells.iter().enumerate() {
                        let point = Point::new(grid_line, Column(col));
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

                        if row_has_search_match && visible_matches.iter().any(|m| m.contains(point))
                        {
                            fg = gtk4::gdk::RGBA::new(0.118, 0.118, 0.180, 1.0);
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
                                if !current_flags.contains(Flags::HIDDEN) {
                                    if line_str.chars().any(|c| c != ' ')
                                        || current_flags.contains(Flags::STRIKEOUT)
                                    {
                                        apply_pango_style(&layout, current_flags);
                                        layout.set_text(&line_str);
                                        row_snap.save();
                                        row_snap.translate(&gtk4::graphene::Point::new(
                                            start_col, offset_y,
                                        ));
                                        row_snap.append_layout(&layout, &current_fg);
                                        row_snap.restore();
                                    }
                                    paint_underline_span(
                                        &row_snap,
                                        current_flags,
                                        current_ucolor,
                                        current_fg,
                                        &palette,
                                        fg_color_default,
                                        dim_factor,
                                        start_col,
                                        start_col + (run_cells as f32) * char_width,
                                        char_width,
                                        char_height,
                                    );
                                }
                                line_str.clear();
                                run_cells = 0;
                            }
                            current_fg = fg;
                            current_flags = style_flags;
                            current_ucolor = ucolor;
                        }
                        if !is_spacer {
                            if line_str.is_empty() {
                                start_col = col as f32 * char_width;
                            }
                            line_str.push(if cell.c == '\0' { ' ' } else { cell.c });
                            if let Some(zw) = cell.zerowidth() {
                                for &c in zw {
                                    line_str.push(c);
                                }
                            }
                            run_cells += if is_wide { 2 } else { 1 };

                            if is_wide {
                                apply_pango_style(&layout, current_flags);
                                layout.set_text(&line_str);
                                let (_, logical) = layout.extents();
                                let actual_width =
                                    logical.width() as f32 / gtk4::pango::SCALE as f32;
                                let target_width = char_width * 2.0;
                                if !current_flags.contains(Flags::HIDDEN) {
                                    row_snap.save();
                                    row_snap.translate(&gtk4::graphene::Point::new(
                                        start_col, offset_y,
                                    ));
                                    if (actual_width - target_width).abs() > 0.1
                                        && actual_width > 0.0
                                    {
                                        let s = target_width / actual_width;
                                        row_snap.scale(s, s);
                                        let ah =
                                            logical.height() as f32 / gtk4::pango::SCALE as f32;
                                        let scaled_h = ah * s;
                                        if scaled_h < char_height {
                                            row_snap.translate(&gtk4::graphene::Point::new(
                                                0.0,
                                                (char_height - scaled_h) / 2.0,
                                            ));
                                        }
                                    }
                                    row_snap.append_layout(&layout, &current_fg);
                                    row_snap.restore();
                                    paint_underline_span(
                                        &row_snap,
                                        current_flags,
                                        current_ucolor,
                                        current_fg,
                                        &palette,
                                        fg_color_default,
                                        dim_factor,
                                        start_col,
                                        start_col + target_width,
                                        char_width,
                                        char_height,
                                    );
                                }
                                line_str.clear();
                                run_cells = 0;
                            } else if is_filling {
                                apply_pango_style(&layout, current_flags);
                                layout.set_text(&line_str);
                                let (_, logical) = layout.extents();
                                let aw =
                                    (logical.width() as f32 / gtk4::pango::SCALE as f32).max(0.1);
                                let ah =
                                    (logical.height() as f32 / gtk4::pango::SCALE as f32).max(0.1);
                                let sx = char_width / aw;
                                let sy = char_height / ah;
                                if !current_flags.contains(Flags::HIDDEN) {
                                    row_snap.save();
                                    row_snap
                                        .translate(&gtk4::graphene::Point::new(start_col, 0.0));
                                    if (sx - 1.0).abs() > 0.02 || (sy - 1.0).abs() > 0.02 {
                                        row_snap.scale(sx, sy);
                                    }
                                    row_snap.append_layout(&layout, &current_fg);
                                    row_snap.restore();
                                    paint_underline_span(
                                        &row_snap,
                                        current_flags,
                                        current_ucolor,
                                        current_fg,
                                        &palette,
                                        fg_color_default,
                                        dim_factor,
                                        start_col,
                                        start_col + char_width,
                                        char_width,
                                        char_height,
                                    );
                                }
                                line_str.clear();
                                run_cells = 0;
                            }
                        }
                    }
                    if !line_str.is_empty() && !current_flags.contains(Flags::HIDDEN) {
                        if line_str.chars().any(|c| c != ' ')
                            || current_flags.contains(Flags::STRIKEOUT)
                        {
                            apply_pango_style(&layout, current_flags);
                            layout.set_text(&line_str);
                            row_snap.save();
                            row_snap.translate(&gtk4::graphene::Point::new(start_col, offset_y));
                            row_snap.append_layout(&layout, &current_fg);
                            row_snap.restore();
                        }
                        paint_underline_span(
                            &row_snap,
                            current_flags,
                            current_ucolor,
                            current_fg,
                            &palette,
                            fg_color_default,
                            dim_factor,
                            start_col,
                            start_col + (run_cells as f32) * char_width,
                            char_width,
                            char_height,
                        );
                    }

                    let built_node = row_snap.to_node();
                    self.row_node_cache
                        .borrow_mut()
                        .insert(row_hash, built_node.clone());
                    built_node
                };

                if let Some(ref node) = row_node_opt {
                    snapshot.save();
                    snapshot.translate(&gtk4::graphene::Point::new(0.0, row as f32 * char_height));
                    snapshot.append_node(node);
                    snapshot.restore();
                }

                // 3. Dynamic per-row overlays (Selection & Hover Underlines — never invalidate row cache).
                // Note: `Flags::SEMANTIC_PROMPT` (OSC 133;A) is intentionally kept as an invisible
                // semantic anchor (for prompt jumping, click-to-move, and Claw context) rather than
                // drawing a left-margin bar, so it never clashes with prompt themes (Tide/Starship)
                // that render box-drawing characters (`╭─`/`╰─`) at column 0.
                if let Some(ref range) = selection_range
                    && grid_line >= range.start.line
                    && grid_line <= range.end.line
                {
                    let mut sel_span_start: Option<usize> = None;
                    for col in 0..state.columns {
                        let point = Point::new(grid_line, Column(col));
                        let selected = range.contains(point)
                            && (!row_has_search_match
                                || !visible_matches.iter().any(|m| m.contains(point)));
                        match (sel_span_start, selected) {
                            (None, true) => sel_span_start = Some(col),
                            (Some(s), false) => {
                                snapshot.append_color(
                                    &selection_color,
                                    &gtk4::graphene::Rect::new(
                                        s as f32 * char_width,
                                        row as f32 * char_height,
                                        (col - s) as f32 * char_width,
                                        char_height,
                                    ),
                                );
                                sel_span_start = None;
                            }
                            _ => {}
                        }
                    }
                    if let Some(s) = sel_span_start {
                        snapshot.append_color(
                            &selection_color,
                            &gtk4::graphene::Rect::new(
                                s as f32 * char_width,
                                row as f32 * char_height,
                                (state.columns - s) as f32 * char_width,
                                char_height,
                            ),
                        );
                    }
                }

                if let Some(ref hov_uri) = hovered_uri {
                    let mut link_span_start: Option<usize> = None;
                    for col in 0..state.columns {
                        let point = Point::new(grid_line, Column(col));
                        let matches_uri = state
                            .cell(point)
                            .hyperlink()
                            .is_some_and(|h| h.uri() == hov_uri);
                        match (link_span_start, matches_uri) {
                            (None, true) => link_span_start = Some(col),
                            (Some(s), false) => {
                                snapshot.append_color(
                                    &link_underline_color,
                                    &gtk4::graphene::Rect::new(
                                        s as f32 * char_width,
                                        row as f32 * char_height + char_height - 1.5,
                                        (col - s) as f32 * char_width,
                                        1.5,
                                    ),
                                );
                                link_span_start = None;
                            }
                            _ => {}
                        }
                    }
                    if let Some(s) = link_span_start {
                        snapshot.append_color(
                            &link_underline_color,
                            &gtk4::graphene::Rect::new(
                                s as f32 * char_width,
                                row as f32 * char_height + char_height - 1.5,
                                (state.columns - s) as f32 * char_width,
                                1.5,
                            ),
                        );
                    }
                }

                if let Some((r, start_c, end_c)) = self.hovered_regex_match.get()
                    && r == row as usize
                    && end_c > start_c
                {
                    let clamped_end = end_c.min(state.columns);
                    if clamped_end > start_c {
                        snapshot.append_color(
                            &link_underline_color,
                            &gtk4::graphene::Rect::new(
                                start_c as f32 * char_width,
                                row as f32 * char_height + char_height - 1.5,
                                (clamped_end - start_c) as f32 * char_width,
                                1.5,
                            ),
                        );
                    }
                }
            }
            drop(visible_matches);
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

            // 4. Cursor overlay (respects DECTCEM `?25h`/`?25l`, DECSCUSR shape/blinking,
            //    reverse-video character legibility under Block cursor, and hollow outline when unfocused)
            let effective_shape = state
                .cursor_style
                .map(|s| s.shape)
                .unwrap_or_else(|| self.cursor_shape.get());
            let effective_blinking = state
                .cursor_style
                .map(|s| s.blinking)
                .unwrap_or_else(|| self.cursor_blinking.get());
            let is_focused = self.obj().has_focus();
            let cursor_shown = state
                .mode
                .contains(crate::engine::term::TermMode::SHOW_CURSOR)
                && effective_shape != crate::engine::ansi::CursorShape::Hidden
                && (!is_focused || !effective_blinking || self.cursor_visible.get());

            if cursor_shown {
                let mut cp = state.cursor_point;
                let cr = cp.line.0 + display_offset;
                if cr >= 0 && cr < state.screen_lines as i32 {
                    let mut cell = state.cell(cp);
                    let mut wide = cell.flags.contains(Flags::WIDE_CHAR);
                    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) && cp.column.0 > 0 {
                        cp.column.0 -= 1;
                        cell = state.cell(cp);
                        wide = true;
                    }
                    let (cw, ch, cy) = match effective_shape {
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
                    let cursor_rect = gtk4::graphene::Rect::new(
                        cp.column.0 as f32 * char_width,
                        cr as f32 * char_height + cy,
                        cw,
                        ch,
                    );

                    let is_block = matches!(
                        effective_shape,
                        crate::engine::ansi::CursorShape::Block
                            | crate::engine::ansi::CursorShape::HollowBlock
                    );
                    if (!is_focused && is_block)
                        || effective_shape == crate::engine::ansi::CursorShape::HollowBlock
                    {
                        let rounded = gtk4::gsk::RoundedRect::from_rect(cursor_rect, 0.0);
                        snapshot.append_border(
                            &rounded,
                            &[1.5, 1.5, 1.5, 1.5],
                            &[cc, cc, cc, cc],
                        );
                    } else {
                        snapshot.append_color(&cc, &cursor_rect);
                        // Redraw the character under a solid focused Block cursor in reverse video
                        // so the character remains legible underneath the cursor.
                        if is_block
                            && !cell.flags.contains(Flags::HIDDEN)
                            && cell.c != ' '
                            && cell.c != '\0'
                        {
                            let mut cursor_str = String::with_capacity(8);
                            cursor_str.push(cell.c);
                            if let Some(zw) = cell.zerowidth() {
                                for &c in zw {
                                    cursor_str.push(c);
                                }
                            }
                            apply_pango_style(&layout, cell.flags & STYLE_FLAGS);
                            layout.set_text(&cursor_str);
                            snapshot.save();
                            snapshot.translate(&gtk4::graphene::Point::new(
                                cp.column.0 as f32 * char_width,
                                cr as f32 * char_height + offset_y,
                            ));
                            let text_on_cursor = gtk4::gdk::RGBA::new(
                                bg_color.red(),
                                bg_color.green(),
                                bg_color.blue(),
                                1.0,
                            );
                            snapshot.append_layout(&layout, &text_on_cursor);
                            snapshot.restore();
                        }
                    }
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
