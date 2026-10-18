use std::collections::HashMap;
use std::sync::Arc;

use log::debug;

use crate::engine::ansi::CursorShape;
use crate::engine::event::EventListener;
use crate::engine::grid::{Dimensions, GridCell, GridIterator};
use crate::engine::index::{Column, Line, Point};
use crate::engine::selection::SelectionRange;
use crate::engine::term::cell::{Cell, Flags};
use crate::engine::term::{Term, TermMode, color};

#[derive(Clone, Default)]
pub struct RenderState {
    pub generation: u64,
    pub cells: Vec<Cell>,
    pub columns: usize,
    pub screen_lines: usize,
    pub total_lines: usize,
    pub display_offset: i32,
    pub cursor_point: Point,
    pub selection_range: Option<SelectionRange>,
    pub selection_text: Option<String>,
    pub kitty_placements: Vec<crate::engine::kitty::Placement>,
    pub kitty_images: HashMap<u32, Arc<crate::engine::kitty::KittyImage>>,
    pub cell_width: u16,
    pub cell_height: u16,
    pub is_alt_screen: bool,
    pub cl_mode: Option<String>,
    pub mode: TermMode,
}

impl RenderState {
    pub fn new<T>(term: &Term<T>) -> Self {
        let grid = term.grid();
        let columns = grid.columns();
        let screen_lines = grid.screen_lines();
        let total_lines = grid.total_lines();
        let display_offset = grid.display_offset() as i32;

        let mut cells = Vec::with_capacity(columns * screen_lines);
        for row in 0..screen_lines {
            for col in 0..columns {
                let point = Point::new(Line(row as i32 - display_offset), Column(col));
                cells.push(grid[point.line][point.column].clone());
            }
        }

        let selection_range = term.selection.as_ref().and_then(|s| s.to_range(term));
        let selection_text = term.selection_to_string().filter(|s| !s.is_empty());
        let is_alt_screen = term.mode.contains(TermMode::ALT_SCREEN);
        let mode = term.mode;
        let cl_mode = term.cl_mode.clone();

        Self {
            generation: 0,
            cells,
            columns,
            screen_lines,
            total_lines,
            display_offset,
            cursor_point: grid.cursor.point,
            selection_range,
            selection_text,
            kitty_placements: term.kitty_graphics.placements.clone(),
            kitty_images: term.kitty_graphics.images.clone(),
            cell_width: term.cell_width,
            cell_height: term.cell_height,
            is_alt_screen,
            cl_mode,
            mode,
        }
    }

    /// Updates this `RenderState` in-place from `term`, reusing its `cells`
    /// heap allocation without allocating a new `Vec<Cell>`.
    pub fn update_from<T>(&mut self, term: &mut Term<T>, generation: u64) {
        let grid = term.grid();
        let columns = grid.columns();
        let screen_lines = grid.screen_lines();
        let total_lines = grid.total_lines();
        let display_offset = grid.display_offset() as i32;
        let is_alt_screen = term.mode.contains(TermMode::ALT_SCREEN);

        self.cells.clear();
        self.cells.reserve(columns * screen_lines);
        for row in 0..screen_lines {
            let line = Line(row as i32 - display_offset);
            self.cells.extend_from_slice(&grid[line][..Column(columns)]);
        }

        term.reset_damage();

        self.generation = generation;
        self.columns = columns;
        self.screen_lines = screen_lines;
        self.total_lines = total_lines;
        self.display_offset = display_offset;
        self.cursor_point = term.grid().cursor.point;
        self.selection_range = term.selection.as_ref().and_then(|s| s.to_range(term));
        self.selection_text = term.selection_to_string().filter(|s| !s.is_empty());
        self.kitty_placements.clone_from(&term.kitty_graphics.placements);
        self.kitty_images.clone_from(&term.kitty_graphics.images);
        self.cell_width = term.cell_width;
        self.cell_height = term.cell_height;
        self.is_alt_screen = is_alt_screen;
        self.cl_mode.clone_from(&term.cl_mode);
        self.mode = term.mode;
    }

    pub fn cell(&self, point: Point) -> &Cell {
        let row = point.line.0 + self.display_offset;
        &self.cells[(row as usize) * self.columns + point.column.0]
    }

    /// Return the selected text from this render snapshot.
    ///
    /// When `RenderState` is produced by `Term` (`new` / `update_from`),
    /// `self.selection_text` is pre-populated via `Term::selection_to_string()`,
    /// which covers the full scrollback history as well as the visible screen.
    /// If `self.selection_text` was not populated (e.g. a manually constructed
    /// `RenderState`), falls back to extracting from `self.cells` only when the
    /// entire selection range lies within the visible viewport.
    pub fn selection_text(&self) -> Option<String> {
        if self.selection_text.is_some() {
            return self.selection_text.clone();
        }

        let range = self.selection_range?;
        if self.columns == 0 || self.screen_lines == 0 {
            return None;
        }

        // If the selection extends outside the visible viewport, `self.cells`
        // does not contain the off-screen lines; return `None` so callers fall
        // back to querying `Term` on the event loop.
        let start_row = range.start.line.0 + self.display_offset;
        let end_row = range.end.line.0 + self.display_offset;
        if start_row < 0 || end_row >= self.screen_lines as i32 {
            return None;
        }

        let mut res = String::new();

        for line_idx in range.start.line.0..=range.end.line.0 {
            let row = (line_idx + self.display_offset) as usize;
            let row_slice = &self.cells[row * self.columns..(row + 1) * self.columns];

            let mut start_col = if range.is_block || line_idx == range.start.line.0 {
                range.start.column.0
            } else {
                0
            };
            let end_col = if range.is_block || line_idx == range.end.line.0 {
                range.end.column.0
            } else {
                self.columns.saturating_sub(1)
            };

            if start_col < row_slice.len()
                && row_slice[start_col].flags.contains(Flags::WIDE_CHAR_SPACER)
                && start_col > 0
            {
                start_col -= 1;
            }

            let row_len = row_slice
                .iter()
                .rposition(|c| !c.is_empty())
                .map_or(0, |i| i + 1)
                .min(end_col + 1);

            let mut line_str = String::new();
            for cell in row_slice
                .iter()
                .take(row_len)
                .skip(start_col.min(row_len))
            {
                if !cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    line_str.push(if cell.c == '\0' { ' ' } else { cell.c });
                    for c in cell.zerowidth().into_iter().flatten() {
                        line_str.push(*c);
                    }
                }
            }

            if range.is_block {
                res.push_str(line_str.trim_end());
                if line_idx != range.end.line.0 {
                    res.push('\n');
                }
            } else {
                res.push_str(&line_str);
                let is_wrapped = row_len > 0
                    && row_slice[row_len - 1].flags.contains(Flags::WRAPLINE);
                if line_idx != range.end.line.0
                    && end_col >= self.columns.saturating_sub(1)
                    && !is_wrapped
                {
                    res.push('\n');
                }
            }
        }

        if res.is_empty() { None } else { Some(res) }
    }

    /// Serializes the visible terminal viewport (including cell colors, SGR flags,
    /// alt-screen mode, and cursor position) into an ANSI byte stream so a detached
    /// session can restore its pre-detach screen on reattach even if the running
    /// process emits 0 new bytes while in the background.
    pub fn export_ansi_snapshot(&self) -> Vec<u8> {
        use crate::engine::ansi::{Color, NamedColor};
        use std::fmt::Write;

        if self.cells.is_empty() || self.columns == 0 || self.screen_lines == 0 {
            return Vec::new();
        }

        let style_mask = Flags::from_bits_truncate(
            Flags::BOLD.bits()
                | Flags::DIM.bits()
                | Flags::ITALIC.bits()
                | Flags::UNDERLINE.bits()
                | Flags::INVERSE.bits()
                | Flags::STRIKEOUT.bits(),
        );

        let cursor_row = (self.cursor_point.line.0 + self.display_offset)
            .clamp(0, self.screen_lines as i32 - 1) as usize;
        let cursor_col = self.cursor_point.column.0.min(self.columns.saturating_sub(1));

        // Find the last row that has any visible content (or the cursor row).
        let mut last_row = cursor_row;
        for r in (0..self.screen_lines).rev() {
            let row_slice = &self.cells[r * self.columns..(r + 1) * self.columns];
            if row_slice.iter().any(|c| !c.is_empty()) {
                last_row = last_row.max(r);
                break;
            }
        }

        let mut out = String::with_capacity(self.columns * (last_row + 1));
        if self.is_alt_screen {
            out.push_str("\x1b[?1049h\x1b[H");
        }

        let default_fg = Color::Named(NamedColor::Foreground);
        let default_bg = Color::Named(NamedColor::Background);

        for r in 0..=last_row {
            let row_slice = &self.cells[r * self.columns..(r + 1) * self.columns];
            let mut last_col = row_slice
                .iter()
                .rposition(|c| !c.is_empty())
                .map(|idx| idx + 1)
                .unwrap_or(0);
            if r == cursor_row {
                last_col = last_col.max(cursor_col);
            }

            let mut cur_fg = default_fg;
            let mut cur_bg = default_bg;
            let mut cur_flags = Flags::empty();

            for cell in row_slice.iter().take(last_col) {
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }

                let cell_flags = cell.flags & style_mask;
                if cell.fg != cur_fg || cell.bg != cur_bg || cell_flags != cur_flags {
                    cur_fg = cell.fg;
                    cur_bg = cell.bg;
                    cur_flags = cell_flags;

                    out.push_str("\x1b[0");
                    if cur_flags.contains(Flags::BOLD) {
                        out.push_str(";1");
                    }
                    if cur_flags.contains(Flags::DIM) {
                        out.push_str(";2");
                    }
                    if cur_flags.contains(Flags::ITALIC) {
                        out.push_str(";3");
                    }
                    if cur_flags.contains(Flags::UNDERLINE) {
                        out.push_str(";4");
                    }
                    if cur_flags.contains(Flags::INVERSE) {
                        out.push_str(";7");
                    }
                    if cur_flags.contains(Flags::STRIKEOUT) {
                        out.push_str(";9");
                    }

                    match cur_fg {
                        Color::Named(NamedColor::Foreground) => {}
                        Color::Named(NamedColor::BrightForeground) => out.push_str(";1"),
                        Color::Named(NamedColor::DimForeground) => out.push_str(";2"),
                        Color::Named(n) => {
                            let idx = n as u16;
                            if idx < 8 {
                                let _ = write!(out, ";{}", 30 + idx);
                            } else if idx < 16 {
                                let _ = write!(out, ";{}", 90 + (idx - 8));
                            }
                        }
                        Color::Indexed(idx) => {
                            let _ = write!(out, ";38;5;{idx}");
                        }
                        Color::Spec(rgb) => {
                            let _ = write!(out, ";38;2;{};{};{}", rgb.r, rgb.g, rgb.b);
                        }
                    }

                    match cur_bg {
                        Color::Named(NamedColor::Background) => {}
                        Color::Named(n) => {
                            let idx = n as u16;
                            if idx < 8 {
                                let _ = write!(out, ";{}", 40 + idx);
                            } else if idx < 16 {
                                let _ = write!(out, ";{}", 100 + (idx - 8));
                            }
                        }
                        Color::Indexed(idx) => {
                            let _ = write!(out, ";48;5;{idx}");
                        }
                        Color::Spec(rgb) => {
                            let _ = write!(out, ";48;2;{};{};{}", rgb.r, rgb.g, rgb.b);
                        }
                    }

                    out.push('m');
                }

                let ch = if cell.c == '\0' { ' ' } else { cell.c };
                out.push(ch);
            }

            if cur_fg != default_fg || cur_bg != default_bg || !cur_flags.is_empty() {
                out.push_str("\x1b[0m");
            }

            if r < last_row {
                out.push_str("\r\n");
            }
        }

        if self.is_alt_screen {
            let _ = write!(out, "\x1b[{};{}H", cursor_row + 1, cursor_col + 1);
        } else {
            let rows_up = last_row.saturating_sub(cursor_row);
            if rows_up > 0 {
                let _ = write!(out, "\x1b[{rows_up}A");
            }
            out.push('\r');
            if cursor_col > 0 {
                let _ = write!(out, "\x1b[{cursor_col}C");
            }
        }
        out.into_bytes()
    }

    pub fn calculate_character_delta(&self, start: Point, end: Point) -> isize {
        let mut char_delta = 0isize;
        let cur_line = start.line.0;
        let cur_col = start.column.0;
        let tar_line = end.line.0;
        let tar_col = end.column.0;

        let (start_line, start_col, end_line, end_col, mult) =
            if tar_line > cur_line || (tar_line == cur_line && tar_col > cur_col) {
                (cur_line, cur_col, tar_line, tar_col, 1isize)
            } else {
                (tar_line, tar_col, cur_line, cur_col, -1isize)
            };

        for line in start_line..=end_line {
            let row_in_viewport = (line + self.display_offset) as usize;
            let row_cells =
                &self.cells[row_in_viewport * self.columns..(row_in_viewport + 1) * self.columns];

            let s_col = if line == start_line { start_col } else { 0 };
            let e_col = if line == end_line {
                end_col
            } else {
                let mut e = row_cells.len();
                if !row_cells[row_cells.len() - 1]
                    .flags
                    .contains(Flags::WRAPLINE)
                {
                    let last_cmd = row_cells
                        .iter()
                        .rposition(|c| c.flags.contains(Flags::SEMANTIC_CMD));
                    let last_content = row_cells.iter().rposition(|c| !c.is_empty());
                    e = last_cmd
                        .map(|l| l + 1)
                        .unwrap_or(0)
                        .max(last_content.map(|l| l + 1).unwrap_or(0));
                }
                e
            };

            let first_cmd = row_cells
                .iter()
                .position(|c| c.flags.contains(Flags::SEMANTIC_CMD));
            let last_prompt = row_cells
                .iter()
                .rposition(|c| c.flags.contains(Flags::SEMANTIC_PROMPT));

            let line_start_bound =
                first_cmd.unwrap_or_else(|| last_prompt.map(|p| p + 1).unwrap_or(0));

            let actual_s_col = s_col.max(line_start_bound);
            let actual_e_col = e_col.max(line_start_bound);

            if actual_s_col < actual_e_col {
                for i in actual_s_col..actual_e_col {
                    let flags = row_cells[i].flags;
                    if i < row_cells.len()
                        && !flags.contains(Flags::WIDE_CHAR_SPACER)
                        && !flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
                        && !flags.contains(Flags::SEMANTIC_PROMPT)
                    {
                        char_delta += 1;
                    }
                }
            }

            if line < end_line
                && !row_cells[row_cells.len() - 1]
                    .flags
                    .contains(Flags::WRAPLINE)
            {
                char_delta += 1;
            }
        }

        char_delta * mult
    }

    /// Returns true if every row between line_a and line_b (exclusive of max) has WRAPLINE set,
    /// meaning they form a single logical line in the shell's view.
    fn is_in_same_wrap_chain(&self, line_a: Line, line_b: Line) -> bool {
        let min_line = line_a.0.min(line_b.0);
        let max_line = line_a.0.max(line_b.0);
        let n_rows = self.cells.len() / self.columns;
        for l in min_line..max_line {
            let row_idx = (l + self.display_offset) as usize;
            if (row_idx + 1) * self.columns > self.cells.len() || row_idx >= n_rows {
                return false;
            }
            let last_cell = &self.cells[(row_idx + 1) * self.columns - 1];
            if !last_cell.flags.contains(Flags::WRAPLINE) {
                return false;
            }
        }
        true
    }

    pub fn calculate_navigation_sequence(&self, point: Point) -> Option<String> {
        // Only fires when the shell has explicitly advertised cl=line support via OSC 133;A.
        // CLICK_REPORT (mode 2031) is handled upstream in the GTK click handler — fish moves
        // its own cursor via SGR mouse events and must never receive arrow sequences here.
        let cl_mode_str = self.cl_mode.as_deref()?;
        if !cl_mode_str.split(',').any(|m| m == "line") {
            debug!("nav: not line mode, cl_mode={:?}", self.cl_mode);
            return None;
        }

        let is_app_cursor = self.mode.contains(TermMode::APP_CURSOR);
        let (right_seq, left_seq) = if is_app_cursor {
            ("\x1bOC", "\x1bOD")
        } else {
            ("\x1b[C", "\x1b[D")
        };

        let is_same_line = point.line == self.cursor_point.line;
        let is_soft_wrap =
            !is_same_line && self.is_in_same_wrap_chain(self.cursor_point.line, point.line);
        debug!(
            "nav: click={:?} cursor={:?} same={} soft_wrap={}",
            point, self.cursor_point, is_same_line, is_soft_wrap
        );

        if is_same_line || is_soft_wrap {
            // Case 1 & 2: same logical line (exact match or soft-wrapped).
            // The shell sees this as one line; Left/Right arrows navigate it transparently.
            let row_in_viewport = (point.line.0 + self.display_offset) as usize;
            let mut target_col = point.column.0;
            let line_cells =
                &self.cells[row_in_viewport * self.columns..(row_in_viewport + 1) * self.columns];

            if target_col < line_cells.len()
                && line_cells[target_col]
                    .flags
                    .contains(Flags::WIDE_CHAR_SPACER)
                && target_col > 0
            {
                target_col -= 1;
            }

            // For same-line clicks: enforce semantic bounds (don't move into prompt area).
            if is_same_line {
                let first_cmd = line_cells
                    .iter()
                    .position(|c| c.flags.contains(Flags::SEMANTIC_CMD));
                let last_prompt = line_cells
                    .iter()
                    .rposition(|c| c.flags.contains(Flags::SEMANTIC_PROMPT));
                if let Some(first) = first_cmd {
                    if target_col < first {
                        target_col = first;
                    }
                } else if let Some(last) = last_prompt
                    && target_col <= last
                {
                    target_col = last + 1;
                }
                let last_cmd = line_cells
                    .iter()
                    .rposition(|c| c.flags.contains(Flags::SEMANTIC_CMD));
                let last_content = line_cells.iter().rposition(|c| !c.is_empty());
                let line_end_bound = last_cmd
                    .map(|l| l + 1)
                    .unwrap_or(0)
                    .max(last_content.map(|l| l + 1).unwrap_or(0));
                let command_end = line_end_bound.max(self.cursor_point.column.0);
                if target_col > command_end {
                    target_col = command_end;
                }
            }

            let char_delta = self.calculate_character_delta(
                self.cursor_point,
                Point::new(point.line, Column(target_col)),
            );

            let mut seq = String::new();
            if char_delta > 0 {
                seq.push_str(&right_seq.repeat(char_delta as usize));
            } else if char_delta < 0 {
                seq.push_str(&left_seq.repeat((-char_delta) as usize));
            }
            if seq.is_empty() { None } else { Some(seq) }
        } else {
            // Case 3: hard-newline multiline command.
            // Left/Right cannot cross hard newlines; use Up/Down to change logical lines,
            // then Left/Right to adjust to the target column.
            let n_rows = self.cells.len() / self.columns;
            let cursor_row_idx = (self.cursor_point.line.0 + self.display_offset) as usize;
            let target_row_idx = (point.line.0 + self.display_offset) as usize;
            if target_row_idx >= n_rows {
                return None;
            }

            let cursor_row =
                &self.cells[cursor_row_idx * self.columns..(cursor_row_idx + 1) * self.columns];
            let target_row =
                &self.cells[target_row_idx * self.columns..(target_row_idx + 1) * self.columns];

            debug!(
                "nav B: cursor_idx={} target_idx={} display_offset={}",
                cursor_row_idx, target_row_idx, self.display_offset
            );

            // Don't navigate when the user has scrolled into history — Up/Down would move
            // through command history instead of within the current multiline command.
            if self.display_offset != 0 {
                debug!("nav B: rejected — scrolled into history");
                return None;
            }

            // Cursor must be on an active command line (OSC 133;B sets SEMANTIC_CMD).
            // Without this guard, Up/Down would navigate shell history instead of within the command.
            // Continuation lines in fish may carry SEMANTIC_PROMPT (fish omits OSC 133;B for them),
            // so we accept SEMANTIC_CMD | SEMANTIC_PROMPT on the target.
            let cursor_has_cmd = cursor_row
                .iter()
                .any(|c| c.flags.contains(Flags::SEMANTIC_CMD));
            let target_is_editable = target_row.iter().any(|c| {
                c.flags.contains(Flags::SEMANTIC_CMD) || c.flags.contains(Flags::SEMANTIC_PROMPT)
            });
            if !cursor_has_cmd || !target_is_editable {
                debug!(
                    "nav B: rejected — semantic guard (cursor_cmd={} target_editable={})",
                    cursor_has_cmd, target_is_editable
                );
                return None;
            }

            // No blank rows between cursor and target. A blank line separates different command
            // "clusters" (e.g. old output above vs the current prompt below), so a gap means
            // the click is on a different command's output and we should not navigate.
            let min_row = target_row_idx.min(cursor_row_idx);
            let max_row = target_row_idx.max(cursor_row_idx);
            let has_gap = (min_row..=max_row).any(|r| {
                (r + 1) * self.columns <= self.cells.len()
                    && self.cells[r * self.columns..(r + 1) * self.columns]
                        .iter()
                        .all(|c| c.is_empty())
            });
            if has_gap {
                debug!("nav B: rejected — blank row between cursor and target");
                return None;
            }

            let line_delta = point.line.0 - self.cursor_point.line.0;
            if line_delta == 0 {
                return None;
            }

            // Compute target column.
            // Do NOT apply the "skip past prompt" clamping here: when fish omits 133;B for
            // continuation lines, user-typed content is also tagged SEMANTIC_PROMPT, so
            // last_prompt would include the actual content and the clamp would overshoot.
            let mut target_col = point.column.0;
            if target_col < target_row.len()
                && target_row[target_col]
                    .flags
                    .contains(Flags::WIDE_CHAR_SPACER)
                && target_col > 0
            {
                target_col -= 1;
            }
            // Right bound: don't click into empty space past content.
            let last_content = target_row.iter().rposition(|c| !c.is_empty());
            let content_end = last_content
                .map(|l| l + 1)
                .unwrap_or(0)
                .max(self.cursor_point.column.0);
            if target_col > content_end {
                target_col = content_end;
            }

            // After Up/Down, the shell places the cursor at min(current_col, content_end).
            let post_nav_col = self.cursor_point.column.0.min(content_end);
            let col_adjustment = target_col as isize - post_nav_col as isize;

            let (up_seq, down_seq) = if is_app_cursor {
                ("\x1bOA", "\x1bOB")
            } else {
                ("\x1b[A", "\x1b[B")
            };
            let mut seq = String::new();
            if line_delta > 0 {
                seq.push_str(&down_seq.repeat(line_delta as usize));
            } else {
                seq.push_str(&up_seq.repeat((-line_delta) as usize));
            }
            if col_adjustment > 0 {
                seq.push_str(&right_seq.repeat(col_adjustment as usize));
            } else if col_adjustment < 0 {
                seq.push_str(&left_seq.repeat((-col_adjustment) as usize));
            }
            debug!(
                "nav B: line_delta={} post_nav_col={} target_col={} col_adj={} seq_len={}",
                line_delta,
                post_nav_col,
                target_col,
                col_adjustment,
                seq.len()
            );
            if seq.is_empty() { None } else { Some(seq) }
        }
    }
}

/// Terminal cursor rendering information.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct RenderableCursor {
    pub shape: CursorShape,
    pub point: Point,
}

impl RenderableCursor {
    fn new<T>(term: &Term<T>) -> Self {
        // Cursor position.
        let vi_mode = term.mode().contains(TermMode::VI);
        let mut point = if vi_mode {
            term.vi_mode_cursor.point
        } else {
            term.grid.cursor.point
        };
        if term.grid[point].flags.contains(Flags::WIDE_CHAR_SPACER) {
            point.column -= 1;
        }

        // Cursor shape.
        let shape = if !vi_mode && !term.mode().contains(TermMode::SHOW_CURSOR) {
            CursorShape::Hidden
        } else {
            term.cursor_style().shape
        };

        Self { shape, point }
    }
}

/// Visible terminal content.
///
/// This contains all content required to render the current terminal view.
pub struct RenderableContent<'a> {
    pub display_iter: GridIterator<'a, Cell>,
    pub selection: Option<SelectionRange>,
    pub cursor: RenderableCursor,
    pub display_offset: usize,
    pub colors: &'a color::Colors,
    pub mode: TermMode,
}

impl<'a> RenderableContent<'a> {
    pub(super) fn new<T>(term: &'a Term<T>) -> Self {
        Self {
            display_iter: term.grid().display_iter(),
            display_offset: term.grid().display_offset(),
            cursor: RenderableCursor::new(term),
            selection: term.selection.as_ref().and_then(|s| s.to_range(term)),
            colors: &term.colors,
            mode: *term.mode(),
        }
    }
}

impl<T> Term<T> {
    /// Terminal content required for rendering.
    #[inline]
    pub fn renderable_content(&self) -> RenderableContent<'_>
    where
        T: EventListener,
    {
        RenderableContent::new(self)
    }
}
