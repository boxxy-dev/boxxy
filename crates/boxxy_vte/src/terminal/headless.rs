//! Headless terminal harness for deterministic protocol conformance,
//! chunk-boundary parsing, query-response verification, and reflow testing.

use std::sync::{Arc, Mutex};

use crate::engine::ansi::{self, NamedColor, Rgb};
use crate::engine::event::{Event, EventListener};
use crate::engine::grid::Dimensions;
use crate::engine::index::{Column, Line};
use crate::engine::term::{Config, RenderState, Term, TermMode};

#[derive(Clone, Default)]
pub struct HeadlessEventLog {
    pub events: Arc<Mutex<Vec<Event>>>,
    pub default_bg: Arc<Mutex<Rgb>>,
}

impl HeadlessEventLog {
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            // Default dark terminal background (#0d0d0d)
            default_bg: Arc::new(Mutex::new(Rgb {
                r: 13,
                g: 13,
                b: 13,
            })),
        }
    }
}

impl EventListener for HeadlessEventLog {
    fn send_event(&self, event: Event) {
        if let Event::ColorRequest(index, ref formatter) = event
            && index == NamedColor::Background as usize
        {
            let bg = *self.default_bg.lock().unwrap();
            let reply = formatter(bg);
            self.events.lock().unwrap().push(Event::PtyWrite(reply));
            return;
        }
        self.events.lock().unwrap().push(event);
    }
}

#[derive(Clone, Copy, Debug)]
struct HeadlessSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for HeadlessSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Deterministic headless terminal emulator combining `ansi::Processor` and `Term`.
pub struct HeadlessTerminal {
    pub term: Term<HeadlessEventLog>,
    pub parser: ansi::Processor,
    pub listener: HeadlessEventLog,
}

impl HeadlessTerminal {
    pub fn new(columns: usize, screen_lines: usize) -> Self {
        let size = HeadlessSize {
            columns,
            screen_lines,
        };
        let listener = HeadlessEventLog::new();
        let term = Term::new(Config::default(), &size, listener.clone());
        Self {
            term,
            parser: ansi::Processor::new(),
            listener,
        }
    }

    /// Feed a byte slice into the terminal parser in a single chunk.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Feed a byte slice into the terminal parser in fixed-size chunks to test
    /// state machine resilience across arbitrary split boundaries.
    pub fn feed_chunked(&mut self, bytes: &[u8], chunk_size: usize) {
        let step = chunk_size.max(1);
        for chunk in bytes.chunks(step) {
            self.parser.advance(&mut self.term, chunk);
        }
    }

    /// Set the default background color used when answering `Event::ColorRequest(257, ...)`.
    pub fn set_default_background(&mut self, rgb: Rgb) {
        *self.listener.default_bg.lock().unwrap() = rgb;
    }

    /// Resize the terminal grid to `(columns, screen_lines)`.
    pub fn resize(&mut self, columns: usize, screen_lines: usize) {
        let size = HeadlessSize {
            columns,
            screen_lines,
        };
        self.term.resize(size);
    }

    /// Drain and return all `Event::PtyWrite` responses emitted by the terminal.
    pub fn take_pty_writes(&mut self) -> Vec<String> {
        let mut events = self.listener.events.lock().unwrap();
        let mut writes = Vec::new();
        events.retain(|ev| {
            if let Event::PtyWrite(s) = ev {
                writes.push(s.clone());
                false
            } else {
                true
            }
        });
        writes
    }

    /// Drain and return all recorded terminal events.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut *self.listener.events.lock().unwrap())
    }

    /// Return the trimmed text of a visible viewport row (`0..screen_lines`).
    pub fn row_text_trimmed(&self, row: usize) -> String {
        let grid = self.term.grid();
        let cols = grid.columns();
        let mut s = String::with_capacity(cols);
        for col in 0..cols {
            let cell = &grid[Line(row as i32)][Column(col)];
            if cell.flags.intersects(
                crate::engine::term::cell::Flags::WIDE_CHAR_SPACER
                    | crate::engine::term::cell::Flags::LEADING_WIDE_CHAR_SPACER,
            ) {
                continue;
            }
            s.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(zw) = cell.zerowidth() {
                for &c in zw {
                    s.push(c);
                }
            }
        }
        s.trim_end().to_string()
    }

    /// Build a `RenderState` snapshot of the current terminal state.
    pub fn render_state(&self) -> RenderState {
        RenderState::new(&self.term)
    }

    /// Return the active `TermMode` bitflags.
    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::index::Point;
    use crate::terminal::imp::MatchRule;
    use crate::terminal::match_row_cells;

    #[test]
    fn decset_2031_color_scheme_updates_does_not_clobber_mouse_mode() {
        let mut h = HeadlessTerminal::new(80, 24);

        // App enables mouse click/motion reporting + SGR mouse, then subscribes to
        // DECSET 2031 (Color Scheme Updates, as Neovim 0.10+, Helix, and Fish 4.1 do).
        h.feed(b"\x1b[?1002h\x1b[?1006h\x1b[?2031h");

        assert!(
            h.mode().contains(TermMode::MOUSE_DRAG),
            "DECSET 2031 must not clear MOUSE_DRAG / MOUSE_MODE"
        );
        assert!(h.mode().contains(TermMode::SGR_MOUSE));
        assert!(h.mode().contains(TermMode::COLOR_SCHEME_UPDATES));

        // DECRQM for 2031 should report 1 (set)
        h.feed(b"\x1b[?2031$p");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?2031;1$y"]);

        // Disable 2031 and verify DECRQM reports 2 (reset)
        h.feed(b"\x1b[?2031l\x1b[?2031$p");
        assert!(!h.mode().contains(TermMode::COLOR_SCHEME_UPDATES));
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?2031;2$y"]);
    }

    #[test]
    fn dsr_996_reports_dark_and_light_color_scheme() {
        let mut h = HeadlessTerminal::new(80, 24);

        // Dark background (#0d0d0d) -> CSI ? 997 ; 1 n
        h.set_default_background(Rgb {
            r: 13,
            g: 13,
            b: 13,
        });
        h.feed(b"\x1b[?996n");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?997;1n"]);

        // Light background (#fafafa) -> CSI ? 997 ; 2 n
        h.set_default_background(Rgb {
            r: 250,
            g: 250,
            b: 250,
        });
        h.feed(b"\x1b[?996n");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?997;2n"]);

        // Explicit OSC 11 background override also updates DSR 996 response
        h.feed(b"\x1b]11;rgb:10/10/10\x07\x1b[?996n");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?997;1n"]);
    }

    #[test]
    fn osc_133_click_events_and_command_execution_lifecycle() {
        let mut h = HeadlessTerminal::new(80, 24);

        // Fish-style OSC 133;A;click_events=1 followed by prompt, OSC 133;B, and command text
        h.feed(b"\x1b]133;A;click_events=1\x07$ \x1b]133;B\x07echo hello");

        let state_at_prompt = h.render_state();
        assert_eq!(state_at_prompt.cl_mode.as_deref(), Some("line"));
        // Cursor is at col 12 (end of "$ echo hello"). Clicking at col 7 ('h') should move left 5.
        let nav = state_at_prompt.calculate_navigation_sequence(Point::new(Line(0), Column(7)));
        assert_eq!(nav, Some("\x1b[D".repeat(5)));

        // Once OSC 133;C fires (command is executing), cl_mode must be cleared so clicks
        // do not inject arrow keys into the running foreground process.
        h.feed(b"\x1b]133;C\x07\r\nhello\r\n");
        let state_running = h.render_state();
        assert_eq!(state_running.cl_mode, None);
        assert_eq!(
            state_running.calculate_navigation_sequence(Point::new(Line(0), Column(7))),
            None
        );
    }

    #[test]
    fn chunk_boundary_invariance_for_utf8_csi_and_osc() {
        let payload = concat!(
            "\x1b]7;file://localhost/var/tmp\x07",
            "\x1b]133;A;cl=line\x07",
            "\x1b[38;2;255;128;0m🚀 日本語\x1b[0m ",
            "\x1b]8;;https://example.com/docs\x1b\\link\x1b]8;;\x1b\\",
            "\x1b[?2026h\r\nsecond line\x1b[?2026l",
        )
        .as_bytes();

        let mut whole = HeadlessTerminal::new(40, 10);
        whole.feed(payload);

        for chunk_size in [1, 2, 3, 5, 7, 13] {
            let mut chunked = HeadlessTerminal::new(40, 10);
            chunked.feed_chunked(payload, chunk_size);

            assert_eq!(
                chunked.row_text_trimmed(0),
                whole.row_text_trimmed(0),
                "row 0 mismatch at chunk_size={chunk_size}"
            );
            assert_eq!(
                chunked.row_text_trimmed(1),
                whole.row_text_trimmed(1),
                "row 1 mismatch at chunk_size={chunk_size}"
            );
            assert_eq!(
                chunked.term.grid().cursor.point,
                whole.term.grid().cursor.point,
                "cursor mismatch at chunk_size={chunk_size}"
            );

            // Check hyperlink cell preservation on row 0 ("🚀 日本語 " is 2+1+6+1 = 10 cols, so "link" is at cols 10..14)
            let link_cell = &chunked.term.grid()[Line(0)][Column(10)];
            assert_eq!(
                link_cell.hyperlink().map(|h| h.uri().to_string()),
                Some("https://example.com/docs".to_string())
            );
        }
    }

    #[test]
    fn match_row_cells_handles_wide_chars_and_combining_marks() {
        let rules = vec![
            MatchRule {
                id: 1,
                regex: regex::Regex::new(r#"https?://[^\s)>\]'"]+"#).unwrap(),
                cursor_name: "pointer".to_string(),
            },
            MatchRule {
                id: 2,
                regex: regex::Regex::new(r#"(?:~|\.{0,2})?/[^\s:,;)>\]|*?\\'"]+"#).unwrap(),
                cursor_name: "pointer".to_string(),
            },
        ];

        // Case 1: CJK and Emoji (WIDE_CHAR + WIDE_CHAR_SPACER) before a URL on the same row.
        // "🚀 日本: https://example.com/test"
        // Cols:
        // 0..2: '🚀' (2 cols)
        // 2: ' '
        // 3..5: '日' (2 cols)
        // 5..7: '本' (2 cols)
        // 7: ':'
        // 8: ' '
        // 9..33: "https://example.com/test" (24 cols, 9..33)
        let mut h = HeadlessTerminal::new(50, 4);
        h.feed("🚀 日本: https://example.com/test".as_bytes());
        let state = h.render_state();
        let row_cells = &state.cells[0..50];

        // Hovering before the URL (on '日本' at col 4) -> no match
        assert_eq!(match_row_cells(row_cells, 0, 4, &rules), (None, 0, None));

        // Hovering first column of URL (col 9) -> matches full URL at cols 9..33
        let (matched, tag, bounds) = match_row_cells(row_cells, 0, 9, &rules);
        assert_eq!(matched.as_deref(), Some("https://example.com/test"));
        assert_eq!(tag, 1);
        assert_eq!(bounds, Some((0, 9, 33)));

        // Hovering last column of URL (col 32) -> matches full URL at cols 9..33
        let (matched_end, _, bounds_end) = match_row_cells(row_cells, 0, 32, &rules);
        assert_eq!(matched_end.as_deref(), Some("https://example.com/test"));
        assert_eq!(bounds_end, Some((0, 9, 33)));

        // Hovering col 33 (after URL) -> no match
        assert_eq!(match_row_cells(row_cells, 0, 33, &rules), (None, 0, None));

        // Case 2: Path containing wide CJK characters (verifying spacer cells don't inject spaces)
        // "./docs/日本語/file.txt"
        // "./docs/" = 7 cols (0..7)
        // "日本語" = 6 cols (7..13)
        // "/file.txt" = 9 cols (13..22)
        // Total = 22 cols (0..22)
        let mut h2 = HeadlessTerminal::new(50, 4);
        h2.feed("./docs/日本語/file.txt".as_bytes());
        let state2 = h2.render_state();
        let row_cells2 = &state2.cells[0..50];

        // Hovering on a wide-char spacer column (col 8, right half of '日') must still match the full path!
        let (path_match, path_tag, path_bounds) = match_row_cells(row_cells2, 0, 8, &rules);
        assert_eq!(path_match.as_deref(), Some("./docs/日本語/file.txt"));
        assert_eq!(path_tag, 2);
        assert_eq!(path_bounds, Some((0, 0, 22)));

        // Case 3: Combining mark (`e` + `\u{0301}`) before a URL
        let mut h3 = HeadlessTerminal::new(50, 4);
        h3.feed("cafe\u{0301} https://example.com/a".as_bytes());
        let state3 = h3.render_state();
        let row_cells3 = &state3.cells[0..50];
        assert!(row_cells3[3].zerowidth().is_some());

        // "café " is 5 columns (0..5), URL is 21 columns (5..26)
        let (comb_match, _, comb_bounds) = match_row_cells(row_cells3, 0, 5, &rules);
        assert_eq!(comb_match.as_deref(), Some("https://example.com/a"));
        assert_eq!(comb_bounds, Some((0, 5, 26)));
    }
}
