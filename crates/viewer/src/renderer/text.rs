use crate::parser::blocks::ContentBlock;
use crate::renderer::BlockRenderer;
use gtk4 as gtk;
use gtk4::prelude::*;

pub struct TextRenderer;

impl BlockRenderer for TextRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(
            block,
            ContentBlock::Paragraph(_)
                | ContentBlock::Heading { .. }
                | ContentBlock::Blockquote(_)
                | ContentBlock::List { .. }
        )
    }

    fn render(&self, block: &ContentBlock) -> gtk::Widget {
        match block {
            ContentBlock::Paragraph(markup) => {
                let label = gtk::Label::new(None);
                label.set_use_markup(true);
                label.set_wrap(true);
                label.set_wrap_mode(pango::WrapMode::WordChar);
                label.set_xalign(0.0); // Align left
                label.set_halign(gtk::Align::Fill);
                label.set_hexpand(true);
                label.set_selectable(true);
                label.set_markup(markup);
                label.set_margin_bottom(8); // Add some spacing between paragraphs
                label.upcast()
            }
            ContentBlock::Heading { level, markup } => {
                let label = gtk::Label::new(None);
                label.set_use_markup(true);
                label.set_wrap(true);
                label.set_xalign(0.0);
                label.set_halign(gtk::Align::Fill);
                label.set_hexpand(true);
                label.set_selectable(true);

                // Map header levels to GTK/Libadwaita CSS classes or larger sizes
                let (css_class, size_tag) = match level {
                    1 => ("title-1", "xx-large"),
                    2 => ("title-2", "x-large"),
                    3 => ("title-3", "large"),
                    4 => ("title-4", "medium"),
                    _ => ("heading", "medium"), // Default for 5, 6
                };

                label.add_css_class(css_class);

                // Also wrap in a span just to ensure it's bold and sized properly even without the theme
                let full_markup = format!("<span size=\"{}\"><b>{}</b></span>", size_tag, markup);
                label.set_markup(&full_markup);
                label.set_margin_top(12);
                label.set_margin_bottom(8);

                label.upcast()
            }
            ContentBlock::Blockquote(markup) => {
                let frame = gtk::Frame::new(None);
                frame.add_css_class("view"); // Gives it a background/border in Libadwaita
                frame.set_hexpand(true);

                let label = gtk::Label::new(None);
                label.set_use_markup(true);
                label.set_wrap(true);
                label.set_xalign(0.0);
                label.set_halign(gtk::Align::Fill);
                label.set_hexpand(true);
                label.set_selectable(true);
                label.set_markup(markup);

                label.set_margin_start(12);
                label.set_margin_end(12);
                label.set_margin_top(8);
                label.set_margin_bottom(8);

                frame.set_child(Some(&label));
                frame.set_margin_start(8); // Indent the quote
                frame.set_margin_bottom(8);
                frame.upcast()
            }
            ContentBlock::List { ordered, items } => {
                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
                vbox.set_margin_bottom(8);
                vbox.set_margin_start(16); // Indent the list
                vbox.set_hexpand(true);

                for (i, item_markup) in items.iter().enumerate() {
                    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    hbox.set_hexpand(true);

                    let bullet_text = if *ordered {
                        format!("{}.", i + 1)
                    } else {
                        "•".to_string()
                    };

                    let bullet_label = gtk::Label::new(Some(&bullet_text));
                    bullet_label.set_yalign(0.0); // Align to top of the item line

                    let content_label = gtk::Label::new(None);
                    content_label.set_use_markup(true);
                    content_label.set_wrap(true);
                    content_label.set_xalign(0.0);
                    content_label.set_halign(gtk::Align::Fill);
                    content_label.set_hexpand(true);
                    content_label.set_selectable(true);
                    content_label.set_markup(item_markup);

                    hbox.append(&bullet_label);
                    hbox.append(&content_label);
                    vbox.append(&hbox);
                }

                vbox.upcast()
            }
            _ => unreachable!(), // can_render prevents this
        }
    }
}
