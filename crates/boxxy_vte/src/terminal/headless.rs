//! Headless terminal harness for deterministic protocol conformance,
//! chunk-boundary parsing, query-response verification, and reflow testing.

use parking_lot::Mutex;
use std::sync::Arc;

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
            let bg = *self.default_bg.lock();
            let reply = formatter(bg);
            self.events.lock().push(Event::PtyWrite(reply));
            return;
        }
        self.events.lock().push(event);
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
        *self.listener.default_bg.lock() = rgb;
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
        let mut events = self.listener.events.lock();
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
        std::mem::take(&mut *self.listener.events.lock())
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

    #[test]
    fn decscusr_and_dectcem_populate_render_state_cursor_style_and_visibility() {
        use crate::engine::ansi::CursorShape;

        let mut h = HeadlessTerminal::new(80, 24);
        let initial = h.render_state();
        assert!(initial.mode.contains(TermMode::SHOW_CURSOR));
        assert_eq!(initial.cursor_style, None);

        // DECSCUSR 6 -> steady Beam cursor (e.g. Neovim Insert mode)
        h.feed(b"\x1b[6 q");
        let beam_state = h.render_state();
        assert_eq!(
            beam_state.cursor_style.map(|s| (s.shape, s.blinking)),
            Some((CursorShape::Beam, false))
        );

        // DECSCUSR 3 -> blinking Underline cursor
        h.feed(b"\x1b[3 q");
        let ul_state = h.render_state();
        assert_eq!(
            ul_state.cursor_style.map(|s| (s.shape, s.blinking)),
            Some((CursorShape::Underline, true))
        );

        // DECTCEM ?25l -> hide cursor (e.g. TUI apps like btop/fzf)
        h.feed(b"\x1b[?25l");
        assert!(!h.render_state().mode.contains(TermMode::SHOW_CURSOR));

        // DECTCEM ?25h + DECSCUSR 0 -> show cursor and restore default user cursor style
        h.feed(b"\x1b[?25h\x1b[0 q");
        let restored = h.render_state();
        assert!(restored.mode.contains(TermMode::SHOW_CURSOR));
        assert_eq!(restored.cursor_style, None);
    }

    #[test]
    fn sgr_underline_styles_and_colors_update_cell_flags_and_row_hash() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        use crate::engine::ansi::Color;
        use crate::engine::term::cell::Flags;

        let hash_row = |cells: &[crate::engine::term::cell::Cell]| -> u64 {
            let mut hasher = DefaultHasher::new();
            cells.hash(&mut hasher);
            hasher.finish()
        };

        let mut h = HeadlessTerminal::new(20, 4);
        h.feed(b"plain");
        let base_hash = hash_row(&h.render_state().cells[0..20]);

        // Undercurl (4:3) with truecolor underline color (58:2::255:64:32)
        h.feed(b"\r\x1b[4:3;58:2::255:64:32mplain\x1b[0m");
        let state_curl = h.render_state();
        let cell0 = &state_curl.cells[0];
        assert!(cell0.flags.contains(Flags::UNDERCURL));
        assert_eq!(
            cell0.underline_color(),
            Some(Color::Spec(Rgb {
                r: 255,
                g: 64,
                b: 32
            }))
        );
        let curl_hash = hash_row(&state_curl.cells[0..20]);
        assert_ne!(base_hash, curl_hash);

        // Dotted (4:4), Dashed (4:5), Double (4:2) each produce distinct flags and hashes
        h.feed(b"\r\x1b[4:4mplain\x1b[0m");
        let state_dotted = h.render_state();
        assert!(state_dotted.cells[0].flags.contains(Flags::DOTTED_UNDERLINE));
        let dotted_hash = hash_row(&state_dotted.cells[0..20]);
        assert_ne!(curl_hash, dotted_hash);

        h.feed(b"\r\x1b[4:5mplain\x1b[0m");
        let state_dashed = h.render_state();
        assert!(state_dashed.cells[0].flags.contains(Flags::DASHED_UNDERLINE));
        let dashed_hash = hash_row(&state_dashed.cells[0..20]);
        assert_ne!(dotted_hash, dashed_hash);

        h.feed(b"\r\x1b[4:2mplain\x1b[0m");
        let state_double = h.render_state();
        assert!(state_double.cells[0].flags.contains(Flags::DOUBLE_UNDERLINE));
        let double_hash = hash_row(&state_double.cells[0..20]);
        assert_ne!(dashed_hash, double_hash);
    }

    #[test]
    fn kitty_keyboard_stack_set_query_and_alt_screen_isolation() {
        let mut h = HeadlessTerminal::new(80, 24);

        // Initial query -> 0
        h.feed(b"\x1b[?u");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?0u"]);

        // Direct set CSI = 1 u (DISAMBIGUATE_ESC_CODES) and query CSI ? u -> 1
        h.feed(b"\x1b[=1u\x1b[?u");
        assert!(h.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?1u"]);

        // Union CSI = 2;2 u (add REPORT_EVENT_TYPES) -> 3
        h.feed(b"\x1b[=2;2u\x1b[?u");
        assert!(
            h.mode()
                .contains(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES)
        );
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?3u"]);

        // Push stack CSI > 1 u then CSI > 15 u, then enter alt screen (?1049h)
        h.feed(b"\x1b[>1u\x1b[>15u\x1b[?u");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?15u"]);

        // Entering alt screen swaps to clean inactive keyboard stack (0)
        h.feed(b"\x1b[?1049h\x1b[?u");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?0u"]);

        // App on alt screen pushes mode 7, then crashes/exits alt screen (?1049l) -> restores main stack (15)
        h.feed(b"\x1b[>7u\x1b[?1049l\x1b[?u");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?15u"]);

        // Pop 1 -> back to 1
        h.feed(b"\x1b[<1u\x1b[?u");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?1u"]);
    }

    #[test]
    fn xterm_modify_other_keys_set_and_query() {
        let mut h = HeadlessTerminal::new(80, 24);

        h.feed(b"\x1b[?4m");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[>4;0m"]);

        h.feed(b"\x1b[>4;2m\x1b[?4m");
        assert!(h.mode().contains(TermMode::MODIFY_OTHER_KEYS_2));
        assert_eq!(h.take_pty_writes(), vec!["\x1b[>4;2m"]);

        h.feed(b"\x1b[>4;1m\x1b[?4m");
        assert!(h.mode().contains(TermMode::MODIFY_OTHER_KEYS_1));
        assert!(!h.mode().contains(TermMode::MODIFY_OTHER_KEYS_2));
        assert_eq!(h.take_pty_writes(), vec!["\x1b[>4;1m"]);

        h.feed(b"\x1b[>4m\x1b[?4m");
        assert!(!h.mode().intersects(TermMode::MODIFY_OTHER_KEYS_1 | TermMode::MODIFY_OTHER_KEYS_2));
        assert_eq!(h.take_pty_writes(), vec!["\x1b[>4;0m"]);
    }

    #[test]
    fn decset_2027_grapheme_clustering_zwj_skin_tone_vs16_and_flags() {
        use crate::engine::term::cell::Flags;

        let mut h = HeadlessTerminal::new(40, 6);

        // Mode 2027 is enabled by default and reportable via DECRQM (?2027$p -> ?2027;1$y)
        assert!(h.mode().contains(TermMode::GRAPHEME_CLUSTERING));
        h.feed(b"\x1b[?2027$p");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?2027;1$y"]);

        // 1. Emoji ZWJ sequence: 👨‍👩‍👧‍👦 (7 codepoints) must occupy exactly 2 columns (0..2)
        h.feed("👨‍👩‍👧‍👦!".as_bytes());
        assert_eq!(h.term.grid().cursor.point, Point::new(Line(0), Column(3)));
        assert!(h.term.grid()[Line(0)][Column(0)].flags.contains(Flags::WIDE_CHAR));
        assert!(
            h.term.grid()[Line(0)][Column(1)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
        );
        assert_eq!(h.term.grid()[Line(0)][Column(2)].c, '!');
        assert_eq!(h.row_text_trimmed(0), "👨‍👩‍👧‍👦!");

        // 2. Skin-tone modifier: 👍🏽 (\u{1F44D}\u{1F3FD}) must occupy 2 columns, not 4
        h.feed("\r\n👍🏽ok".as_bytes());
        assert_eq!(h.term.grid().cursor.point, Point::new(Line(1), Column(4)));
        assert_eq!(h.term.grid()[Line(1)][Column(2)].c, 'o');
        assert_eq!(h.term.grid()[Line(1)][Column(3)].c, 'k');
        assert_eq!(h.row_text_trimmed(1), "👍🏽ok");

        // 3. VS16 (\u{FE0F}): ❤️ (\u{2764}\u{FE0F}) and ⚠️ (\u{26A0}\u{FE0F}) widen from 1 col to 2 cols
        h.feed("\r\n❤️⚠️x".as_bytes());
        assert_eq!(h.term.grid().cursor.point, Point::new(Line(2), Column(5)));
        assert!(h.term.grid()[Line(2)][Column(0)].flags.contains(Flags::WIDE_CHAR));
        assert!(h.term.grid()[Line(2)][Column(2)].flags.contains(Flags::WIDE_CHAR));
        assert_eq!(h.term.grid()[Line(2)][Column(4)].c, 'x');
        assert_eq!(h.row_text_trimmed(2), "❤️⚠️x");

        // 4. Regional Indicator flags: 🇺🇸🇯🇵 (4 regional indicators -> 2 flags -> 4 columns total)
        h.feed("\r\n🇺🇸🇯🇵end".as_bytes());
        assert_eq!(h.term.grid().cursor.point, Point::new(Line(3), Column(7)));
        assert_eq!(h.term.grid()[Line(3)][Column(0)].c, '🇺');
        assert_eq!(h.term.grid()[Line(3)][Column(0)].zerowidth(), Some(&['🇸'][..]));
        assert_eq!(h.term.grid()[Line(3)][Column(2)].c, '🇯');
        assert_eq!(h.term.grid()[Line(3)][Column(2)].zerowidth(), Some(&['🇵'][..]));
        assert_eq!(h.term.grid()[Line(3)][Column(4)].c, 'e');
        assert_eq!(h.row_text_trimmed(3), "🇺🇸🇯🇵end");

        // 5. Engine regex search (`Term::search_next`) matches both columns (0..=1) of 🇺🇸 and 👨‍👩‍👧‍👦
        let mut flag_search =
            crate::engine::term::search::RegexSearch::new("🇺🇸", false).expect("compile flag regex");
        let flag_match = h.term.search_next(
            &mut flag_search,
            Point::new(Line(0), Column(0)),
            crate::engine::index::Direction::Right,
            crate::engine::index::Side::Left,
            None,
        );
        assert_eq!(
            flag_match,
            Some(Point::new(Line(3), Column(0))..=Point::new(Line(3), Column(1)))
        );

        let mut zwj_search = crate::engine::term::search::RegexSearch::new("👨‍👩‍👧‍👦", false)
            .expect("compile zwj regex");
        let zwj_match = h.term.search_next(
            &mut zwj_search,
            Point::new(Line(0), Column(0)),
            crate::engine::index::Direction::Right,
            crate::engine::index::Side::Left,
            None,
        );
        assert_eq!(
            zwj_match,
            Some(Point::new(Line(0), Column(0))..=Point::new(Line(0), Column(1)))
        );

        // 6. Disabling mode 2027 (?2027l) reports 2 via DECRQM and reverts to codepoint width
        h.feed(b"\x1b[?2027l\x1b[?2027$p");
        assert!(!h.mode().contains(TermMode::GRAPHEME_CLUSTERING));
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?2027;2$y"]);
    }

    #[test]
    fn prompt_jump_navigation_and_last_command_output_extraction() {
        use crate::engine::index::Direction;

        let mut h = HeadlessTerminal::new(40, 5);

        // Emit 3 commands with OSC 133 A/B/C/D markers so earlier prompts scroll into history
        for i in 1..=3 {
            let seq = format!(
                "\x1b]133;A\x07prompt-{i}$ \x1b]133;B\x07echo out-{i}\r\n\x1b]133;C\x07lineA-{i}\r\nlineB-{i}\r\n\x1b]133;D;0\x07"
            );
            h.feed(seq.as_bytes());
        }
        // Final idle prompt at bottom
        h.feed(b"\x1b]133;A\x07prompt-4$ \x1b]133;B\x07");

        // Last command output should be the output block of command 3
        assert_eq!(
            h.term.last_command_output().as_deref(),
            Some("lineA-3\nlineB-3")
        );

        // Initially at bottom (display_offset == 0)
        assert_eq!(h.term.grid().display_offset(), 0);

        // Jump to previous prompt (prompt-3 -> prompt-2 -> prompt-1)
        assert!(h.term.scroll_to_prompt(Direction::Left));
        let offset_first = h.term.grid().display_offset();
        assert!(offset_first > 0);
        assert!(h.render_state().cells[0..40].iter().any(|c| c.c == 'p'));

        assert!(h.term.scroll_to_prompt(Direction::Left));
        let offset_second = h.term.grid().display_offset();
        assert!(offset_second > offset_first);

        // Jump forward (Direction::Right) back toward the bottom
        assert!(h.term.scroll_to_prompt(Direction::Right));
        assert_eq!(h.term.grid().display_offset(), offset_first);

        assert!(h.term.scroll_to_prompt(Direction::Right));
        assert_eq!(h.term.grid().display_offset(), 0);
    }

    #[test]
    fn xtgettcap_decset_2048_in_band_resize_and_osc_3008_context() {
        use crate::engine::event::Osc3008Context;

        let mut h = HeadlessTerminal::new(80, 24);

        // 1. XTGETTCAP (DCS + q <hex> ST): query "TN" (544e), "Smulx" (536d756c78), and unknown "foo" (666f6f)
        h.feed(b"\x1bP+q544e;536d756c78;666f6f\x1b\\");
        let writes = h.take_pty_writes();
        assert_eq!(
            writes,
            vec![
                // "TN" = "xterm-256color"
                "\x1bP1+r544E=787465726D2D323536636F6C6F72\x1b\\",
                // "Smulx" = "\x1b[4:%p1%dm"
                "\x1bP1+r536D756C78=1B5B343A25703125646D\x1b\\",
                // "foo" -> unsupported (0+r)
                "\x1bP0+r666F6F\x1b\\",
            ]
        );

        // 2. DECSET 2048 (In-Band Resize Notifications): enabling emits initial size immediately,
        //    and resizing emits updated CSI 48 ; rows ; cols ; pixel_h ; pixel_w t.
        h.feed(b"\x1b[?2048h");
        assert!(h.mode().contains(TermMode::IN_BAND_RESIZE));
        assert_eq!(h.take_pty_writes(), vec!["\x1b[48;24;80;0;0t"]);

        h.resize(100, 30);
        assert_eq!(h.take_pty_writes(), vec!["\x1b[48;30;100;0;0t"]);

        h.feed(b"\x1b[?2048$p\x1b[?2048l");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?2048;1$y"]);
        assert!(!h.mode().contains(TermMode::IN_BAND_RESIZE));

        // 3. OSC 3008 (systemd v258+ / sudo context signaling)
        h.feed(b"\x1b]3008;start=abc123;user=alice;targetuser=root;pid=4242;comm=sudo\x07");
        let events = h.take_events();
        let osc_ctx = events.into_iter().find_map(|ev| match ev {
            Event::Osc3008(ctx) => Some(*ctx),
            _ => None,
        });
        assert_eq!(
            osc_ctx,
            Some(Osc3008Context {
                start: Some("abc123".to_string()),
                user: Some("alice".to_string()),
                target_user: Some("root".to_string()),
                pid: Some(4242),
                comm: Some("sudo".to_string()),
                ..Osc3008Context::default()
            })
        );
    }

    #[test]
    fn select_all_spans_scrollback_and_viewport_for_copy() {
        // 1. Scrollback + full viewport
        let mut h = HeadlessTerminal::new(20, 3);
        h.feed(b"line-1\r\nline-2\r\nline-3\r\nline-4\r\nline-5");
        h.term.select_all();

        let state_after = h.render_state();
        assert_eq!(
            state_after.selection_text().as_deref(),
            Some("line-1\nline-2\nline-3\nline-4\nline-5")
        );

        // 2. Partial viewport with trailing blank rows below cursor (should not append blank lines)
        let mut h_sparse = HeadlessTerminal::new(20, 10);
        h_sparse.feed(b"first\r\nsecond");
        h_sparse.term.select_all();

        let sparse_state = h_sparse.render_state();
        assert_eq!(
            sparse_state.selection_text().as_deref(),
            Some("first\nsecond")
        );
    }

    #[test]
    fn adjust_selection_expands_and_contracts_semantic_and_simple_selections() {
        use crate::engine::index::Side;
        use crate::engine::selection::{Selection, SelectionAdjust, SelectionType};

        let mut h = HeadlessTerminal::new(30, 4);
        h.feed(b"hello world testing\r\nsecond line here");

        // Start with a double-click Semantic selection on "world" (col 6..=10 on line 0)
        h.term.selection = Some(Selection::new(
            SelectionType::Semantic,
            Point::new(Line(0), Column(7)),
            Side::Left,
        ));
        assert_eq!(h.render_state().selection_text().as_deref(), Some("world"));

        // Shift+Right 4 times -> expands character-by-character to "world tes"
        for _ in 0..4 {
            assert!(h.term.adjust_selection(SelectionAdjust::Right));
        }
        assert_eq!(
            h.render_state().selection_text().as_deref(),
            Some("world tes")
        );

        // Shift+Left 2 times -> shrinks back to "world t"
        for _ in 0..2 {
            assert!(h.term.adjust_selection(SelectionAdjust::Left));
        }
        assert_eq!(
            h.render_state().selection_text().as_deref(),
            Some("world t")
        );

        // Shift+End -> extends to end of line 0 ("world testing")
        assert!(h.term.adjust_selection(SelectionAdjust::EndOfLine));
        assert_eq!(
            h.render_state().selection_text().as_deref(),
            Some("world testing")
        );

        // Shift+Down -> extends to line 1
        assert!(h.term.adjust_selection(SelectionAdjust::Down));
        assert_eq!(
            h.render_state().selection_text().as_deref(),
            Some("world testing\nsecond line here")
        );
    }

    #[test]
    fn decset_2026_sync_update_decrqm_and_dump_and_reset() {
        use crate::engine::ansi::Handler;

        let mut h = HeadlessTerminal::new(30, 3);

        // 1. Query ?2026$p inside a synchronized update block (?2026h ... ?2026l) -> reports 1 (set),
        //    then query ?2026$p after ?2026l -> reports 2 (reset).
        h.feed(b"\x1b[?2026h\x1b[?2026$p\x1b[?2026l\x1b[?2026$p");
        assert_eq!(
            h.take_pty_writes(),
            vec!["\x1b[?2026;1$y", "\x1b[?2026;2$y"]
        );

        // 2. dump_buffer_to_string(true) vs dump_buffer_to_string(false)
        h.feed(b"hist-1\r\nhist-2\r\nvis-1\r\nvis-2");
        assert_eq!(
            h.term.dump_buffer_to_string(true),
            "hist-1\nhist-2\nvis-1\nvis-2"
        );
        assert_eq!(h.term.dump_buffer_to_string(false), "hist-2\nvis-1\nvis-2");

        // 3. Reset state clears stuck mouse reporting (?1003h) and alt screen (?1049h)
        h.feed(b"\x1b[?1049h\x1b[?1003h");
        assert!(h.mode().contains(TermMode::ALT_SCREEN | TermMode::MOUSE_MOTION));
        h.term.reset_state();
        assert!(!h.mode().intersects(TermMode::ALT_SCREEN | TermMode::MOUSE_MODE));
    }

    #[test]
    fn bulk_osc_and_apc_chunk_invariance_and_can_sub_abort() {
        use base64::Engine;

        // 1. Large OSC 52 clipboard payload across arbitrary split chunk sizes
        let clip_text = "Clipboard test payload ".repeat(120);
        let clip_b64 = base64::engine::general_purpose::STANDARD.encode(clip_text.as_bytes());
        let osc52_seq = format!("\x1b]52;c;{clip_b64}\x07");

        for chunk_size in [1, 2, 3, 7, 64, 512] {
            let mut h = HeadlessTerminal::new(80, 24);
            h.feed_chunked(osc52_seq.as_bytes(), chunk_size);
            let stored = h.take_events().into_iter().find_map(|ev| match ev {
                Event::ClipboardStore(ty, text) => Some((ty, text)),
                _ => None,
            });
            assert_eq!(
                stored,
                Some((crate::engine::term::ClipboardType::Clipboard, clip_text.clone())),
                "OSC 52 mismatch at chunk_size={chunk_size}"
            );
        }

        // 2. CAN (0x18) and SUB (0x1A) abort an in-progress OSC or APC sequence without dispatching it
        let mut h = HeadlessTerminal::new(80, 24);
        h.feed(b"\x1b]2;Valid Title\x07");
        h.feed(b"\x1b]2;Aborted By CAN\x18");
        h.feed(b"\x1b]2;Aborted By SUB\x1a");
        let titles: Vec<String> = h
            .take_events()
            .into_iter()
            .filter_map(|ev| match ev {
                Event::Title(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(titles, vec!["Valid Title".to_string()]);

        // Aborted Kitty APC sequence must not leave placements or emit a response
        h.feed(b"\x1b_Ga=T,f=24,s=1,v=1,i=99;AAAA\x18");
        assert!(h.take_pty_writes().is_empty());
        assert!(!h.term.kitty_graphics.images.contains_key(&99));

        // Valid Kitty APC sequence split across 3-byte chunks still decodes and places image
        let valid_apc = b"\x1b_Ga=T,f=24,s=1,v=1,i=42;AAAA\x1b\\";
        h.feed_chunked(valid_apc, 3);
        assert_eq!(h.take_pty_writes(), vec!["\x1b_Gi=42;OK\x1b\\"]);
        assert!(h.term.kitty_graphics.images.contains_key(&42));
    }

    #[test]
    fn bulk_ascii_fast_path_wrapping_insert_line_drawing_and_rep() {
        // 1. Multi-line wrapping, INSERT mode, DEC line drawing, and wide-char overwrite
        let mut h_whole = HeadlessTerminal::new(12, 6);
        let stream = concat!(
            "0123456789abcdefghij\r\n",
            "wide:🚀end",
            "\rwide:XY!\r\n",
            "ins-test\x1b[4D\x1b[4hABCD\x1b[4l\r\n",
            "\x1b(0lqqqqk\x1b(B\r\n",
            "X\x1b[5bY"
        )
        .as_bytes();
        h_whole.feed(stream);

        // Compare against 1-byte chunked feed (which exercises single-char print(c))
        let mut h_char = HeadlessTerminal::new(12, 6);
        h_char.feed_chunked(stream, 1);

        for r in 0..6 {
            assert_eq!(
                h_whole.row_text_trimmed(r),
                h_char.row_text_trimmed(r),
                "row {r} mismatch between bulk ASCII and 1-byte feed"
            );
        }
        assert_eq!(h_whole.row_text_trimmed(0), "0123456789ab");
        assert_eq!(h_whole.row_text_trimmed(1), "cdefghij");
        assert_eq!(h_whole.row_text_trimmed(2), "wide:XY!nd");
        assert_eq!(h_whole.row_text_trimmed(3), "ins-ABCDtest");
        assert_eq!(h_whole.row_text_trimmed(4), "┌────┐");
        assert_eq!(h_whole.row_text_trimmed(5), "XXXXXXY");
    }

    #[test]
    fn zerowidth_combining_cap_and_scrollback_shrink_to_fit() {
        use crate::engine::ansi::Handler;
        use crate::engine::term::cell::MAX_ZEROWIDTH_CHARS;

        // 1. Combining mark cap per cell (64 max)
        let mut h = HeadlessTerminal::new(20, 4);
        let mut seq = String::from("a");
        for _ in 0..200 {
            seq.push('\u{0301}');
        }
        h.feed(seq.as_bytes());
        let zw_len = h.term.grid()[Line(0)][Column(0)]
            .zerowidth()
            .map(|z| z.len())
            .unwrap_or(0);
        assert_eq!(zw_len, MAX_ZEROWIDTH_CHARS);

        // 2. Scrollback clear shrinks storage capacity back down to visible lines
        for i in 0..1500 {
            h.feed(format!("line {i}\r\n").as_bytes());
        }
        assert!(h.term.grid().total_lines() > 1000);
        h.term.clear_screen(ansi::ClearMode::Saved);
        assert_eq!(h.term.grid().total_lines(), 4);
    }

    #[test]
    fn render_state_deduplicates_kitty_and_selection_snapshots() {
        let mut h = HeadlessTerminal::new(40, 10);
        let mut state = RenderState::default();

        h.feed(b"\x1b_Ga=T,f=24,s=1,v=1,i=7;AAAA\x1b\\");
        state.update_from(&mut h.term, 1);
        assert_eq!(state.kitty_generation, 1);
        assert_eq!(state.kitty_placements.len(), 1);
        assert_eq!(state.selection_text, None);

        // Subsequent plain text turns do not bump kitty_generation
        h.feed(b"hello world");
        state.update_from(&mut h.term, 2);
        assert_eq!(state.kitty_generation, 1);
        assert_eq!(state.kitty_placements.len(), 1);
    }

    #[test]
    fn decset_2033_visibility_reports_and_dsr_998_query() {
        let mut h = HeadlessTerminal::new(80, 24);
        assert!(h.term.is_visible);

        // 1. DSR ? 998 n query reports visible (`\x1b[?999;1n`) by default
        h.feed(b"\x1b[?998n");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?999;1n"]);

        // 2. When hidden (`is_visible = false`), DSR ? 998 n reports not visible (`\x1b[?999;2n`)
        h.term.is_visible = false;
        h.feed(b"\x1b[?998n");
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?999;2n"]);

        // 3. DECSET 2033 (`?2033h` / `?2033l`) immediately emits current visibility state,
        //    and DECRQM (`?2033$p`) reports mode state.
        h.feed(b"\x1b[?2033h\x1b[?2033$p");
        assert!(h.mode().contains(TermMode::VISIBILITY_REPORTS));
        assert_eq!(
            h.take_pty_writes(),
            vec!["\x1b[?999;2n", "\x1b[?2033;1$y"]
        );

        h.feed(b"\x1b[?2033l\x1b[?2033$p");
        assert!(!h.mode().contains(TermMode::VISIBILITY_REPORTS));
        assert_eq!(h.take_pty_writes(), vec!["\x1b[?2033;2$y"]);
    }

    #[test]
    fn decstr_soft_reset_decxcpr_csi_19t_decbkm_and_wrap_hygiene() {
        use crate::engine::ansi::{Color, NamedColor};
        use crate::engine::term::cell::Flags;

        let mut h = HeadlessTerminal::new(20, 6);

        // 1. DECXCPR (CSI ? 6 n) and CSI 19 t (report screen size in chars)
        h.feed(b"\x1b[3;7H\x1b[?6n\x1b[19t");
        assert_eq!(
            h.take_pty_writes(),
            vec!["\x1b[?3;7;1R", "\x1b[9;6;20t"]
        );

        // 2. DECBKM (Private Mode 67): enable, query via DECRQM, disable
        h.feed(b"\x1b[?67h\x1b[?67$p\x1b[?67l\x1b[?67$p");
        assert_eq!(
            h.take_pty_writes(),
            vec!["\x1b[?67;1$y", "\x1b[?67;2$y"]
        );

        // 3. DECSTR (CSI ! p): resets modes (INSERT, ORIGIN, APP_CURSOR, SHOW_CURSOR, LINE_WRAP),
        //    scroll margins, and SGR template attributes without clearing screen text!
        h.feed(b"\x1b[1;1HKeptText\x1b[2;4r\x1b[?6h\x1b[4h\x1b[?1h\x1b[?25l\x1b[?7l\x1b[1;31m");
        assert!(h.mode().contains(TermMode::ORIGIN | TermMode::INSERT | TermMode::APP_CURSOR));
        assert!(!h.mode().contains(TermMode::SHOW_CURSOR | TermMode::LINE_WRAP));

        h.feed(b"\x1b[!p");
        assert!(!h.mode().intersects(TermMode::ORIGIN | TermMode::INSERT | TermMode::APP_CURSOR));
        assert!(h.mode().contains(TermMode::SHOW_CURSOR | TermMode::LINE_WRAP));
        assert_eq!(h.term.grid().cursor.template.flags, Flags::empty());
        assert_eq!(
            h.term.grid().cursor.template.fg,
            Color::Named(NamedColor::Foreground)
        );
        assert_eq!(h.row_text_trimmed(0), "KeptText");

        // 4. Printing at the last column with autowrap (?7l) disabled must not leave a
        //    stale pending wrap when autowrap (?7h) is re-enabled.
        let mut h2 = HeadlessTerminal::new(5, 3);
        h2.feed(b"\x1b[?7l123456\x1b[?7hA");
        // '6' overwrote col 4 on row 0 and cleared pending wrap; 'A' overwrote col 4 on row 0
        // rather than spuriously wrapping to row 1 before printing 'A'.
        assert_eq!(h2.row_text_trimmed(0), "1234A");
        assert_eq!(h2.row_text_trimmed(1), "");

        // 5. RIS (\x1bc) restores palette colors modified via OSC 4
        h2.feed(b"\x1b]4;1;rgb:ff/00/00\x07");
        assert!(h2.term.colors()[1].is_some());
        h2.feed(b"\x1bc");
        assert!(h2.term.colors()[1].is_none());
    }

    #[test]
    fn kitty_placements_isolated_across_alt_screen_swap() {
        let mut h = HeadlessTerminal::new(40, 10);
        let mut state = RenderState::default();

        // 1. Display a Kitty image on the primary screen
        h.feed(b"\x1b_Ga=T,f=24,s=1,v=1,i=10;AAAA\x1b\\");
        state.update_from(&mut h.term, 1);
        assert_eq!(state.kitty_placements.len(), 1);
        assert_eq!(state.kitty_placements[0].image_id, 10);

        // 2. Enter alternate screen (?1049h, e.g. launching btop/nvim):
        //    primary screen placements must be hidden on the alternate screen,
        //    even if the TUI clears the alternate screen (CSI 2 J).
        h.feed(b"\x1b[?1049h\x1b[2J");
        state.update_from(&mut h.term, 2);
        assert!(
            state.kitty_placements.is_empty(),
            "primary screen Kitty placements must not bleed onto ALT_SCREEN"
        );
        assert!(
            h.term.kitty_graphics.images.contains_key(&10),
            "primary screen image must remain retained while in ALT_SCREEN"
        );

        // 3. Leave alternate screen (?1049l, e.g. quitting btop):
        //    primary screen placements are restored automatically!
        h.feed(b"\x1b[?1049l");
        state.update_from(&mut h.term, 3);
        assert_eq!(state.kitty_placements.len(), 1);
        assert_eq!(state.kitty_placements[0].image_id, 10);
    }

    #[test]
    fn fast_crlf_unicode_str_consecutive_csi_and_render_state_copy_invariance() {
        // Exercises:
        // - `crlf_and_input_ascii_slice` & `Row::reset_after` when a shorter line follows
        //   a longer line after scrolling, and when background color changes before `\r\n`
        // - `crlf_and_input_str` / `input_str` across mixed ASCII, CJK, Box Drawing, Math
        //   combining marks (`B⃗`), Emoji ZWJ (`👩‍💻`), VS16 (`🏳️‍🌈`), and Regional Indicators (`🇺🇸`)
        // - `advance_csi_slice` back-to-back CSI sequences (`\x1b[38;2;...m\x1b[48;2;...m\x1b[1m`)
        // - `RenderState::update_from` bitwise copy path (`!has_any_extra`) transitioning to
        //   `has_any_extra = true` when combining marks / hyperlinks / underline colors appear
        let mut payload = Vec::new();
        for i in 0..25usize {
            use std::io::Write;
            let _ = write!(
                payload,
                "Long ASCII line number {i:04} with trailing characters to fill columns!\r\n"
            );
            let _ = write!(payload, "Short {i:02}\r\n");
            let _ = write!(
                payload,
                "\x1b[38;2;10;20;30m\x1b[48;2;40;50;60m\x1b[1mColor {i:02}\x1b[0m\r\n"
            );
            let _ = write!(
                payload,
                "[{i:04}] ⚡ 🚀 🦀 測試 🔥 👩‍💻 日本語 🏳️‍🌈 🇺🇸 ∀x ∈ ℝ: B⃗ café ┌─┬┐\r\n"
            );
        }

        let mut h_bulk = HeadlessTerminal::new(50, 8);
        let mut state = RenderState::default();
        // First feed pure ASCII to exercise `!has_any_extra` fast copy in `RenderState::update_from`
        h_bulk.feed(b" Pure ASCII line 1\r\nPure ASCII line 2\r\n");
        state.update_from(&mut h_bulk.term, 1);
        assert!(!state.has_any_extra);

        // Now feed the full mixed payload in one bulk chunk vs 1-byte chunks
        h_bulk.feed(&payload);
        state.update_from(&mut h_bulk.term, 2);
        assert!(state.has_any_extra);

        let mut h_byte = HeadlessTerminal::new(50, 8);
        h_byte.feed_chunked(b" Pure ASCII line 1\r\nPure ASCII line 2\r\n", 1);
        h_byte.feed_chunked(&payload, 1);
        let state_byte = h_byte.render_state();

        for r in 0..8 {
            assert_eq!(
                h_bulk.row_text_trimmed(r),
                h_byte.row_text_trimmed(r),
                "row {r} text mismatch between bulk and 1-byte feed"
            );
        }
        assert_eq!(state.cells, state_byte.cells);
        assert_eq!(state.cursor_point, state_byte.cursor_point);
    }

    #[test]
    fn kitty_unicode_placeholder_u1_c1_q2_and_unique_id_invalidation() {
        let mut h = HeadlessTerminal::new(40, 10);

        // 1. Move cursor to (row 2, col 5 -> 1-based 3;6H) and emit Yazi-style
        //    `q=2,a=T,C=1,U=1,f=24,s=1,v=1,i=0x0F8A74;AAAA` (image_id = 1018484 = 0x0F8A74 -> RGB 15, 138, 116).
        h.feed(b"\x1b[3;6H\x1b_Gq=2,a=T,C=1,U=1,f=24,s=1,v=1,i=1018484;AAAA\x1b\\");

        // `q=2` must suppress the `OK` response, and `C=1`/`U=1` must not move the cursor.
        assert!(h.take_pty_writes().is_empty());
        assert_eq!(h.term.grid().cursor.point, Point::new(Line(2), Column(5)));

        let uid1 = h.term.kitty_graphics.images.get(&1018484).unwrap().unique_id;

        // 2. Emit a 3x2 grid of `\u{10EEEE}` placeholder cells with 24-bit fg `rgb(15, 138, 116)`
        //    and row/column diacritics (`\u{0305}\u{0305}`), then clear screen (`CSI 2 J`) and
        //    redraw the `\u{10EEEE}` cells at (row 4, col 10 -> 1-based 5;11H).
        h.feed(
            "\x1b[2J\x1b[38;2;15;138;116m\x1b[5;11H\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{030D}\u{10EEEE}\u{0305}\u{030E}\x1b[6;11H\u{10EEEE}\u{030D}\u{0305}\u{10EEEE}\u{030D}\u{030D}\u{10EEEE}\u{030D}\u{030E}\x1b[0m"
                .as_bytes(),
        );

        let state = h.render_state();
        // Virtual (`U=1`) placement survived `CSI 2 J`
        assert_eq!(state.kitty_placements.len(), 1);
        assert!(state.kitty_placements[0].unicode_placeholder);

        // Replacing the same `image_id` (1018484) bumps `unique_id` so the GPU texture cache invalidates
        h.feed(b"\x1b_Gq=2,a=T,C=1,U=1,f=24,s=1,v=1,i=1018484;////\x1b\\");
        let uid2 = h.term.kitty_graphics.images.get(&1018484).unwrap().unique_id;
        assert_ne!(uid1, uid2);
    }
}
