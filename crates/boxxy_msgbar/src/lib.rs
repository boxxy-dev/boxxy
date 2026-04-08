pub mod attachment;
pub mod autocomplete;
pub mod history;

use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use attachment::{Attachment, AttachmentManager};

pub struct MsgBarComponent {
    pub widget: gtk::Box,
    pub entry: gtk::Entry,
    pub is_active: Rc<Cell<bool>>,
    pub attachment_mgr: AttachmentManager,
    pub history: Rc<RefCell<history::MsgHistory>>,
    pub claw_toggle: gtk::Button,
    pub proactive_toggle: gtk::Button,
    pub pin_toggle: gtk::Button,
    pub web_search_toggle: gtk::Button,
    pub claw_state: Rc<Cell<bool>>,
    pub proactive_state: Rc<Cell<bool>>,
    pub pinned_state: Rc<Cell<bool>>,
    pub web_search_state: Rc<Cell<bool>>,
    _autocomplete: Rc<boxxy_core_widgets::autocomplete::AutocompleteController>,
}

impl MsgBarComponent {
    pub fn new<
        F: Fn((String, Vec<String>)) + 'static,
        C: Fn() + 'static,
        T1: Fn(bool) + 'static,
        T2: Fn(bool) + 'static,
        T3: Fn(bool) + 'static,
        T4: Fn(bool) + 'static,
    >(
        on_submit: F,
        on_cancel: C,
        on_claw_toggle: T1,
        on_proactive_toggle: T2,
        on_pin_toggle: T3,
        on_web_search_toggle: T4,
    ) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.set_halign(gtk::Align::Fill);
        widget.set_valign(gtk::Align::Start);
        // We style it to look native
        widget.add_css_class("boxxy-msgbar");
        widget.add_css_class("background");
        // Remove app-notification as it adds margins and rounded corners
        widget.set_visible(false);

        let entry = gtk::Entry::builder()
            .hexpand(true)
            .has_frame(false) // removes borders
            .placeholder_text(
                "Ask Boxxy-Claw... (Ctrl+V: attach, @agent: direct, /resume: session)",
            )
            .build();

        entry.add_css_class("monospace");

        let icon = gtk::Image::from_icon_name("boxxy-boxxyclaw-symbolic");

        let claw_toggle = gtk::Button::builder()
            .child(&icon)
            .css_classes(["flat", "image-button"])
            .tooltip_text("Toggle Claw for this pane")
            .margin_start(4)
            .margin_end(0)
            .valign(gtk::Align::Center)
            .can_focus(false)
            .build();

        let claw_state = Rc::new(Cell::new(false));
        let claw_state_clone = claw_state.clone();
        let claw_entry_focus = entry.clone();
        claw_toggle.connect_clicked(move |_| {
            let next = !claw_state_clone.get();
            on_claw_toggle(next);
            claw_entry_focus.grab_focus();
        });

        widget.append(&claw_toggle);

        let proactive_img = gtk::Image::from_icon_name("boxxy-walking2-symbolic");
        let proactive_toggle = gtk::Button::builder()
            .child(&proactive_img)
            .css_classes(["flat", "image-button"])
            .tooltip_text("Lazy Diagnosis Mode")
            .margin_start(0)
            .margin_end(0)
            .valign(gtk::Align::Center)
            .can_focus(false)
            .build();

        let proactive_state = Rc::new(Cell::new(false));
        let proactive_state_clone = proactive_state.clone();
        let proactive_entry_focus = entry.clone();
        proactive_toggle.connect_clicked(move |_| {
            let next = !proactive_state_clone.get();
            on_proactive_toggle(next);
            proactive_entry_focus.grab_focus();
        });

        widget.append(&proactive_toggle);

        let pin_img = gtk::Image::from_icon_name("boxxy-view-pin-symbolic");
        let pin_toggle = gtk::Button::builder()
            .child(&pin_img)
            .css_classes(["flat", "image-button"])
            .tooltip_text("Pin this session")
            .margin_start(0)
            .margin_end(0)
            .valign(gtk::Align::Center)
            .can_focus(false)
            .build();

        let pinned_state = Rc::new(Cell::new(false));
        let pinned_state_clone = pinned_state.clone();
        let pin_entry_focus = entry.clone();
        pin_toggle.connect_clicked(move |_| {
            let next = !pinned_state_clone.get();
            on_pin_toggle(next);
            pin_entry_focus.grab_focus();
        });

        widget.append(&pin_toggle);

        let web_search_img = gtk::Image::from_icon_name("boxxy-globe-symbolic");
        let web_search_toggle = gtk::Button::builder()
            .child(&web_search_img)
            .css_classes(["flat", "image-button"])
            .tooltip_text("Enable Web Search")
            .margin_start(0)
            .margin_end(0)
            .valign(gtk::Align::Center)
            .can_focus(false)
            .build();

        let web_search_state = Rc::new(Cell::new(false));
        let web_search_state_clone = web_search_state.clone();
        let web_search_entry_focus = entry.clone();
        web_search_toggle.connect_clicked(move |_| {
            let next = !web_search_state_clone.get();
            on_web_search_toggle(next);
            web_search_entry_focus.grab_focus();
        });

        widget.append(&web_search_toggle);

        let attachment_mgr = AttachmentManager::new();
        widget.append(attachment_mgr.widget());

        widget.append(&entry);

        let is_active = Rc::new(Cell::new(false));

        let history = Rc::new(RefCell::new(history::MsgHistory::new()));

        let mut providers: Vec<Box<dyn boxxy_core_widgets::autocomplete::CompletionProvider>> = vec![
            Box::new(autocomplete::AgentCompletionProvider),
            Box::new(autocomplete::CommandCompletionProvider),
            Box::new(autocomplete::ResumeCompletionProvider),
        ];
        // Sort by trigger length descending to ensure "/resume " matches before "/"
        providers.sort_by(|a, b| b.trigger().len().cmp(&a.trigger().len()));

        let c_entry_activate = entry.clone();
        let autocomplete_ctrl = boxxy_core_widgets::autocomplete::AutocompleteController::new(
            &entry,
            providers,
            Some(Box::new(move |replacement| {
                if replacement.starts_with("/resume ") {
                    c_entry_activate.emit_activate();
                }
            })),
        );

        let c_active = is_active.clone();
        let c_widget = widget.clone();
        let c_attachment_mgr = attachment_mgr.clone();
        let c_history = history.clone();

        let on_submit_rc = Rc::new(on_submit);
        let on_cancel_rc = Rc::new(on_cancel);

        let c_submit = on_submit_rc;

        let c_history_activate = c_history.clone();
        entry.connect_activate(move |e| {
            let original_text = e.text().to_string();
            let (text, images) = c_attachment_mgr.build_payload(&original_text);

            if !text.trim().is_empty() || !images.is_empty() {
                c_history_activate
                    .borrow_mut()
                    .push(original_text, c_attachment_mgr.get_attachments());
                c_submit((text, images));
                e.set_text("");
            }

            c_attachment_mgr.clear();

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
        let k_attachment_mgr = attachment_mgr.clone();
        let k_history = c_history;

        let k_autocomplete = autocomplete_ctrl.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, state| {
            let is_ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);

            if k_autocomplete.is_visible() {
                // If autocomplete is visible, let it handle its own navigation/selection keys
                if key == gtk::gdk::Key::Up
                    || key == gtk::gdk::Key::Down
                    || key == gtk::gdk::Key::Return
                    || key == gtk::gdk::Key::Tab
                    || key == gtk::gdk::Key::Escape
                {
                    return glib::Propagation::Proceed;
                }
            }

            if key == gtk::gdk::Key::Up {
                let current_text = k_entry.text().to_string();
                let current_atts = k_attachment_mgr.get_attachments();
                if let Some(item) = k_history
                    .borrow_mut()
                    .navigate_up(current_text, current_atts)
                {
                    k_entry.set_text(&item.text);
                    k_entry.set_position(-1);
                    k_attachment_mgr.load_attachments(item.attachments);
                }
                return glib::Propagation::Stop;
            }

            if key == gtk::gdk::Key::Down {
                if let Some(item) = k_history.borrow_mut().navigate_down() {
                    k_entry.set_text(&item.text);
                    k_entry.set_position(-1);
                    k_attachment_mgr.load_attachments(item.attachments);
                }
                return glib::Propagation::Stop;
            }

            if is_ctrl && (key == gtk::gdk::Key::v || key == gtk::gdk::Key::V) {
                k_attachment_mgr.handle_paste(&k_entry);
                return glib::Propagation::Stop;
            }

            if key == gtk::gdk::Key::Escape {
                k_active.set(false);
                k_widget.set_visible(false);
                k_entry.set_text("");
                k_history.borrow_mut().reset();
                k_attachment_mgr.clear();
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
            attachment_mgr,
            history,
            claw_toggle,
            proactive_toggle,
            pin_toggle,
            web_search_toggle,
            claw_state,
            proactive_state,
            pinned_state,
            web_search_state,
            _autocomplete: autocomplete_ctrl,
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

    pub fn update_ui(&self, active: bool, proactive: bool, pinned: bool, web_search: bool) {
        self.claw_state.set(active);
        self.proactive_state.set(proactive);
        self.pinned_state.set(pinned);
        self.web_search_state.set(web_search);

        if active {
            self.claw_toggle.add_css_class("accent");
        } else {
            self.claw_toggle.remove_css_class("accent");
        }

        if proactive {
            self.proactive_toggle
                .set_icon_name("boxxy-running-symbolic");
            self.proactive_toggle
                .set_tooltip_text(Some("Proactive Diagnosis Mode"));
            self.proactive_toggle.add_css_class("accent");
        } else {
            self.proactive_toggle
                .set_icon_name("boxxy-walking2-symbolic");
            self.proactive_toggle
                .set_tooltip_text(Some("Lazy Diagnosis Mode"));
            self.proactive_toggle.remove_css_class("accent");
        }

        if pinned {
            self.pin_toggle.add_css_class("accent");
            self.pin_toggle.set_tooltip_text(Some("Unpin this session"));
        } else {
            self.pin_toggle.remove_css_class("accent");
            self.pin_toggle.set_tooltip_text(Some("Pin this session"));
        }

        if web_search {
            self.web_search_toggle.add_css_class("accent");
            self.web_search_toggle
                .set_tooltip_text(Some("Disable Web Search"));
        } else {
            self.web_search_toggle.remove_css_class("accent");
            self.web_search_toggle
                .set_tooltip_text(Some("Enable Web Search"));
        }
    }

    pub fn set_web_search_visible(&self, visible: bool) {
        self.web_search_toggle.set_visible(visible);
    }

    pub fn apply_font(&self, font_desc: &gtk::pango::FontDescription) {
        // GTK4 requires us to set the font on the widget via CSS or attributes
        // The most reliable way for an Entry is via a custom Pango attr list
        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrFontDesc::new(font_desc));
        self.entry.set_attributes(&attrs);
    }
}
