pub mod autocomplete;
pub mod history;

use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use history::{HistoryItem, MsgHistory};

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub label: String,
    pub content: String,
    pub is_image: bool,
}

pub struct MsgBarComponent {
    pub widget: gtk::Box,
    pub entry: gtk::Entry,
    pub is_active: Rc<Cell<bool>>,
    pub tags_box: gtk::Box,
    pub attachments: Rc<RefCell<Vec<Attachment>>>,
    pub history: Rc<RefCell<history::MsgHistory>>,
    _autocomplete: Rc<autocomplete::AutocompleteController>,
}

impl MsgBarComponent {
    pub fn new<F: Fn((String, Vec<String>)) + 'static, C: Fn() + 'static>(
        on_submit: F,
        on_cancel: C,
    ) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.set_halign(gtk::Align::Fill);
        widget.set_valign(gtk::Align::Start);
        // We style it to look native
        widget.add_css_class("boxxy-msgbar");
        widget.add_css_class("background");
        // Remove app-notification as it adds margins and rounded corners
        widget.set_visible(false);

        let icon = gtk::Image::from_icon_name("boxxyclaw");
        icon.add_css_class("accent");
        icon.set_margin_start(8); // slightly more margin
        icon.set_margin_end(4);
        widget.append(&icon);

        let tags_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tags_box.set_valign(gtk::Align::Center);
        widget.append(&tags_box);

        let entry = gtk::Entry::builder()
            .hexpand(true)
            .has_frame(false) // removes borders
            .placeholder_text("Ask Boxxy-Claw... (Ctrl+V to attach Image/Text, @agent to direct)")
            .build();

        entry.add_css_class("monospace");
        widget.append(&entry);

        let is_active = Rc::new(Cell::new(false));
        let attachments = Rc::new(RefCell::new(Vec::<Attachment>::new()));

        let history = Rc::new(RefCell::new(MsgHistory::new()));

        let providers: Vec<Box<dyn autocomplete::CompletionProvider>> =
            vec![Box::new(autocomplete::AgentCompletionProvider)];

        let autocomplete = autocomplete::AutocompleteController::new(&entry, providers, None);

        let c_active = is_active.clone();
        let c_widget = widget.clone();
        let c_attachments = attachments.clone();
        let c_tags_box = tags_box.clone();
        let c_history = history.clone();

        let on_submit_rc = Rc::new(on_submit);
        let on_cancel_rc = Rc::new(on_cancel);

        let c_submit = on_submit_rc;

        entry.connect_activate(move |e| {
            let original_text = e.text().to_string();
            let mut text = original_text.clone();
            let mut images = Vec::new();

            // Append attachments to the prompt
            let atts = c_attachments.borrow();
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

            if !text.trim().is_empty() || !images.is_empty() {
                c_history.borrow_mut().push(original_text, atts.clone());
                c_submit((text, images));
                e.set_text("");
            }

            // Clear attachments after submit
            drop(atts);
            c_attachments.borrow_mut().clear();
            while let Some(child) = c_tags_box.first_child() {
                c_tags_box.remove(&child);
            }

            c_active.set(false);
            c_widget.set_visible(false);
        });

        // To truly intercept Ctrl+V, it's easier to use the key controller
        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let k_active = is_active.clone();
        let k_widget = widget.clone();
        let k_entry = entry.clone();
        let k_cancel = on_cancel_rc;
        let k_attachments = attachments.clone();
        let k_tags_box = tags_box.clone();
        let k_history = history.clone();

        key_ctrl.connect_key_pressed(move |_, key, _, state| {
            let is_ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);

            if key == gtk::gdk::Key::Up {
                let current_text = k_entry.text().to_string();
                let current_atts = k_attachments.borrow().clone();
                if let Some(item) = k_history
                    .borrow_mut()
                    .navigate_up(current_text, current_atts)
                {
                    Self::load_history_item(&k_entry, &k_attachments, &k_tags_box, item);
                }
                return glib::Propagation::Stop;
            }

            if key == gtk::gdk::Key::Down {
                if let Some(item) = k_history.borrow_mut().navigate_down() {
                    Self::load_history_item(&k_entry, &k_attachments, &k_tags_box, item);
                }
                return glib::Propagation::Stop;
            }

            if is_ctrl && (key == gtk::gdk::Key::v || key == gtk::gdk::Key::V) {
                let clipboard = gtk::gdk::Display::default().unwrap().clipboard();
                let attachments = k_attachments.clone();
                let tags_box = k_tags_box.clone();
                let entry = k_entry.clone();

                // Check text first
                let clipboard_clone = clipboard.clone();
                clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |res| {
                    if let Ok(Some(text)) = res {
                        let text_str = text.to_string();
                        if text_str.len() > 250 || text_str.contains('\n') {
                            Self::add_attachment_static(
                                &attachments,
                                &tags_box,
                                "Text Snippet".to_string(),
                                text_str,
                                false,
                            );
                        } else {
                            // Manually insert text at cursor using Editable trait
                            let mut pos = entry.position();
                            entry.insert_text(&text_str, &mut pos);
                        }
                    } else {
                        // Check image if no text
                        let attachments_img = attachments.clone();
                        let tags_box_img = tags_box.clone();
                        clipboard_clone.read_texture_async(
                            None::<&gtk::gio::Cancellable>,
                            move |res| {
                                if let Ok(Some(texture)) = res {
                                    let bytes = texture.save_to_png_bytes();
                                    use base64::prelude::*;
                                    let b64 = BASE64_STANDARD.encode(&bytes);

                                    Self::add_attachment_static(
                                        &attachments_img,
                                        &tags_box_img,
                                        "Image".to_string(),
                                        b64,
                                        true,
                                    );
                                }
                            },
                        );
                    }
                });
                return glib::Propagation::Stop;
            }

            if key == gtk::gdk::Key::Escape {
                k_active.set(false);
                k_widget.set_visible(false);
                k_entry.set_text("");
                k_history.borrow_mut().reset();
                // Clear attachments on cancel
                k_attachments.borrow_mut().clear();
                while let Some(child) = k_tags_box.first_child() {
                    k_tags_box.remove(&child);
                }
                k_cancel();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        entry.add_controller(key_ctrl);

        Self {
            widget,
            entry,
            is_active,
            tags_box,
            attachments,
            history,
            _autocomplete: autocomplete,
        }
    }

    pub fn show_at_y(&self, y_offset: i32, height: i32) {
        self.widget.set_margin_top(y_offset);
        self.widget.set_height_request(height);
        self.widget.set_visible(true);
        self.is_active.set(true);
        // Force the widget to realize/map before grabbing focus,
        // or queue it for the next tick if needed. GTK4 grab_focus
        // usually works immediately if visible.
        self.entry.grab_focus();
    }

    pub fn hide(&self) {
        self.widget.set_visible(false);
        self.is_active.set(false);
        self.entry.set_text("");
        self.history.borrow_mut().reset();
    }

    pub fn apply_font(&self, font_desc: &gtk::pango::FontDescription) {
        // GTK4 requires us to set the font on the widget via CSS or attributes
        // The most reliable way for an Entry is via a custom Pango attr list
        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrFontDesc::new(font_desc));
        self.entry.set_attributes(&attrs);
    }

    fn load_history_item(
        entry: &gtk::Entry,
        attachments: &Rc<RefCell<Vec<Attachment>>>,
        tags_box: &gtk::Box,
        item: HistoryItem,
    ) {
        entry.set_text(&item.text);
        entry.set_position(-1);

        attachments.borrow_mut().clear();
        while let Some(child) = tags_box.first_child() {
            tags_box.remove(&child);
        }

        for att in item.attachments {
            Self::add_attachment_static(
                attachments,
                tags_box,
                att.label,
                att.content,
                att.is_image,
            );
        }
    }

    fn add_attachment_static(
        attachments: &Rc<RefCell<Vec<Attachment>>>,
        tags_box: &gtk::Box,
        label: String,
        content: String,
        is_image: bool,
    ) {
        let id = uuid::Uuid::new_v4().to_string();
        let att = Attachment {
            id: id.clone(),
            label: label.clone(),
            content,
            is_image,
        };

        attachments.borrow_mut().push(att);

        // UI Chip
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chip.add_css_class("msgbar-tag");
        chip.set_widget_name(&id);

        let lbl = gtk::Label::new(Some(&label));
        chip.append(&lbl);

        let close_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .build();

        let atts_clone = attachments.clone();
        let tags_clone = tags_box.clone();
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
        tags_box.append(&chip);
    }
}
