use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub label: String,
    pub content: String,
    pub is_image: bool,
}

#[derive(Clone)]
pub struct AttachmentManager {
    pub tags_box: gtk::FlowBox,
    pub attachments: Rc<RefCell<Vec<Attachment>>>,
}

impl Default for AttachmentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachmentManager {
    pub fn new() -> Self {
        let tags_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(6)
            .row_spacing(4)
            .max_children_per_line(12)
            .min_children_per_line(1)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .visible(false)
            .css_classes(["msgbar-tags-flow"])
            .build();

        Self {
            tags_box,
            attachments: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn widget(&self) -> &gtk::FlowBox {
        &self.tags_box
    }

    pub fn clear(&self) {
        self.attachments.borrow_mut().clear();
        while let Some(child) = self.tags_box.first_child() {
            self.tags_box.remove(&child);
        }
        self.tags_box.set_visible(false);
    }

    pub fn get_attachments(&self) -> Vec<Attachment> {
        self.attachments.borrow().clone()
    }

    pub fn load_attachments(&self, attachments: Vec<Attachment>) {
        self.clear();
        for att in attachments {
            self.add_attachment(att.label, att.content, att.is_image);
        }
    }

    pub fn add_attachment(&self, label: String, content: String, is_image: bool) {
        let id = uuid::Uuid::new_v4().to_string();
        let tooltip = if is_image {
            label.clone()
        } else {
            build_text_preview_tooltip(&content)
        };

        let att = Attachment {
            id: id.clone(),
            label: label.clone(),
            content,
            is_image,
        };

        self.attachments.borrow_mut().push(att);

        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chip.add_css_class("msgbar-tag");
        chip.set_tooltip_text(Some(&tooltip));

        let lbl = gtk::Label::new(Some(&label));
        chip.append(&lbl);

        let close_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .valign(gtk::Align::Center)
            .tooltip_text("Remove attachment")
            .can_focus(false)
            .build();
        chip.append(&close_btn);

        let flow_child = gtk::FlowBoxChild::builder()
            .child(&chip)
            .can_focus(false)
            .build();

        let atts_clone = self.attachments.clone();
        let tags_clone = self.tags_box.clone();
        let flow_child_clone = flow_child.clone();
        let id_clone = id;
        close_btn.connect_clicked(move |_| {
            let is_empty = {
                let mut atts = atts_clone.borrow_mut();
                atts.retain(|a| a.id != id_clone);
                atts.is_empty()
            };
            tags_clone.remove(&flow_child_clone);
            tags_clone.set_visible(!is_empty);
        });

        self.tags_box.append(&flow_child);
        self.tags_box.set_visible(true);
    }

    pub fn build_payload(&self, base_text: &str) -> (String, Vec<String>) {
        let mut text = base_text.to_string();
        let mut images = Vec::new();

        let atts = self.attachments.borrow();
        if !atts.is_empty() {
            let mut text_attachments_present = false;
            for att in atts.iter() {
                if att.is_image {
                    images.push(att.content.clone());
                } else {
                    if !text_attachments_present {
                        text.push_str("\n\n--- ATTACHMENTS ---");
                        text_attachments_present = true;
                    }
                    text.push_str(&format!(
                        "\n\n**{}**\n```text\n{}\n```",
                        att.label, att.content
                    ));
                }
            }
        }

        (text, images)
    }

    pub fn handle_paste(&self, entry: &gtk::TextView) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let entry_clone = entry.clone();
        let clipboard_clone = clipboard.clone();
        let self_clone = self.clone();

        clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |res| {
            let mut handled = false;
            if let Ok(Some(text)) = res {
                let text_str = text.to_string();
                if !text_str.is_empty() {
                    handled = true;
                    if let Some(texture) = try_load_image_from_pasted_path(&text_str) {
                        self_clone.attach_texture(texture);
                    } else if text_str.len() > 250 || text_str.contains('\n') {
                        self_clone.add_attachment("Text Snippet".to_string(), text_str, false);
                    } else {
                        let buf = entry_clone.buffer();
                        buf.delete_selection(true, true);
                        buf.insert_at_cursor(&text_str);
                    }
                }
            }

            if !handled {
                let self_clone_img = self_clone.clone();
                clipboard_clone.read_texture_async(None::<&gtk::gio::Cancellable>, move |res| {
                    if let Ok(Some(texture)) = res {
                        self_clone_img.attach_texture(texture);
                    }
                });
            }
        });
    }

    fn attach_texture(&self, texture: gdk::Texture) {
        let bytes = texture.save_to_png_bytes();
        let b64 = glib::base64_encode(&bytes).to_string();
        self.add_attachment("Image".to_string(), b64, true);
    }
}

fn build_text_preview_tooltip(content: &str) -> String {
    const MAX_PREVIEW_LINES: usize = 8;
    const MAX_PREVIEW_CHARS: usize = 320;

    let mut preview: String = content
        .lines()
        .take(MAX_PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if preview.chars().count() > MAX_PREVIEW_CHARS {
        preview = preview.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
        preview.push('…');
    } else if content.lines().count() > MAX_PREVIEW_LINES {
        preview.push_str("\n…");
    }
    preview
}

fn try_load_image_from_pasted_path(text: &str) -> Option<gdk::Texture> {
    let trimmed = text.trim();
    if trimmed.contains('\n') {
        return None;
    }
    let path_str = if let Some(stripped) = trimmed.strip_prefix("file://") {
        glib::Uri::unescape_string(stripped, None)
            .map_or_else(|| stripped.to_string(), |g| g.to_string())
    } else {
        trimmed.to_string()
    };

    let lower = path_str.to_lowercase();
    let is_supported_img = lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp");
    if !is_supported_img {
        return None;
    }

    let path = Path::new(&path_str);
    if !path.is_file() {
        return None;
    }
    gdk::Texture::from_filename(path).ok()
}
