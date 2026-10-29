use std::ops::Range;
use std::{cmp, mem};

use log::{debug, trace};

use crate::engine::ansi::{Handler, KeyboardModes, KeyboardModesApplyBehavior};
use crate::engine::event::{Event, EventListener};
use crate::engine::grid::{Dimensions, Scroll};
use crate::engine::index::{Column, Direction, Line, Point};
use crate::engine::selection::{Selection, SelectionRange, SelectionType};
use crate::engine::term::cell::{Flags, LineLength};
use crate::engine::term::{Term, TermMode};

impl<T> Term<T> {
    #[inline]
    pub fn scroll_display(&mut self, scroll: Scroll)
    where
        T: EventListener,
    {
        let old_display_offset = self.grid.display_offset();
        self.grid.scroll_display(scroll);
        self.event_proxy.send_event(Event::MouseCursorDirty);

        // Clamp vi mode cursor to the viewport.
        let viewport_start = -(self.grid.display_offset() as i32);
        let viewport_end = viewport_start + self.bottommost_line().0;
        let vi_cursor_line = &mut self.vi_mode_cursor.point.line.0;
        *vi_cursor_line = cmp::min(viewport_end, cmp::max(viewport_start, *vi_cursor_line));
        self.vi_mode_recompute_selection();

        // Damage everything if display offset changed.
        if old_display_offset != self.grid().display_offset() {
            self.mark_fully_damaged();
        }
    }

    /// Scroll the viewport to the previous (`Direction::Left`) or next (`Direction::Right`)
    /// semantic prompt mark (`Flags::SEMANTIC_PROMPT`, emitted by `OSC 133;A`) in scrollback.
    ///
    /// Note: Like Ghostty and Kitty, `Flags::SEMANTIC_PROMPT` is kept strictly as an
    /// invisible semantic anchor in the cell grid rather than painting a visible left-gutter
    /// bar in `painter.rs`, avoiding visual collisions with modern prompt themes (e.g. Tide,
    /// Starship, Powerlevel10k) that draw box-drawing connectors (`╭─`/`╰─`) at column 0.
    /// Returns `true` if the viewport offset changed.
    pub fn scroll_to_prompt(&mut self, direction: Direction) -> bool
    where
        T: EventListener,
    {
        if self.mode.contains(TermMode::ALT_SCREEN) || self.history_size() == 0 {
            return false;
        }

        let topmost = self.topmost_line();
        let current_offset = self.grid.display_offset();
        let viewport_top = Line(-(current_offset as i32));

        let row_has_prompt = |line: Line| -> bool {
            self.grid[line][..]
                .iter()
                .any(|c| c.flags.contains(Flags::SEMANTIC_PROMPT))
        };
        let is_prompt_start = |line: Line| -> bool {
            row_has_prompt(line) && (line <= topmost || !row_has_prompt(line - 1))
        };

        match direction {
            Direction::Left => {
                let mut line = viewport_top - 1;
                while line >= topmost {
                    if is_prompt_start(line) {
                        let target_offset = (-line.0).clamp(0, self.history_size() as i32) as usize;
                        let delta = target_offset as i32 - current_offset as i32;
                        if delta != 0 {
                            self.scroll_display(Scroll::Delta(delta));
                            return true;
                        }
                        break;
                    }
                    line -= 1;
                }
                false
            }
            Direction::Right => {
                if current_offset == 0 {
                    return false;
                }
                let mut line = viewport_top + 1;
                while line <= Line(0) {
                    if is_prompt_start(line) {
                        let target_offset = (-line.0).max(0) as usize;
                        let delta = target_offset as i32 - current_offset as i32;
                        if delta != 0 {
                            self.scroll_display(Scroll::Delta(delta));
                            return true;
                        }
                        break;
                    }
                    line += 1;
                }
                self.scroll_display(Scroll::Bottom);
                self.grid.display_offset() != current_offset
            }
        }
    }

    /// Extract the text of the most recent command output block (`Flags::SEMANTIC_OUTPUT`,
    /// bounded by `OSC 133;C` and `OSC 133;D`).
    pub fn last_command_output(&self) -> Option<String> {
        let topmost = self.topmost_line();
        let bottommost = self.bottommost_line();

        let row_has_output = |line: Line| -> bool {
            self.grid[line][..]
                .iter()
                .any(|c| c.flags.contains(Flags::SEMANTIC_OUTPUT))
        };

        let mut end_line = None;
        let mut line = bottommost;
        while line >= topmost {
            if row_has_output(line) {
                end_line = Some(line);
                break;
            }
            line -= 1;
        }

        let end_line = end_line?;
        let mut start_line = end_line;
        while start_line > topmost && row_has_output(start_line - 1) {
            start_line -= 1;
        }

        let start = Point::new(start_line, Column(0));
        let end = Point::new(end_line, self.last_column());
        let text = self.bounds_to_string(start, end);
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Select all non-empty content from the top of scrollback history down to
    /// the last non-empty screen line (or the cursor line).
    pub fn select_all(&mut self) {
        let cols = self.columns();
        let lines = self.screen_lines();
        if cols == 0 || lines == 0 {
            return;
        }
        let start = Point::new(self.topmost_line(), Column(0));
        let mut last_line = self.bottommost_line();
        while last_line > self.grid.cursor.point.line
            && last_line > start.line
            && self.grid[last_line].line_length() == Column(0)
        {
            last_line -= 1;
        }
        let end = Point::new(last_line, Column(cols - 1));
        let mut sel = Selection::new(
            SelectionType::Simple,
            start,
            crate::engine::index::Side::Left,
        );
        sel.update(end, crate::engine::index::Side::Right);
        self.selection = Some(sel);
    }

    /// Adjust the active selection endpoint (`Shift + Arrow/Home/End`).
    /// Returns `true` if a selection existed and was updated.
    pub fn adjust_selection(&mut self, adjust: crate::engine::selection::SelectionAdjust) -> bool
    where
        T: EventListener,
    {
        use crate::engine::index::{Boundary, Side};
        use crate::engine::selection::SelectionAdjust;

        let mut sel = match self.selection.take() {
            Some(sel) => sel,
            None => return false,
        };

        let Some(mut end_pt) = sel.normalize_for_keyboard_adjust(self) else {
            self.selection = Some(sel);
            return false;
        };

        let topmost = self.topmost_line();
        let bottommost = self.bottommost_line();
        let last_col = self.last_column();
        let mut side = Side::Right;

        match adjust {
            SelectionAdjust::Left => {
                end_pt = end_pt.sub(self, Boundary::Grid, 1);
                if self.grid[end_pt.line][end_pt.column]
                    .flags
                    .contains(Flags::WIDE_CHAR_SPACER)
                    && end_pt.column > Column(0)
                {
                    end_pt.column -= 1;
                }
                side = Side::Left;
            }
            SelectionAdjust::Right => {
                if self.grid[end_pt.line][end_pt.column]
                    .flags
                    .contains(Flags::WIDE_CHAR)
                {
                    end_pt = end_pt.add(self, Boundary::Grid, 1);
                }
                end_pt = end_pt.add(self, Boundary::Grid, 1);
                side = Side::Right;
            }
            SelectionAdjust::Up => {
                end_pt.line = cmp::max(end_pt.line - 1, topmost);
            }
            SelectionAdjust::Down => {
                end_pt.line = cmp::min(end_pt.line + 1, bottommost);
            }
            SelectionAdjust::PageUp => {
                end_pt.line = cmp::max(end_pt.line - self.screen_lines() as i32, topmost);
            }
            SelectionAdjust::PageDown => {
                end_pt.line = cmp::min(end_pt.line + self.screen_lines() as i32, bottommost);
            }
            SelectionAdjust::BeginningOfLine => {
                end_pt.column = Column(0);
                side = Side::Left;
            }
            SelectionAdjust::EndOfLine => {
                end_pt.column = last_col;
                side = Side::Right;
            }
            SelectionAdjust::Home => {
                end_pt = Point::new(topmost, Column(0));
                side = Side::Left;
            }
            SelectionAdjust::End => {
                end_pt = Point::new(bottommost, last_col);
                side = Side::Right;
            }
        }

        sel.update(end_pt, side);
        self.selection = Some(sel);
        self.scroll_to_point(end_pt);
        self.mark_fully_damaged();
        true
    }

    /// Extract plain text of the visible screen (or full scrollback + screen when
    /// `include_scrollback` is `true`), trimming trailing blank rows below the cursor.
    pub fn dump_buffer_to_string(&self, include_scrollback: bool) -> String {
        let cols = self.columns();
        let lines = self.screen_lines();
        if cols == 0 || lines == 0 {
            return String::new();
        }
        let start_line = if include_scrollback {
            self.topmost_line()
        } else {
            Line(0)
        };
        let mut last_line = self.bottommost_line();
        while last_line > self.grid.cursor.point.line
            && last_line > start_line
            && self.grid[last_line].line_length() == Column(0)
        {
            last_line -= 1;
        }
        self.bounds_to_string(
            Point::new(start_line, Column(0)),
            Point::new(last_line, Column(cols - 1)),
        )
    }

    /// Convert the active selection to a String.
    pub fn selection_to_string(&self) -> Option<String> {
        let selection_range = self.selection.as_ref().and_then(|s| s.to_range(self))?;
        let SelectionRange { start, end, .. } = selection_range;

        let mut res = String::new();

        match self.selection.as_ref() {
            Some(Selection {
                ty: SelectionType::Block,
                ..
            }) => {
                for line in (start.line.0..end.line.0).map(Line::from) {
                    res += self
                        .line_to_string(line, start.column..end.column, start.column.0 != 0)
                        .trim_end();
                    res += "\n";
                }

                res += self
                    .line_to_string(end.line, start.column..end.column, true)
                    .trim_end();
            }
            Some(Selection {
                ty: SelectionType::Lines,
                ..
            }) => {
                res = self.bounds_to_string(start, end) + "\n";
            }
            _ => {
                res = self.bounds_to_string(start, end);
            }
        }

        Some(res)
    }

    /// Convert range between two points to a String.
    pub fn bounds_to_string(&self, start: Point, end: Point) -> String {
        let line_count = (end.line.0 - start.line.0).max(0) as usize + 1;
        let mut res = String::with_capacity(line_count * (self.columns() / 2).max(16));

        for line in (start.line.0..=end.line.0).map(Line::from) {
            let start_col = if line == start.line {
                start.column
            } else {
                Column(0)
            };
            let end_col = if line == end.line {
                end.column
            } else {
                self.last_column()
            };

            self.append_line_to_string(&mut res, line, start_col..end_col, line == end.line);
        }

        if res.ends_with('\n') {
            res.pop();
        }
        res
    }

    /// Convert range between two points to a String, including semantic boundary tags.
    pub fn semantic_bounds_to_string(&self, start: Point, end: Point) -> String {
        let mut res = String::new();
        let mut current_semantic: Option<u32> = None;

        for line in (start.line.0..=end.line.0).map(Line::from) {
            let start_col = if line == start.line {
                start.column
            } else {
                Column(0)
            };
            let end_col = if line == end.line {
                end.column
            } else {
                self.last_column()
            };

            let grid_line = &self.grid[line];
            let line_length = cmp::min(grid_line.line_length(), end_col + 1);

            let mut tab_mode = false;
            for column in (start_col.0..line_length.0).map(Column::from) {
                let cell = &grid_line[column];

                let cell_semantic = cell.flags.bits()
                    & (Flags::SEMANTIC_PROMPT.bits()
                        | Flags::SEMANTIC_CMD.bits()
                        | Flags::SEMANTIC_OUTPUT.bits());
                if current_semantic != Some(cell_semantic) {
                    if current_semantic.is_some() && current_semantic.unwrap() != 0 {
                        res.push_str("\n```\n");
                    }
                    if cell_semantic != 0 {
                        if cell_semantic == Flags::SEMANTIC_PROMPT.bits() {
                            res.push_str("\n[PROMPT]\n```text\n");
                        } else if cell_semantic == Flags::SEMANTIC_CMD.bits() {
                            res.push_str("\n[COMMAND]\n```bash\n");
                        } else if cell_semantic == Flags::SEMANTIC_OUTPUT.bits() {
                            res.push_str("\n[OUTPUT]\n```text\n");
                        }
                    }
                    current_semantic = Some(cell_semantic);
                }

                if tab_mode {
                    if self.tabs[column] || cell.c != ' ' {
                        tab_mode = false;
                    } else {
                        continue;
                    }
                }
                if cell.c == '\t' {
                    tab_mode = true;
                }

                if !cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    res.push(cell.c);
                    for c in cell.zerowidth().into_iter().flatten() {
                        res.push(*c);
                    }
                }
            }

            if end_col >= self.columns() - 1
                && (line_length.0 == 0
                    || !grid_line[line_length - 1].flags.contains(Flags::WRAPLINE))
            {
                res.push('\n');
            }
        }

        if current_semantic.is_some() && current_semantic.unwrap() != 0 {
            res.push_str("\n```\n");
        }

        res.strip_suffix('\n').map(str::to_owned).unwrap_or(res)
    }

    /// Convert a single line in the grid to a String.
    fn line_to_string(
        &self,
        line: Line,
        cols: Range<Column>,
        include_wrapped_wide: bool,
    ) -> String {
        let mut text = String::with_capacity(cols.end.0.saturating_sub(cols.start.0) + 2);
        self.append_line_to_string(&mut text, line, cols, include_wrapped_wide);
        text
    }

    /// Append a single line in the grid directly into `text` without intermediate allocation.
    fn append_line_to_string(
        &self,
        text: &mut String,
        line: Line,
        mut cols: Range<Column>,
        include_wrapped_wide: bool,
    ) {
        let grid_line = &self.grid[line];
        let line_length = cmp::min(grid_line.line_length(), cols.end + 1);

        // Include wide char when trailing spacer is selected.
        if grid_line[cols.start]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
        {
            cols.start -= 1;
        }

        let mut tab_mode = false;
        for column in (cols.start.0..line_length.0).map(Column::from) {
            let cell = &grid_line[column];

            // Skip over cells until next tab-stop once a tab was found.
            if tab_mode {
                if self.tabs[column] || cell.c != ' ' {
                    tab_mode = false;
                } else {
                    continue;
                }
            }

            if cell.c == '\t' {
                tab_mode = true;
            }

            if !cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                // Push cells primary character.
                text.push(cell.c);

                // Push zero-width characters.
                for c in cell.zerowidth().into_iter().flatten() {
                    text.push(*c);
                }
            }
        }

        if cols.end >= self.columns() - 1
            && (line_length.0 == 0
                || !self.grid[line][line_length - 1]
                    .flags
                    .contains(Flags::WRAPLINE))
        {
            text.push('\n');
        }

        // If wide char is not part of the selection, but leading spacer is, include it.
        if line_length == self.columns()
            && line_length.0 >= 2
            && grid_line[line_length - 1]
                .flags
                .contains(Flags::LEADING_WIDE_CHAR_SPACER)
            && include_wrapped_wide
        {
            text.push(self.grid[line - 1i32][Column(0)].c);
        }
    }

    /// Resize terminal to new dimensions.
    pub fn resize<S: Dimensions>(&mut self, size: S)
    where
        T: EventListener,
    {
        let old_cols = self.columns();
        let old_lines = self.screen_lines();

        let num_cols = size.columns();
        let num_lines = size.screen_lines();

        self.cell_width = size.cell_width();
        self.cell_height = size.cell_height();

        if old_cols == num_cols && old_lines == num_lines {
            debug!("Term::resize dimensions unchanged");
            return;
        }

        let is_alt = self.mode.contains(TermMode::ALT_SCREEN);
        let delta = self.grid.resize(!is_alt, num_lines, num_cols);
        let inactive_delta = self.inactive_grid.resize(is_alt, num_lines, num_cols);

        // Move vi mode cursor with the content.
        self.vi_mode_cursor.point.line += delta;

        let max_lines = cmp::max(num_lines, old_lines) as i32;
        let range = Line(0)..Line(max_lines);
        self.kitty_graphics.rotate_placements(&range, -delta);
        self.kitty_graphics
            .rotate_inactive_placements(&range, -inactive_delta);

        // Invalidate selection and tabs only when necessary.
        if old_cols != num_cols {
            self.selection = None;

            // Recreate tabs list.
            self.tabs.resize(num_cols);
        } else if let Some(selection) = self.selection.take() {
            self.selection = selection.rotate(self, &range, -delta);
        }

        // Clamp vi cursor to viewport.
        let vi_point = self.vi_mode_cursor.point;
        let viewport_top = Line(-(self.grid.display_offset() as i32));
        let viewport_bottom = viewport_top + self.bottommost_line();
        self.vi_mode_cursor.point.line =
            cmp::max(cmp::min(vi_point.line, viewport_bottom), viewport_top);
        self.vi_mode_cursor.point.column = cmp::min(vi_point.column, self.last_column());

        // Reset scrolling region.
        self.scroll_region = Line(0)..Line(self.screen_lines() as i32);

        // Resize damage information.
        self.damage.resize(num_cols, num_lines);

        // Emit in-band resize report when DECSET 2048 is active.
        if self.mode.contains(TermMode::IN_BAND_RESIZE) {
            let pixel_h = (num_lines as u32) * (self.cell_height as u32);
            let pixel_w = (num_cols as u32) * (self.cell_width as u32);
            self.event_proxy.send_event(Event::PtyWrite(format!(
                "\x1b[48;{num_lines};{num_cols};{pixel_h};{pixel_w}t"
            )));
        }
    }

    /// Swap primary and alternate screen buffer.
    pub fn swap_alt(&mut self) {
        let entering_alt = !self.mode.contains(TermMode::ALT_SCREEN);
        if entering_alt {
            // Set alt screen cursor to the current primary screen cursor.
            self.inactive_grid.cursor = self.grid.cursor.clone();

            // Drop information about the primary screens saved cursor.
            self.grid.saved_cursor = self.grid.cursor.clone();

            // Reset alternate screen contents.
            self.inactive_grid.reset_region(..);
        }

        mem::swap(
            &mut self.keyboard_mode_stack,
            &mut self.inactive_keyboard_mode_stack,
        );
        let keyboard_mode = self
            .keyboard_mode_stack
            .last()
            .copied()
            .unwrap_or(KeyboardModes::NO_MODE)
            .into();
        self.set_keyboard_mode(keyboard_mode, KeyboardModesApplyBehavior::Replace);

        mem::swap(&mut self.grid, &mut self.inactive_grid);
        self.kitty_graphics.swap_alt(entering_alt);
        self.mode ^= TermMode::ALT_SCREEN;
        self.selection = None;
        self.mark_fully_damaged();
    }

    /// Scroll screen down.
    ///
    /// Text moves down; clear at bottom
    /// Expects origin to be in scroll range.
    #[inline]
    pub(super) fn scroll_down_relative(&mut self, origin: Line, mut lines: usize) {
        trace!("Scrolling down relative: origin={origin}, lines={lines}");

        lines = cmp::min(
            lines,
            (self.scroll_region.end - self.scroll_region.start).0 as usize,
        );
        lines = cmp::min(lines, (self.scroll_region.end - origin).0 as usize);

        let region = origin..self.scroll_region.end;

        // Scroll selection.
        self.selection = self
            .selection
            .take()
            .and_then(|s| s.rotate(self, &region, -(lines as i32)));

        // Scroll kitty placements.
        self.kitty_graphics
            .rotate_placements(&region, -(lines as i32));

        // Scroll vi mode cursor.
        let line = &mut self.vi_mode_cursor.point.line;
        if region.start <= *line && region.end > *line {
            *line = cmp::min(*line + lines, region.end - 1);
        }

        // Scroll between origin and bottom
        self.grid.scroll_down(&region, lines);
        self.mark_fully_damaged();
    }

    /// Scroll screen up
    ///
    /// Text moves up; clear at top
    /// Expects origin to be in scroll range.
    #[inline]
    pub(super) fn scroll_up_relative(&mut self, origin: Line, mut lines: usize) {
        trace!("Scrolling up relative: origin={origin}, lines={lines}");

        lines = cmp::min(
            lines,
            (self.scroll_region.end - self.scroll_region.start).0 as usize,
        );

        let region = origin..self.scroll_region.end;

        // Scroll selection.
        self.selection = self
            .selection
            .take()
            .and_then(|s| s.rotate(self, &region, lines as i32));

        // Scroll kitty placements.
        self.kitty_graphics.rotate_placements(&region, lines as i32);

        self.grid.scroll_up(&region, lines);

        // Scroll vi mode cursor.
        let viewport_top = Line(-(self.grid.display_offset() as i32));
        let top = if region.start == 0 {
            viewport_top
        } else {
            region.start
        };
        let line = &mut self.vi_mode_cursor.point.line;
        if (top <= *line) && region.end > *line {
            *line = cmp::max(*line - lines, top);
        }
        self.mark_fully_damaged();
    }

    pub(super) fn deccolm(&mut self)
    where
        T: EventListener,
    {
        // Setting 132 column font makes no sense, but run the other side effects.
        // Clear scrolling region.
        self.set_scrolling_region(1, None);

        // Clear grid.
        self.grid.reset_region(..);
        self.mark_fully_damaged();
    }
}
