use gtk4 as gtk;
use gtk::prelude::*;
use gtk::pango;
use sourceview5::prelude::*;
use crate::types::{ChatMessage, Role};
use crate::markdown::{parse_segments, Segment, to_pango, push_escaped};

pub fn build_code_block(lang: &str, code: &str) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.add_css_class("code-block");

    // ── Header: language label + copy button ─────────────────────────────────
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.add_css_class("code-block-header");

    let lang_label = gtk::Label::new(Some(if lang.is_empty() { "code" } else { lang }));
    lang_label.add_css_class("code-block-lang");
    lang_label.set_hexpand(true);
    lang_label.set_halign(gtk::Align::Start);
    lang_label.set_margin_start(10);
    header.append(&lang_label);

    let copy_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy_btn.add_css_class("flat");
    copy_btn.set_tooltip_text(Some("Copy"));
    copy_btn.set_margin_end(4);
    let code_clone = code.to_string();
    copy_btn.connect_clicked(move |btn| {
        btn.display().clipboard().set_text(&code_clone);
    });
    header.append(&copy_btn);
    outer.append(&header);

    // ── Source view ───────────────────────────────────────────────────────────
    let buffer = sourceview5::Buffer::new(None);

    // Syntax highlighting by language
    if !lang.is_empty()
        && let Some(language) = sourceview5::LanguageManager::default().language(lang) {
            buffer.set_language(Some(&language));
        }

    // Pick a style scheme that matches dark/light mode
    let scheme_id = if libadwaita::StyleManager::default().is_dark() {
        "Adwaita-dark"
    } else {
        "Adwaita"
    };
    let sm = sourceview5::StyleSchemeManager::default();
    if let Some(scheme) = sm.scheme(scheme_id).or_else(|| sm.scheme("classic")) {
        buffer.set_style_scheme(Some(&scheme));
    }

    buffer.set_text(code);

    let view = sourceview5::View::with_buffer(&buffer);
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    view.set_left_margin(12);
    view.set_right_margin(12);
    view.add_css_class("code-block-view");

    // Scrolled window — horizontal scroll for long lines, natural height.
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_hscrollbar_policy(gtk::PolicyType::Automatic);
    scroll.set_propagate_natural_width(false);
    scroll.set_child(Some(&view));
    outer.append(&scroll);

    outer
}

pub fn make_label(markup: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_markup(markup);
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label
}

pub fn build_text_bubble(css_class: &str) -> gtk::Box {
    let bubble = gtk::Box::new(gtk::Orientation::Vertical, 4);
    bubble.add_css_class("message-bubble");
    bubble.add_css_class(css_class);
    bubble
}

pub fn build_message_widget(msg: &ChatMessage) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.set_margin_top(4);
    container.set_margin_bottom(4);

    if msg.role == Role::User {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_halign(gtk::Align::End);
        row.set_margin_start(48);
        row.set_margin_end(8);

        let bubble = build_text_bubble("user-message");
        let mut escaped = String::new();
        push_escaped(&mut escaped, &msg.content);
        bubble.append(&make_label(&escaped));

        row.append(&bubble);
        container.append(&row);
    } else {
        for segment in parse_segments(&msg.content) {
            match segment {
                Segment::Text(text) => {
                    let paras: Vec<&str> = text
                        .split("\n\n")
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .collect();
                    if paras.is_empty() {
                        continue;
                    }

                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    row.set_margin_start(8);
                    row.set_margin_end(8);

                    let bubble = build_text_bubble("assistant-message");
                    bubble.set_hexpand(true);
                    for para in paras {
                        bubble.append(&make_label(&to_pango(para)));
                    }

                    row.append(&bubble);
                    container.append(&row);
                }
                Segment::Code { lang, code } => {
                    let card = build_code_block(&lang, &code);
                    card.set_margin_start(8);
                    card.set_margin_end(8);
                    container.append(&card);
                }
            }
        }
    }

    container
}
