use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;
use std::cell::RefCell;

#[derive(Clone)]
#[allow(dead_code)]
pub struct ClawPopover {
    revealer: gtk::Revealer,
    title_label: gtk::Label,
    diagnosis_label: gtk::Label,
    command_view: gtk::TextView,
    reply_entry: gtk::Entry,
    accept_btn: gtk::Button,
    reject_btn: gtk::Button,
    ok_btn: gtk::Button,
    reply_btn: gtk::Button,
    
    // File Write specific widgets
    reject_file_btn: gtk::Button,
    approve_file_btn: gtk::Button,
    inspect_btn: gtk::Button,
    command_frame: gtk::Frame,
    chat_box: gtk::Box,
    file_action_box: gtk::Box,
    action_box: gtk::Box,
    current_proposal: Rc<RefCell<crate::ClawProposal>>,
}

impl ClawPopover {
    pub fn new<F1: Fn(String) + 'static, F2: Fn(String) + 'static, F3: Fn(bool) + 'static, F4: Fn(crate::ClawProposal) + 'static, F5: Fn() + 'static>(
        on_accept: F1, 
        on_reply: F2,
        on_file_reply: F3,
        on_add_to_sidebar: F4,
        on_cancel: F5,
    ) -> Self {
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        revealer.set_halign(gtk::Align::Center);
        revealer.set_valign(gtk::Align::Center); // Center it in the terminal

        let frame = gtk::Frame::new(None);
        frame.add_css_class("app-notification"); // Use Adwaita style for floating notifications
        frame.add_css_class("claw-widget");
        frame.add_css_class("background");
        frame.add_css_class("view");
        
        let overlay = gtk::Overlay::new();
        frame.set_child(Some(&overlay));

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.set_width_request(450);
        overlay.set_child(Some(&vbox));

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::from_icon_name("boxxyclaw");
        icon.add_css_class("accent");
        header.append(&icon);
        
        let title_label = gtk::Label::new(Some("Boxxy-Claw"));
        title_label.add_css_class("heading");
        title_label.set_halign(gtk::Align::Start);
        title_label.set_hexpand(true);
        header.append(&title_label);
        
        vbox.append(&header);

        let diag_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .max_content_height(400)
            .propagate_natural_height(true)
            .build();

        let diagnosis_label = gtk::Label::new(None);
        diagnosis_label.set_wrap(true);
        diagnosis_label.set_halign(gtk::Align::Start);
        diagnosis_label.set_selectable(true);
        diag_scroll.set_child(Some(&diagnosis_label));
        vbox.append(&diag_scroll);

        let command_frame = gtk::Frame::new(None);
        command_frame.add_css_class("view");
        
        // Use a text view to allow editing the suggested command
        let command_view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .editable(true)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .css_classes(["monospace"])
            .build();

        command_frame.set_child(Some(&command_view));
        vbox.append(&command_frame);

        // Reply area
        let reply_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);

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
        vbox.append(&reply_box);

        let file_action_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        file_action_box.set_halign(gtk::Align::End);
        
        let reject_file_btn = gtk::Button::with_label("Reject");
        reject_file_btn.add_css_class("destructive-action");
        file_action_box.append(&reject_file_btn);

        let approve_file_btn = gtk::Button::with_label("Approve & Write");
        approve_file_btn.add_css_class("suggested-action");
        file_action_box.append(&approve_file_btn);

        vbox.append(&file_action_box);

        let action_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        action_box.set_halign(gtk::Align::End);

        let inspect_btn = gtk::Button::builder()
            .icon_name("bug-symbolic")
            .css_classes(["flat"])
            .tooltip_text("Open in Sidebar")
            .build();
        action_box.append(&inspect_btn);
        
        let reject_btn = gtk::Button::with_label("Reject");
        reject_btn.add_css_class("destructive-action");
        action_box.append(&reject_btn);
        
        let ok_btn = gtk::Button::with_label("Okay");
        action_box.append(&ok_btn);
        
        let accept_btn = gtk::Button::with_label("Accept & Run");
        accept_btn.add_css_class("suggested-action");
        action_box.append(&accept_btn);

        vbox.append(&action_box);

        frame.set_child(Some(&vbox));
        revealer.set_child(Some(&frame));

        let current_proposal = Rc::new(RefCell::new(crate::ClawProposal::None));

        let p_clone_reject_cmd = revealer.clone();
        let on_cancel_rc = Rc::new(on_cancel);
        let cb_cancel_cmd = on_cancel_rc.clone();
        reject_btn.connect_clicked(move |_| {
            cb_cancel_cmd();
            p_clone_reject_cmd.set_reveal_child(false);
        });

        let p_clone_ok_cmd = revealer.clone();
        let cb_ok_cmd = on_cancel_rc.clone();
        ok_btn.connect_clicked(move |_| {
            cb_ok_cmd();
            p_clone_ok_cmd.set_reveal_child(false);
        });

        let p_clone_approve = revealer.clone();
        let on_file_reply_rc = std::rc::Rc::new(on_file_reply);
        let cb_approve = on_file_reply_rc.clone();
        approve_file_btn.connect_clicked(move |_| {
            cb_approve(true);
            p_clone_approve.set_reveal_child(false);
        });

        let p_clone_reject = revealer.clone();
        let cb_reject = on_file_reply_rc.clone();
        reject_file_btn.connect_clicked(move |_| {
            cb_reject(false);
            p_clone_reject.set_reveal_child(false);
        });

        let cp_clone2 = current_proposal.clone();
        let on_add_to_sidebar_rc = std::rc::Rc::new(on_add_to_sidebar);
        let cb_sidebar2 = on_add_to_sidebar_rc.clone();
        inspect_btn.connect_clicked(move |_| {
            let proposal = cp_clone2.borrow().clone();
            cb_sidebar2(proposal);
        });

        let p_clone2 = revealer.clone();
        let cmd_view_clone = command_view.clone();
        accept_btn.connect_clicked(move |_| {
            let buffer = cmd_view_clone.buffer();
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            let cmd = buffer.text(&start, &end, false).to_string();
            on_accept(cmd);
            p_clone2.set_reveal_child(false);
        });

        let p_clone3 = revealer.clone();
        let reply_entry_clone = reply_entry.clone();
        let on_reply = std::rc::Rc::new(on_reply);
        let on_reply_clone = on_reply.clone();
        
        let do_reply = move || {
            let text = reply_entry_clone.text().to_string();
            if !text.is_empty() {
                on_reply_clone(text);
                reply_entry_clone.set_text("");
                p_clone3.set_reveal_child(false);
            }
        };

        let do_reply_clone = do_reply.clone();
        reply_btn.connect_clicked(move |_| {
            do_reply_clone();
        });

        reply_entry.connect_activate(move |_| {
            do_reply();
        });

        Self {
            revealer,
            title_label,
            diagnosis_label,
            command_view,
            reply_entry,
            accept_btn,
            reject_btn,
            ok_btn,
            reply_btn,
            reject_file_btn,
            approve_file_btn,
            inspect_btn,
            command_frame,
            chat_box: reply_box,
            file_action_box,
            action_box,
            current_proposal,
        }
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.revealer
    }

    pub fn show(&self, title: &str, diagnosis: &str, proposal: crate::ClawProposal) {
        self.title_label.set_label(title);
        self.diagnosis_label.set_label(diagnosis);
        self.reply_entry.set_text("");
        *self.current_proposal.borrow_mut() = proposal.clone();

        // Reset visibility
        self.command_frame.set_visible(false);
        self.action_box.set_visible(false);
        self.accept_btn.set_visible(false);
        self.reject_btn.set_visible(false);
        self.ok_btn.set_visible(false);
        self.file_action_box.set_visible(false);
        self.chat_box.set_visible(true); // Always show chat by default (contains debug icon)

        match proposal {
            crate::ClawProposal::Command(cmd) => {
                self.command_view.buffer().set_text(&cmd);
                self.command_frame.set_visible(true);
                self.action_box.set_visible(true);
                self.accept_btn.set_visible(true);
                self.reject_btn.set_visible(true);
            }
            crate::ClawProposal::FileWrite(_path, _content) => {
                // For file writes, we hide the normal action box and show the file action box
                self.file_action_box.set_visible(true);
            }
            crate::ClawProposal::None => {
                // Just chat mode
                self.action_box.set_visible(true);
                self.ok_btn.set_visible(true);
            }
        }

        self.revealer.set_reveal_child(true);
        self.reply_entry.grab_focus();
    }
    
    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
    }

    pub fn grab_reply_focus(&self) {
        self.reply_entry.grab_focus();
    }

    pub fn is_visible(&self) -> bool {
        self.revealer.reveals_child()
    }
}
