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
                | ContentBlock::Rule
                | ContentBlock::Image { .. }
        )
    }

    fn render(&self, block: &ContentBlock, registry: &crate::registry::ViewerRegistry) -> gtk::Widget {
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

                let (css_class, size_tag) = match level {
                    1 => ("title-1", "xx-large"),
                    2 => ("title-2", "x-large"),
                    3 => ("title-3", "large"),
                    4 => ("title-4", "medium"),
                    _ => ("heading", "medium"),
                };

                label.add_css_class(css_class);

                let full_markup = format!("<span size=\"{}\"><b>{}</b></span>", size_tag, markup);
                label.set_markup(&full_markup);
                label.set_margin_top(12);
                label.set_margin_bottom(8);

                label.upcast()
            }
            ContentBlock::Blockquote(markup) => {
                let frame = gtk::Frame::new(None);
                frame.add_css_class("view");
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
                frame.set_margin_start(8);
                frame.set_margin_bottom(8);
                frame.upcast()
            }
            ContentBlock::List { ordered, items } => {
                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
                vbox.set_margin_bottom(8);
                vbox.set_margin_start(16);
                vbox.set_hexpand(true);

                for (i, item) in items.iter().enumerate() {
                    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    hbox.set_hexpand(true);
                    hbox.set_valign(gtk::Align::Start);

                    if let Some(checked) = item.checked {
                        let check = gtk::CheckButton::new();
                        check.set_active(checked);
                        check.set_sensitive(false); // Read-only in the viewer
                        check.set_valign(gtk::Align::Start);
                        hbox.append(&check);
                    } else {
                        let bullet_text = if *ordered {
                            format!("{}.", i + 1)
                        } else {
                            "•".to_string()
                        };

                        let bullet_label = gtk::Label::new(Some(&bullet_text));
                        bullet_label.set_yalign(0.0);
                        bullet_label.add_css_class("dim-label");
                        bullet_label.set_valign(gtk::Align::Start);
                        hbox.append(&bullet_label);
                    }

                    let item_vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    item_vbox.set_hexpand(true);
                    item_vbox.set_halign(gtk::Align::Fill);

                    for block in &item.blocks {
                        if let Some(widget) = registry.render_block(block) {
                            item_vbox.append(&widget);
                        }
                    }

                    hbox.append(&item_vbox);
                    vbox.append(&hbox);
                }

                vbox.upcast()
            }
            ContentBlock::Rule => {
                let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
                separator.set_margin_top(12);
                separator.set_margin_bottom(12);
                separator.upcast()
            }
            ContentBlock::Image { url, title, alt } => {
                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
                vbox.set_margin_bottom(12);

                // For now, we'll just show a link or a placeholder
                let link = gtk::LinkButton::with_label(url, if alt.is_empty() { url } else { alt });
                if !title.is_empty() {
                    link.set_tooltip_text(Some(title));
                }
                vbox.append(&link);
                vbox.upcast()
            }
            _ => unreachable!(),
        }
    }
}
