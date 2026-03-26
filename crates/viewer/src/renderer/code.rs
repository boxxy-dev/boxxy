use crate::parser::blocks::ContentBlock;
use crate::parser::markdown::escape_pango;
use crate::renderer::BlockRenderer;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

fn get_syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn get_theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(ThemeSet::load_defaults)
}

pub struct CodeRenderer;

impl BlockRenderer for CodeRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Code { .. })
    }

    fn render(&self, block: &ContentBlock) -> gtk::Widget {
        if let ContentBlock::Code { lang, code } = block {
            let frame = gtk::Frame::new(None);
            frame.add_css_class("view");
            frame.set_margin_bottom(12);

            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);

            let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            header.set_margin_start(8);
            header.set_margin_end(8);
            header.set_margin_top(4);
            header.set_margin_bottom(4);

            let lang_label = gtk::Label::new(Some(if lang.is_empty() { "text" } else { lang }));
            lang_label.add_css_class("dim-label");
            lang_label.add_css_class("caption");
            lang_label.set_hexpand(true);
            lang_label.set_halign(gtk::Align::Start);
            header.append(&lang_label);

            let copy_btn = gtk::Button::builder()
                .icon_name("boxxy-edit-copy-symbolic")
                .css_classes(["flat", "circular"])
                .tooltip_text("Copy to clipboard")
                .valign(gtk::Align::Center)
                .build();

            let code_clone = code.clone();
            copy_btn.connect_clicked(move |btn| {
                if let Some(display) = gtk::gdk::Display::default() {
                    let clipboard = display.clipboard();
                    clipboard.set_text(&code_clone);
                    btn.set_icon_name("boxxy-object-select-symbolic");
                    let b = btn.clone();
                    gtk::glib::timeout_add_local_once(
                        std::time::Duration::from_millis(1500),
                        move || {
                            b.set_icon_name("boxxy-edit-copy-symbolic");
                        },
                    );
                }
            });

            header.append(&copy_btn);
            vbox.append(&header);

            let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
            vbox.append(&separator);

            let label = gtk::Label::new(None);
            label.set_use_markup(true);
            label.set_selectable(true);
            label.set_margin_top(8);
            label.set_margin_bottom(8);
            label.set_margin_start(12);
            label.set_margin_end(12);
            label.set_halign(gtk::Align::Start);
            label.set_xalign(0.0);
            label.add_css_class("monospace");

            // Initial plain text content (escaped)
            label.set_markup(&format!("<tt>{}</tt>", escape_pango(code)));

            // Asynchronous highlighting
            if !code.is_empty() {
                let code_for_highlight = code.clone();
                let lang_for_highlight = lang.clone();
                let is_dark = libadwaita::StyleManager::default().is_dark();

                let (tx, rx) = async_channel::bounded::<String>(1);

                std::thread::spawn(move || {
                    let ss = get_syntax_set();
                    let ts = get_theme_set();

                    let syntax = ss
                        .find_syntax_by_token(&lang_for_highlight)
                        .unwrap_or_else(|| ss.find_syntax_plain_text());

                    let theme = if is_dark {
                        &ts.themes["base16-ocean.dark"]
                    } else {
                        &ts.themes["base16-ocean.light"]
                    };

                    let mut h = HighlightLines::new(syntax, theme);
                    let mut pango_markup = String::with_capacity(code_for_highlight.len() * 2);
                    pango_markup.push_str("<tt>");

                    for line in LinesWithEndings::from(&code_for_highlight) {
                        if let Ok(ranges) = h.highlight_line(line, ss) {
                            for (style, text) in ranges {
                                let color = style.foreground;
                                pango_markup.push_str(&format!(
                                    "<span foreground=\"#{:02x}{:02x}{:02x}\">{}</span>",
                                    color.r,
                                    color.g,
                                    color.b,
                                    escape_pango(text)
                                ));
                            }
                        } else {
                            pango_markup.push_str(&escape_pango(line));
                        }
                    }
                    pango_markup.push_str("</tt>");
                    let _ = tx.send_blocking(pango_markup);
                });

                let label_clone = label.clone();
                gtk::glib::spawn_future_local(async move {
                    if let Ok(markup) = rx.recv().await {
                        label_clone.set_markup(&markup);
                    }
                });
            }

            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_width(true)
                .child(&label)
                .build();

            vbox.append(&scroll);
            frame.set_child(Some(&vbox));

            frame.upcast()
        } else {
            unreachable!()
        }
    }
}
