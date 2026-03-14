use gtk4 as gtk;
use libadwaita as adw;
use gtk::prelude::*;
use std::rc::Rc;
use std::cell::Cell;

pub struct ClawSidebarComponent {
    widget: gtk::Box,
    status_page: adw::StatusPage,
    scroll: gtk::ScrolledWindow,
    is_active: Rc<Cell<bool>>,
    mode_toggle_btn: gtk::Button,
    chat_mode_btn: gtk::Button,
    toggle_btn: gtk::Button,
    current_list: Rc<std::cell::RefCell<Option<gtk::ListBox>>>,
}

impl ClawSidebarComponent {
    #[must_use]
    pub fn new<F: Fn(bool) + 'static>(on_active_toggled: F) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        widget.set_margin_top(6);
        widget.set_margin_bottom(6);
        widget.set_margin_start(6);
        widget.set_margin_end(6);

        let status_page = adw::StatusPage::builder()
            .title("Claw Mode")
            .description("System-eccentric agentic control.")
            .icon_name("boxxyclaw")
            .vexpand(true)
            .build();

        widget.append(&status_page);

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .visible(false)
            .build();
        
        widget.append(&scroll);

        let is_active = Rc::new(Cell::new(false));
        let current_list: Rc<std::cell::RefCell<Option<gtk::ListBox>>> = Rc::new(std::cell::RefCell::new(None));

        // Command panel
        let command_panel = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        command_panel.set_halign(gtk::Align::Center);

        // 1. Clear Button
        let clear_btn = gtk::Button::builder()
            .icon_name("edit-clear-all-symbolic")
            .css_classes(["flat"])
            .tooltip_text("Clear History")
            .build();

        let current_list_clear = current_list.clone();
        let status_page_clone = status_page.clone();
        let scroll_clone = scroll.clone();
        clear_btn.connect_clicked(move |_| {
            if let Some(list) = current_list_clear.borrow().as_ref() {
                while let Some(row) = list.row_at_index(0) {
                    list.remove(&row);
                }
            }
            status_page_clone.set_visible(true);
            scroll_clone.set_visible(false);
        });

        // 2. Chat Mode Button
        let s = boxxy_preferences::Settings::load();
        let initial_icon = if s.claw_terminal_suggestions {
            "chat-symbolic"
        } else {
            "chat-none-symbolic"
        };
        
        let mut chat_mode_builder = gtk::Button::builder()
            .icon_name(initial_icon)
            .css_classes(["flat"]);
            
        if s.claw_terminal_suggestions {
            chat_mode_builder = chat_mode_builder.css_classes(["flat", "suggested-action"])
                .tooltip_text("Disable Terminal Suggestions");
        } else {
            chat_mode_builder = chat_mode_builder.css_classes(["flat", "destructive-action"])
                .tooltip_text("Enable Terminal Suggestions");
        }
            
        let chat_mode_btn = chat_mode_builder.build();

        let chat_btn_clone = chat_mode_btn.clone();
        chat_mode_btn.connect_clicked(move |_| {
            let mut s = boxxy_preferences::Settings::load();
            s.claw_terminal_suggestions = !s.claw_terminal_suggestions;
            
            if s.claw_terminal_suggestions {
                chat_btn_clone.set_icon_name("chat-symbolic");
                chat_btn_clone.set_tooltip_text(Some("Disable Terminal Suggestions"));
                chat_btn_clone.remove_css_class("destructive-action");
                chat_btn_clone.add_css_class("suggested-action");
            } else {
                chat_btn_clone.set_icon_name("chat-none-symbolic");
                chat_btn_clone.set_tooltip_text(Some("Enable Terminal Suggestions"));
                chat_btn_clone.remove_css_class("suggested-action");
                chat_btn_clone.add_css_class("destructive-action");
            }
            s.save();
        });

        // 3. Play/Stop Button
        let toggle_btn = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .css_classes(["flat", "suggested-action"])
            .tooltip_text("Activate Claw")
            .build();
            
        let active_clone = is_active.clone();
        let btn_clone = toggle_btn.clone();
        let on_toggled_rc = std::rc::Rc::new(on_active_toggled);
        let on_toggled_clone = on_toggled_rc.clone();
        toggle_btn.connect_clicked(move |_| {
            let active = !active_clone.get();
            active_clone.set(active);
            if active {
                btn_clone.set_icon_name("media-playback-stop-symbolic");
                btn_clone.set_tooltip_text(Some("Deactivate Claw"));
                btn_clone.remove_css_class("suggested-action");
                btn_clone.add_css_class("destructive-action");
            } else {
                btn_clone.set_icon_name("media-playback-start-symbolic");
                btn_clone.set_tooltip_text(Some("Activate Claw"));
                btn_clone.remove_css_class("destructive-action");
                btn_clone.add_css_class("suggested-action");
            }
            on_toggled_clone(active);
        });

        let mode_toggle_btn = gtk::Button::builder()
            .icon_name("running-symbolic")
            .css_classes(["flat", "accent"])
            .tooltip_text("Proactive Diagnosis Mode")
            .build();
            
        let mode_toggle_btn_clone = mode_toggle_btn.clone();
        mode_toggle_btn.connect_clicked(move |_| {
            let mut s = boxxy_preferences::Settings::load();
            if s.claw_auto_diagnosis_mode == boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy {
                s.claw_auto_diagnosis_mode = boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive;
                mode_toggle_btn_clone.set_icon_name("running-symbolic");
                mode_toggle_btn_clone.set_tooltip_text(Some("Proactive Diagnosis Mode"));
                mode_toggle_btn_clone.add_css_class("accent");
            } else {
                s.claw_auto_diagnosis_mode = boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy;
                mode_toggle_btn_clone.set_icon_name("walking2-symbolic");
                mode_toggle_btn_clone.set_tooltip_text(Some("Lazy Diagnosis Mode"));
                mode_toggle_btn_clone.remove_css_class("accent");
            }
            s.save();
        });
        
        command_panel.append(&clear_btn);
        command_panel.append(&chat_mode_btn);
        command_panel.append(&mode_toggle_btn);
        command_panel.append(&toggle_btn);
        widget.append(&command_panel);

        Self {
            widget,
            status_page,
            scroll,
            is_active,
            mode_toggle_btn: mode_toggle_btn.clone(),
            chat_mode_btn: chat_mode_btn.clone(),
            toggle_btn: toggle_btn.clone(),
            current_list,
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.is_active.get()
    }

    pub fn set_history_widget(&self, list: &gtk::ListBox) {
        if let Some(old) = self.current_list.borrow().as_ref()
            && old == list {
                self.refresh_visibility();
                return;
        }
        
        if list.parent().is_some() {
            list.unparent();
        }

        self.scroll.set_child(Some(list));
        *self.current_list.borrow_mut() = Some(list.clone());
        self.refresh_visibility();
    }

    pub fn refresh_visibility(&self) {
        if let Some(list) = self.current_list.borrow().as_ref() {
            let has_items = list.row_at_index(0).is_some();
            self.status_page.set_visible(!has_items);
            self.scroll.set_visible(has_items);
        }
    }

    pub fn scroll_to_bottom(&self) {
        let adj = self.scroll.vadjustment();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
            adj.set_value(adj.upper() - adj.page_size());
        });
    }

    pub fn update_diagnosis_mode(&self, mode: &boxxy_preferences::config::ClawAutoDiagnosisMode) {
        match mode {
            boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive => {
                self.mode_toggle_btn.set_icon_name("running-symbolic");
                self.mode_toggle_btn.set_tooltip_text(Some("Proactive Diagnosis Mode"));
                self.mode_toggle_btn.add_css_class("accent");
            }
            boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy => {
                self.mode_toggle_btn.set_icon_name("walking2-symbolic");
                self.mode_toggle_btn.set_tooltip_text(Some("Lazy Diagnosis Mode"));
                self.mode_toggle_btn.remove_css_class("accent");
            }
        }
    }

    pub fn update_terminal_suggestions(&self, enabled: bool) {
        if enabled {
            self.chat_mode_btn.set_icon_name("chat-symbolic");
            self.chat_mode_btn.set_tooltip_text(Some("Disable Terminal Suggestions"));
            self.chat_mode_btn.remove_css_class("destructive-action");
            self.chat_mode_btn.add_css_class("suggested-action");
        } else {
            self.chat_mode_btn.set_icon_name("chat-none-symbolic");
            self.chat_mode_btn.set_tooltip_text(Some("Enable Terminal Suggestions"));
            self.chat_mode_btn.remove_css_class("suggested-action");
            self.chat_mode_btn.add_css_class("destructive-action");
        }
    }

    pub fn update_active(&self, active: bool) {
        self.is_active.set(active);
        if active {
            self.toggle_btn.set_icon_name("media-playback-stop-symbolic");
            self.toggle_btn.set_tooltip_text(Some("Deactivate Claw"));
            self.toggle_btn.remove_css_class("suggested-action");
            self.toggle_btn.add_css_class("destructive-action");
        } else {
            self.toggle_btn.set_icon_name("media-playback-start-symbolic");
            self.toggle_btn.set_tooltip_text(Some("Activate Claw"));
            self.toggle_btn.remove_css_class("destructive-action");
            self.toggle_btn.add_css_class("suggested-action");
        }
    }

    #[must_use]
    pub const fn widget(&self) -> &gtk::Box {
        &self.widget
    }
}

impl Default for ClawSidebarComponent {
    fn default() -> Self {
        Self::new(|_| {})
    }
}

pub fn create_claw_message_list() -> gtk::ListBox {
    gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build()
}

pub fn add_diagnosis_row(list: &gtk::ListBox, pane_id: String, diagnosis: &str) {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(8);
    vbox.set_margin_start(8);
    vbox.set_margin_end(8);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name("boxxyclaw");
    icon.add_css_class("accent");
    header.append(&icon);
    
    let title = gtk::Label::new(Some("Diagnosis"));
    title.add_css_class("heading");
    title.set_halign(gtk::Align::Start);
    header.append(&title);
    
    let pane_lbl = gtk::Label::new(Some(&format!("Pane {}", if pane_id.len() >= 7 { &pane_id[..7] } else { &pane_id })));
    pane_lbl.add_css_class("caption");
    pane_lbl.add_css_class("dim-label");
    header.append(&pane_lbl);
    
    vbox.append(&header);

    let text_view = gtk::TextView::builder()
        .editable(false)
        .wrap_mode(gtk::WrapMode::Word)
        .cursor_visible(false)
        .css_classes(["claw-diagnosis"])
        .build();
    text_view.buffer().set_text(diagnosis);
    
    vbox.append(&text_view);
    row.set_child(Some(&vbox));

    list.append(&row);
}

pub fn add_approval_row(
    list: &gtk::ListBox, 
    pane_id: String, 
    command: &str,
    on_text_reply: impl Fn(String) + 'static,
) -> gtk::Box {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(8);
    vbox.set_margin_start(8);
    vbox.set_margin_end(8);
    
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    icon.add_css_class("warning");
    header.append(&icon);
    
    let title = gtk::Label::new(Some("Suggested Action"));
    title.add_css_class("heading");
    title.set_halign(gtk::Align::Start);
    header.append(&title);
    
    let pane_lbl = gtk::Label::new(Some(&format!("Pane {}", if pane_id.len() >= 7 { &pane_id[..7] } else { &pane_id })));
    pane_lbl.add_css_class("caption");
    pane_lbl.add_css_class("dim-label");
    header.append(&pane_lbl);
    
    vbox.append(&header);

    let cmd_label = gtk::Label::new(Some(command));
    cmd_label.set_halign(gtk::Align::Start);
    cmd_label.set_wrap(true);
    cmd_label.set_selectable(true);
    cmd_label.add_css_class("monospace");
    cmd_label.add_css_class("dim-label");
    vbox.append(&cmd_label);

    let action_container = gtk::Box::new(gtk::Orientation::Vertical, 6);

    let reply_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    reply_box.set_margin_top(4);
    
    let reply_entry = gtk::Entry::builder()
        .placeholder_text("Reply to Boxxy-Claw...")
        .hexpand(true)
        .build();
    
    let reply_btn = gtk::Button::builder()
        .icon_name("paper-plane-symbolic")
        .css_classes(["flat"])
        .tooltip_text("Send Reply")
        .build();
        
    reply_box.append(&reply_entry);
    reply_box.append(&reply_btn);
    action_container.append(&reply_box);

    let help_label = gtk::Label::new(Some("Press Enter in the terminal to execute, or Ctrl+C to cancel."));
    help_label.set_halign(gtk::Align::Start);
    help_label.set_wrap(true);
    help_label.add_css_class("caption");
    help_label.add_css_class("success");
    action_container.append(&help_label);
    
    vbox.append(&action_container);

    let on_text_reply_rc = std::rc::Rc::new(on_text_reply);
    let on_text_reply_clone1 = on_text_reply_rc.clone();
    let entry_clone1 = reply_entry.clone();
    let action_container_clone = action_container.clone();
    
    let do_reply = move || {
        let text = entry_clone1.text().to_string();
        if !text.is_empty() {
            on_text_reply_clone1(text);
            action_container_clone.set_visible(false);
        }
    };

    let do_reply_clone = do_reply.clone();
    reply_btn.connect_clicked(move |_| {
        do_reply_clone();
    });

    reply_entry.connect_activate(move |_| {
        do_reply();
    });

    row.set_child(Some(&vbox));
    list.append(&row);

    action_container
}

pub fn add_file_write_approval_row(
    list: &gtk::ListBox,
    pane_id: String,
    path: &str,
    content: &str,
    on_reply: impl Fn(bool) + 'static,
    on_text_reply: impl Fn(String) + 'static,
) -> gtk::Box {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(8);
    vbox.set_margin_start(8);
    vbox.set_margin_end(8);
    
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name("document-edit-symbolic");
    icon.add_css_class("accent");
    header.append(&icon);
    
    let title = gtk::Label::new(Some("Propose File Edit"));
    title.add_css_class("heading");
    title.set_halign(gtk::Align::Start);
    header.append(&title);
    
    let pane_lbl = gtk::Label::new(Some(&format!("Pane {}", if pane_id.len() >= 7 { &pane_id[..7] } else { &pane_id })));
    pane_lbl.add_css_class("caption");
    pane_lbl.add_css_class("dim-label");
    header.append(&pane_lbl);
    
    vbox.append(&header);

    let path_label = gtk::Label::new(Some(path));
    path_label.set_halign(gtk::Align::Start);
    path_label.set_wrap(true);
    path_label.add_css_class("monospace");
    vbox.append(&path_label);

    let preview = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    preview.buffer().set_text(content);
    
    let scroll_view = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(100)
        .max_content_height(300)
        .child(&preview)
        .build();
    vbox.append(&scroll_view);

    let action_container = gtk::Box::new(gtk::Orientation::Vertical, 6);

    let reply_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    reply_box.set_margin_top(4);
    reply_box.set_margin_bottom(4);
    
    let reply_entry = gtk::Entry::builder()
        .placeholder_text("Reply to Boxxy-Claw...")
        .hexpand(true)
        .build();
    
    let reply_btn = gtk::Button::builder()
        .icon_name("paper-plane-symbolic")
        .css_classes(["flat"])
        .tooltip_text("Send Reply")
        .build();
        
    reply_box.append(&reply_entry);
    reply_box.append(&reply_btn);
    action_container.append(&reply_box);

    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    btn_box.set_halign(gtk::Align::End);

    let reject_btn = gtk::Button::builder()
        .label("Reject")
        .css_classes(["destructive-action"])
        .build();
        
    let approve_btn = gtk::Button::builder()
        .label("Approve & Write")
        .css_classes(["suggested-action"])
        .build();

    btn_box.append(&reject_btn);
    btn_box.append(&approve_btn);
    action_container.append(&btn_box);
    vbox.append(&action_container);

    let on_reply_rc = std::rc::Rc::new(on_reply);
    let cb_approve = on_reply_rc.clone();
    let action_container_clone1 = action_container.clone();
    approve_btn.connect_clicked(move |_| {
        cb_approve(true);
        action_container_clone1.set_visible(false);
    });

    let cb_reject = on_reply_rc.clone();
    let action_container_clone2 = action_container.clone();
    reject_btn.connect_clicked(move |_| {
        cb_reject(false);
        action_container_clone2.set_visible(false);
    });

    let on_text_reply_rc = std::rc::Rc::new(on_text_reply);
    let on_text_reply_clone1 = on_text_reply_rc.clone();
    let entry_clone1 = reply_entry.clone();
    let action_container_clone3 = action_container.clone();
    
    let do_reply = move || {
        let text = entry_clone1.text().to_string();
        if !text.is_empty() {
            on_text_reply_clone1(text);
            action_container_clone3.set_visible(false);
        }
    };

    let do_reply_clone = do_reply.clone();
    reply_btn.connect_clicked(move |_| {
        do_reply_clone();
    });

    reply_entry.connect_activate(move |_| {
        do_reply();
    });

    row.set_child(Some(&vbox));
    list.append(&row);
    
    action_container
}
