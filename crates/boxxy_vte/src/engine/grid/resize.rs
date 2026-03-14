//! Grid resize and reflow.

use std::cmp::{Ordering, max, min};
use std::mem;

use crate::engine::index::{Boundary, Column, Line};
use crate::engine::term::cell::{Flags, ResetDiscriminant};

use crate::engine::grid::row::Row;
use crate::engine::grid::{Dimensions, Grid, GridCell};

impl<T: GridCell + Default + PartialEq> Grid<T> {
    /// Resize the grid's width and/or height.
    pub fn resize<D>(&mut self, reflow: bool, lines: usize, columns: usize) -> i32
    where
        T: ResetDiscriminant<D>,
        D: PartialEq,
    {
        // Use empty template cell for resetting cells due to resize.
        let template = mem::take(&mut self.cursor.template);

        let line_delta = match self.lines.cmp(&lines) {
            Ordering::Less => self.grow_lines(lines),
            Ordering::Greater => self.shrink_lines(lines),
            Ordering::Equal => 0,
        };

        match self.columns.cmp(&columns) {
            Ordering::Less => self.grow_columns(reflow, columns),
            Ordering::Greater => self.shrink_columns(reflow, columns),
            Ordering::Equal => (),
        }

        // Restore template cell.
        self.cursor.template = template;

        line_delta
    }

    /// Add lines to the visible area.
    ///
    /// If the cursor is at the bottom (shell mode), history lines are pulled in
    /// from the top to fill the new rows (bottom-anchored). Otherwise, new blank
    /// rows are appended at the bottom without disturbing the visible content
    /// (top-anchored).
    fn grow_lines<D>(&mut self, target: usize) -> i32
    where
        T: ResetDiscriminant<D>,
        D: PartialEq,
    {
        let old_lines = self.lines;
        let lines_added = target - old_lines;

        // Need to resize before updating buffer.
        self.raw.grow_visible_lines(target);
        self.lines = target;

        // grow_visible_lines increases visible_lines, which shifts the logical→physical
        // mapping so that `lines_added` new blank rows appear at the TOP of the viewport
        // (Line 0..lines_added-1) and all existing content shifts down by lines_added.
        //
        // We correct this with a net rotation of (from_history - lines_added):
        //   - The negative part scrolls new blank rows to the bottom.
        //   - The positive part (from_history) promotes that many history lines to the
        //     top of the visible area (already there after grow_visible_lines, just need
        //     the cursor/delta adjustment).
        //
        // Cursor-aware policy:
        //   - Bottom (shell mode): pull from history, move cursor down.
        //   - Not at bottom: top-anchored, no history pull, cursor stays put.
        let cursor_at_bottom = self.cursor.point.line.0 as usize == old_lines - 1;
        let from_history = if cursor_at_bottom {
            min(self.history_size(), lines_added)
        } else {
            0
        };

        let rotation = from_history as isize - lines_added as isize;
        self.raw.rotate(rotation);

        let line_delta = from_history as i32;
        if from_history > 0 {
            self.saved_cursor.point.line += from_history;
            self.cursor.point.line += from_history;
            self.decrease_scroll_limit(from_history);
        }

        self.display_offset = self.display_offset.saturating_sub(lines_added);

        line_delta
    }

    /// Remove lines from the visible area.
    ///
    /// Ghostty-style cursor-aware shrinking: trailing blank rows at the bottom
    /// of the viewport are trimmed first without touching scrollback history.
    /// Scroll up (pushing to history) only happens when non-blank content would
    /// otherwise be cut off below the cursor.
    ///
    /// The `rotate(shrinkage)` + `shrink_visible_lines(target)` pair correctly
    /// removes rows from the bottom: `rotate` advances `zero` by `shrinkage` so
    /// those bottom rows end up at the highest positive offsets, and
    /// `shrink_visible_lines` then drops exactly them.
    fn shrink_lines<D>(&mut self, target: usize) -> i32
    where
        T: ResetDiscriminant<D>,
        D: PartialEq,
    {
        let shrinkage = self.lines - target;
        let mut line_delta = 0;

        // Count consecutive blank rows from the bottom of the visible area.
        let trailing_blanks = (0..self.lines)
            .rev()
            .take_while(|&i| self.raw[Line(i as i32)].is_clear())
            .count();

        // If there are enough trailing blank rows to cover the entire shrinkage,
        // we trim them silently without creating any history. Otherwise scroll up
        // just enough to keep the cursor inside the new viewport.
        if trailing_blanks < shrinkage {
            let required_scrolling =
                (self.cursor.point.line.0 as usize + 1).saturating_sub(target);
            if required_scrolling > 0 {
                self.scroll_up(&(Line(0)..Line(self.lines as i32)), required_scrolling);
                self.cursor.point.line =
                    min(self.cursor.point.line, Line(target as i32 - 1));
                line_delta -= required_scrolling as i32;
            }
        }

        // Clamp both cursors to the new viewport (covers the case where the cursor
        // was sitting inside the blank trailing zone that is about to be trimmed).
        self.cursor.point.line = min(self.cursor.point.line, Line(target as i32 - 1));
        self.saved_cursor.point.line = min(self.saved_cursor.point.line, Line(target as i32 - 1));

        // Remove `shrinkage` rows from the bottom (including trailing blanks).
        self.raw.rotate(shrinkage as isize);
        self.raw.shrink_visible_lines(target);
        self.lines = target;

        line_delta
    }

    /// Grow number of columns in each row, reflowing if necessary.
    fn grow_columns(&mut self, reflow: bool, columns: usize) {
        // Check if a row needs to be wrapped.
        let should_reflow = |row: &Row<T>| -> bool {
            let len = Column(row.len());
            reflow && len.0 > 0 && len < columns && row[len - 1].flags().contains(Flags::WRAPLINE)
        };

        self.columns = columns;

        let mut reversed: Vec<Row<T>> = Vec::with_capacity(self.raw.len());
        let mut cursor_line_delta = 0;

        // Remove the linewrap special case, by moving the cursor outside of the grid.
        if self.cursor.input_needs_wrap && reflow {
            self.cursor.input_needs_wrap = false;
            self.cursor.point.column += 1;
        }

        let mut rows = self.raw.take_all();

        for (i, mut row) in rows.drain(..).enumerate().rev() {
            // Check if reflowing should be performed.
            let last_row = match reversed.last_mut() {
                Some(last_row) if should_reflow(last_row) => last_row,
                _ => {
                    reversed.push(row);
                    continue;
                },
            };

            // Remove wrap flag before appending additional cells.
            if let Some(cell) = last_row.last_mut() {
                cell.flags_mut().remove(Flags::WRAPLINE);
            }

            // Remove leading spacers when reflowing wide char to the previous line.
            let mut last_len = last_row.len();
            if last_len >= 1
                && last_row[Column(last_len - 1)].flags().contains(Flags::LEADING_WIDE_CHAR_SPACER)
            {
                last_row.shrink(last_len - 1);
                last_len -= 1;
            }

            // Don't try to pull more cells from the next line than available.
            let mut num_wrapped = columns - last_len;
            let len = min(row.len(), num_wrapped);

            // Insert leading spacer when there's not enough room for reflowing wide char.
            let mut cells = if row[Column(len - 1)].flags().contains(Flags::WIDE_CHAR) {
                num_wrapped -= 1;

                let mut cells = row.front_split_off(len - 1);

                let mut spacer = T::default();
                spacer.flags_mut().insert(Flags::LEADING_WIDE_CHAR_SPACER);
                cells.push(spacer);

                cells
            } else {
                row.front_split_off(len)
            };

            // Add removed cells to previous row and reflow content.
            last_row.append(&mut cells);

            let cursor_buffer_line = self.lines - self.cursor.point.line.0 as usize - 1;

            if i == cursor_buffer_line && reflow {
                // Resize cursor's line and reflow the cursor if necessary.
                let mut target = self.cursor.point.sub(self, Boundary::Cursor, num_wrapped);

                // Clamp to the last column, if no content was reflown with the cursor.
                if target.column.0 == 0 && row.is_clear() {
                    self.cursor.input_needs_wrap = true;
                    target = target.sub(self, Boundary::Cursor, 1);
                }
                self.cursor.point.column = target.column;

                // Get required cursor line changes. Since `num_wrapped` is smaller than `columns`
                // this will always be either `0` or `1`.
                let line_delta = self.cursor.point.line - target.line;

                if line_delta != 0 && row.is_clear() {
                    continue;
                }

                cursor_line_delta += line_delta.0 as usize;
            } else if row.is_clear() {
                if i < self.display_offset {
                    // Since we removed a line, rotate down the viewport.
                    self.display_offset = self.display_offset.saturating_sub(1);
                }

                // Rotate cursor down if content below them was pulled from history.
                if i < cursor_buffer_line {
                    self.cursor.point.line += 1;
                }

                // Don't push line into the new buffer.
                continue;
            }

            if let Some(cell) = last_row.last_mut() {
                // Set wrap flag if next line still has cells.
                cell.flags_mut().insert(Flags::WRAPLINE);
            }

            reversed.push(row);
        }

        // Make sure we have at least the viewport filled.
        if reversed.len() < self.lines {
            let delta = (self.lines - reversed.len()) as i32;
            self.cursor.point.line = max(self.cursor.point.line - delta, Line(0));
            reversed.resize_with(self.lines, || Row::new(columns));
        }

        // Pull content down to put cursor in correct position, or move cursor up if there's no
        // more lines to delete below the cursor.
        if cursor_line_delta != 0 {
            let cursor_buffer_line = self.lines - self.cursor.point.line.0 as usize - 1;
            let available = min(cursor_buffer_line, reversed.len() - self.lines);
            let overflow = cursor_line_delta.saturating_sub(available);
            reversed.truncate(reversed.len() + overflow - cursor_line_delta);
            self.cursor.point.line = max(self.cursor.point.line - overflow, Line(0));
        }

        // Reverse iterator and fill all rows that are still too short.
        let mut new_raw = Vec::with_capacity(reversed.len());
        for mut row in reversed.drain(..).rev() {
            if row.len() < columns {
                row.grow(columns);
            }
            new_raw.push(row);
        }

        self.raw.replace_inner(new_raw);

        // Clamp display offset in case lines above it got merged.
        self.display_offset = min(self.display_offset, self.history_size());
    }

    /// Shrink number of columns in each row, reflowing if necessary.
    fn shrink_columns(&mut self, reflow: bool, columns: usize) {
        self.columns = columns;

        // Remove the linewrap special case, by moving the cursor outside of the grid.
        if self.cursor.input_needs_wrap && reflow {
            self.cursor.input_needs_wrap = false;
            self.cursor.point.column += 1;
        }

        let mut new_raw = Vec::with_capacity(self.raw.len());
        let mut buffered: Option<Vec<T>> = None;

        let mut rows = self.raw.take_all();
        for (i, mut row) in rows.drain(..).enumerate().rev() {
            // Append lines left over from the previous row.
            if let Some(buffered) = buffered.take() {
                // Add a column for every cell added before the cursor, if it goes beyond the new
                // width it is then later reflown.
                let cursor_buffer_line = self.lines - self.cursor.point.line.0 as usize - 1;
                if i == cursor_buffer_line {
                    self.cursor.point.column += buffered.len();
                }

                row.append_front(buffered);
            }

            loop {
                // Remove all cells which require reflowing.
                let mut wrapped = match row.shrink(columns) {
                    Some(wrapped) if reflow => wrapped,
                    _ => {
                        let cursor_buffer_line = self.lines - self.cursor.point.line.0 as usize - 1;
                        if reflow && i == cursor_buffer_line && self.cursor.point.column > columns {
                            // If there are empty cells before the cursor, we assume it is explicit
                            // whitespace and need to wrap it like normal content.
                            Vec::new()
                        } else {
                            // Since it fits, just push the existing line without any reflow.
                            new_raw.push(row);
                            break;
                        }
                    },
                };

                // Insert spacer if a wide char would be wrapped into the last column.
                if row.len() >= columns
                    && row[Column(columns - 1)].flags().contains(Flags::WIDE_CHAR)
                {
                    let mut spacer = T::default();
                    spacer.flags_mut().insert(Flags::LEADING_WIDE_CHAR_SPACER);

                    let wide_char = mem::replace(&mut row[Column(columns - 1)], spacer);
                    wrapped.insert(0, wide_char);
                }

                // Remove wide char spacer before shrinking.
                let len = wrapped.len();
                if len > 0 && wrapped[len - 1].flags().contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                    if len == 1 {
                        row[Column(columns - 1)].flags_mut().insert(Flags::WRAPLINE);
                        new_raw.push(row);
                        break;
                    } else {
                        // Remove the leading spacer from the end of the wrapped row.
                        wrapped[len - 2].flags_mut().insert(Flags::WRAPLINE);
                        wrapped.truncate(len - 1);
                    }
                }

                new_raw.push(row);

                // Set line as wrapped if cells got removed.
                if let Some(cell) = new_raw.last_mut().and_then(|r| r.last_mut()) {
                    cell.flags_mut().insert(Flags::WRAPLINE);
                }

                if wrapped
                    .last()
                    .map(|c| c.flags().contains(Flags::WRAPLINE) && i >= 1)
                    .unwrap_or(false)
                    && wrapped.len() < columns
                {
                    // Make sure previous wrap flag doesn't linger around.
                    if let Some(cell) = wrapped.last_mut() {
                        cell.flags_mut().remove(Flags::WRAPLINE);
                    }

                    // Add removed cells to start of next row.
                    buffered = Some(wrapped);
                    break;
                } else {
                    // Reflow cursor if a line below it is deleted.
                    let cursor_buffer_line = self.lines - self.cursor.point.line.0 as usize - 1;
                    if (i == cursor_buffer_line && self.cursor.point.column < columns)
                        || i < cursor_buffer_line
                    {
                        self.cursor.point.line = max(self.cursor.point.line - 1, Line(0));
                    }

                    // Reflow the cursor if it is on this line beyond the width.
                    if i == cursor_buffer_line && self.cursor.point.column >= columns {
                        // Since only a single new line is created, we subtract only `columns`
                        // from the cursor instead of reflowing it completely.
                        self.cursor.point.column -= columns;
                    }

                    // Make sure new row is at least as long as new width.
                    let occ = wrapped.len();
                    if occ < columns {
                        wrapped.resize_with(columns, T::default);
                    }
                    row = Row::from_vec(wrapped, occ);

                    if i < self.display_offset {
                        // Since we added a new line, rotate up the viewport.
                        self.display_offset += 1;
                    }
                }
            }
        }

        // When reflow splits rows, new_raw can grow beyond self.lines, pushing
        // the top-most visible content into scrollback history.  Before that
        // happens, try to absorb the excess by trimming blank rows from the
        // bottom of the viewport (end of new_raw) — the same strategy used by
        // shrink_lines.  Only rows *below* the cursor are eligible; content at
        // or above the cursor must never be silently discarded.
        if reflow {
            let excess = new_raw.len().saturating_sub(self.lines);
            if excess > 0 {
                let cursor_line = self.cursor.point.line.0 as usize;
                let rows_below_cursor = self.lines.saturating_sub(1 + cursor_line);
                let trailing_blanks =
                    new_raw.iter().rev().take_while(|r| r.is_clear()).count();
                let trim = excess.min(trailing_blanks).min(rows_below_cursor);
                if trim > 0 {
                    new_raw.truncate(new_raw.len() - trim);
                    // Trimming blank rows below the cursor shifts all content
                    // (including the cursor) toward the bottom of the viewport.
                    self.cursor.point.line =
                        min(self.cursor.point.line + trim, Line(self.lines as i32 - 1));
                    self.saved_cursor.point.line = min(
                        self.saved_cursor.point.line + trim,
                        Line(self.lines as i32 - 1),
                    );
                }
            }
        }

        // Reverse iterator and use it as the new grid storage.
        let mut reversed: Vec<Row<T>> = new_raw.drain(..).rev().collect();
        reversed.truncate(self.max_scroll_limit + self.lines);
        self.raw.replace_inner(reversed);

        // Clamp display offset in case some lines went off.
        self.display_offset = min(self.display_offset, self.history_size());

        // Reflow the primary cursor, or clamp it if reflow is disabled.
        if !reflow {
            self.cursor.point.column = min(self.cursor.point.column, Column(columns - 1));
        } else if self.cursor.point.column == columns
            && !self[self.cursor.point.line][Column(columns - 1)].flags().contains(Flags::WRAPLINE)
        {
            self.cursor.input_needs_wrap = true;
            self.cursor.point.column -= 1;
        } else {
            self.cursor.point = self.cursor.point.grid_clamp(self, Boundary::Cursor);
        }

        // Clamp the saved cursor to the grid.
        self.saved_cursor.point.column = min(self.saved_cursor.point.column, Column(columns - 1));
    }
}
