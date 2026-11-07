use crate::cache::{format_syntect_span, resolve_syntax, syntax_set, theme_set};
use crate::parser::blocks::ContentBlock;
use crate::parser::markdown::escape_pango;
use crate::renderer::BlockRenderer;
use crate::widget::StructuredViewer;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use syntect::easy::HighlightLines;
use syntect::util::LinesWithEndings;

pub struct CodeRenderer;

fn highlight_code_to_pango(lang: &str, code: &str, is_dark: bool) -> String {
    let ss = syntax_set();
    let ts = theme_set();
    let syntax = resolve_syntax(ss, lang, Some(code));
    let theme = if is_dark {
        &ts.themes["base16-ocean.dark"]
    } else {
        &ts.themes["base16-ocean.light"]
    };
    let default_fg = theme.settings.foreground;

    let mut h = HighlightLines::new(syntax, theme);
    let mut pango_markup = String::with_capacity(code.len() * 2);
    pango_markup.push_str("<tt>");

    for line in LinesWithEndings::from(code) {
        if let Ok(ranges) = h.highlight_line(line, ss) {
            for (style, text) in ranges {
                pango_markup.push_str(&format_syntect_span(style, default_fg, text, is_dark));
            }
        } else {
            pango_markup.push_str(&escape_pango(line));
        }
    }
    pango_markup.push_str("</tt>");
    pango_markup
}

impl BlockRenderer for CodeRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Code { .. })
    }

    fn render(
        &self,
        block: &ContentBlock,
        _registry: &crate::registry::ViewerRegistry,
    ) -> gtk::Widget {
        if let ContentBlock::Code { lang, code } = block {
            let frame = gtk::Frame::new(None);
            frame.add_css_class("view");
            frame.set_margin_bottom(12);

            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
            vbox.set_hexpand(true);

            let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            header.add_css_class("code-header");

            static CODE_HEADER_CSS_ONCE: std::sync::Once = std::sync::Once::new();
            CODE_HEADER_CSS_ONCE.call_once(|| {
                if let Some(display) = gtk::gdk::Display::default() {
                    let css = "
                        .code-header {
                            background-color: alpha(currentColor, 0.05);
                            border-bottom: 1px solid alpha(currentColor, 0.10);
                            padding: 4px 8px;
                        }
                    ";
                    let provider = gtk::CssProvider::new();
                    provider.load_from_string(css);
                    gtk::style_context_add_provider_for_display(
                        &display,
                        &provider,
                        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                    );
                }
            });

            let lang_label = gtk::Label::new(Some(if lang.is_empty() { "text" } else { lang }));
            lang_label.add_css_class("dim-label");
            lang_label.add_css_class("caption");
            lang_label.set_hexpand(true);
            lang_label.set_halign(gtk::Align::Start);
            lang_label.set_margin_start(4);
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

            // Create a local viewer instance to track generation.
            // This allows us to cancel async highlighting if the row is recycled by GtkListView.
            let viewer = StructuredViewer::new(Rc::new(
                crate::registry::ViewerRegistry::new_with_defaults(),
            ));
            unsafe {
                vbox.set_data("viewer", viewer);
            }

            let label = gtk::Label::new(None);
            label.set_use_markup(true);
            label.set_selectable(true);
            label.set_margin_top(8);
            label.set_margin_bottom(8);
            label.set_margin_start(12);
            label.set_margin_end(12);
            label.set_halign(gtk::Align::Fill);
            label.set_hexpand(true);
            label.set_xalign(0.0);
            label.add_css_class("monospace");

            // Fast-path: check the highlight LRU cache synchronously (0ms on GtkListView scroll-back)
            let is_dark = crate::cache::is_card_surface_dark();
            if let Some(cached_markup) = crate::cache::get_cached_highlight(lang, code, is_dark) {
                label.set_markup(&cached_markup);
            } else {
                // Initial plain text content (escaped) while background thread highlights
                label.set_markup(&format!("<tt>{}</tt>", escape_pango(code)));

                // Asynchronous highlighting with atomic generation check for virtualization safety.
                // This ensures that if the row is recycled before the highlight finishes,
                // we don't overwrite the new content with the old one.
                if !code.is_empty() {
                    let code_for_highlight = code.clone();
                    let lang_for_highlight = lang.clone();

                    // Capture the shared atomic and the current generation ID
                    let (gen_handle, start_gen) =
                        if let Some(v_ptr) = unsafe { vbox.data::<StructuredViewer>("viewer") } {
                            let v = unsafe { v_ptr.as_ref() };
                            (v.generation_handle(), v.generation())
                        } else {
                            // Fallback should not happen in practice
                            (std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)), 0)
                        };

                    let (tx, rx) = async_channel::bounded::<String>(1);

                    boxxy_sys_utils::runtime().spawn_blocking(move || {
                        let pango_markup =
                            highlight_code_to_pango(&lang_for_highlight, &code_for_highlight, is_dark);
                        crate::cache::insert_cached_highlight(
                            &lang_for_highlight,
                            &code_for_highlight,
                            is_dark,
                            pango_markup.clone(),
                        );
                        let _ = tx.send_blocking(pango_markup);
                    });

                    let label_clone = label.clone();
                    gtk::glib::spawn_future_local(async move {
                        if let Ok(markup) = rx.recv().await {
                            // Only apply the highlighted markup if the viewer generation still matches.
                            if gen_handle.load(std::sync::atomic::Ordering::Relaxed) == start_gen {
                                label_clone.set_markup(&markup);
                            }
                        }
                    });
                }
            }

            if !code.is_empty() {
                let label_weak = label.downgrade();
                let lang_for_theme = lang.clone();
                let code_for_theme = code.clone();
                let last_dark = Rc::new(Cell::new(is_dark));
                let sm = libadwaita::StyleManager::default();
                let handler_id = sm.connect_dark_notify(move |_| {
                    let new_dark = crate::cache::is_card_surface_dark();
                    if last_dark.replace(new_dark) == new_dark {
                        return;
                    }
                    let Some(lbl) = label_weak.upgrade() else {
                        return;
                    };
                    if let Some(cached) =
                        crate::cache::get_cached_highlight(&lang_for_theme, &code_for_theme, new_dark)
                    {
                        lbl.set_markup(&cached);
                    } else {
                        let markup =
                            highlight_code_to_pango(&lang_for_theme, &code_for_theme, new_dark);
                        crate::cache::insert_cached_highlight(
                            &lang_for_theme,
                            &code_for_theme,
                            new_dark,
                            markup.clone(),
                        );
                        lbl.set_markup(&markup);
                    }
                });
                let handler_cell = RefCell::new(Some(handler_id));
                frame.connect_destroy(move |_| {
                    if let Some(id) = handler_cell.borrow_mut().take() {
                        libadwaita::StyleManager::default().disconnect(id);
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
