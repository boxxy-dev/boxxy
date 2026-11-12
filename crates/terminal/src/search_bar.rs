use boxxy_vte::terminal::TerminalWidget;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const PCRE2_CASELESS: u32 = 0x00000008;
const PCRE2_MULTILINE: u32 = 0x00000400;

fn escape_regex(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if "\\.*+?()|[]{}^$".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn apply_search_query(
    term: &glib::WeakRef<TerminalWidget>,
    entry: &gtk::SearchEntry,
    regex: bool,
    words: bool,
    case: bool,
    force_on_alt_screen: bool,
) {
    if let Some(t) = term.upgrade() {
        let text = entry.text();
        if text.is_empty() {
            t.search_set_regex(None, 0);
            return;
        }

        // On active TUI / alternate-screen views (`top`, `btop`, `vim`), wait for
        // at least 2 characters while passively typing so single-letter prefixes
        // don't highlight hundreds of cells across 120 Hz redraws. Explicit
        // navigation (`Enter`, `Shift+Enter`, `^` / `v`) bypasses this via `force_on_alt_screen`.
        if !force_on_alt_screen && t.is_alt_screen() && text.chars().nth(1).is_none() {
            t.search_set_regex(None, 0);
            return;
        }

        t.search_set_wrap_around(true);

        if !regex && !words {
            let escaped = escape_regex(text.as_str());
            t.search_set_literal(text.as_str(), &escaped, case);
            return;
        }

        let mut flags = PCRE2_MULTILINE;
        if !case {
            flags |= PCRE2_CASELESS;
        }

        let mut query = text.to_string();
        if !regex {
            query = escape_regex(&query);
        }
        if words {
            query = format!("\\b{query}\\b");
        }

        t.search_set_regex(Some(&query), flags);
    }
}

fn dismiss_search_bar(revealer: &gtk::Revealer, terminal: &glib::WeakRef<TerminalWidget>) {
    revealer.set_can_target(false);
    revealer.set_reveal_child(false);
    if let Some(term) = terminal.upgrade() {
        term.search_set_regex(None, 0);
        term.grab_focus();
    }
}

pub struct SearchBarComponent {
    revealer: gtk::Revealer,
    entry: gtk::SearchEntry,
    terminal: Rc<glib::WeakRef<TerminalWidget>>,
    use_regex: Rc<Cell<bool>>,
    whole_words: Rc<Cell<bool>>,
    match_case: Rc<Cell<bool>>,
}

impl Default for SearchBarComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchBarComponent {
    pub fn new() -> Self {
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        revealer.set_halign(gtk::Align::Center);
        revealer.set_valign(gtk::Align::End);
        revealer.set_margin_bottom(12);
        revealer.set_can_target(false);

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        container.add_css_class("toolbar");
        container.add_css_class("osd");
        container.add_css_class("search-bar-osd");
        container.set_margin_start(12);
        container.set_margin_end(12);

        let entry = gtk::SearchEntry::new();
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Search History"));

        let up_btn = gtk::Button::from_icon_name("boxxy-up-symbolic");
        let down_btn = gtk::Button::from_icon_name("boxxy-down-symbolic");

        // ── Settings button (plain Button, not MenuButton) ──────────────────
        let settings_btn = gtk::Button::new();
        settings_btn.set_icon_name("boxxy-settings-symbolic");

        // Build the popover and parent it directly to the button widget.
        // Parenting here (not via MenuButton::set_popover) gives us full
        // control over placement; GTK then measures available space relative
        // to the button and respects PositionType::Top reliably.
        let popover = gtk::Popover::new();
        popover.set_position(gtk::PositionType::Top);
        popover.set_has_arrow(false);
        popover.set_autohide(true);
        popover.add_css_class("search-popover");
        popover.set_parent(&settings_btn);
        let popover_for_destroy = popover.clone();
        settings_btn.connect_destroy(move |_| {
            popover_for_destroy.unparent();
        });

        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        popover_box.set_margin_top(8);
        popover_box.set_margin_bottom(8);
        popover_box.set_margin_start(8);
        popover_box.set_margin_end(8);

        let match_case_chk = gtk::CheckButton::builder().label("Match Case").build();
        let whole_words_chk = gtk::CheckButton::builder().label("Whole Words").build();
        let use_regex_chk = gtk::CheckButton::builder()
            .label("Use Regular Expressions")
            .build();

        popover_box.append(&match_case_chk);
        popover_box.append(&whole_words_chk);
        popover_box.append(&use_regex_chk);
        popover.set_child(Some(&popover_box));

        let popover_toggle = popover;
        settings_btn.connect_clicked(move |_| {
            if popover_toggle.is_visible() {
                popover_toggle.popdown();
            } else {
                popover_toggle.popup();
            }
        });

        let close_btn = gtk::Button::from_icon_name("boxxy-cross-small-symbolic");

        container.append(&entry);
        container.append(&up_btn);
        container.append(&down_btn);
        container.append(&settings_btn);
        container.append(&close_btn);

        revealer.set_child(Some(&container));

        let component = Self {
            revealer,
            entry,
            terminal: Rc::new(glib::WeakRef::new()),
            use_regex: Rc::new(Cell::new(false)),
            whole_words: Rc::new(Cell::new(false)),
            match_case: Rc::new(Cell::new(false)),
        };

        component.setup_signals(
            &up_btn,
            &down_btn,
            &close_btn,
            &match_case_chk,
            &whole_words_chk,
            &use_regex_chk,
        );

        component
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.revealer
    }

    pub fn set_terminal(&self, terminal: &TerminalWidget) {
        self.terminal.set(Some(terminal));
    }

    pub fn reveal(&self) {
        let mut updated_entry = false;
        if let Some(term) = self.terminal.upgrade()
            && let Some(sel) = term.selected_text()
            && let Some(first_line) = sel.lines().map(str::trim).find(|l| !l.is_empty())
            && self.entry.text().as_str() != first_line
        {
            self.entry.set_text(first_line);
            updated_entry = true;
        }
        if !updated_entry {
            self.apply_current_query();
        }
        self.revealer.set_can_target(true);
        self.revealer.set_reveal_child(true);
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    fn apply_current_query(&self) {
        apply_search_query(
            &self.terminal,
            &self.entry,
            self.use_regex.get(),
            self.whole_words.get(),
            self.match_case.get(),
            false,
        );
    }

    pub fn hide(&self) {
        dismiss_search_bar(&self.revealer, &self.terminal);
    }

    pub fn is_visible(&self) -> bool {
        self.revealer.reveals_child()
    }

    fn setup_signals(
        &self,
        up_btn: &gtk::Button,
        down_btn: &gtk::Button,
        close_btn: &gtk::Button,
        match_case_chk: &gtk::CheckButton,
        whole_words_chk: &gtk::CheckButton,
        use_regex_chk: &gtk::CheckButton,
    ) {
        let navigate = {
            let term = self.terminal.clone();
            let entry = self.entry.clone();
            let regex = self.use_regex.clone();
            let words = self.whole_words.clone();
            let case = self.match_case.clone();
            Rc::new(move |upward: bool| {
                // Flush any un-debounced `SearchEntry` text (and bypass the 1-char
                // alt-screen filter) before stepping through scrollback.
                apply_search_query(&term, &entry, regex.get(), words.get(), case.get(), true);
                if let Some(t) = term.upgrade() {
                    if upward {
                        t.search_find_previous();
                    } else {
                        t.search_find_next();
                    }
                }
            })
        };

        let term_clone1 = self.terminal.clone();
        let regex1 = self.use_regex.clone();
        let words1 = self.whole_words.clone();
        let case1 = self.match_case.clone();
        self.entry.connect_search_changed(move |e| {
            apply_search_query(
                &term_clone1,
                e,
                regex1.get(),
                words1.get(),
                case1.get(),
                false,
            );
        });

        // Enter key – search upward
        let nav_activate = navigate.clone();
        self.entry.connect_activate(move |_| {
            nav_activate(true);
        });

        // SearchEntry next/prev (Ctrl+G / Ctrl+Shift+G, swapped intentionally for terminal bottom-up order)
        let nav_next = navigate.clone();
        self.entry.connect_next_match(move |_| {
            nav_next(true);
        });

        let nav_prev = navigate.clone();
        self.entry.connect_previous_match(move |_| {
            nav_prev(false);
        });

        let match_case_ref_c = self.match_case.clone();
        let term_clone2 = self.terminal.clone();
        let entry_clone2 = self.entry.clone();
        let regex2 = self.use_regex.clone();
        let words2 = self.whole_words.clone();
        match_case_chk.connect_toggled(move |c| {
            match_case_ref_c.set(c.is_active());
            apply_search_query(
                &term_clone2,
                &entry_clone2,
                regex2.get(),
                words2.get(),
                c.is_active(),
                false,
            );
        });

        let whole_words_ref_c = self.whole_words.clone();
        let term_clone3 = self.terminal.clone();
        let entry_clone3 = self.entry.clone();
        let regex3 = self.use_regex.clone();
        let case3 = self.match_case.clone();
        whole_words_chk.connect_toggled(move |c| {
            whole_words_ref_c.set(c.is_active());
            apply_search_query(
                &term_clone3,
                &entry_clone3,
                regex3.get(),
                c.is_active(),
                case3.get(),
                false,
            );
        });

        let use_regex_ref_c = self.use_regex.clone();
        let term_clone4 = self.terminal.clone();
        let entry_clone4 = self.entry.clone();
        let words4 = self.whole_words.clone();
        let case4 = self.match_case.clone();
        use_regex_chk.connect_toggled(move |c| {
            use_regex_ref_c.set(c.is_active());
            apply_search_query(
                &term_clone4,
                &entry_clone4,
                c.is_active(),
                words4.get(),
                case4.get(),
                false,
            );
        });

        let nav_down = navigate.clone();
        down_btn.connect_clicked(move |_| {
            nav_down(false);
        });

        let nav_up = navigate.clone();
        up_btn.connect_clicked(move |_| {
            nav_up(true);
        });

        let rev_clone = self.revealer.downgrade();
        let term_focus = self.terminal.clone();
        close_btn.connect_clicked(move |_| {
            if let Some(rev) = rev_clone.upgrade() {
                dismiss_search_bar(&rev, &term_focus);
            }
        });

        let rev_clone2 = self.revealer.downgrade();
        let term_focus2 = self.terminal.clone();
        let nav_shift_enter = navigate;
        let ev_ctrl = gtk::EventControllerKey::new();
        ev_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        ev_ctrl.connect_key_pressed(move |_, keyval, _, state| {
            if keyval == gtk::gdk::Key::Escape {
                if let Some(rev) = rev_clone2.upgrade() {
                    dismiss_search_bar(&rev, &term_focus2);
                }
                gtk::glib::Propagation::Stop
            } else if (keyval == gtk::gdk::Key::Return || keyval == gtk::gdk::Key::KP_Enter)
                && state.contains(gtk::gdk::ModifierType::SHIFT_MASK)
            {
                nav_shift_enter(false);
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        self.entry.add_controller(ev_ctrl);
    }
}

#[cfg(test)]
mod tests {
    use super::escape_regex;

    #[test]
    fn escape_regex_escapes_pcre_metacharacters() {
        assert_eq!(escape_regex("foo.bar[0]+(a|b)"), r"foo\.bar\[0\]\+\(a\|b\)");
        assert_eq!(escape_regex("plain_word"), "plain_word");
    }
}
