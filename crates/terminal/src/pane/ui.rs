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
    gtk::DrawingArea,
    gtk::Box,
    gtk::Box,
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
    terminal.set_mouse_autohide(false);
    terminal.set_scroll_on_output(false);
    terminal.set_scroll_on_keystroke(true);

    scrolled_window.set_child(Some(&terminal));
    widget.set_child(Some(&scrolled_window));

    terminal.set_vadjustment(Some(&scrolled_window.vadjustment()));

    let drop_overlay = gtk::DrawingArea::new();
    drop_overlay.set_halign(gtk::Align::Fill);
    drop_overlay.set_valign(gtk::Align::Fill);
    drop_overlay.set_can_target(false);
    drop_overlay.set_can_focus(false);
    drop_overlay.add_css_class("split-drop-overlay");
    widget.add_overlay(&drop_overlay);

    // Full-width translucent lane at the top of the pane. Passive (`can_target: false`)
    // so it never blocks terminal text selection or GtkPaned separator resizing.
    let drag_lane = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    drag_lane.set_halign(gtk::Align::Fill);
    drag_lane.set_valign(gtk::Align::Start);
    drag_lane.set_height_request(20);
    drag_lane.set_can_target(false);
    drag_lane.set_can_focus(false);
    drag_lane.add_css_class("split-drag-lane");
    widget.add_overlay(&drag_lane);

    // Centered three-dots drag handle. Matching its height (20px) and icon pixel_size (20px)
    // to the lane guarantees the dots are 100% vertically centered in the lane.
    let drag_handle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    drag_handle.set_halign(gtk::Align::Center);
    drag_handle.set_valign(gtk::Align::Start);
    drag_handle.set_height_request(20);
    drag_handle.set_can_target(false);
    drag_handle.set_can_focus(false);
    drag_handle.add_css_class("split-drag-handle");
    drag_handle.set_cursor_from_name(Some("grab"));

    let drag_icon = gtk::Image::from_icon_name("boxxy-drag-handle-symbolic");
    drag_icon.set_pixel_size(20);
    drag_icon.set_halign(gtk::Align::Center);
    drag_icon.set_valign(gtk::Align::Center);
    drag_icon.set_hexpand(true);
    drag_handle.append(&drag_icon);
    widget.add_overlay(&drag_handle);

    let size_label = gtk::Label::new(None);
    size_label.add_css_class("terminal-size-osd");
    size_label.set_can_target(false);

    let size_revealer = gtk::Revealer::new();
    size_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    size_revealer.set_child(Some(&size_label));
    size_revealer.set_halign(gtk::Align::Center);
    size_revealer.set_valign(gtk::Align::Center);
    size_revealer.set_can_target(false);
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
    progress_bar.set_can_target(false);
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
        drop_overlay,
        drag_lane,
        drag_handle,
    )
}
