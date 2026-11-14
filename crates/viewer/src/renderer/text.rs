use crate::parser::blocks::ContentBlock;
use crate::renderer::{BlockRenderer, RenderContext};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gio, glib};

const HEADING_CLASSES: [&str; 5] = ["title-1", "title-2", "title-3", "title-4", "heading"];

fn heading_style(level: u8) -> (&'static str, &'static str) {
    match level {
        1 => ("title-1", "160%"),
        2 => ("title-2", "135%"),
        3 => ("title-3", "118%"),
        4 => ("title-4", "106%"),
        _ => ("heading", "100%"),
    }
}

/// Attaches a portal-safe link handler to a `gtk::Label` with `use_markup = true`
/// so `<a href="...">` links open via `UriLauncher` (HTTP/HTTPS) or `FileLauncher` (local files).
pub fn connect_label_links(label: &gtk::Label) {
    label.connect_activate_link(|lbl, uri| {
        let parent_win = lbl.root().and_then(|r| r.downcast::<gtk::Window>().ok());
        if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("mailto:") {
            let launcher = gtk::UriLauncher::new(uri);
            glib::spawn_future_local(async move {
                let _ = launcher.launch_future(parent_win.as_ref()).await;
            });
            glib::Propagation::Stop
        } else if uri.starts_with("file://") || uri.starts_with('/') {
            let file = if uri.starts_with("file://") {
                gio::File::for_uri(uri)
            } else {
                gio::File::for_path(uri)
            };
            let launcher = gtk::FileLauncher::new(Some(&file));
            glib::spawn_future_local(async move {
                let _ = launcher.launch_future(parent_win.as_ref()).await;
            });
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
}

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
        )
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
        registry: &crate::registry::ViewerRegistry,
        ctx: &RenderContext,
    ) -> gtk::Widget {
        match block {
            ContentBlock::Paragraph(markup) => {
                let label = gtk::Label::new(None);
                label.add_css_class("viewer-paragraph");
                label.set_use_markup(true);
                label.set_wrap(true);
                label.set_wrap_mode(pango::WrapMode::WordChar);
                label.set_xalign(0.0);
                label.set_halign(gtk::Align::Fill);
                label.set_hexpand(true);
                label.set_selectable(true);
                label.set_markup(markup);
                label.set_margin_bottom(8);
                connect_label_links(&label);
                label.upcast()
            }
            ContentBlock::Heading { level, markup } => {
                let label = gtk::Label::new(None);
                label.add_css_class("viewer-heading");
                label.set_use_markup(true);
                label.set_wrap(true);
                label.set_xalign(0.0);
                label.set_halign(gtk::Align::Fill);
                label.set_hexpand(true);
                label.set_selectable(true);

                let (css_class, size_tag) = heading_style(*level);
                label.add_css_class(css_class);

                let full_markup = format!("<span size=\"{size_tag}\"><b>{markup}</b></span>");
                label.set_markup(&full_markup);
                label.set_margin_top(12);
                label.set_margin_bottom(8);
                connect_label_links(&label);

                label.upcast()
            }
            ContentBlock::Blockquote(inner_blocks) => {
                let frame = gtk::Frame::new(None);
                frame.add_css_class("view");
                frame.add_css_class("viewer-blockquote");
                frame.set_hexpand(true);

                let inner_vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
                inner_vbox.set_hexpand(true);
                inner_vbox.set_margin_start(12);
                inner_vbox.set_margin_end(12);
                inner_vbox.set_margin_top(8);
                inner_vbox.set_margin_bottom(8);

                populate_blockquote_children(&inner_vbox, inner_blocks, registry, ctx);

                frame.set_child(Some(&inner_vbox));
                frame.set_margin_start(8);
                frame.set_margin_bottom(8);
                frame.upcast()
            }
            ContentBlock::List {
                ordered,
                start,
                items,
            } => {
                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
                vbox.add_css_class("viewer-list");
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
                        check.set_sensitive(false);
                        check.set_valign(gtk::Align::Start);
                        hbox.append(&check);
                    } else {
                        let bullet_text = if *ordered {
                            format!("{}.", start.saturating_add(i as u64))
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

                    for child_block in &item.blocks {
                        if let Some(widget) = registry.render_block_with_context(child_block, ctx) {
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
                separator.add_css_class("viewer-rule");
                separator.set_margin_top(12);
                separator.set_margin_bottom(12);
                separator.upcast()
            }
            _ => unreachable!(),
        }
    }

    fn update(
        &self,
        block: &ContentBlock,
        widget: &gtk::Widget,
        registry: &crate::registry::ViewerRegistry,
        ctx: &RenderContext,
    ) -> bool {
        match block {
            ContentBlock::Paragraph(markup) => {
                if !widget.has_css_class("viewer-paragraph") {
                    return false;
                }
                let Some(label) = widget.downcast_ref::<gtk::Label>() else {
                    return false;
                };
                label.set_margin_bottom(8);
                label.set_markup(markup);
                true
            }
            ContentBlock::Heading { level, markup } => {
                if !widget.has_css_class("viewer-heading") {
                    return false;
                }
                let Some(label) = widget.downcast_ref::<gtk::Label>() else {
                    return false;
                };
                for cls in HEADING_CLASSES {
                    label.remove_css_class(cls);
                }
                let (css_class, size_tag) = heading_style(*level);
                label.add_css_class(css_class);
                label.set_margin_top(12);
                label.set_margin_bottom(8);
                label.set_markup(&format!("<span size=\"{size_tag}\"><b>{markup}</b></span>"));
                true
            }
            ContentBlock::Rule => {
                if !widget.has_css_class("viewer-rule") {
                    return false;
                }
                let Some(sep) = widget.downcast_ref::<gtk::Separator>() else {
                    return false;
                };
                sep.set_margin_top(12);
                sep.set_margin_bottom(12);
                true
            }
            ContentBlock::Blockquote(inner_blocks) => {
                if !widget.has_css_class("viewer-blockquote") {
                    return false;
                }
                let Some(frame) = widget.downcast_ref::<gtk::Frame>() else {
                    return false;
                };
                let Some(inner_vbox) = frame.child().and_downcast::<gtk::Box>() else {
                    return false;
                };
                while let Some(child) = inner_vbox.first_child() {
                    inner_vbox.remove(&child);
                }
                populate_blockquote_children(&inner_vbox, inner_blocks, registry, ctx);
                frame.set_margin_start(8);
                frame.set_margin_bottom(8);
                true
            }
            _ => false,
        }
    }
}

fn populate_blockquote_children(
    inner_vbox: &gtk::Box,
    inner_blocks: &[ContentBlock],
    registry: &crate::registry::ViewerRegistry,
    ctx: &RenderContext,
) {
    let last_idx = inner_blocks.len().saturating_sub(1);
    for (idx, child_block) in inner_blocks.iter().enumerate() {
        if let Some(widget) = registry.render_block_with_context(child_block, ctx) {
            if idx == last_idx {
                widget.set_margin_bottom(0);
            }
            inner_vbox.append(&widget);
        }
    }
}
