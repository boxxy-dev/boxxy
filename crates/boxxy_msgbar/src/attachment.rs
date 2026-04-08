use gtk4 as gtk;
use gtk4::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub label: String,
    pub content: String,
    pub is_image: bool,
}

#[derive(Clone)]
pub struct AttachmentManager {
    pub tags_box: gtk::Box,
    pub attachments: Rc<RefCell<Vec<Attachment>>>,
}

impl Default for AttachmentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachmentManager {
    pub fn new() -> Self {
        let tags_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tags_box.set_valign(gtk::Align::Center);

        Self {
            tags_box,
            attachments: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.tags_box
    }

    pub fn clear(&self) {
        self.attachments.borrow_mut().clear();
        while let Some(child) = self.tags_box.first_child() {
            self.tags_box.remove(&child);
        }
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
        let att = Attachment {
            id: id.clone(),
            label: label.clone(),
            content,
            is_image,
        };

        self.attachments.borrow_mut().push(att);

        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chip.add_css_class("msgbar-tag");
        chip.set_widget_name(&id);

        let lbl = gtk::Label::new(Some(&label));
        chip.append(&lbl);

        let close_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .can_focus(false)
            .build();

        let atts_clone = self.attachments.clone();
        let tags_clone = self.tags_box.clone();
        let id_clone = id;
        close_btn.connect_clicked(move |_| {
            atts_clone.borrow_mut().retain(|a| a.id != id_clone);
            if let Some(child) = tags_clone.first_child() {
                let mut curr = Some(child);
                while let Some(c) = curr {
                    if c.widget_name() == id_clone {
                        tags_clone.remove(&c);
                        break;
                    }
                    curr = c.next_sibling();
                }
            }
        });

        chip.append(&close_btn);
        self.tags_box.append(&chip);
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
                    text.push_str(&format!("\n\n[{}]\n{}", att.label, att.content));
                }
            }
        }

        (text, images)
    }

    pub fn handle_paste(&self, entry: &gtk::Entry) {
        let clipboard = gtk::gdk::Display::default().unwrap().clipboard();
        let entry_clone = entry.clone();
        let clipboard_clone = clipboard.clone();

        let self_clone = self.clone();

        clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |res| {
            let mut has_text = false;
            if let Ok(Some(text)) = res {
                let text_str = text.to_string();
                if !text_str.is_empty() {
                    has_text = true;
                    if text_str.len() > 250 || text_str.contains('\n') {
                        self_clone.add_attachment("Text Snippet".to_string(), text_str, false);
                    } else {
                        let mut pos = entry_clone.position();
                        entry_clone.insert_text(&text_str, &mut pos);
                    }
                }
            }

            if !has_text {
                let self_clone_img = self_clone.clone();
                clipboard_clone.read_texture_async(None::<&gtk::gio::Cancellable>, move |res| {
                    if let Ok(Some(texture)) = res {
                        let bytes = texture.save_to_png_bytes();
                        let b64 = gtk::glib::base64_encode(&bytes);
                        self_clone_img.add_attachment("Image".to_string(), b64.to_string(), true);
                    }
                });
            }
        });
    }
}
