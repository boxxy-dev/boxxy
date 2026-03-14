//! Tests for the Grid.

use super::*;

use crate::engine::term::cell::Cell;

impl GridCell for usize {
    fn is_empty(&self) -> bool {
        *self == 0
    }

    fn reset(&mut self, template: &Self) {
        *self = *template;
    }

    fn flags(&self) -> &Flags {
        unimplemented!();
    }

    fn flags_mut(&mut self) -> &mut Flags {
        unimplemented!();
    }
}

// Scroll up moves lines upward.
#[test]
fn scroll_up() {
    let mut grid = Grid::<usize>::new(10, 1, 0);
    for i in 0..10 {
        grid[Line(i as i32)][Column(0)] = i;
    }

    grid.scroll_up::<usize>(&(Line(0)..Line(10)), 2);

    assert_eq!(grid[Line(0)][Column(0)], 2);
    assert_eq!(grid[Line(0)].occ, 1);
    assert_eq!(grid[Line(1)][Column(0)], 3);
    assert_eq!(grid[Line(1)].occ, 1);
    assert_eq!(grid[Line(2)][Column(0)], 4);
    assert_eq!(grid[Line(2)].occ, 1);
    assert_eq!(grid[Line(3)][Column(0)], 5);
    assert_eq!(grid[Line(3)].occ, 1);
    assert_eq!(grid[Line(4)][Column(0)], 6);
    assert_eq!(grid[Line(4)].occ, 1);
    assert_eq!(grid[Line(5)][Column(0)], 7);
    assert_eq!(grid[Line(5)].occ, 1);
    assert_eq!(grid[Line(6)][Column(0)], 8);
    assert_eq!(grid[Line(6)].occ, 1);
    assert_eq!(grid[Line(7)][Column(0)], 9);
    assert_eq!(grid[Line(7)].occ, 1);
    assert_eq!(grid[Line(8)][Column(0)], 0); // was 0.
    assert_eq!(grid[Line(8)].occ, 0);
    assert_eq!(grid[Line(9)][Column(0)], 0); // was 1.
    assert_eq!(grid[Line(9)].occ, 0);
}

// Scroll down moves lines downward.
#[test]
fn scroll_down() {
    let mut grid = Grid::<usize>::new(10, 1, 0);
    for i in 0..10 {
        grid[Line(i as i32)][Column(0)] = i;
    }

    grid.scroll_down::<usize>(&(Line(0)..Line(10)), 2);

    assert_eq!(grid[Line(0)][Column(0)], 0); // was 8.
    assert_eq!(grid[Line(0)].occ, 0);
    assert_eq!(grid[Line(1)][Column(0)], 0); // was 9.
    assert_eq!(grid[Line(1)].occ, 0);
    assert_eq!(grid[Line(2)][Column(0)], 0);
    assert_eq!(grid[Line(2)].occ, 1);
    assert_eq!(grid[Line(3)][Column(0)], 1);
    assert_eq!(grid[Line(3)].occ, 1);
    assert_eq!(grid[Line(4)][Column(0)], 2);
    assert_eq!(grid[Line(4)].occ, 1);
    assert_eq!(grid[Line(5)][Column(0)], 3);
    assert_eq!(grid[Line(5)].occ, 1);
    assert_eq!(grid[Line(6)][Column(0)], 4);
    assert_eq!(grid[Line(6)].occ, 1);
    assert_eq!(grid[Line(7)][Column(0)], 5);
    assert_eq!(grid[Line(7)].occ, 1);
    assert_eq!(grid[Line(8)][Column(0)], 6);
    assert_eq!(grid[Line(8)].occ, 1);
    assert_eq!(grid[Line(9)][Column(0)], 7);
    assert_eq!(grid[Line(9)].occ, 1);
}

#[test]
fn scroll_down_with_history() {
    let mut grid = Grid::<usize>::new(10, 1, 1);
    grid.increase_scroll_limit(1);
    for i in 0..10 {
        grid[Line(i as i32)][Column(0)] = i;
    }

    grid.scroll_down::<usize>(&(Line(0)..Line(10)), 2);

    assert_eq!(grid[Line(0)][Column(0)], 0); // was 8.
    assert_eq!(grid[Line(0)].occ, 0);
    assert_eq!(grid[Line(1)][Column(0)], 0); // was 9.
    assert_eq!(grid[Line(1)].occ, 0);
    assert_eq!(grid[Line(2)][Column(0)], 0);
    assert_eq!(grid[Line(2)].occ, 1);
    assert_eq!(grid[Line(3)][Column(0)], 1);
    assert_eq!(grid[Line(3)].occ, 1);
    assert_eq!(grid[Line(4)][Column(0)], 2);
    assert_eq!(grid[Line(4)].occ, 1);
    assert_eq!(grid[Line(5)][Column(0)], 3);
    assert_eq!(grid[Line(5)].occ, 1);
    assert_eq!(grid[Line(6)][Column(0)], 4);
    assert_eq!(grid[Line(6)].occ, 1);
    assert_eq!(grid[Line(7)][Column(0)], 5);
    assert_eq!(grid[Line(7)].occ, 1);
    assert_eq!(grid[Line(8)][Column(0)], 6);
    assert_eq!(grid[Line(8)].occ, 1);
    assert_eq!(grid[Line(9)][Column(0)], 7);
    assert_eq!(grid[Line(9)].occ, 1);
}

// Test that GridIterator works.
#[test]
fn test_iter() {
    let assert_indexed = |value: usize, indexed: Option<Indexed<&usize>>| {
        assert_eq!(Some(&value), indexed.map(|indexed| indexed.cell));
    };

    let mut grid = Grid::<usize>::new(5, 5, 0);
    for i in 0..5 {
        for j in 0..5 {
            grid[Line(i)][Column(j)] = i as usize * 5 + j;
        }
    }

    let mut iter = grid.iter_from(Point::new(Line(0), Column(0)));

    assert_eq!(None, iter.prev());
    assert_indexed(1, iter.next());
    assert_eq!(Column(1), iter.point().column);
    assert_eq!(0, iter.point().line);

    assert_indexed(2, iter.next());
    assert_indexed(3, iter.next());
    assert_indexed(4, iter.next());

    // Test line-wrapping.
    assert_indexed(5, iter.next());
    assert_eq!(Column(0), iter.point().column);
    assert_eq!(1, iter.point().line);

    assert_indexed(4, iter.prev());
    assert_eq!(Column(4), iter.point().column);
    assert_eq!(0, iter.point().line);

    // Make sure iter.cell() returns the current iterator position.
    assert_eq!(&4, iter.cell());

    // Test that iter ends at end of grid.
    let mut final_iter = grid.iter_from(Point { line: Line(4), column: Column(4) });
    assert_eq!(None, final_iter.next());
    assert_indexed(23, final_iter.prev());
}

#[test]
fn shrink_reflow() {
    let mut grid = Grid::<Cell>::new(1, 5, 2);
    grid[Line(0)][Column(0)] = cell('1');
    grid[Line(0)][Column(1)] = cell('2');
    grid[Line(0)][Column(2)] = cell('3');
    grid[Line(0)][Column(3)] = cell('4');
    grid[Line(0)][Column(4)] = cell('5');

    grid.resize(true, 1, 2);

    assert_eq!(grid.total_lines(), 3);

    assert_eq!(grid[Line(-2)].len(), 2);
    assert_eq!(grid[Line(-2)][Column(0)], cell('1'));
    assert_eq!(grid[Line(-2)][Column(1)], wrap_cell('2'));

    assert_eq!(grid[Line(-1)].len(), 2);
    assert_eq!(grid[Line(-1)][Column(0)], cell('3'));
    assert_eq!(grid[Line(-1)][Column(1)], wrap_cell('4'));

    assert_eq!(grid[Line(0)].len(), 2);
    assert_eq!(grid[Line(0)][Column(0)], cell('5'));
    assert_eq!(grid[Line(0)][Column(1)], Cell::default());
}

#[test]
fn shrink_reflow_twice() {
    let mut grid = Grid::<Cell>::new(1, 5, 2);
    grid[Line(0)][Column(0)] = cell('1');
    grid[Line(0)][Column(1)] = cell('2');
    grid[Line(0)][Column(2)] = cell('3');
    grid[Line(0)][Column(3)] = cell('4');
    grid[Line(0)][Column(4)] = cell('5');

    grid.resize(true, 1, 4);
    grid.resize(true, 1, 2);

    assert_eq!(grid.total_lines(), 3);

    assert_eq!(grid[Line(-2)].len(), 2);
    assert_eq!(grid[Line(-2)][Column(0)], cell('1'));
    assert_eq!(grid[Line(-2)][Column(1)], wrap_cell('2'));

    assert_eq!(grid[Line(-1)].len(), 2);
    assert_eq!(grid[Line(-1)][Column(0)], cell('3'));
    assert_eq!(grid[Line(-1)][Column(1)], wrap_cell('4'));

    assert_eq!(grid[Line(0)].len(), 2);
    assert_eq!(grid[Line(0)][Column(0)], cell('5'));
    assert_eq!(grid[Line(0)][Column(1)], Cell::default());
}

#[test]
fn shrink_reflow_empty_cell_inside_line() {
    let mut grid = Grid::<Cell>::new(1, 5, 3);
    grid[Line(0)][Column(0)] = cell('1');
    grid[Line(0)][Column(1)] = Cell::default();
    grid[Line(0)][Column(2)] = cell('3');
    grid[Line(0)][Column(3)] = cell('4');
    grid[Line(0)][Column(4)] = Cell::default();

    grid.resize(true, 1, 2);

    assert_eq!(grid.total_lines(), 2);

    assert_eq!(grid[Line(-1)].len(), 2);
    assert_eq!(grid[Line(-1)][Column(0)], cell('1'));
    assert_eq!(grid[Line(-1)][Column(1)], wrap_cell(' '));

    assert_eq!(grid[Line(0)].len(), 2);
    assert_eq!(grid[Line(0)][Column(0)], cell('3'));
    assert_eq!(grid[Line(0)][Column(1)], cell('4'));

    grid.resize(true, 1, 1);

    assert_eq!(grid.total_lines(), 4);

    assert_eq!(grid[Line(-3)].len(), 1);
    assert_eq!(grid[Line(-3)][Column(0)], wrap_cell('1'));

    assert_eq!(grid[Line(-2)].len(), 1);
    assert_eq!(grid[Line(-2)][Column(0)], wrap_cell(' '));

    assert_eq!(grid[Line(-1)].len(), 1);
    assert_eq!(grid[Line(-1)][Column(0)], wrap_cell('3'));

    assert_eq!(grid[Line(0)].len(), 1);
    assert_eq!(grid[Line(0)][Column(0)], cell('4'));
}

#[test]
fn grow_reflow() {
    let mut grid = Grid::<Cell>::new(2, 2, 0);
    grid[Line(0)][Column(0)] = cell('1');
    grid[Line(0)][Column(1)] = wrap_cell('2');
    grid[Line(1)][Column(0)] = cell('3');
    grid[Line(1)][Column(1)] = Cell::default();

    grid.resize(true, 2, 3);

    assert_eq!(grid.total_lines(), 2);

    assert_eq!(grid[Line(0)].len(), 3);
    assert_eq!(grid[Line(0)][Column(0)], cell('1'));
    assert_eq!(grid[Line(0)][Column(1)], cell('2'));
    assert_eq!(grid[Line(0)][Column(2)], cell('3'));

    // Make sure rest of grid is empty.
    assert_eq!(grid[Line(1)].len(), 3);
    assert_eq!(grid[Line(1)][Column(0)], Cell::default());
    assert_eq!(grid[Line(1)][Column(1)], Cell::default());
    assert_eq!(grid[Line(1)][Column(2)], Cell::default());
}

#[test]
fn grow_reflow_multiline() {
    let mut grid = Grid::<Cell>::new(3, 2, 0);
    grid[Line(0)][Column(0)] = cell('1');
    grid[Line(0)][Column(1)] = wrap_cell('2');
    grid[Line(1)][Column(0)] = cell('3');
    grid[Line(1)][Column(1)] = wrap_cell('4');
    grid[Line(2)][Column(0)] = cell('5');
    grid[Line(2)][Column(1)] = cell('6');

    grid.resize(true, 3, 6);

    assert_eq!(grid.total_lines(), 3);

    assert_eq!(grid[Line(0)].len(), 6);
    assert_eq!(grid[Line(0)][Column(0)], cell('1'));
    assert_eq!(grid[Line(0)][Column(1)], cell('2'));
    assert_eq!(grid[Line(0)][Column(2)], cell('3'));
    assert_eq!(grid[Line(0)][Column(3)], cell('4'));
    assert_eq!(grid[Line(0)][Column(4)], cell('5'));
    assert_eq!(grid[Line(0)][Column(5)], cell('6'));

    // Make sure rest of grid is empty.
    for r in (1..3).map(Line::from) {
        assert_eq!(grid[r].len(), 6);
        for c in 0..6 {
            assert_eq!(grid[r][Column(c)], Cell::default());
        }
    }
}

#[test]
fn grow_reflow_disabled() {
    let mut grid = Grid::<Cell>::new(2, 2, 0);
    grid[Line(0)][Column(0)] = cell('1');
    grid[Line(0)][Column(1)] = wrap_cell('2');
    grid[Line(1)][Column(0)] = cell('3');
    grid[Line(1)][Column(1)] = Cell::default();

    grid.resize(false, 2, 3);

    assert_eq!(grid.total_lines(), 2);

    assert_eq!(grid[Line(0)].len(), 3);
    assert_eq!(grid[Line(0)][Column(0)], cell('1'));
    assert_eq!(grid[Line(0)][Column(1)], wrap_cell('2'));
    assert_eq!(grid[Line(0)][Column(2)], Cell::default());

    assert_eq!(grid[Line(1)].len(), 3);
    assert_eq!(grid[Line(1)][Column(0)], cell('3'));
    assert_eq!(grid[Line(1)][Column(1)], Cell::default());
    assert_eq!(grid[Line(1)][Column(2)], Cell::default());
}

#[test]
fn shrink_reflow_disabled() {
    let mut grid = Grid::<Cell>::new(1, 5, 2);
    grid[Line(0)][Column(0)] = cell('1');
    grid[Line(0)][Column(1)] = cell('2');
    grid[Line(0)][Column(2)] = cell('3');
    grid[Line(0)][Column(3)] = cell('4');
    grid[Line(0)][Column(4)] = cell('5');

    grid.resize(false, 1, 2);

    assert_eq!(grid.total_lines(), 1);

    assert_eq!(grid[Line(0)].len(), 2);
    assert_eq!(grid[Line(0)][Column(0)], cell('1'));
    assert_eq!(grid[Line(0)][Column(1)], cell('2'));
}

#[test]
fn accurate_size_hint() {
    let grid = Grid::<Cell>::new(5, 5, 2);

    size_hint_matches_count(grid.iter_from(Point::new(Line(0), Column(0))));
    size_hint_matches_count(grid.iter_from(Point::new(Line(2), Column(3))));
    size_hint_matches_count(grid.iter_from(Point::new(Line(4), Column(4))));
    size_hint_matches_count(grid.iter_from(Point::new(Line(4), Column(2))));
    size_hint_matches_count(grid.iter_from(Point::new(Line(10), Column(10))));
    size_hint_matches_count(grid.iter_from(Point::new(Line(2), Column(10))));

    let mut iterator = grid.iter_from(Point::new(Line(3), Column(1)));
    iterator.next();
    iterator.next();
    size_hint_matches_count(iterator);

    size_hint_matches_count(grid.display_iter());
}

fn size_hint_matches_count<T>(iter: impl Iterator<Item = T>) {
    let iterator = iter.into_iter();
    let (lower, upper) = iterator.size_hint();
    let count = iterator.count();
    assert_eq!(lower, count);
    assert_eq!(upper, Some(count));
}

// https://github.com/rust-lang/rust-clippy/pull/6375
#[allow(clippy::all)]
fn cell(c: char) -> Cell {
    let mut cell = Cell::default();
    cell.c = c;
    cell
}

fn wrap_cell(c: char) -> Cell {
    let mut cell = cell(c);
    cell.flags.insert(Flags::WRAPLINE);
    cell
}

// Horizontal grow: content on last line should survive widening.
#[test]
fn grow_columns_preserves_content() {
    // 5 lines, 10 cols, no scrollback.
    let mut grid = Grid::<Cell>::new(5, 10, 0);
    // "ls" at Line(4) (bottom), cursor after it.
    grid[Line(4)][Column(0)] = cell('l');
    grid[Line(4)][Column(1)] = cell('s');
    grid.cursor.point.line = Line(4);
    grid.cursor.point.column = Column(2);

    grid.resize(true, 5, 15);

    // Lines above should be blank.
    for r in 0..4 {
        for c in 0..15 {
            assert_eq!(grid[Line(r)][Column(c)], Cell::default(), "row {r} col {c} should be blank");
        }
    }
    // Bottom line must still hold "ls".
    assert_eq!(grid[Line(4)][Column(0)], cell('l'), "col 0 should be 'l'");
    assert_eq!(grid[Line(4)][Column(1)], cell('s'), "col 1 should be 's'");
}

// Horizontal shrink: content on last line should survive narrowing.
#[test]
fn shrink_columns_preserves_content() {
    // 5 lines, 10 cols, no scrollback.
    let mut grid = Grid::<Cell>::new(5, 10, 0);
    // "ls" at Line(4) (bottom), cursor after it.
    grid[Line(4)][Column(0)] = cell('l');
    grid[Line(4)][Column(1)] = cell('s');
    grid.cursor.point.line = Line(4);
    grid.cursor.point.column = Column(2);

    grid.resize(true, 5, 8);

    // "ls" should survive at the bottom.
    assert_eq!(grid[Line(4)][Column(0)], cell('l'), "col 0 should be 'l'");
    assert_eq!(grid[Line(4)][Column(1)], cell('s'), "col 1 should be 's'");
}

// Grow then shrink: content must survive the round-trip.
#[test]
fn grow_then_shrink_columns_preserves_content() {
    let mut grid = Grid::<Cell>::new(5, 10, 0);
    grid[Line(4)][Column(0)] = cell('l');
    grid[Line(4)][Column(1)] = cell('s');
    grid.cursor.point.line = Line(4);
    grid.cursor.point.column = Column(2);

    grid.resize(true, 5, 15);
    grid.resize(true, 5, 10);

    assert_eq!(grid[Line(4)][Column(0)], cell('l'), "col 0 should be 'l' after round-trip");
    assert_eq!(grid[Line(4)][Column(1)], cell('s'), "col 1 should be 's' after round-trip");
}

// Content at the bottom survives both vertical and horizontal grow simultaneously.
#[test]
fn grow_lines_and_columns_preserves_content() {
    // 5 lines, 10 cols.  "ls" at the bottom, cursor at bottom.
    let mut grid = Grid::<Cell>::new(5, 10, 10);
    grid[Line(4)][Column(0)] = cell('l');
    grid[Line(4)][Column(1)] = cell('s');
    grid.cursor.point.line = Line(4);
    grid.cursor.point.column = Column(2);

    // Grow both dimensions.
    grid.resize(true, 8, 15);

    // "ls" must be visible somewhere in the new viewport.
    let found = (0..8).any(|r| {
        grid[Line(r)][Column(0)] == cell('l') && grid[Line(r)][Column(1)] == cell('s')
    });
    assert!(found, "'ls' should be visible after growing both dimensions");
}

// Content at the bottom survives both vertical and horizontal shrink simultaneously.
#[test]
fn shrink_lines_and_columns_preserves_content() {
    // 10 lines, 20 cols.  "ls" near the bottom; blank rows below.
    let mut grid = Grid::<Cell>::new(10, 20, 10);
    grid[Line(7)][Column(0)] = cell('l');
    grid[Line(7)][Column(1)] = cell('s');
    // Cursor two rows below "ls", leaving blank trailing rows.
    grid.cursor.point.line = Line(8);
    grid.cursor.point.column = Column(0);

    // Shrink both dimensions.
    grid.resize(true, 8, 15);

    // "ls" must still be visible.
    let found = (0..8).any(|r| {
        grid[Line(r)][Column(0)] == cell('l') && grid[Line(r)][Column(1)] == cell('s')
    });
    assert!(found, "'ls' should be visible after shrinking both dimensions");
}

// Content in the middle of the screen stays visible when shrinking columns.
// Cursor is NOT at the bottom (simulates a prompt that appeared before a few blank lines).
#[test]
fn shrink_columns_preserves_content_cursor_not_at_bottom() {
    // 5 lines, 10 cols.  "ls" at Line(2); blank lines below; cursor at Line(2).
    let mut grid = Grid::<Cell>::new(5, 10, 0);
    grid[Line(2)][Column(0)] = cell('l');
    grid[Line(2)][Column(1)] = cell('s');
    grid.cursor.point.line = Line(2);
    grid.cursor.point.column = Column(2);

    grid.resize(true, 5, 8);

    assert_eq!(grid[Line(2)][Column(0)], cell('l'), "col 0 should be 'l'");
    assert_eq!(grid[Line(2)][Column(1)], cell('s'), "col 1 should be 's'");
}

// Content must survive the combined shrink-lines (trailing-blanks path) + shrink-columns.
#[test]
fn shrink_lines_trailing_blanks_then_shrink_columns() {
    // 5 lines, 10 cols.  "ls" at Line(2).  Lines 3-4 are blank (trailing blanks).
    // Cursor at Line(3).
    let mut grid = Grid::<Cell>::new(5, 10, 10);
    grid[Line(2)][Column(0)] = cell('l');
    grid[Line(2)][Column(1)] = cell('s');
    grid.cursor.point.line = Line(3);
    grid.cursor.point.column = Column(0);

    // Shrink by 2 lines (trailing blanks >= shrinkage, so no scroll_up).
    // Also shrink columns.
    grid.resize(true, 3, 8);

    // "ls" must be in the new viewport (3 lines).
    let found = (0..3).any(|r| {
        grid[Line(r)][Column(0)] == cell('l') && grid[Line(r)][Column(1)] == cell('s')
    });
    assert!(found, "'ls' should still be visible after combined shrink");
}

// Regression: shrink_columns must not push visible non-blank rows into history
// when blank rows at the bottom of the viewport can absorb the reflow overflow.
//
// Scenario that triggered the original bug:
//   - 5-line, 20-col terminal.
//   - Line(0): "$ ls" command (4 chars).
//   - Line(1): "file1  file2  file3" ls output (19 chars, fills > half).
//   - Line(2): "$ " prompt (cursor here, col 1).
//   - Lines 3-4: blank.
//
// When shrinking to 10 cols, Line(1) splits into two rows ("file1  fi" and
// "le2  file3"), creating 6 rows total for 5 visible lines.  Without the fix,
// "$ ls" (Line(0)) gets pushed to scrollback history and disappears from the
// viewport.  With the fix, one trailing blank row is trimmed instead.
#[test]
fn shrink_columns_reflow_keeps_content_visible() {
    let mut grid = Grid::<Cell>::new(5, 20, 10);

    // "$ ls" at Line(0).
    grid[Line(0)][Column(0)] = cell('$');
    grid[Line(0)][Column(1)] = cell(' ');
    grid[Line(0)][Column(2)] = cell('l');
    grid[Line(0)][Column(3)] = cell('s');

    // ls output at Line(1): 15 chars — wider than the target 10 cols.
    for (c, ch) in "file1  file2  file3".chars().enumerate() {
        if c < 20 {
            grid[Line(1)][Column(c)] = cell(ch);
        }
    }
    // Mark as WRAPLINE so it participates in reflow.
    grid[Line(1)][Column(19)].flags_mut().insert(Flags::WRAPLINE);

    // "$ " prompt at Line(2), cursor here.
    grid[Line(2)][Column(0)] = cell('$');
    grid[Line(2)][Column(1)] = cell(' ');
    grid.cursor.point.line = Line(2);
    grid.cursor.point.column = Column(1);

    // Lines 3-4 are blank (trailing).

    // Shrink to 10 cols — Line(1) will split into two rows.
    grid.resize(true, 5, 10);

    // "$ ls" must remain in the visible viewport.
    let cmd_visible = (0..5).any(|r| {
        grid[Line(r)][Column(0)] == cell('$') && grid[Line(r)][Column(2)] == cell('l')
    });
    assert!(cmd_visible, "'$ ls' should stay visible after column shrink; history_size={}", grid.history_size());

    // No content should have gone to history for this scenario.
    assert_eq!(grid.history_size(), 0, "blank trailing rows should have absorbed the overflow");
}
