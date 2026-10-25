use super::*;

use std::mem;

use crate::engine::ansi::{
    self, CharsetIndex, Handler, NamedColor, NamedMode, NamedPrivateMode, Rgb, StandardCharset,
};
use crate::engine::event::VoidListener;
use crate::engine::grid::{Grid, Scroll};
use crate::engine::index::{Column, Point, Side};
use crate::engine::selection::{Selection, SelectionType};
use crate::engine::term::cell::{Cell, Flags};
use crate::engine::term::handler::version_number;
use crate::engine::term::test::TermSize;

#[test]
fn scroll_display_page_up() {
    let size = TermSize::new(5, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 11 lines of scrollback.
    for _ in 0..20 {
        term.newline();
    }

    // Scrollable amount to top is 11.
    term.scroll_display(Scroll::PageUp);
    assert_eq!(term.vi_mode_cursor.point, Point::new(Line(-1), Column(0)));
    assert_eq!(term.grid.display_offset(), 10);

    // Scrollable amount to top is 1.
    term.scroll_display(Scroll::PageUp);
    assert_eq!(term.vi_mode_cursor.point, Point::new(Line(-2), Column(0)));
    assert_eq!(term.grid.display_offset(), 11);

    // Scrollable amount to top is 0.
    term.scroll_display(Scroll::PageUp);
    assert_eq!(term.vi_mode_cursor.point, Point::new(Line(-2), Column(0)));
    assert_eq!(term.grid.display_offset(), 11);
}

#[test]
fn scroll_display_page_down() {
    let size = TermSize::new(5, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 11 lines of scrollback.
    for _ in 0..20 {
        term.newline();
    }

    // Change display_offset to topmost.
    term.grid_mut().scroll_display(Scroll::Top);
    term.vi_mode_cursor = ViModeCursor::new(Point::new(Line(-11), Column(0)));

    // Scrollable amount to bottom is 11.
    term.scroll_display(Scroll::PageDown);
    assert_eq!(term.vi_mode_cursor.point, Point::new(Line(-1), Column(0)));
    assert_eq!(term.grid.display_offset(), 1);

    // Scrollable amount to bottom is 1.
    term.scroll_display(Scroll::PageDown);
    assert_eq!(term.vi_mode_cursor.point, Point::new(Line(0), Column(0)));
    assert_eq!(term.grid.display_offset(), 0);

    // Scrollable amount to bottom is 0.
    term.scroll_display(Scroll::PageDown);
    assert_eq!(term.vi_mode_cursor.point, Point::new(Line(0), Column(0)));
    assert_eq!(term.grid.display_offset(), 0);
}

#[test]
fn simple_selection_works() {
    let size = TermSize::new(5, 5);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let grid = term.grid_mut();
    for i in 0..4 {
        if i == 1 {
            continue;
        }

        grid[Line(i)][Column(0)].c = '"';

        for j in 1..4 {
            grid[Line(i)][Column(j)].c = 'a';
        }

        grid[Line(i)][Column(4)].c = '"';
    }
    grid[Line(2)][Column(0)].c = ' ';
    grid[Line(2)][Column(4)].c = ' ';
    grid[Line(2)][Column(4)].flags.insert(Flags::WRAPLINE);
    grid[Line(3)][Column(0)].c = ' ';

    // Multiple lines contain an empty line.
    term.selection = Some(Selection::new(
        SelectionType::Simple,
        Point {
            line: Line(0),
            column: Column(0),
        },
        Side::Left,
    ));
    if let Some(s) = term.selection.as_mut() {
        s.update(
            Point {
                line: Line(2),
                column: Column(4),
            },
            Side::Right,
        );
    }
    assert_eq!(
        term.selection_to_string(),
        Some(String::from("\"aaa\"\n\n aaa "))
    );

    // A wrapline.
    term.selection = Some(Selection::new(
        SelectionType::Simple,
        Point {
            line: Line(2),
            column: Column(0),
        },
        Side::Left,
    ));
    if let Some(s) = term.selection.as_mut() {
        s.update(
            Point {
                line: Line(3),
                column: Column(4),
            },
            Side::Right,
        );
    }
    assert_eq!(
        term.selection_to_string(),
        Some(String::from(" aaa  aaa\""))
    );
}

#[test]
fn semantic_selection_works() {
    let size = TermSize::new(5, 3);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let mut grid: Grid<Cell> = Grid::new(3, 5, 0);
    for i in 0..5 {
        for j in 0..2 {
            grid[Line(j)][Column(i)].c = 'a';
        }
    }
    grid[Line(0)][Column(0)].c = '"';
    grid[Line(0)][Column(3)].c = '"';
    grid[Line(1)][Column(2)].c = '"';
    grid[Line(0)][Column(4)].flags.insert(Flags::WRAPLINE);

    let mut escape_chars = String::from("\"");

    mem::swap(&mut term.grid, &mut grid);
    mem::swap(&mut term.config.semantic_escape_chars, &mut escape_chars);

    {
        term.selection = Some(Selection::new(
            SelectionType::Semantic,
            Point {
                line: Line(0),
                column: Column(1),
            },
            Side::Left,
        ));
        assert_eq!(term.selection_to_string(), Some(String::from("aa")));
    }

    {
        term.selection = Some(Selection::new(
            SelectionType::Semantic,
            Point {
                line: Line(0),
                column: Column(4),
            },
            Side::Left,
        ));
        assert_eq!(term.selection_to_string(), Some(String::from("aaa")));
    }

    {
        term.selection = Some(Selection::new(
            SelectionType::Semantic,
            Point {
                line: Line(1),
                column: Column(1),
            },
            Side::Left,
        ));
        assert_eq!(term.selection_to_string(), Some(String::from("aaa")));
    }
}

#[test]
fn line_selection_works() {
    let size = TermSize::new(5, 1);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let mut grid: Grid<Cell> = Grid::new(1, 5, 0);
    for i in 0..5 {
        grid[Line(0)][Column(i)].c = 'a';
    }
    grid[Line(0)][Column(0)].c = '"';
    grid[Line(0)][Column(3)].c = '"';

    mem::swap(&mut term.grid, &mut grid);

    term.selection = Some(Selection::new(
        SelectionType::Lines,
        Point {
            line: Line(0),
            column: Column(3),
        },
        Side::Left,
    ));
    assert_eq!(term.selection_to_string(), Some(String::from("\"aa\"a\n")));
}

#[test]
fn block_selection_works() {
    let size = TermSize::new(5, 5);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let grid = term.grid_mut();
    for i in 1..4 {
        grid[Line(i)][Column(0)].c = '"';

        for j in 1..4 {
            grid[Line(i)][Column(j)].c = 'a';
        }

        grid[Line(i)][Column(4)].c = '"';
    }
    grid[Line(2)][Column(2)].c = ' ';
    grid[Line(2)][Column(4)].flags.insert(Flags::WRAPLINE);
    grid[Line(3)][Column(4)].c = ' ';

    term.selection = Some(Selection::new(
        SelectionType::Block,
        Point {
            line: Line(0),
            column: Column(3),
        },
        Side::Left,
    ));

    // The same column.
    if let Some(s) = term.selection.as_mut() {
        s.update(
            Point {
                line: Line(3),
                column: Column(3),
            },
            Side::Right,
        );
    }
    assert_eq!(term.selection_to_string(), Some(String::from("\na\na\na")));

    // The first column.
    if let Some(s) = term.selection.as_mut() {
        s.update(
            Point {
                line: Line(3),
                column: Column(0),
            },
            Side::Left,
        );
    }
    assert_eq!(
        term.selection_to_string(),
        Some(String::from("\n\"aaa\n\"a a\n\"aaa"))
    );

    // The last column.
    if let Some(s) = term.selection.as_mut() {
        s.update(
            Point {
                line: Line(3),
                column: Column(4),
            },
            Side::Right,
        );
    }
    assert_eq!(
        term.selection_to_string(),
        Some(String::from("\na\"\na\"\na"))
    );
}

/// Check that the grid can be serialized back and forth losslessly.
///
/// This test is in the term module as opposed to the grid since we want to
/// test this property with a T=Cell.
#[test]
#[cfg(feature = "serde")]
fn grid_serde() {
    let grid: Grid<Cell> = Grid::new(24, 80, 0);
    let serialized = serde_json::to_string(&grid).expect("ser");
    let deserialized = serde_json::from_str::<Grid<Cell>>(&serialized).expect("de");

    assert_eq!(deserialized, grid);
}

#[test]
fn input_line_drawing_character() {
    let size = TermSize::new(7, 17);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let cursor = Point::new(Line(0), Column(0));
    term.configure_charset(
        CharsetIndex::G0,
        StandardCharset::SpecialCharacterAndLineDrawing,
    );
    term.input('a');

    assert_eq!(term.grid()[cursor].c, '▒');
}

#[test]
fn clearing_viewport_keeps_history_position() {
    let size = TermSize::new(10, 20);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..29 {
        term.newline();
    }

    // Change the display area.
    term.scroll_display(Scroll::Top);

    assert_eq!(term.grid.display_offset(), 10);

    // Clear the viewport.
    term.clear_screen(ansi::ClearMode::All);

    assert_eq!(term.grid.display_offset(), 10);
}

#[test]
fn clearing_viewport_with_vi_mode_keeps_history_position() {
    let size = TermSize::new(10, 20);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..29 {
        term.newline();
    }

    // Enable vi mode.
    term.toggle_vi_mode();

    // Change the display area and the vi cursor position.
    term.scroll_display(Scroll::Top);
    term.vi_mode_cursor.point = Point::new(Line(-5), Column(3));

    assert_eq!(term.grid.display_offset(), 10);

    // Clear the viewport.
    term.clear_screen(ansi::ClearMode::All);

    assert_eq!(term.grid.display_offset(), 10);
    assert_eq!(term.vi_mode_cursor.point, Point::new(Line(-5), Column(3)));
}

#[test]
fn clearing_scrollback_resets_display_offset() {
    let size = TermSize::new(10, 20);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..29 {
        term.newline();
    }

    // Change the display area.
    term.scroll_display(Scroll::Top);

    assert_eq!(term.grid.display_offset(), 10);

    // Clear the scrollback buffer.
    term.clear_screen(ansi::ClearMode::Saved);

    assert_eq!(term.grid.display_offset(), 0);
}

#[test]
fn clearing_scrollback_sets_vi_cursor_into_viewport() {
    let size = TermSize::new(10, 20);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..29 {
        term.newline();
    }

    // Enable vi mode.
    term.toggle_vi_mode();

    // Change the display area and the vi cursor position.
    term.scroll_display(Scroll::Top);
    term.vi_mode_cursor.point = Point::new(Line(-5), Column(3));

    assert_eq!(term.grid.display_offset(), 10);

    // Clear the scrollback buffer.
    term.clear_screen(ansi::ClearMode::Saved);

    assert_eq!(term.grid.display_offset(), 0);
    assert_eq!(term.vi_mode_cursor.point, Point::new(Line(0), Column(3)));
}

#[test]
fn clear_saved_lines() {
    let size = TermSize::new(7, 17);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Add one line of scrollback.
    term.grid.scroll_up(&(Line(0)..Line(1)), 1);

    // Clear the history.
    term.clear_screen(ansi::ClearMode::Saved);

    // Make sure that scrolling does not change the grid.
    let mut scrolled_grid = term.grid.clone();
    scrolled_grid.scroll_display(Scroll::Top);

    // Truncate grids for comparison.
    scrolled_grid.truncate();
    term.grid.truncate();

    assert_eq!(term.grid, scrolled_grid);
}

#[test]
fn vi_cursor_keep_pos_on_scrollback_buffer() {
    let size = TermSize::new(5, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 11 lines of scrollback.
    for _ in 0..20 {
        term.newline();
    }

    // Enable vi mode.
    term.toggle_vi_mode();

    term.scroll_display(Scroll::Top);
    term.vi_mode_cursor.point.line = Line(-11);

    term.linefeed();
    assert_eq!(term.vi_mode_cursor.point.line, Line(-12));
}

#[test]
fn grow_lines_top_anchored() {
    let mut size = TermSize::new(100, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..19 {
        term.newline();
    }
    assert_eq!(term.history_size(), 10);

    // Move cursor to top to ensure top-anchoring.
    term.grid.cursor.point.line = Line(0);

    // Increase visible lines.
    size.screen_lines = 30;
    term.resize(size);

    // History remains untouched.
    assert_eq!(term.history_size(), 10);
    assert_eq!(term.grid.cursor.point, Point::new(Line(0), Column(0)));
}

#[test]
fn grow_lines_bottom_pinned() {
    let mut size = TermSize::new(100, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..19 {
        term.newline();
    }
    assert_eq!(term.history_size(), 10);
    assert_eq!(term.grid.cursor.point, Point::new(Line(9), Column(0)));

    // Increase visible lines.
    size.screen_lines = 30;
    term.resize(size);

    // All 10 lines of history are pulled into view.
    assert_eq!(term.history_size(), 0);
    assert_eq!(term.grid.cursor.point, Point::new(Line(19), Column(0)));
}

#[test]
fn grow_lines_updates_inactive_cursor_pos() {
    let mut size = TermSize::new(100, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..19 {
        term.newline();
    }
    assert_eq!(term.history_size(), 10);

    // Move cursor to top.
    term.grid.cursor.point.line = Line(0);

    // Enter alt screen.
    term.set_private_mode(NamedPrivateMode::SwapScreenAndSetRestoreCursor.into());

    // Increase visible lines.
    size.screen_lines = 30;
    term.resize(size);

    // Leave alt screen.
    term.unset_private_mode(NamedPrivateMode::SwapScreenAndSetRestoreCursor.into());

    assert_eq!(term.history_size(), 10);
    assert_eq!(term.grid.cursor.point, Point::new(Line(0), Column(0)));
}

#[test]
fn shrink_lines_updates_active_cursor_pos() {
    let mut size = TermSize::new(100, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..19 {
        term.newline();
    }
    assert_eq!(term.history_size(), 10);
    assert_eq!(term.grid.cursor.point, Point::new(Line(9), Column(0)));

    // Decrease visible lines.
    size.screen_lines = 5;
    term.resize(size);

    // Since the lines were empty, they are truncated instead of pushed to history.
    assert_eq!(term.history_size(), 10);
    assert_eq!(term.grid.cursor.point, Point::new(Line(4), Column(0)));
}

#[test]
fn shrink_lines_updates_inactive_cursor_pos() {
    let mut size = TermSize::new(100, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Create 10 lines of scrollback.
    for _ in 0..19 {
        term.newline();
    }
    assert_eq!(term.history_size(), 10);
    assert_eq!(term.grid.cursor.point, Point::new(Line(9), Column(0)));

    // Enter alt screen.
    term.set_private_mode(NamedPrivateMode::SwapScreenAndSetRestoreCursor.into());

    // Decrease visible lines.
    size.screen_lines = 5;
    term.resize(size);

    // Leave alt screen.
    term.unset_private_mode(NamedPrivateMode::SwapScreenAndSetRestoreCursor.into());

    assert_eq!(term.history_size(), 10);
    assert_eq!(term.grid.cursor.point, Point::new(Line(4), Column(0)));
}

#[test]
fn damage_public_usage() {
    let size = TermSize::new(10, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    // Reset terminal for partial damage tests since it's initialized as fully damaged.
    term.reset_damage();

    // Test that we damage input form [`Term::input`].

    let left = term.grid.cursor.point.column.0;
    term.input('d');
    term.input('a');
    term.input('m');
    term.input('a');
    term.input('g');
    term.input('e');
    let right = term.grid.cursor.point.column.0;

    let mut damaged_lines = match term.damage() {
        TermDamage::Full => panic!("Expected partial damage, however got Full"),
        TermDamage::Partial(damaged_lines) => damaged_lines,
    };
    assert_eq!(
        damaged_lines.next(),
        Some(LineDamageBounds {
            line: 0,
            left,
            right
        })
    );
    assert_eq!(damaged_lines.next(), None);
    term.reset_damage();

    // Create scrollback.
    for _ in 0..20 {
        term.newline();
    }

    match term.damage() {
        TermDamage::Full => (),
        TermDamage::Partial(_) => panic!("Expected Full damage, however got Partial "),
    };
    term.reset_damage();

    term.scroll_display(Scroll::Delta(10));
    term.reset_damage();

    // No damage when scrolled into viewport.
    for idx in 0..term.columns() {
        term.goto(idx as i32, idx);
    }
    let mut damaged_lines = match term.damage() {
        TermDamage::Full => panic!("Expected partial damage, however got Full"),
        TermDamage::Partial(damaged_lines) => damaged_lines,
    };
    assert_eq!(damaged_lines.next(), None);

    // Scroll back into the viewport, so we have 2 visible lines which terminal can write
    // to.
    term.scroll_display(Scroll::Delta(-2));
    term.reset_damage();

    term.goto(0, 0);
    term.goto(1, 0);
    term.goto(2, 0);
    let display_offset = term.grid().display_offset();
    let mut damaged_lines = match term.damage() {
        TermDamage::Full => panic!("Expected partial damage, however got Full"),
        TermDamage::Partial(damaged_lines) => damaged_lines,
    };
    assert_eq!(
        damaged_lines.next(),
        Some(LineDamageBounds {
            line: display_offset,
            left: 0,
            right: 0
        })
    );
    assert_eq!(
        damaged_lines.next(),
        Some(LineDamageBounds {
            line: display_offset + 1,
            left: 0,
            right: 0
        })
    );
    assert_eq!(damaged_lines.next(), None);
}

#[test]
fn damage_cursor_movements() {
    let size = TermSize::new(10, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let num_cols = term.columns();
    // Reset terminal for partial damage tests since it's initialized as fully damaged.
    term.reset_damage();

    term.goto(1, 1);

    // NOTE While we can use `[Term::damage]` to access terminal damage information, in the
    // following tests we will be accessing `term.damage.lines` directly to avoid adding extra
    // damage information (like cursor and Vi cursor), which we're not testing.

    assert_eq!(
        term.damage.lines[0],
        LineDamageBounds {
            line: 0,
            left: 0,
            right: 0
        }
    );
    assert_eq!(
        term.damage.lines[1],
        LineDamageBounds {
            line: 1,
            left: 1,
            right: 1
        }
    );
    term.damage.reset(num_cols);

    term.move_forward(3);
    assert_eq!(
        term.damage.lines[1],
        LineDamageBounds {
            line: 1,
            left: 1,
            right: 4
        }
    );
    term.damage.reset(num_cols);

    term.move_backward(8);
    assert_eq!(
        term.damage.lines[1],
        LineDamageBounds {
            line: 1,
            left: 0,
            right: 4
        }
    );
    term.goto(5, 5);
    term.damage.reset(num_cols);

    term.backspace();
    term.backspace();
    assert_eq!(
        term.damage.lines[5],
        LineDamageBounds {
            line: 5,
            left: 3,
            right: 5
        }
    );
    term.damage.reset(num_cols);

    term.move_up(1);
    assert_eq!(
        term.damage.lines[5],
        LineDamageBounds {
            line: 5,
            left: 3,
            right: 3
        }
    );
    assert_eq!(
        term.damage.lines[4],
        LineDamageBounds {
            line: 4,
            left: 3,
            right: 3
        }
    );
    term.damage.reset(num_cols);

    term.move_down(1);
    term.move_down(1);
    assert_eq!(
        term.damage.lines[4],
        LineDamageBounds {
            line: 4,
            left: 3,
            right: 3
        }
    );
    assert_eq!(
        term.damage.lines[5],
        LineDamageBounds {
            line: 5,
            left: 3,
            right: 3
        }
    );
    assert_eq!(
        term.damage.lines[6],
        LineDamageBounds {
            line: 6,
            left: 3,
            right: 3
        }
    );
    term.damage.reset(num_cols);

    term.wrapline();
    assert_eq!(
        term.damage.lines[6],
        LineDamageBounds {
            line: 6,
            left: 3,
            right: 3
        }
    );
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 0,
            right: 0
        }
    );
    term.move_forward(3);
    term.move_up(1);
    term.damage.reset(num_cols);

    term.linefeed();
    assert_eq!(
        term.damage.lines[6],
        LineDamageBounds {
            line: 6,
            left: 3,
            right: 3
        }
    );
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 3,
            right: 3
        }
    );
    term.damage.reset(num_cols);

    term.carriage_return();
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 0,
            right: 3
        }
    );
    term.damage.reset(num_cols);

    term.erase_chars(5);
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 0,
            right: 5
        }
    );
    term.damage.reset(num_cols);

    term.delete_chars(3);
    let right = term.columns() - 1;
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 0,
            right
        }
    );
    term.move_forward(term.columns());
    term.damage.reset(num_cols);

    term.move_backward_tabs(1);
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 8,
            right
        }
    );
    term.save_cursor_position();
    term.goto(1, 1);
    term.damage.reset(num_cols);

    term.restore_cursor_position();
    assert_eq!(
        term.damage.lines[1],
        LineDamageBounds {
            line: 1,
            left: 1,
            right: 1
        }
    );
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 8,
            right: 8
        }
    );
    term.damage.reset(num_cols);

    term.clear_line(ansi::LineClearMode::All);
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 0,
            right
        }
    );
    term.damage.reset(num_cols);

    term.clear_line(ansi::LineClearMode::Left);
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 0,
            right: 8
        }
    );
    term.damage.reset(num_cols);

    term.clear_line(ansi::LineClearMode::Right);
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 8,
            right
        }
    );
    term.damage.reset(num_cols);

    term.reverse_index();
    assert_eq!(
        term.damage.lines[7],
        LineDamageBounds {
            line: 7,
            left: 8,
            right: 8
        }
    );
    assert_eq!(
        term.damage.lines[6],
        LineDamageBounds {
            line: 6,
            left: 8,
            right: 8
        }
    );
}

#[test]
fn full_damage() {
    let size = TermSize::new(100, 10);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    assert!(term.damage.full);
    for _ in 0..20 {
        term.newline();
    }
    term.reset_damage();

    term.clear_screen(ansi::ClearMode::Above);
    assert!(term.damage.full);
    term.reset_damage();

    term.scroll_display(Scroll::Top);
    assert!(term.damage.full);
    term.reset_damage();

    // Sequential call to scroll display without doing anything shouldn't damage.
    term.scroll_display(Scroll::Top);
    assert!(!term.damage.full);
    term.reset_damage();

    term.set_options(Config::default());
    assert!(term.damage.full);
    term.reset_damage();

    term.scroll_down_relative(Line(5), 2);
    assert!(term.damage.full);
    term.reset_damage();

    term.scroll_up_relative(Line(3), 2);
    assert!(term.damage.full);
    term.reset_damage();

    term.deccolm();
    assert!(term.damage.full);
    term.reset_damage();

    term.decaln();
    assert!(term.damage.full);
    term.reset_damage();

    term.set_mode(NamedMode::Insert.into());
    // Just setting `Insert` mode shouldn't mark terminal as damaged.
    assert!(!term.damage.full);
    term.reset_damage();

    let color_index = 257;
    term.set_color(color_index, Rgb::default());
    assert!(term.damage.full);
    term.reset_damage();

    // Setting the same color once again shouldn't trigger full damage.
    term.set_color(color_index, Rgb::default());
    assert!(!term.damage.full);

    term.reset_color(color_index);
    assert!(term.damage.full);
    term.reset_damage();

    // We shouldn't trigger fully damage when cursor gets update.
    term.set_color(NamedColor::Cursor as usize, Rgb::default());
    assert!(!term.damage.full);

    // However requesting terminal damage should mark terminal as fully damaged in `Insert`
    // mode.
    let _ = term.damage();
    assert!(term.damage.full);
    term.reset_damage();

    term.unset_mode(NamedMode::Insert.into());
    assert!(term.damage.full);
    term.reset_damage();

    // Keep this as a last check, so we don't have to deal with restoring from alt-screen.
    term.swap_alt();
    assert!(term.damage.full);
    term.reset_damage();

    let size = TermSize::new(10, 10);
    term.resize(size);
    assert!(term.damage.full);
}

#[test]
fn window_title() {
    let size = TermSize::new(7, 17);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Title None by default.
    assert_eq!(term.title, None);

    // Title can be set.
    term.set_title(Some("Test".into()));
    assert_eq!(term.title, Some("Test".into()));

    // Title can be pushed onto stack.
    term.push_title();
    term.set_title(Some("Next".into()));
    assert_eq!(term.title, Some("Next".into()));
    assert_eq!(term.title_stack.first().unwrap(), &Some("Test".into()));

    // Title can be popped from stack and set as the window title.
    term.pop_title();
    assert_eq!(term.title, Some("Test".into()));
    assert!(term.title_stack.is_empty());

    // Title stack doesn't grow infinitely.
    for _ in 0..4097 {
        term.push_title();
    }
    assert_eq!(term.title_stack.len(), 4096);

    // Title and title stack reset when terminal state is reset.
    term.push_title();
    term.reset_state();
    assert_eq!(term.title, None);
    assert!(term.title_stack.is_empty());

    // Title stack pops back to default.
    term.title = None;
    term.push_title();
    term.set_title(Some("Test".into()));
    term.pop_title();
    assert_eq!(term.title, None);

    // Title can be reset to default.
    term.title = Some("Test".into());
    term.set_title(None);
    assert_eq!(term.title, None);
}

#[test]
fn parse_cargo_version() {
    assert!(version_number(env!("CARGO_PKG_VERSION")) >= 1_00);
    assert_eq!(version_number("0.0.1-dev"), 1);
    assert_eq!(version_number("0.1.2-dev"), 1_02);
    assert_eq!(version_number("1.2.3-dev"), 1_02_03);
    assert_eq!(version_number("999.99.99"), 9_99_99_99);
}

// VTE-level resize integration tests.
//
// These tests feed characters through the actual Term/VTE input handler (not
// the grid directly) and then check that the content is preserved through a
// column or combined resize.

#[test]
fn vte_grow_columns_preserves_content() {
    let mut size = TermSize::new(80, 24);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    // Write "ls" at Line(0), Column(0..1).
    term.input('l');
    term.input('s');
    assert_eq!(term.grid[Line(0)][Column(0)].c, 'l');
    assert_eq!(term.grid[Line(0)][Column(1)].c, 's');
    // Grow columns.
    size.columns = 100;
    term.resize(size);
    assert_eq!(
        term.grid[Line(0)][Column(0)].c,
        'l',
        "'l' should survive grow"
    );
    assert_eq!(
        term.grid[Line(0)][Column(1)].c,
        's',
        "'s' should survive grow"
    );
}

#[test]
fn vte_shrink_columns_preserves_content() {
    let mut size = TermSize::new(80, 24);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    term.input('l');
    term.input('s');
    // Shrink columns (still wider than "ls").
    size.columns = 60;
    term.resize(size);
    assert_eq!(
        term.grid[Line(0)][Column(0)].c,
        'l',
        "'l' should survive shrink"
    );
    assert_eq!(
        term.grid[Line(0)][Column(1)].c,
        's',
        "'s' should survive shrink"
    );
}

#[test]
fn vte_combined_resize_preserves_content_at_bottom() {
    // Simulate a terminal where "ls" was typed at the bottom of the screen.
    let mut size = TermSize::new(80, 24);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    // Scroll to fill the terminal so the cursor is at the bottom line.
    for _ in 0..23 {
        term.newline();
    }
    assert_eq!(term.grid.cursor.point.line, Line(23));
    // Write "ls" at the bottom line.
    term.input('l');
    term.input('s');
    assert_eq!(term.grid[Line(23)][Column(0)].c, 'l');
    assert_eq!(term.grid[Line(23)][Column(1)].c, 's');
    // Combined resize: grow both lines and columns.
    size.screen_lines = 28;
    size.columns = 100;
    term.resize(size);
    // "ls" must be somewhere in the new viewport.
    let found = (0..28)
        .any(|r| term.grid[Line(r)][Column(0)].c == 'l' && term.grid[Line(r)][Column(1)].c == 's');
    assert!(found, "'ls' must be visible after combined grow");
}

#[test]
fn vte_combined_shrink_preserves_content_at_bottom() {
    // Terminal with content at the bottom and blank trailing rows.
    let mut size = TermSize::new(80, 24);
    let mut term = Term::new(Config::default(), &size, VoidListener);
    // Scroll to line 20 (leaves 3 blank lines below).
    for _ in 0..20 {
        term.newline();
    }
    term.input('l');
    term.input('s');
    assert_eq!(term.grid[Line(20)][Column(0)].c, 'l');
    assert_eq!(term.grid[Line(20)][Column(1)].c, 's');
    // Combined shrink: fewer lines (trim blanks) and fewer columns.
    size.screen_lines = 22;
    size.columns = 60;
    term.resize(size);
    let found = (0..22)
        .any(|r| term.grid[Line(r)][Column(0)].c == 'l' && term.grid[Line(r)][Column(1)].c == 's');
    assert!(found, "'ls' must be visible after combined shrink");
}

#[test]
fn test_calculate_character_delta() {
    use crate::engine::index::{Column, Line, Point};
    use crate::engine::term::cell::Flags;
    use crate::engine::term::test::mock_term;

    // Test 1: Simple Same Line Navigation
    let term = mock_term("hello world\r");
    let state = RenderState::new(&term);
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(0), Column(0)),
            Point::new(Line(0), Column(5))
        ),
        5
    );
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(0), Column(5)),
            Point::new(Line(0), Column(0))
        ),
        -5
    );

    // Test 2: Skipping Prompts
    let mut term = mock_term("> echo hi\r");
    term.grid[Line(0)][Column(0)]
        .flags
        .insert(Flags::SEMANTIC_PROMPT);
    term.grid[Line(0)][Column(1)]
        .flags
        .insert(Flags::SEMANTIC_PROMPT); // " "
    let state = RenderState::new(&term);
    // Start is column 2 (e), target is column 6 (h). The delta is purely the difference because prompt is behind start.
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(0), Column(2)),
            Point::new(Line(0), Column(6))
        ),
        4
    );

    // Test 3: Hard Newlines (+1)
    let term = mock_term("cmd\r\narg\r");
    let state = RenderState::new(&term);
    // Move from (0,0) to (1,0). Should be 3 (cmd) + 1 (\n) = 4
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(0), Column(0)),
            Point::new(Line(1), Column(0))
        ),
        4
    );
    // Reverse
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(1), Column(0)),
            Point::new(Line(0), Column(0))
        ),
        -4
    );

    // Test 4: Wrapped Lines (No +1)
    let term = mock_term("cmd\narg\r"); // \n in mock_term generates WRAPLINE
    let state = RenderState::new(&term);
    // Move from (0,0) to (1,0). Should be 3 (cmd)
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(0), Column(0)),
            Point::new(Line(1), Column(0))
        ),
        3
    );
    // Reverse
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(1), Column(0)),
            Point::new(Line(0), Column(0))
        ),
        -3
    );

    // Test 5: Wide Characters
    let term = mock_term("echo 🚀 test\r");
    // mock_term adds WIDE_CHAR and WIDE_CHAR_SPACER automatically.
    let state = RenderState::new(&term);
    // "echo " (5) + "🚀" (1) + " t" (2) = 8 jumps to reach 'e'
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(0), Column(0)),
            Point::new(Line(0), Column(9))
        ),
        8
    );
    assert_eq!(
        state.calculate_character_delta(
            Point::new(Line(0), Column(9)),
            Point::new(Line(0), Column(0))
        ),
        -8
    );
}

#[test]
fn test_calculate_navigation_sequence() {
    use crate::engine::index::{Column, Line, Point};
    use crate::engine::term::cell::{Cell, Flags};

    /// Build a minimal RenderState from row strings.
    /// '\n'-split rows; each char sets the cell's `c` field.
    /// cursor_line/cursor_col set the cursor position.
    fn make_state(rows: &[&str], cursor_line: usize, cursor_col: usize) -> RenderState {
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
        let num_rows = rows.len().max(1);
        let mut cells = vec![Cell::default(); num_rows * cols];
        for (r, row_str) in rows.iter().enumerate() {
            for (c, ch) in row_str.chars().enumerate() {
                cells[r * cols + c].c = ch;
            }
        }
        RenderState {
            cells,
            columns: cols,
            screen_lines: num_rows,
            total_lines: num_rows,
            display_offset: 0,
            cursor_point: Point::new(Line(cursor_line as i32), Column(cursor_col)),
            cl_mode: Some("line".to_string()),
            ..RenderState::default()
        }
    }

    // 1. cl_mode=None → always None
    let mut state = make_state(&["hello"], 0, 0);
    state.cl_mode = None;
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(0), Column(3))),
        None
    );

    // 2. Same line — click right of cursor
    let state = make_state(&["hello world"], 0, 0);
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(0), Column(5))),
        Some("\x1b[C".repeat(5)),
    );

    // 3. Same line — click left of cursor
    let state = make_state(&["hello world"], 0, 5);
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(0), Column(0))),
        Some("\x1b[D".repeat(5)),
    );

    // 4. Same line — click at cursor column → no movement → None
    let state = make_state(&["hello world"], 0, 3);
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(0), Column(3))),
        None
    );

    // 5. Soft-wrapped rows — cursor on row 0, click on row 1.
    //    WRAPLINE on last cell of row 0 makes rows 0+1 one logical shell line.
    //    char_delta("hello"→row1:col0) = 5 (no +1 hard newline) → 5 Right.
    let mut state = make_state(&["hello", "world"], 0, 0);
    state.cells[state.columns - 1].flags.insert(Flags::WRAPLINE);
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(1), Column(0))),
        Some("\x1b[C".repeat(5)),
    );

    // Helper: tag every cell in a row with a semantic flag.
    let set_row_flags = |state: &mut RenderState, row: usize, flag: Flags| {
        let start = row * state.columns;
        let end = start + state.columns;
        for cell in &mut state.cells[start..end] {
            cell.flags.insert(flag);
        }
    };

    // 6. Hard-newline multiline — cursor on row 0, click below (Down + col adjust).
    let mut state = make_state(&["for x", "  echo"], 0, 0);
    set_row_flags(&mut state, 0, Flags::SEMANTIC_CMD);
    set_row_flags(&mut state, 1, Flags::SEMANTIC_CMD);
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(1), Column(2))),
        Some(format!("{}{}", "\x1b[B", "\x1b[C".repeat(2))),
    );

    // 7. Hard-newline multiline — cursor on row 1, click above (Up + col adjust).
    let mut state = make_state(&["for x", "  echo"], 1, 2);
    set_row_flags(&mut state, 0, Flags::SEMANTIC_CMD);
    set_row_flags(&mut state, 1, Flags::SEMANTIC_CMD);
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(0), Column(3))),
        Some(format!("{}\x1b[C", "\x1b[A")),
    );

    // 7b. Target row has SEMANTIC_PROMPT (fish continuation line) — should still navigate.
    let mut state = make_state(&["for x", "  echo"], 0, 0);
    set_row_flags(&mut state, 0, Flags::SEMANTIC_CMD);
    set_row_flags(&mut state, 1, Flags::SEMANTIC_PROMPT);
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(1), Column(2))),
        Some(format!("{}{}", "\x1b[B", "\x1b[C".repeat(2))),
    );

    // 8. Hard-newline rejected — blank row between cursor and target → has_gap → None.
    let mut state = make_state(&["for x", "     ", "echo"], 0, 0);
    set_row_flags(&mut state, 0, Flags::SEMANTIC_CMD);
    set_row_flags(&mut state, 2, Flags::SEMANTIC_CMD);
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(2), Column(0))),
        None
    );

    // 9. Hard-newline rejected — cursor row has no SEMANTIC_CMD → would navigate history.
    let mut state = make_state(&["for x", "  echo"], 0, 0);
    set_row_flags(&mut state, 1, Flags::SEMANTIC_CMD); // only target flagged, not cursor
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(1), Column(2))),
        None
    );

    // 10. Hard-newline rejected — target row has no semantic flag (plain output) → None.
    let mut state = make_state(&["for x", "  output"], 0, 0);
    set_row_flags(&mut state, 0, Flags::SEMANTIC_CMD); // cursor flagged, target not
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(1), Column(2))),
        None
    );

    // 11. Hard-newline rejected — scrolled into history (display_offset != 0) → None.
    let mut state = make_state(&["history", "for x", "  echo"], 0, 0);
    set_row_flags(&mut state, 0, Flags::SEMANTIC_CMD);
    set_row_flags(&mut state, 1, Flags::SEMANTIC_CMD);
    set_row_flags(&mut state, 2, Flags::SEMANTIC_CMD);
    state.display_offset = 1;
    assert_eq!(
        state.calculate_navigation_sequence(Point::new(Line(1), Column(0))),
        None
    );
}

#[test]
fn render_state_update_from_reuses_allocation_and_applies_partial_damage() {
    let size = TermSize::new(10, 4);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    let mut state = RenderState::default();
    state.update_from(&mut term, 1);
    assert_eq!(state.generation, 1);
    assert_eq!(state.cells.len(), 40);
    let initial_ptr = state.cells.as_ptr();

    // Type 'A' and 'B' -> partial damage on line 0
    term.input('A');
    term.input('B');
    state.update_from(&mut term, 2);

    assert_eq!(state.generation, 2);
    // Underlying Vec<Cell> buffer must be reused in-place without reallocation
    assert_eq!(state.cells.as_ptr(), initial_ptr);
    assert_eq!(state.cell(Point::new(Line(0), Column(0))).c, 'A');
    assert_eq!(state.cell(Point::new(Line(0), Column(1))).c, 'B');
}

#[test]
fn calculate_navigation_sequence_while_scrolled_into_history_does_not_panic() {
    let size = TermSize::new(20, 4);
    let mut term = Term::new(Config::default(), &size, VoidListener);

    // Fill scrollback so display_offset can be scrolled far past screen_lines
    for i in 0..30 {
        for ch in format!("line {i}").chars() {
            term.input(ch);
        }
        term.newline();
        term.carriage_return();
    }

    // Simulate OSC 133;A;cl=line prompt at the bottom of the terminal
    term.cl_mode = Some("line".to_string());
    term.scroll_display(Scroll::Delta(15));
    assert!(term.grid().display_offset() > 0);

    let state = RenderState::new(&term);
    // Clicking anywhere in the scrolled viewport must return None without indexing
    // off-screen cursor rows past `state.cells`.
    let clicked = Point::new(Line(1 - state.display_offset), Column(2));
    assert_eq!(state.calculate_navigation_sequence(clicked), None);
}


