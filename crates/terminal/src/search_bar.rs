use gtk4::prelude::*;
use gtk4 as gtk;
use std::rc::Rc;
use std::cell::RefCell;
use boxxy_vte::terminal::TerminalWidget;

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

pub struct SearchBarComponent {
    revealer: gtk::Revealer,
    entry: gtk::SearchEntry,
    terminal: Rc<RefCell<Option<TerminalWidget>>>,
    use_regex: Rc<RefCell<bool>>,
    whole_words: Rc<RefCell<bool>>,
    match_case: Rc<RefCell<bool>>,
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

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        container.add_css_class("toolbar");
        container.add_css_class("osd");
        container.add_css_class("search-bar-osd");
        container.set_margin_start(12);
        container.set_margin_end(12);

        let entry = gtk::SearchEntry::new();
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Search History"));

        let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
        let down_btn = gtk::Button::from_icon_name("go-down-symbolic");

        // ── Settings button (plain Button, not MenuButton) ──────────────────
        let settings_btn = gtk::Button::new();
        settings_btn.set_icon_name("emblem-system-symbolic");

        // Build the popover and parent it directly to the button widget.
        // Parenting here (not via MenuButton::set_popover) gives us full
        // control over placement; GTK then measures available space relative
        // to the button and respects PositionType::Top reliably.
        let popover = gtk::Popover::new();
        popover.set_position(gtk::PositionType::Top);
        popover.set_has_arrow(false);
        popover.set_autohide(true);
        popover.add_css_class("search-popover");
        // Parent AFTER the button exists but BEFORE it is realised – that is
        // fine because set_parent only stores the reference.
        popover.set_parent(&settings_btn);

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

        // Toggle popover on button click
        let popover_toggle = popover.clone();
        settings_btn.connect_clicked(move |_| {
            if popover_toggle.is_visible() {
                popover_toggle.popdown();
            } else {
                popover_toggle.popup();
            }
        });

        let close_btn = gtk::Button::from_icon_name("window-close-symbolic");

        container.append(&entry);
        container.append(&up_btn);
        container.append(&down_btn);
        container.append(&settings_btn);
        container.append(&close_btn);

        revealer.set_child(Some(&container));

        let use_regex = Rc::new(RefCell::new(false));
        let whole_words = Rc::new(RefCell::new(false));
        let match_case = Rc::new(RefCell::new(false));

        let terminal = Rc::new(RefCell::new(None));

        let component = Self {
            revealer,
            entry,
            terminal,
            use_regex,
            whole_words,
            match_case,
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

    pub fn set_terminal(&self, terminal: TerminalWidget) {
        *self.terminal.borrow_mut() = Some(terminal);
    }

    pub fn reveal(&self) {
        self.revealer.set_reveal_child(true);
        self.entry.grab_focus();
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
        if let Some(term) = self.terminal.borrow().as_ref() {
            term.grab_focus();
        }
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
        let update_search = Rc::new(
            move |term: &Rc<RefCell<Option<TerminalWidget>>>,
                  entry: &gtk::SearchEntry,
                  regex: bool,
                  words: bool,
                  case: bool| {
                if let Some(t) = term.borrow().as_ref() {
                    let text = entry.text();
                    if text.is_empty() {
                        t.search_set_regex(None, 0);
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
                        query = format!("\\b{}\\b", query);
                    }

                    t.search_set_regex(Some(&query), flags);
                    t.search_set_wrap_around(true);
                }
            },
        );

        // Entry changed
        let update_clone = update_search.clone();
        let term_clone1 = self.terminal.clone();
        let regex1 = self.use_regex.clone();
        let words1 = self.whole_words.clone();
        let case1 = self.match_case.clone();
        self.entry.connect_search_changed(move |e| {
            update_clone(
                &term_clone1,
                e,
                *regex1.borrow(),
                *words1.borrow(),
                *case1.borrow(),
            );
        });

        // Enter key – search upward
        let term_activate = self.terminal.clone();
        self.entry.connect_activate(move |_| {
            if let Some(t) = term_activate.borrow().as_ref() {
                t.search_find_previous();
            }
        });

        // SearchEntry next/prev (swapped intentionally)
        let term_next_match = self.terminal.clone();
        self.entry.connect_next_match(move |_| {
            if let Some(t) = term_next_match.borrow().as_ref() {
                t.search_find_previous();
            }
        });

        let term_prev_match = self.terminal.clone();
        self.entry.connect_previous_match(move |_| {
            if let Some(t) = term_prev_match.borrow().as_ref() {
                t.search_find_next();
            }
        });

        // Match Case toggle
        let match_case_ref_c = self.match_case.clone();
        let update_clone2 = update_search.clone();
        let term_clone2 = self.terminal.clone();
        let entry_clone2 = self.entry.clone();
        let regex2 = self.use_regex.clone();
        let words2 = self.whole_words.clone();
        match_case_chk.connect_toggled(move |c| {
            *match_case_ref_c.borrow_mut() = c.is_active();
            update_clone2(
                &term_clone2,
                &entry_clone2,
                *regex2.borrow(),
                *words2.borrow(),
                c.is_active(),
            );
        });

        // Whole Words toggle
        let whole_words_ref_c = self.whole_words.clone();
        let update_clone3 = update_search.clone();
        let term_clone3 = self.terminal.clone();
        let entry_clone3 = self.entry.clone();
        let regex3 = self.use_regex.clone();
        let case3 = self.match_case.clone();
        whole_words_chk.connect_toggled(move |c| {
            *whole_words_ref_c.borrow_mut() = c.is_active();
            update_clone3(
                &term_clone3,
                &entry_clone3,
                *regex3.borrow(),
                c.is_active(),
                *case3.borrow(),
            );
        });

        // Use Regex toggle
        let use_regex_ref_c = self.use_regex.clone();
        let update_clone4 = update_search.clone();
        let term_clone4 = self.terminal.clone();
        let entry_clone4 = self.entry.clone();
        let words4 = self.whole_words.clone();
        let case4 = self.match_case.clone();
        use_regex_chk.connect_toggled(move |c| {
            *use_regex_ref_c.borrow_mut() = c.is_active();
            update_clone4(
                &term_clone4,
                &entry_clone4,
                c.is_active(),
                *words4.borrow(),
                *case4.borrow(),
            );
        });

        // Down / Up buttons
        let term_down = self.terminal.clone();
        down_btn.connect_clicked(move |_| {
            if let Some(t) = term_down.borrow().as_ref() {
                t.search_find_next();
            }
        });

        let term_up = self.terminal.clone();
        up_btn.connect_clicked(move |_| {
            if let Some(t) = term_up.borrow().as_ref() {
                t.search_find_previous();
            }
        });

        // Close button
        let rev_clone = self.revealer.clone();
        let term_focus = self.terminal.clone();
        close_btn.connect_clicked(move |_| {
            rev_clone.set_reveal_child(false);
            if let Some(t) = term_focus.borrow().as_ref() {
                t.grab_focus();
            }
        });

        // Escape key closes the bar
        let rev_clone2 = self.revealer.clone();
        let term_focus2 = self.terminal.clone();
        let ev_ctrl = gtk::EventControllerKey::new();
        ev_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                rev_clone2.set_reveal_child(false);
                if let Some(t) = term_focus2.borrow().as_ref() {
                    t.grab_focus();
                }
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        self.entry.add_controller(ev_ctrl);
    }
}