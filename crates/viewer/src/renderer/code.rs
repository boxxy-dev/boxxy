use crate::cache::{
    HighlightedCode, SyntaxSpan, get_cached_highlight, insert_cached_highlight,
    is_card_surface_dark, is_shell_language, resolve_syntect_fg, resolve_syntax, syntax_set,
    theme_set,
};
use crate::parser::blocks::ContentBlock;
use crate::parser::markdown::normalize_fence_language;
use crate::renderer::{BlockRenderer, RenderContext};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, pango};
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use syntect::easy::HighlightLines;
use syntect::highlighting::{HighlightState, Highlighter};
use syntect::parsing::{ParseState, ScopeStack};
use syntect::util::LinesWithEndings;

/// Matches OpenCode's benchmarked `virtualizeLineThreshold = 200`:
/// blocks `<= 200` lines use a single `GtkLabel`, while blocks `> 200` lines split
/// into `CODE_CHUNK_LINES`-line `GtkLabel` slices so GTK4/GSK frustum-culls off-screen slices.
pub const VIRTUALIZE_LINE_THRESHOLD: usize = 200;
pub const CODE_CHUNK_LINES: usize = 100;

const SYNC_HIGHLIGHT_MAX_BYTES: usize = 8_192;
const SYNC_HIGHLIGHT_MAX_LINES: usize = 80;
const MAX_HIGHLIGHT_LINE_BYTES: usize = 2_048;
/// Matches OpenCode's `500_000` large-file tokenization cutoff (`packages/session-ui/src/components/file.tsx`).
const MAX_HIGHLIGHT_TOTAL_BYTES: usize = 500_000;
const MAX_POOLED_CHUNK_LABELS: usize = 4;

/// Strips at most one trailing fence newline (`\r\n` or `\n`) emitted by CommonMark parsers
/// so code cards do not display a phantom empty bottom line while keeping raw source for Copy.
#[must_use]
pub fn strip_trailing_fence_newline(code: &str) -> &str {
    code.strip_suffix("\r\n")
        .or_else(|| code.strip_suffix('\n'))
        .unwrap_or(code)
}

fn scan_newline_offsets(text: &str) -> Vec<u32> {
    let mut offsets = Vec::new();
    for (idx, &b) in text.as_bytes().iter().enumerate() {
        if b == b'\n'
            && let Ok(pos) = u32::try_from(idx)
        {
            offsets.push(pos);
        }
    }
    offsets
}

/// Appends a foreground color span to `spans`, trimming invisible leading/trailing whitespace
/// and coalescing adjacent same-color tokens on the same line (including across spaces/tabs).
fn push_coalesced_span(
    spans: &mut Vec<SyntaxSpan>,
    full_text: &str,
    line_start_byte: u32,
    raw_start: u32,
    raw_end: u32,
    rgb: (u8, u8, u8),
) {
    let slice = &full_text[raw_start as usize..raw_end as usize];
    let lead_ws = slice
        .as_bytes()
        .iter()
        .take_while(|&&b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .count();
    let trail_ws = slice.as_bytes()[lead_ws..]
        .iter()
        .rev()
        .take_while(|&&b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .count();

    #[allow(clippy::cast_possible_truncation)]
    let start_byte = raw_start + lead_ws as u32;
    #[allow(clippy::cast_possible_truncation)]
    let end_byte = raw_end - trail_ws as u32;
    if start_byte >= end_byte {
        return;
    }

    let (r, g, b) = rgb;
    if let Some(last) = spans.last_mut()
        && last.end_byte >= line_start_byte
        && last.r == r
        && last.g == g
        && last.b == b
        && full_text[last.end_byte as usize..start_byte as usize]
            .bytes()
            .all(|ch| matches!(ch, b' ' | b'\t'))
    {
        last.end_byte = end_byte;
        return;
    }

    spans.push(SyntaxSpan {
        start_byte,
        end_byte,
        r,
        g,
        b,
    });
}

struct LineHighlightEnv<'a> {
    ss: &'a syntect::parsing::SyntaxSet,
    default_fg: Option<syntect::highlighting::Color>,
    is_dark: bool,
    full_text: &'a str,
}

fn highlight_single_line(
    hl: &mut HighlightLines<'_>,
    env: &LineHighlightEnv<'_>,
    line: &str,
    line_start_byte: u32,
    spans: &mut Vec<SyntaxSpan>,
) {
    if line.len() > MAX_HIGHLIGHT_LINE_BYTES {
        return;
    }
    let Ok(ranges) = hl.highlight_line(line, env.ss) else {
        return;
    };
    let mut cursor = line_start_byte;
    for (style, token) in ranges {
        #[allow(clippy::cast_possible_truncation)]
        let next_cursor = cursor + token.len() as u32;
        if let Some(rgb) = resolve_syntect_fg(style, env.default_fg, env.is_dark) {
            push_coalesced_span(
                spans,
                env.full_text,
                line_start_byte,
                cursor,
                next_cursor,
                rgb,
            );
        }
        cursor = next_cursor;
    }
}

/// Computes coalesced `HighlightedCode` spans and `\n` offsets for `display_code`.
#[must_use]
pub fn compute_highlighted_code(
    lang: &str,
    display_code: &str,
    is_dark: bool,
) -> Arc<HighlightedCode> {
    let newline_offsets = scan_newline_offsets(display_code);

    if display_code.is_empty()
        || is_shell_language(lang)
        || display_code.len() > MAX_HIGHLIGHT_TOTAL_BYTES
        || u32::try_from(display_code.len()).is_err()
    {
        return Arc::new(HighlightedCode {
            spans: Box::new([]),
            newline_offsets: newline_offsets.into_boxed_slice(),
        });
    }

    let ss = syntax_set();
    let syntax = resolve_syntax(ss, lang, Some(display_code));
    if syntax.name == ss.find_syntax_plain_text().name {
        return Arc::new(HighlightedCode {
            spans: Box::new([]),
            newline_offsets: newline_offsets.into_boxed_slice(),
        });
    }

    let ts = theme_set();
    let theme = if is_dark {
        &ts.themes["base16-ocean.dark"]
    } else {
        &ts.themes["base16-ocean.light"]
    };
    let default_fg = theme.settings.foreground;

    let mut hl = HighlightLines::new(syntax, theme);
    let mut spans = Vec::with_capacity(newline_offsets.len().saturating_add(1) * 3);
    let mut line_start_byte: u32 = 0;

    let env = LineHighlightEnv {
        ss,
        default_fg,
        is_dark,
        full_text: display_code,
    };

    for line in LinesWithEndings::from(display_code) {
        highlight_single_line(&mut hl, &env, line, line_start_byte, &mut spans);
        #[allow(clippy::cast_possible_truncation)]
        {
            line_start_byte += line.len() as u32;
        }
    }

    Arc::new(HighlightedCode {
        spans: spans.into_boxed_slice(),
        newline_offsets: newline_offsets.into_boxed_slice(),
    })
}

/// OpenCode-style stateful `stable` + `unstable` streaming line lexer:
/// completed lines (`.*\n`) permanently advance `parse_state` & `highlight_state`, while only the
/// growing final line is re-lexed per 16ms delta—turning O(N²) streaming highlights into O(N).
struct StreamCodeLexer {
    lang: String,
    is_dark: bool,
    is_plain: bool,
    stable_source: String,
    stable_spans: Vec<SyntaxSpan>,
    stable_newline_offsets: Vec<u32>,
    highlight_state: HighlightState,
    parse_state: ParseState,
}

impl StreamCodeLexer {
    fn new(lang: &str, first_sample: &str, is_dark: bool) -> Self {
        let ss = syntax_set();
        let ts = theme_set();
        let theme = if is_dark {
            &ts.themes["base16-ocean.dark"]
        } else {
            &ts.themes["base16-ocean.light"]
        };
        let syntax = resolve_syntax(ss, lang, Some(first_sample));
        let is_plain =
            is_shell_language(lang) || syntax.name == ss.find_syntax_plain_text().name;
        let highlighter = Highlighter::new(theme);
        let highlight_state = HighlightState::new(&highlighter, ScopeStack::new());
        let parse_state = ParseState::new(syntax);

        Self {
            lang: lang.to_string(),
            is_dark,
            is_plain,
            stable_source: String::new(),
            stable_spans: Vec::new(),
            stable_newline_offsets: Vec::new(),
            highlight_state,
            parse_state,
        }
    }

    fn matches(&self, lang: &str, display_code: &str, is_dark: bool) -> bool {
        self.lang == lang && self.is_dark == is_dark && display_code.starts_with(&self.stable_source)
    }

    /// Advances `stable_*` across newly completed lines (`\n`-terminated) and returns
    /// temporary spans covering the final incomplete `unstable` line.
    fn advance_and_lex_tail(&mut self, display_code: &str) -> Vec<SyntaxSpan> {
        if self.is_plain
            || display_code.len() > MAX_HIGHLIGHT_TOTAL_BYTES
            || u32::try_from(display_code.len()).is_err()
        {
            // Still maintain `stable_newline_offsets` incrementally for 100-line chunking
            let prev_len = self.stable_source.len();
            let delta = &display_code[prev_len..];
            if let Some(last_nl_rel) = delta.rfind('\n') {
                let commit_end = prev_len + last_nl_rel + 1;
                for (i, &b) in display_code.as_bytes()[prev_len..commit_end]
                    .iter()
                    .enumerate()
                {
                    if b == b'\n'
                        && let Ok(pos) = u32::try_from(prev_len + i)
                    {
                        self.stable_newline_offsets.push(pos);
                    }
                }
                self.stable_source
                    .push_str(&display_code[prev_len..commit_end]);
            }
            return Vec::new();
        }

        let ss = syntax_set();
        let ts = theme_set();
        let theme = if self.is_dark {
            &ts.themes["base16-ocean.dark"]
        } else {
            &ts.themes["base16-ocean.light"]
        };
        let default_fg = theme.settings.foreground;

        let env = LineHighlightEnv {
            ss,
            default_fg,
            is_dark: self.is_dark,
            full_text: display_code,
        };

        let prev_len = self.stable_source.len();
        let delta = &display_code[prev_len..];

        if let Some(last_nl_rel) = delta.rfind('\n') {
            let commit_slice = &delta[..=last_nl_rel];
            let mut hl = HighlightLines::from_state(
                theme,
                self.highlight_state.clone(),
                self.parse_state.clone(),
            );
            #[allow(clippy::cast_possible_truncation)]
            let mut line_start_byte = prev_len as u32;

            for line in LinesWithEndings::from(commit_slice) {
                highlight_single_line(
                    &mut hl,
                    &env,
                    line,
                    line_start_byte,
                    &mut self.stable_spans,
                );
                #[allow(clippy::cast_possible_truncation)]
                let line_len = line.len() as u32;
                if line.ends_with('\n') {
                    self.stable_newline_offsets
                        .push(line_start_byte + line_len - 1);
                }
                line_start_byte += line_len;
            }

            let (next_hs, next_ps) = hl.state();
            self.highlight_state = next_hs;
            self.parse_state = next_ps;
            self.stable_source.push_str(commit_slice);
        }

        let unstable_tail = &display_code[self.stable_source.len()..];
        let mut unstable_spans = Vec::new();
        if !unstable_tail.is_empty() {
            let mut temp_hl = HighlightLines::from_state(
                theme,
                self.highlight_state.clone(),
                self.parse_state.clone(),
            );
            #[allow(clippy::cast_possible_truncation)]
            let tail_start = self.stable_source.len() as u32;
            highlight_single_line(
                &mut temp_hl,
                &env,
                unstable_tail,
                tail_start,
                &mut unstable_spans,
            );
        }

        unstable_spans
    }

    fn finalize(mut self, display_code: &str) -> Arc<HighlightedCode> {
        let mut unstable = self.advance_and_lex_tail(display_code);
        self.stable_spans.append(&mut unstable);
        Arc::new(HighlightedCode {
            spans: self.stable_spans.into_boxed_slice(),
            newline_offsets: self.stable_newline_offsets.into_boxed_slice(),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ChunkStamp {
    start_byte: usize,
    end_byte: usize,
    text_hash: u64,
    spans_hash: u64,
}

fn hash_bytes_and_spans(text: &str, spans: &[SyntaxSpan], chunk_start: usize, chunk_end: usize) -> (u64, u64) {
    let mut th = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut th);

    let mut sh = std::collections::hash_map::DefaultHasher::new();
    for s in spans {
        let st = (s.start_byte as usize).max(chunk_start) - chunk_start;
        let en = (s.end_byte as usize).min(chunk_end) - chunk_start;
        (st, en, s.r, s.g, s.b).hash(&mut sh);
    }
    (th.finish(), sh.finish())
}

#[must_use]
fn compute_chunk_ranges(total_bytes: usize, newline_offsets: &[u32]) -> Vec<(usize, usize)> {
    let line_count = newline_offsets.len() + 1;
    if total_bytes == 0 || line_count <= VIRTUALIZE_LINE_THRESHOLD {
        return vec![(0, total_bytes)];
    }

    let num_chunks = line_count.div_ceil(CODE_CHUNK_LINES);
    let mut ranges = Vec::with_capacity(num_chunks);
    let mut start = 0usize;

    let mut nl_idx = CODE_CHUNK_LINES - 1;
    while nl_idx < newline_offsets.len() {
        let end = newline_offsets[nl_idx] as usize;
        ranges.push((start, end));
        start = end + 1;
        nl_idx += CODE_CHUNK_LINES;
    }

    if start <= total_bytes {
        ranges.push((start, total_bytes));
    }
    ranges
}

fn build_chunk_attr_list(
    spans: &[SyntaxSpan],
    chunk_start: usize,
    chunk_end: usize,
) -> Option<pango::AttrList> {
    if spans.is_empty() || chunk_start >= chunk_end {
        return None;
    }
    let first = spans.partition_point(|s| (s.end_byte as usize) <= chunk_start);
    let mut list: Option<pango::AttrList> = None;

    for s in &spans[first..] {
        let s_start = s.start_byte as usize;
        if s_start >= chunk_end {
            break;
        }
        let s_end = s.end_byte as usize;
        let local_start = s_start.max(chunk_start) - chunk_start;
        let local_end = s_end.min(chunk_end) - chunk_start;
        if local_start >= local_end {
            continue;
        }

        let attr_list = list.get_or_insert_with(pango::AttrList::new);
        let mut attr = pango::AttrColor::new_foreground(
            u16::from(s.r) * 257,
            u16::from(s.g) * 257,
            u16::from(s.b) * 257,
        );
        #[allow(clippy::cast_possible_truncation)]
        {
            attr.set_start_index(local_start as u32);
            attr.set_end_index(local_end as u32);
        }
        attr_list.insert(attr);
    }

    list
}

fn make_chunk_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_wrap(false);
    label.set_halign(gtk::Align::Fill);
    label.set_hexpand(true);
    label.set_xalign(0.0);
    label.add_css_class("monospace");
    label.add_css_class("viewer-code-chunk");
    label
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CodeBlockWidget {
        pub(super) frame: RefCell<Option<gtk::Frame>>,
        pub(super) scroll: RefCell<Option<gtk::ScrolledWindow>>,
        pub(super) chunks_box: RefCell<Option<gtk::Box>>,
        pub(super) lang_pill: RefCell<Option<gtk::Label>>,
        pub(super) copy_btn: RefCell<Option<gtk::Button>>,
        pub(super) chunk_labels: RefCell<Vec<gtk::Label>>,
        pub(super) applied_chunks: RefCell<Vec<ChunkStamp>>,
        pub(super) lang: RefCell<String>,
        pub(super) raw_code: RefCell<String>,
        pub(super) last_card_dark: Cell<Option<bool>>,
        pub(super) local_serial: Cell<u64>,
        pub(super) stream_lexer: RefCell<Option<StreamCodeLexer>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CodeBlockWidget {
        const NAME: &'static str = "BoxxyViewerCodeBlock";
        type Type = super::CodeBlockWidget;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CodeBlockWidget {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for CodeBlockWidget {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if let Some(child) = self.obj().first_child() {
                child.measure(orientation, for_size)
            } else {
                (0, 0, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                let _ = child.measure(gtk::Orientation::Vertical, width);
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let current_dark = is_card_surface_dark();
            if let Some(prev) = self.last_card_dark.replace(Some(current_dark))
                && prev != current_dark
            {
                let obj_weak = self.obj().downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(obj) = obj_weak.upgrade() {
                        obj.refresh_for_card_darkness(current_dark);
                    }
                });
            }
            self.parent_snapshot(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct CodeBlockWidget(ObjectSubclass<imp::CodeBlockWidget>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for CodeBlockWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeBlockWidget {
    #[must_use]
    pub fn new() -> Self {
        let obj: Self = glib::Object::new();
        obj.set_hexpand(true);
        obj.set_halign(gtk::Align::Fill);
        obj.set_margin_bottom(12);

        let frame = gtk::Frame::new(None);
        frame.add_css_class("view");
        frame.add_css_class("viewer-code-block");
        frame.set_overflow(gtk::Overflow::Hidden);
        frame.set_hexpand(true);

        let overlay = gtk::Overlay::new();
        overlay.set_hexpand(true);

        let chunks_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        chunks_box.set_hexpand(true);
        chunks_box.set_margin_top(10);
        chunks_box.set_margin_bottom(10);
        chunks_box.set_margin_start(12);
        chunks_box.set_margin_end(12);

        let first_label = make_chunk_label();
        chunks_box.append(&first_label);

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .hexpand(true)
            .child(&chunks_box)
            .build();
        overlay.set_child(Some(&scroll));

        // OpenCode-style top-end floating action overlay: subtle language pill + hover-revealed Copy button
        let action_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        action_bar.add_css_class("viewer-code-overlay-bar");
        action_bar.set_halign(gtk::Align::End);
        action_bar.set_valign(gtk::Align::Start);
        action_bar.set_margin_top(5);
        action_bar.set_margin_end(6);

        let lang_pill = gtk::Label::new(None);
        lang_pill.add_css_class("viewer-code-lang-pill");
        lang_pill.add_css_class("caption");
        lang_pill.add_css_class("dim-label");
        lang_pill.set_valign(gtk::Align::Center);
        lang_pill.set_can_target(false);
        lang_pill.set_visible(false);
        action_bar.append(&lang_pill);

        let copy_btn = gtk::Button::builder()
            .icon_name("boxxy-edit-copy-symbolic")
            .css_classes(["flat", "circular", "viewer-code-copy-btn"])
            .tooltip_text("Copy to clipboard")
            .valign(gtk::Align::Center)
            .focus_on_click(false)
            .build();

        let obj_weak = obj.downgrade();
        copy_btn.connect_clicked(move |btn| {
            let Some(widget) = obj_weak.upgrade() else {
                return;
            };
            if let Some(display) = gdk::Display::default() {
                display
                    .clipboard()
                    .set_text(&widget.imp().raw_code.borrow());
                btn.set_icon_name("boxxy-object-select-symbolic");
                btn.add_css_class("copied");
                let b_weak = btn.downgrade();
                glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                    if let Some(b) = b_weak.upgrade() {
                        b.set_icon_name("boxxy-edit-copy-symbolic");
                        b.remove_css_class("copied");
                    }
                });
            }
        });
        action_bar.append(&copy_btn);
        overlay.add_overlay(&action_bar);

        frame.set_child(Some(&overlay));
        frame.set_parent(&obj);

        let imp = obj.imp();
        *imp.frame.borrow_mut() = Some(frame);
        *imp.scroll.borrow_mut() = Some(scroll);
        *imp.chunks_box.borrow_mut() = Some(chunks_box);
        *imp.lang_pill.borrow_mut() = Some(lang_pill);
        *imp.copy_btn.borrow_mut() = Some(copy_btn);
        imp.chunk_labels.borrow_mut().push(first_label);

        obj
    }

    /// Resets transient scroll, selection, and streaming state so the widget can be parked in
    /// `StructuredViewer`'s recycling pool and rebound to another message in 0 allocations.
    pub fn reset_for_pool(&self) {
        let imp = self.imp();
        imp.local_serial
            .set(imp.local_serial.get().wrapping_add(1));
        *imp.stream_lexer.borrow_mut() = None;
        imp.applied_chunks.borrow_mut().clear();
        imp.raw_code.borrow_mut().clear();
        imp.lang.borrow_mut().clear();
        self.set_margin_bottom(12);
        self.remove_css_class("streaming-active");

        if let Some(ref scroll) = *imp.scroll.borrow() {
            scroll.hadjustment().set_value(0.0);
        }
        if let Some(ref btn) = *imp.copy_btn.borrow() {
            btn.set_icon_name("boxxy-edit-copy-symbolic");
            btn.remove_css_class("copied");
        }

        if let Some(ref cbox) = *imp.chunks_box.borrow() {
            let mut labels = imp.chunk_labels.borrow_mut();
            while labels.len() > MAX_POOLED_CHUNK_LABELS {
                if let Some(extra) = labels.pop() {
                    cbox.remove(&extra);
                }
            }
            for (i, lbl) in labels.iter().enumerate() {
                lbl.select_region(0, 0);
                lbl.set_attributes(None);
                lbl.set_visible(i == 0);
            }
        }
    }

    /// Returns the number of currently visible `GtkLabel` line chunks (useful for tests & diagnostics).
    #[must_use]
    pub fn active_chunk_count(&self) -> usize {
        self.imp()
            .chunk_labels
            .borrow()
            .iter()
            .filter(|l| l.is_visible())
            .count()
    }

    /// Updates this code block widget in-place for `lang` and `raw_code`.
    pub fn update_code(&self, lang: &str, raw_code: &str, ctx: &RenderContext) {
        let imp = self.imp();
        let next_serial = imp.local_serial.get().wrapping_add(1);
        imp.local_serial.set(next_serial);

        let norm_lang = normalize_fence_language(lang);
        let is_shell = is_shell_language(&norm_lang);
        let is_dark = is_card_surface_dark();
        imp.last_card_dark.set(Some(is_dark));

        {
            let mut stored_lang = imp.lang.borrow_mut();
            if *stored_lang != norm_lang {
                stored_lang.clear();
                stored_lang.push_str(&norm_lang);
            }
        }
        {
            let mut stored_code = imp.raw_code.borrow_mut();
            if *stored_code != raw_code {
                stored_code.clear();
                stored_code.push_str(raw_code);
            }
        }

        if let Some(ref frame) = *imp.frame.borrow() {
            if is_shell {
                frame.add_css_class("viewer-code-shell");
            } else {
                frame.remove_css_class("viewer-code-shell");
            }
        }

        if let Some(ref pill) = *imp.lang_pill.borrow() {
            if norm_lang.is_empty() {
                pill.set_visible(false);
            } else {
                pill.set_text(&norm_lang);
                pill.set_visible(true);
            }
        }

        let display_code = strip_trailing_fence_newline(raw_code);

        // 1. Streaming path: stateful incremental line-by-line lexer (never pollutes HIGHLIGHT_CACHE)
        if ctx.is_streaming {
            let mut slot = imp.stream_lexer.borrow_mut();
            let needs_new = slot
                .as_ref()
                .is_none_or(|lex| !lex.matches(&norm_lang, display_code, is_dark));
            if needs_new {
                *slot = Some(StreamCodeLexer::new(&norm_lang, display_code, is_dark));
            }
            let lexer = slot.as_mut().unwrap();
            let unstable_spans = lexer.advance_and_lex_tail(display_code);

            let mut combined_spans =
                Vec::with_capacity(lexer.stable_spans.len() + unstable_spans.len());
            combined_spans.extend_from_slice(&lexer.stable_spans);
            combined_spans.extend_from_slice(&unstable_spans);

            let offsets = lexer.stable_newline_offsets.clone();
            drop(slot);
            self.apply_chunks(display_code, &combined_spans, &offsets);
            return;
        }

        // 2. Settled transition from an active stream: finalize the existing stream lexer in O(1)
        if let Some(lexer) = imp.stream_lexer.borrow_mut().take()
            && lexer.matches(&norm_lang, display_code, is_dark)
        {
            let highlighted = lexer.finalize(display_code);
            insert_cached_highlight(&norm_lang, display_code, is_dark, highlighted.clone());
            self.apply_chunks(
                display_code,
                &highlighted.spans,
                &highlighted.newline_offsets,
            );
            return;
        }

        // 3. Shell blocks & empty blocks skip syntax tokenization completely
        if display_code.is_empty() || is_shell {
            let offsets = scan_newline_offsets(display_code);
            self.apply_chunks(display_code, &[], &offsets);
            return;
        }

        // 4. Synchronous LRU cache hit (0 ms on GtkListView scroll-back!)
        if let Some(cached) = get_cached_highlight(&norm_lang, display_code, is_dark) {
            self.apply_chunks(display_code, &cached.spans, &cached.newline_offsets);
            return;
        }

        // 5. Small/medium code blocks: highlight synchronously in <0.2ms so there is zero plain-to-color flash
        let newline_offsets = scan_newline_offsets(display_code);
        if display_code.len() <= SYNC_HIGHLIGHT_MAX_BYTES
            && newline_offsets.len() < SYNC_HIGHLIGHT_MAX_LINES
        {
            let highlighted = compute_highlighted_code(&norm_lang, display_code, is_dark);
            insert_cached_highlight(&norm_lang, display_code, is_dark, highlighted.clone());
            self.apply_chunks(
                display_code,
                &highlighted.spans,
                &highlighted.newline_offsets,
            );
            return;
        }

        // 6. Huge code blocks (> 8 KiB or >= 80 lines) on cache miss:
        //    Render plain chunked text immediately, then offload `syntect` to `spawn_blocking`
        //    guarded by BOTH the parent `StructuredViewer` generation AND `local_serial`.
        self.apply_chunks(display_code, &[], &newline_offsets);

        let gen_handle = ctx.generation.clone();
        let start_gen = ctx.current_generation();
        let lang_owned = norm_lang;
        let display_owned = display_code.to_string();
        let widget_weak = self.downgrade();

        let (tx, rx) = async_channel::bounded::<Arc<HighlightedCode>>(1);
        let gen_for_worker = gen_handle.clone();
        boxxy_sys_utils::runtime().spawn_blocking(move || {
            if gen_for_worker.load(Ordering::Relaxed) != start_gen {
                return;
            }
            let highlighted = compute_highlighted_code(&lang_owned, &display_owned, is_dark);
            insert_cached_highlight(&lang_owned, &display_owned, is_dark, highlighted.clone());
            let _ = tx.send_blocking(highlighted);
        });

        glib::spawn_future_local(async move {
            if let Ok(highlighted) = rx.recv().await
                && gen_handle.load(Ordering::Relaxed) == start_gen
                && let Some(widget) = widget_weak.upgrade()
                && widget.imp().local_serial.get() == next_serial
            {
                let raw_borrow = widget.imp().raw_code.borrow().clone();
                let current_display = strip_trailing_fence_newline(&raw_borrow);
                widget.apply_chunks(
                    current_display,
                    &highlighted.spans,
                    &highlighted.newline_offsets,
                );
            }
        });
    }

    fn refresh_for_card_darkness(&self, is_dark: bool) {
        let imp = self.imp();
        let lang = imp.lang.borrow().clone();
        let raw_code = imp.raw_code.borrow().clone();
        let display_code = strip_trailing_fence_newline(&raw_code);
        if display_code.is_empty() || is_shell_language(&lang) {
            return;
        }

        if let Some(cached) = get_cached_highlight(&lang, display_code, is_dark) {
            self.apply_chunks(display_code, &cached.spans, &cached.newline_offsets);
            return;
        }

        let next_serial = imp.local_serial.get().wrapping_add(1);
        imp.local_serial.set(next_serial);

        if display_code.len() <= SYNC_HIGHLIGHT_MAX_BYTES {
            let highlighted = compute_highlighted_code(&lang, display_code, is_dark);
            insert_cached_highlight(&lang, display_code, is_dark, highlighted.clone());
            self.apply_chunks(
                display_code,
                &highlighted.spans,
                &highlighted.newline_offsets,
            );
            return;
        }

        let display_owned = display_code.to_string();
        let widget_weak = self.downgrade();
        let (tx, rx) = async_channel::bounded::<Arc<HighlightedCode>>(1);
        boxxy_sys_utils::runtime().spawn_blocking(move || {
            let highlighted = compute_highlighted_code(&lang, &display_owned, is_dark);
            insert_cached_highlight(&lang, &display_owned, is_dark, highlighted.clone());
            let _ = tx.send_blocking(highlighted);
        });

        glib::spawn_future_local(async move {
            if let Ok(highlighted) = rx.recv().await
                && let Some(widget) = widget_weak.upgrade()
                && widget.imp().local_serial.get() == next_serial
            {
                let raw_borrow = widget.imp().raw_code.borrow().clone();
                let current_display = strip_trailing_fence_newline(&raw_borrow);
                widget.apply_chunks(
                    current_display,
                    &highlighted.spans,
                    &highlighted.newline_offsets,
                );
            }
        });
    }

    fn apply_chunks(
        &self,
        display_code: &str,
        spans: &[SyntaxSpan],
        newline_offsets: &[u32],
    ) {
        let imp = self.imp();
        let Some(cbox) = imp.chunks_box.borrow().clone() else {
            return;
        };

        let ranges = compute_chunk_ranges(display_code.len(), newline_offsets);
        let needed = ranges.len();

        let mut labels = imp.chunk_labels.borrow_mut();
        while labels.len() < needed {
            let lbl = make_chunk_label();
            cbox.append(&lbl);
            labels.push(lbl);
        }

        let mut stamps = imp.applied_chunks.borrow_mut();
        stamps.truncate(needed);

        for (idx, &(chunk_start, chunk_end)) in ranges.iter().enumerate() {
            let chunk_str = &display_code[chunk_start..chunk_end];
            let first_span = spans.partition_point(|s| (s.end_byte as usize) <= chunk_start);
            let end_span =
                first_span + spans[first_span..].partition_point(|s| (s.start_byte as usize) < chunk_end);
            let chunk_spans = &spans[first_span..end_span];

            let (text_hash, spans_hash) =
                hash_bytes_and_spans(chunk_str, chunk_spans, chunk_start, chunk_end);
            let next_stamp = ChunkStamp {
                start_byte: chunk_start,
                end_byte: chunk_end,
                text_hash,
                spans_hash,
            };

            let lbl = &labels[idx];
            lbl.set_visible(true);

            if stamps.get(idx) == Some(&next_stamp) {
                continue;
            }

            if lbl.text().as_str() != chunk_str {
                lbl.set_text(chunk_str);
            }
            let attrs = build_chunk_attr_list(chunk_spans, chunk_start, chunk_end);
            lbl.set_attributes(attrs.as_ref());

            if idx < stamps.len() {
                stamps[idx] = next_stamp;
            } else {
                stamps.push(next_stamp);
            }
        }

        for lbl in labels.iter().skip(needed) {
            lbl.set_visible(false);
        }
    }
}

pub struct CodeRenderer;

impl BlockRenderer for CodeRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Code { .. })
    }

    fn render(
        &self,
        block: &ContentBlock,
        registry: &crate::registry::ViewerRegistry,
    ) -> gtk::Widget {
        self.render_with_context(block, registry, &RenderContext::default())
    }

    fn render_with_context(
        &self,
        block: &ContentBlock,
        _registry: &crate::registry::ViewerRegistry,
        ctx: &RenderContext,
    ) -> gtk::Widget {
        let ContentBlock::Code { lang, code } = block else {
            unreachable!()
        };
        let widget = CodeBlockWidget::new();
        widget.update_code(lang, code, ctx);
        widget.upcast()
    }

    fn update(
        &self,
        block: &ContentBlock,
        widget: &gtk::Widget,
        _registry: &crate::registry::ViewerRegistry,
        ctx: &RenderContext,
    ) -> bool {
        let ContentBlock::Code { lang, code } = block else {
            return false;
        };
        let Some(code_widget) = widget.downcast_ref::<CodeBlockWidget>() else {
            return false;
        };
        code_widget.set_margin_bottom(12);
        code_widget.update_code(lang, code, ctx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_trailing_fence_newline_removes_single_trailing_newline_only() {
        assert_eq!(strip_trailing_fence_newline("fn main() {}\n"), "fn main() {}");
        assert_eq!(
            strip_trailing_fence_newline("fn main() {}\r\n"),
            "fn main() {}"
        );
        assert_eq!(
            strip_trailing_fence_newline("fn main() {}\n\n"),
            "fn main() {}\n"
        );
    }

    #[test]
    fn compute_highlighted_code_coalesces_spans_and_skips_shell() {
        let rust_src = "pub struct Widget {\n    pub count: usize,\n}";
        let hl = compute_highlighted_code("rust", rust_src, true);
        assert!(!hl.spans.is_empty(), "Rust keywords should produce color spans");
        assert_eq!(hl.newline_offsets.len(), 2);

        // Shell snippets follow OpenCode's monochrome rule (0 syntax spans)
        let sh_src = "cargo test --workspace && echo ok";
        let sh_hl = compute_highlighted_code("bash", sh_src, true);
        assert!(sh_hl.spans.is_empty());
    }

    #[test]
    fn long_minified_line_bypasses_syntect_without_stalling() {
        let minified = format!("let x = \"{}\";\nlet y = 42;", "a".repeat(4096));
        let hl = compute_highlighted_code("js", &minified, true);
        assert_eq!(hl.newline_offsets.len(), 1);
        // Line 2 (`let y = 42;`) is still highlighted normally after skipping oversized line 1
        assert!(!hl.spans.is_empty());
    }

    #[test]
    fn chunk_ranges_activate_above_200_line_threshold() {
        let mut small = String::new();
        for i in 0..200 {
            if i > 0 {
                small.push('\n');
            }
            small.push_str("let v = 1;");
        }
        let small_nl = scan_newline_offsets(&small);
        assert_eq!(compute_chunk_ranges(small.len(), &small_nl).len(), 1);

        let mut large = String::new();
        for i in 0..350 {
            if i > 0 {
                large.push('\n');
            }
            large.push_str(&format!("let line_{i} = {i};"));
        }
        let large_nl = scan_newline_offsets(&large);
        let ranges = compute_chunk_ranges(large.len(), &large_nl);
        assert_eq!(ranges.len(), 4); // 100 + 100 + 100 + 50 lines
        let reconstructed: Vec<&str> = ranges.iter().map(|&(s, e)| &large[s..e]).collect();
        assert_eq!(reconstructed.join("\n"), large);
    }

    #[test]
    fn stream_code_lexer_matches_batch_highlight_on_completion() {
        let full = "fn compute(n: u32) -> u32 {\n    let doubled = n * 2;\n    doubled + 1\n}";
        let mut lexer = StreamCodeLexer::new("rust", "", true);

        for step in [12, 28, 45, 60, full.len()] {
            let _ = lexer.advance_and_lex_tail(&full[..step]);
        }
        let streamed = lexer.finalize(full);
        let batch = compute_highlighted_code("rust", full, true);
        assert_eq!(streamed, batch);
    }

    #[test]
    fn gtk_code_block_widget_updates_in_place_and_chunks_large_files() {
        crate::run_gtk_test(|| {
            let widget = CodeBlockWidget::new();
            let stream_ctx =
                RenderContext::new(Arc::new(std::sync::atomic::AtomicU64::new(0)), true);
            widget.update_code("rust", "fn demo() {\n", &stream_ctx);
            assert_eq!(widget.active_chunk_count(), 1);

            let mut huge = String::new();
            for i in 0..260 {
                huge.push_str(&format!("let item_{i} = {i};\n"));
            }
            let settled_ctx = RenderContext::default();
            widget.update_code("rust", &huge, &settled_ctx);
            assert_eq!(widget.active_chunk_count(), 3);

            widget.reset_for_pool();
            widget.update_code("bash", "ls -la\n", &settled_ctx);
            assert_eq!(widget.active_chunk_count(), 1);
        });
    }
}
