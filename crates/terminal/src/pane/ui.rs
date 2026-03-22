use crate::search_bar::SearchBarComponent;
use boxxy_vte::terminal::TerminalWidget;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::rc::Rc;

pub(super) fn build_ui() -> (
    gtk::Overlay,
    TerminalWidget,
    gtk::ScrolledWindow,
    gtk::Revealer,
    gtk::Label,
    Rc<SearchBarComponent>,
    gtk::ProgressBar,
) {
    let widget = gtk::Overlay::new();
    widget.add_css_class("terminal-pane");
    widget.set_hexpand(true);
    widget.set_vexpand(true);

    let scrolled_window = gtk::ScrolledWindow::new();
    scrolled_window.set_hexpand(true);
    scrolled_window.set_vexpand(true);
    scrolled_window.set_hscrollbar_policy(gtk::PolicyType::Never);
    scrolled_window.set_vscrollbar_policy(gtk::PolicyType::Always);

    let terminal = TerminalWidget::new();
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);
    terminal.set_cursor_blink_mode(true);
    terminal.set_mouse_autohide(true);
    terminal.set_scroll_on_output(true);
    terminal.set_scroll_on_keystroke(true);
    terminal.set_enable_sixel(true);

    scrolled_window.set_child(Some(&terminal));
    widget.set_child(Some(&scrolled_window));

    terminal.set_vadjustment(Some(&scrolled_window.vadjustment()));
    let size_label = gtk::Label::new(None);
    size_label.add_css_class("terminal-size-osd");

    let size_revealer = gtk::Revealer::new();
    size_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    size_revealer.set_child(Some(&size_label));
    size_revealer.set_halign(gtk::Align::Center);
    size_revealer.set_valign(gtk::Align::Center);
    widget.add_overlay(&size_revealer);

    let search_bar = SearchBarComponent::new();
    search_bar.set_terminal(terminal.clone());
    widget.add_overlay(search_bar.widget());

    let search_bar_rc = Rc::new(search_bar);

    let progress_bar = gtk::ProgressBar::new();
    progress_bar.set_valign(gtk::Align::Start);
    progress_bar.set_halign(gtk::Align::Fill);
    progress_bar.add_css_class("terminal-progress-bar");
    progress_bar.set_margin_start(0);
    progress_bar.set_margin_end(0);
    progress_bar.set_margin_top(0);
    progress_bar.set_visible(false);

    // We add it to the overlay so it sits on top of the terminal
    widget.add_overlay(&progress_bar);

    (
        widget,
        terminal,
        scrolled_window,
        size_revealer,
        size_label,
        search_bar_rc,
        progress_bar,
    )
}
