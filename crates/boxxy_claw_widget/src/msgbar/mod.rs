pub mod attachment;
pub mod autocomplete;
pub mod history;

use boxxy_claw_protocol::AgentStatus;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use attachment::{Attachment, AttachmentManager};

pub struct MsgBarComponent {
    pub widget: gtk::Box,
    pub entry: gtk::TextView,
    placeholder_label: gtk::Label,
    pub send_btn: gtk::Button,
    pub is_active: Rc<Cell<bool>>,
    pub attachment_mgr: AttachmentManager,
    pub history: Rc<RefCell<history::MsgHistory>>,
    pub claw_toggle: gtk::Button,
    pub claw_image: gtk::Image,
    pub sleep_toggle: gtk::Button,
    pub pin_toggle: gtk::Button,
    pub web_search_toggle: gtk::Button,
    sleep_switch: gtk::Switch,
    pin_switch: gtk::Switch,
    web_switch: gtk::Switch,
    avatar_sleep_badge: gtk::Box,
    pub claw_state: Rc<Cell<bool>>,
    pub sleep_state: Rc<Cell<bool>>,
    pub pin_state: Rc<Cell<bool>>,
    pub web_search_state: Rc<Cell<bool>>,
    /// Current character UUID for avatar lookup
    pub character_id: Rc<RefCell<Option<String>>>,
    pub avatar: adw::Avatar,
    avatar_stack: gtk::Stack,
    /// When true, the msgbar does not hide itself on Enter/Escape — its
    /// enclosing container (e.g. the claw overlay drawer) owns visibility.
    embedded: Rc<Cell<bool>>,
    on_submit_hook: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
    on_clear_hook: Rc<RefCell<Option<Box<dyn Fn()>>>>,
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
        on_sleep_toggle: T2,
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

        let attachment_mgr = AttachmentManager::new();

        // Group the left status/avatar badges in a box with `valign(Start)` so they stay
        // anchored to the first line when the text input wraps across multiple lines.
        let left_badges = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        left_badges.set_valign(gtk::Align::Start);
        widget.append(&left_badges);

        let entry = gtk::TextView::builder()
            .hexpand(true)
            .valign(gtk::Align::Center)
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .build();

        let placeholder_label = gtk::Label::builder()
            .label("Type your message... (/: commands, Ctrl+V: attach, @character: direct)")
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Center)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .can_target(false)
            .can_focus(false)
            .css_classes(["msgbar-placeholder"])
            .build();

        let entry_overlay = gtk::Overlay::builder()
            .hexpand(true)
            .valign(gtk::Align::Center)
            .child(&entry)
            .build();
        entry_overlay.add_overlay(&placeholder_label);

        let placeholder_for_buf = placeholder_label.clone();
        let entry_for_buf = entry.clone();
        entry.buffer().connect_changed(move |buf| {
            placeholder_for_buf.set_visible(buf.char_count() == 0);
            entry_for_buf.queue_resize();
        });

        // Default app font, not terminal monospace. The msgbar is
        // chat-chrome inside the drawer; monospace made it feel like a
        // shell prompt.

        let claw_image = gtk::Image::from_icon_name("boxxy-boxxyclaw-symbolic");

        let avatar = adw::Avatar::new(32, None, true);
        let profile = boxxy_preferences::user_profile::current();
        boxxy_preferences::user_profile::configure_avatar(&avatar, &profile);

        let avatar_stack = gtk::Stack::new();
        avatar_stack.set_transition_type(gtk::StackTransitionType::None);
        avatar_stack.add_named(&claw_image, Some("icon"));
        avatar_stack.add_named(&avatar, Some("avatar"));
        avatar_stack.set_visible_child_name("avatar");
        avatar_stack.set_margin_top(2);
        avatar_stack.set_margin_bottom(2);
        avatar_stack.set_margin_start(2);
        avatar_stack.set_margin_end(2);

        let avatar_sleep_icon = gtk::Image::builder()
            .icon_name("boxxy-bedtime-symbolic")
            .pixel_size(10)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let avatar_sleep_badge = gtk::Box::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .css_classes(["avatar-sleep-badge", "avatar-sleep-badge-small"])
            .visible(false)
            .build();
        avatar_sleep_badge.append(&avatar_sleep_icon);

        let avatar_overlay = gtk::Overlay::builder()
            .child(&avatar_stack)
            .build();
        avatar_overlay.add_overlay(&avatar_sleep_badge);

        let claw_toggle = gtk::Button::builder()
            .child(&avatar_overlay)
            .css_classes(["flat", "image-button"])
            .tooltip_text("Session Options (/sleep, /pin, /web)")
            .margin_start(4)
            .margin_end(2)
            .valign(gtk::Align::Center)
            .can_focus(false)
            .build();

        let claw_state = Rc::new(Cell::new(false));
        let character_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        left_badges.append(&claw_toggle);

        let status_popover = gtk::Popover::builder()
            .position(gtk::PositionType::Top)
            .has_arrow(true)
            .autohide(true)
            .build();
        status_popover.set_parent(&claw_toggle);
        let popover_for_destroy = status_popover.clone();
        claw_toggle.connect_destroy(move |_| {
            popover_for_destroy.unparent();
        });

        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        popover_box.set_margin_top(4);
        popover_box.set_margin_bottom(4);
        popover_box.set_margin_start(4);
        popover_box.set_margin_end(4);
        status_popover.set_child(Some(&popover_box));

        let popover_for_click = status_popover.clone();
        claw_toggle.connect_clicked(move |_| {
            if popover_for_click.is_visible() {
                popover_for_click.popdown();
            } else {
                popover_for_click.popup();
            }
        });

        let on_sleep_toggle_rc = Rc::new(on_sleep_toggle);
        let on_pin_toggle_rc = Rc::new(on_pin_toggle);
        let on_web_search_toggle_rc = Rc::new(on_web_search_toggle);

        let make_popover_row = |icon_name: &str, title: &str| -> (gtk::Button, gtk::Switch) {
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let img = gtk::Image::from_icon_name(icon_name);
            img.add_css_class("dim-label");
            let title_lbl = gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .hexpand(true)
                .build();
            let sw = gtk::Switch::builder()
                .valign(gtk::Align::Center)
                .can_focus(false)
                .can_target(false)
                .build();
            row_box.append(&img);
            row_box.append(&title_lbl);
            row_box.append(&sw);

            let btn = gtk::Button::builder()
                .child(&row_box)
                .css_classes(["flat", "msgbar-popover-row"])
                .can_focus(false)
                .build();
            (btn, sw)
        };

        let (sleep_toggle, sleep_switch) =
            make_popover_row("boxxy-bedtime-symbolic", "Sleep Mode");
        sleep_toggle.set_tooltip_text(Some("Sleep Mode (Passive Observer)"));

        let sleep_state = Rc::new(Cell::new(false));
        let sleep_state_clone = sleep_state.clone();
        let on_sleep_cb = on_sleep_toggle_rc.clone();
        sleep_toggle.connect_clicked(move |_| {
            let next = !sleep_state_clone.get();
            on_sleep_cb(next);
        });

        popover_box.append(&sleep_toggle);

        let (pin_toggle, pin_switch) =
            make_popover_row("boxxy-view-pin-symbolic", "Pin Session");
        pin_toggle.set_tooltip_text(Some("Pin this session"));

        let pin_state = Rc::new(Cell::new(false));
        let pin_state_clone = pin_state.clone();
        let on_pin_cb = on_pin_toggle_rc.clone();
        pin_toggle.connect_clicked(move |_| {
            let next = !pin_state_clone.get();
            on_pin_cb(next);
        });

        popover_box.append(&pin_toggle);

        let (web_search_toggle, web_switch) =
            make_popover_row("boxxy-globe-symbolic", "Web Search");
        web_search_toggle.set_tooltip_text(Some("Enable Web Search"));

        let web_search_state = Rc::new(Cell::new(false));
        let web_search_state_clone = web_search_state.clone();
        let on_web_cb = on_web_search_toggle_rc.clone();
        web_search_toggle.connect_clicked(move |_| {
            let next = !web_search_state_clone.get();
            on_web_cb(next);
        });

        popover_box.append(&web_search_toggle);

        widget.append(&entry_overlay);

        // Send button — mirrors Enter in the entry. Intentionally NOT
        // appended to `widget`: callers (the claw overlay drawer) place
        // it *outside* the msgbar box so the bar itself can be a rounded
        // field and the send icon floats next to it, matching chat UIs.
        // Anchored to `End` (bottom) so when the input wraps to multiple
        // lines, the send icon sits on the last line.
        let send_btn = gtk::Button::builder()
            .icon_name("boxxy-paper-plane-symbolic")
            .css_classes(["flat", "image-button", "msgbar-send"])
            .tooltip_text("Send")
            .valign(gtk::Align::End)
            .margin_bottom(6)
            .can_focus(false)
            .build();

        let is_active = Rc::new(Cell::new(false));
        let history = Rc::new(RefCell::new(history::MsgHistory::new()));
        let embedded = Rc::new(Cell::new(false));
        let on_submit_hook: Rc<RefCell<Option<Box<dyn Fn(&str)>>>> = Rc::new(RefCell::new(None));
        let on_clear_hook: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));

        let on_submit_rc = Rc::new(on_submit);
        let on_cancel_rc = Rc::new(on_cancel);
        let on_claw_toggle_rc = Rc::new(on_claw_toggle);

        let submit_action: Rc<dyn Fn()> = {
            let entry = entry.clone();
            let attachment_mgr = attachment_mgr.clone();
            let history = history.clone();
            let on_submit = on_submit_rc;
            let on_claw_toggle = on_claw_toggle_rc;
            let on_sleep_toggle = on_sleep_toggle_rc;
            let on_pin_toggle = on_pin_toggle_rc;
            let on_web_search_toggle = on_web_search_toggle_rc;
            let claw_state = claw_state.clone();
            let character_id = character_id.clone();
            let sleep_state = sleep_state.clone();
            let pin_state = pin_state.clone();
            let web_search_state = web_search_state.clone();
            let on_submit_hook = on_submit_hook.clone();
            let on_clear_hook = on_clear_hook.clone();
            let embedded = embedded.clone();
            let is_active = is_active.clone();
            let widget = widget.clone();

            Rc::new(move || {
                let buf = entry.buffer();
                let original_text = buf
                    .text(&buf.start_iter(), &buf.end_iter(), false)
                    .to_string();

                if original_text.trim() == "/models" && attachment_mgr.get_attachments().is_empty()
                {
                    buf.set_text("");
                    if let Some(root) = entry.root() {
                        let _ = gtk::prelude::WidgetExt::activate_action(
                            &root,
                            "win.model-selection",
                            None,
                        );
                    }
                    return;
                }

                if original_text.trim() == "/resume" && attachment_mgr.get_attachments().is_empty()
                {
                    buf.set_text("/resume ");
                    let end = buf.end_iter();
                    buf.place_cursor(&end);
                    return;
                }

                if original_text.trim() == "/sleep" && attachment_mgr.get_attachments().is_empty() {
                    buf.set_text("");
                    if claw_state.get() || character_id.borrow().is_some() {
                        let next = !sleep_state.get();
                        on_sleep_toggle(next);
                    }
                    return;
                }

                if original_text.trim() == "/pin" && attachment_mgr.get_attachments().is_empty() {
                    buf.set_text("");
                    if claw_state.get() || character_id.borrow().is_some() {
                        let next = !pin_state.get();
                        on_pin_toggle(next);
                    }
                    return;
                }

                if original_text.trim() == "/web" && attachment_mgr.get_attachments().is_empty() {
                    buf.set_text("");
                    if boxxy_preferences::Settings::load().enable_web_search {
                        let next = !web_search_state.get();
                        on_web_search_toggle(next);
                    }
                    return;
                }

                if original_text.trim() == "/clear" && attachment_mgr.get_attachments().is_empty() {
                    buf.set_text("");
                    if (claw_state.get() || character_id.borrow().is_some())
                        && let Some(hook) = on_clear_hook.borrow().as_ref()
                    {
                        hook();
                    }
                    return;
                }

                if original_text.trim() == "/exit" && attachment_mgr.get_attachments().is_empty() {
                    buf.set_text("");
                    if claw_state.get() || character_id.borrow().is_some() {
                        on_claw_toggle(false);
                    }
                    return;
                }

                let (text, images) = attachment_mgr.build_payload(&original_text);

                if !text.trim().is_empty() || !images.is_empty() {
                    if let Some(hook) = on_submit_hook.borrow().as_ref() {
                        hook(&text);
                    }
                    history
                        .borrow_mut()
                        .push(original_text, attachment_mgr.get_attachments());
                    on_submit((text, images));
                    buf.set_text("");
                }

                attachment_mgr.clear();

                // When embedded in a drawer, visibility is owned by the
                // container; self-hiding would leave an empty strip in the UI.
                if !embedded.get() {
                    is_active.set(false);
                    widget.set_visible(false);
                }
            })
        };

        let send_submit = submit_action.clone();
        send_btn.connect_clicked(move |_| {
            send_submit();
        });

        let mut providers: Vec<Box<dyn boxxy_core_widgets::autocomplete::CompletionProvider>> = vec![
            Box::new(autocomplete::AgentCompletionProvider),
            Box::new(autocomplete::CommandCompletionProvider::new(
                claw_state.clone(),
                character_id.clone(),
                sleep_state.clone(),
                pin_state.clone(),
                web_search_state.clone(),
            )),
            Box::new(autocomplete::ResumeCompletionProvider::new(Some(
                entry.downgrade(),
            ))),
        ];
        // Sort by trigger length descending to ensure "/resume " matches before "/"
        providers.sort_by(|a, b| b.trigger().len().cmp(&a.trigger().len()));

        let autocomplete_submit = submit_action.clone();
        let autocomplete_ctrl = boxxy_core_widgets::autocomplete::AutocompleteController::new(
            &entry,
            providers,
            Some(Box::new(move |replacement| {
                let is_resume_session = replacement
                    .strip_prefix("/resume ")
                    .is_some_and(|s| !s.trim().is_empty());
                if is_resume_session
                    || matches!(
                        replacement.as_str(),
                        "/models" | "/sleep" | "/pin" | "/web" | "/clear" | "/exit"
                    )
                {
                    autocomplete_submit();
                }
            })),
        );

        // Key controller for Enter submit (Shift+Enter inserts newline),
        // Up/Down history navigation, Ctrl+V paste, and Escape cancel.
        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let k_active = is_active.clone();
        let k_widget = widget.clone();
        let k_embedded = embedded.clone();
        let k_entry = entry.clone();
        let k_cancel = on_cancel_rc;
        let k_attachment_mgr = attachment_mgr.clone();
        let k_history = history.clone();
        let k_submit = submit_action;

        let k_autocomplete = autocomplete_ctrl.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, state| {
            let is_ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let is_shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);

            if k_autocomplete.is_visible() {
                // If autocomplete is visible, let it handle its own navigation/selection keys
                if key == gtk::gdk::Key::Up
                    || key == gtk::gdk::Key::Down
                    || key == gtk::gdk::Key::Return
                    || key == gtk::gdk::Key::KP_Enter
                    || key == gtk::gdk::Key::Tab
                    || key == gtk::gdk::Key::Escape
                {
                    return glib::Propagation::Proceed;
                }
            }

            if key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter {
                if !is_shift {
                    k_submit();
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }

            if key == gtk::gdk::Key::Up {
                let buf = k_entry.buffer();
                let mut iter = buf.iter_at_offset(buf.cursor_position());
                if k_entry.backward_display_line(&mut iter) {
                    return glib::Propagation::Proceed;
                }
                let current_text = buf
                    .text(&buf.start_iter(), &buf.end_iter(), false)
                    .to_string();
                let current_atts = k_attachment_mgr.get_attachments();
                if let Some(item) = k_history
                    .borrow_mut()
                    .navigate_up(current_text, current_atts)
                {
                    buf.set_text(&item.text);
                    let end = buf.end_iter();
                    buf.place_cursor(&end);
                    k_attachment_mgr.load_attachments(item.attachments);
                }
                return glib::Propagation::Stop;
            }

            if key == gtk::gdk::Key::Down {
                let buf = k_entry.buffer();
                let mut iter = buf.iter_at_offset(buf.cursor_position());
                if k_entry.forward_display_line(&mut iter) {
                    return glib::Propagation::Proceed;
                }
                if let Some(item) = k_history.borrow_mut().navigate_down() {
                    buf.set_text(&item.text);
                    let end = buf.end_iter();
                    buf.place_cursor(&end);
                    k_attachment_mgr.load_attachments(item.attachments);
                }
                return glib::Propagation::Stop;
            }

            if is_ctrl && (key == gtk::gdk::Key::v || key == gtk::gdk::Key::V) {
                k_attachment_mgr.handle_paste(&k_entry);
                return glib::Propagation::Stop;
            }

            if key == gtk::gdk::Key::Escape {
                // In embedded mode the container owns visibility; only
                // cancel the in-progress input, the drawer stays open so
                // the user can still read/reply.
                if !k_embedded.get() {
                    k_active.set(false);
                    k_widget.set_visible(false);
                }
                k_entry.buffer().set_text("");
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
            placeholder_label,
            send_btn,
            is_active,
            attachment_mgr,
            history,
            claw_toggle,
            claw_image,
            sleep_toggle,
            pin_toggle,
            web_search_toggle,
            sleep_switch,
            pin_switch,
            web_switch,
            avatar_sleep_badge,
            claw_state,
            sleep_state,
            pin_state,
            web_search_state,
            character_id,
            avatar,
            avatar_stack,
            embedded,
            on_submit_hook,
            on_clear_hook,
            _autocomplete: autocomplete_ctrl,
        }
    }

    pub fn set_on_submit_hook<F: Fn(&str) + 'static>(&self, hook: F) {
        *self.on_submit_hook.borrow_mut() = Some(Box::new(hook));
    }

    pub fn set_on_clear_hook<F: Fn() + 'static>(&self, hook: F) {
        *self.on_clear_hook.borrow_mut() = Some(Box::new(hook));
    }

    pub fn set_text(&self, text: &str) {
        self.entry.buffer().set_text(text);
    }

    /// Refreshes the input bar's left-hand avatar to show the user's own profile avatar.
    pub fn refresh_user_avatar(&self) {
        let profile = boxxy_preferences::user_profile::current();
        boxxy_preferences::user_profile::configure_avatar(&self.avatar, &profile);
        self.avatar_stack.set_visible_child_name("avatar");
    }

    /// Clears the active character binding and restores the default input placeholder.
    pub fn clear_character(&self) {
        *self.character_id.borrow_mut() = None;
        self.placeholder_label.set_label(
            "Type your message... (/: commands, Ctrl+V: attach, @character: direct)",
        );
        self.refresh_user_avatar();
    }

    /// Update the placeholder text based on the active character UUID and refresh the user avatar.
    pub fn set_character(&self, character_id: &str) {
        *self.character_id.borrow_mut() = Some(character_id.to_string());

        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        if let Some(info) = registry.iter().find(|c| c.config.id == character_id) {
            let formatted_name = info
                .config
                .name
                .split('-')
                .map(|word| {
                    let mut c = word.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    }
                })
                .collect::<Vec<String>>()
                .join(" ");

            self.placeholder_label
                .set_label(&format!("Message {}...", formatted_name));
        }

        self.refresh_user_avatar();
    }

    /// Mark the msgbar as embedded inside another container (the claw
    /// overlay drawer). Embedded mode suppresses self-hide on Enter and
    /// Escape — the container owns visibility so the agent stays reachable
    /// after each reply.
    pub fn set_embedded(&self, embedded: bool) {
        self.embedded.set(embedded);
        if embedded {
            // The drawer manages visibility directly.
            self.widget.set_visible(true);
            self.is_active.set(true);
        }
    }

    pub fn set_status(&self, status: AgentStatus) {
        // Reset everything
        for cls in [
            "status-active",
            "status-sleep",
            "status-error",
            "accent",
            "warning",
            "grayscale",
        ] {
            self.claw_toggle.remove_css_class(cls);
            self.sleep_toggle.remove_css_class(cls);
            self.claw_image.remove_css_class(cls);
        }

        match status {
            AgentStatus::Waiting | AgentStatus::Working | AgentStatus::Locking { .. } => {
                self.claw_toggle.add_css_class("status-active");
                self.sleep_toggle
                    .set_tooltip_text(Some("Sleep Mode (Passive Observer)"));
                self.sleep_switch.set_active(false);
                self.avatar_sleep_badge.set_visible(false);
                self.sleep_state.set(false);
            }
            AgentStatus::Sleep => {
                self.claw_toggle.add_css_class("status-sleep");
                self.sleep_toggle
                    .set_tooltip_text(Some("Wake up (Resume from Sleep)"));
                self.sleep_switch.set_active(true);
                self.avatar_sleep_badge.set_visible(true);
                self.sleep_state.set(true);
            }
            AgentStatus::Faulted { .. } => {
                self.claw_toggle.add_css_class("status-error");
                self.sleep_switch.set_active(false);
                self.avatar_sleep_badge.set_visible(false);
            }
            AgentStatus::Off => {
                self.sleep_state.set(false);
                self.sleep_switch.set_active(false);
                self.avatar_sleep_badge.set_visible(false);
                self.claw_image.add_css_class("grayscale");
            }
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
        self.set_text("");
        self.history.borrow_mut().reset();
    }

    pub fn set_input_sensitive(&self, sensitive: bool) {
        if !sensitive && (self.entry.has_focus() || self.entry.focus_child().is_some()) {
            // Move focus out of GtkEntry/GtkText before disabling sensitivity so
            // GtkText receives a proper focus-out event and doesn't emit a GTK warning.
            if let Some(root) = self.entry.root() {
                gtk::prelude::RootExt::set_focus(&root, None::<&gtk::Widget>);
            }
        }
        self.entry.set_sensitive(sensitive);
        self.send_btn.set_sensitive(sensitive);
    }

    pub fn update_ui(&self, status: AgentStatus, pinned: bool, web_search: bool) {
        let active = status != AgentStatus::Off || self.character_id.borrow().is_some();
        self.claw_state.set(active);
        self.sleep_toggle.set_sensitive(active);
        self.pin_toggle.set_sensitive(active);
        self.set_status(status);
        self.refresh_user_avatar();

        self.pin_state.set(pinned);
        self.web_search_state.set(web_search);
        self.pin_switch.set_active(pinned);
        self.web_switch.set_active(web_search);

        if pinned {
            self.pin_toggle.set_tooltip_text(Some("Unpin this session"));
        } else {
            self.pin_toggle.set_tooltip_text(Some("Pin this session"));
        }

        if web_search {
            self.web_search_toggle
                .set_tooltip_text(Some("Disable Web Search"));
        } else {
            self.web_search_toggle
                .set_tooltip_text(Some("Enable Web Search"));
        }
    }

    pub fn set_web_search_visible(&self, visible: bool) {
        self.web_search_toggle.set_visible(visible);
    }

    /// Deliberate no-op since the merge-into-drawer refactor. The
    /// msgbar used to track the terminal font for visual continuity
    /// with the shell; now it's chat chrome and inherits the default
    /// app font. Kept to preserve the public API.
    pub fn apply_font(&self, _font_desc: &gtk::pango::FontDescription) {}
}
