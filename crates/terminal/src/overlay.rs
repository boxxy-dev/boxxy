use boxxy_viewer::StructuredViewer;
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayMode {
    Claw,
    Bookmark,
}

#[derive(Clone)]
pub struct TerminalOverlay {
    revealer: gtk::Revealer,
    outer_scroll: gtk::ScrolledWindow,
    title_label: gtk::Label,
    action_label: gtk::Label,
    title_container: gtk::Box,
    diagnosis_viewer: StructuredViewer,
    command_view: gtk::TextView,
    reply_entry: gtk::Entry,
    template_entry: gtk::Entry,
    accept_btn: gtk::Button,
    reject_btn: gtk::Button,
    ok_btn: gtk::Button,
    #[allow(dead_code)]
    reply_btn: gtk::Button,
    icon: gtk::Image,

    // File Write specific widgets
    #[allow(dead_code)]
    reject_file_btn: gtk::Button,
    #[allow(dead_code)]
    approve_file_btn: gtk::Button,
    inspect_btn: gtk::Button,
    command_frame: gtk::Frame,
    chat_box: gtk::Box,
    template_box: gtk::Box,
    file_action_box: gtk::Box,
    action_box: gtk::Box,
    current_proposal: Rc<RefCell<crate::TerminalProposal>>,
    current_mode: Rc<RefCell<OverlayMode>>,
    stored_max_height: Rc<Cell<i32>>,
    clamp: adw::Clamp,
}

impl TerminalOverlay {
    pub fn new<
        F1: Fn(String) + 'static,
        F2: Fn((String, Vec<String>)) + 'static,
        F3: Fn(bool) + 'static,
        F4: Fn(crate::TerminalProposal) + 'static,
        F5: Fn(OverlayMode) + 'static,
        F6: Fn(bool) + 'static,
    >(
        on_accept: F1,
        on_reply: F2,
        on_file_reply: F3,
        on_add_to_sidebar: F4,
        on_cancel: F5,
        on_visibility_changed: F6,
    ) -> Self {
        let revealer = gtk::Revealer::new();

        let on_vis_rc = Rc::new(on_visibility_changed);
        revealer.connect_reveal_child_notify(move |rev| {
            on_vis_rc(rev.reveals_child());
        });
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        revealer.set_halign(gtk::Align::Fill);
        revealer.set_valign(gtk::Align::Center);
        revealer.set_margin_start(20);
        revealer.set_margin_end(20);

        let s = boxxy_preferences::Settings::load();

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.set_size_request(boxxy_preferences::CLAW_WIDTH_BOUNDS.min, -1);
        vbox.set_hexpand(true);

        // propagate_natural_width: the scroll propagates the content's natural
        // width (e.g., a wide code block) up to the AdwClamp, which then caps
        // it at the user's max setting.  AdwClamp also measures height at the
        // actual clamped width (not the allocation width), so height-for-width
        // is always accurate — no scrollbar or clipping due to measurement
        // mismatch.
        let master_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .max_content_height(s.claw_popover_max_height)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .hexpand(true)
            .build();
        master_scroll.set_child(Some(&vbox));

        let inner_overlay = gtk::Overlay::new();
        inner_overlay.set_hexpand(true);
        inner_overlay.set_child(Some(&master_scroll));

        let frame = gtk::Frame::new(None);
        frame.add_css_class("app-notification");
        frame.add_css_class("claw-widget");
        frame.add_css_class("background");
        frame.add_css_class("view");
        frame.set_halign(gtk::Align::Fill);
        frame.set_child(Some(&inner_overlay));

        // AdwClamp constrains the frame's width to at most maximum_size.
        // When content is narrower than the maximum, the child is left at its
        // natural width (centred within the available space).  The cross-axis
        // (height) is always measured at the child's actual clamped width, so
        // there is no height-for-width mismatch.
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(s.claw_popover_width);
        clamp.set_child(Some(&frame));

        revealer.set_child(Some(&clamp));

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::from_icon_name("boxxy-boxxyclaw-symbolic");
        icon.add_css_class("accent");
        header.append(&icon);

        let title_container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        title_container.set_halign(gtk::Align::Start);
        title_container.set_valign(gtk::Align::Center);
        title_container.set_hexpand(true);

        let title_label = gtk::Label::new(Some("Boxxy-Claw"));
        title_label.add_css_class("heading");
        title_label.set_halign(gtk::Align::Start);
        title_label.set_xalign(0.0);
        title_container.append(&title_label);

        let action_label = gtk::Label::new(None);
        action_label.set_halign(gtk::Align::Start);
        action_label.set_xalign(0.0);
        action_label.set_visible(false);
        title_container.append(&action_label);

        header.append(&title_container);

        vbox.append(&header);

        let diagnosis_viewer = StructuredViewer::new(boxxy_claw::ui::get_claw_viewer_registry());
        vbox.append(diagnosis_viewer.widget());

        let command_frame = gtk::Frame::new(None);
        command_frame.add_css_class("view");

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
            .icon_name("boxxy-paper-plane-symbolic")
            .css_classes(["flat"])
            .tooltip_text("Send Reply")
            .build();

        reply_box.append(&reply_entry);
        reply_box.append(&reply_btn);
        vbox.append(&reply_box);

        // Template variables area
        let template_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let template_entry = gtk::Entry::builder()
            .placeholder_text("Variables...")
            .hexpand(true)
            .build();
        template_box.append(&template_entry);
        vbox.append(&template_box);

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
            .icon_name("boxxy-bug-symbolic")
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

        let current_proposal = Rc::new(RefCell::new(crate::TerminalProposal::None));
        let current_mode = Rc::new(RefCell::new(OverlayMode::Claw));

        let p_clone_reject_cmd = revealer.clone();
        let on_cancel_rc = Rc::new(on_cancel);
        let cb_cancel_cmd = on_cancel_rc.clone();
        let cm_clone_reject = current_mode.clone();
        reject_btn.connect_clicked(move |_| {
            cb_cancel_cmd(*cm_clone_reject.borrow());
            p_clone_reject_cmd.set_reveal_child(false);
        });

        let p_clone_ok_cmd = revealer.clone();
        let cb_ok_cmd = on_cancel_rc.clone();
        let cm_clone_ok = current_mode.clone();
        ok_btn.connect_clicked(move |_| {
            cb_ok_cmd(*cm_clone_ok.borrow());
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
        let current_proposal_for_accept = current_proposal.clone();
        let template_entry_clone = template_entry.clone();
        accept_btn.connect_clicked(move |_| {
            let buffer = cmd_view_clone.buffer();
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            let mut cmd = buffer.text(&start, &end, false).to_string();

            if let crate::TerminalProposal::Bookmark(filename, _cmd, placeholders) =
                current_proposal_for_accept.borrow().clone()
            {
                cmd = _cmd; // Use the original full script instead of the truncated preview buffer
                let input_str = template_entry_clone.text().to_string();
                let values: Vec<String> =
                    input_str.split(',').map(|s| s.trim().to_string()).collect();

                for (i, name) in placeholders.iter().enumerate() {
                    if let Some(val) = values.get(i) {
                        let pattern = format!("{{{{{{{}}}}}}}", name);
                        cmd = cmd.replace(&pattern, val);
                    }
                }

                // Ephemeral Execution Files
                if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal")
                {
                    let runs_dir = dirs
                        .config_dir()
                        .join("cache")
                        .join("bookmarks")
                        .join("runs");
                    if !runs_dir.exists() {
                        let _ = std::fs::create_dir_all(&runs_dir);
                    }

                    let uuid = uuid::Uuid::new_v4().to_string();
                    let short_uuid = &uuid[0..6];

                    // Split the extension from the filename
                    let (stem, ext) = if let Some(idx) = filename.rfind('.') {
                        (&filename[..idx], &filename[idx..])
                    } else {
                        (filename.as_str(), "")
                    };

                    let temp_filename = format!("{}-{}{}", stem, short_uuid, ext);
                    let temp_path = runs_dir.join(&temp_filename);

                    if std::fs::write(&temp_path, &cmd).is_ok() {
                        // Make executable
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Ok(mut perms) =
                                std::fs::metadata(&temp_path).map(|m| m.permissions())
                            {
                                perms.set_mode(0o755);
                                let _ = std::fs::set_permissions(&temp_path, perms);
                            }
                        }

                        // We prefix with a space to avoid history pollution
                        cmd = format!(" {}", temp_path.display());
                    }
                }
            }

            on_accept(cmd);
            p_clone2.set_reveal_child(false);
        });

        let p_clone3 = revealer.clone();
        let reply_entry_clone = reply_entry.clone();
        let on_reply = std::rc::Rc::new(on_reply);
        let on_reply_clone = on_reply.clone();
        let cb_cancel_reply = on_cancel_rc.clone();
        let cm_clone_reply = current_mode.clone();

        let do_reply = move || {
            let text = reply_entry_clone.text().to_string();
            if !text.is_empty() {
                on_reply_clone((text, vec![]));
                reply_entry_clone.set_text("");
                p_clone3.set_reveal_child(false);
            } else {
                cb_cancel_reply(*cm_clone_reply.borrow());
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

        let accept_btn_clone = accept_btn.clone();
        template_entry.connect_activate(move |_| {
            if accept_btn_clone.is_visible() && accept_btn_clone.is_sensitive() {
                accept_btn_clone.emit_clicked();
            }
        });

        TerminalOverlay {
            revealer,
            outer_scroll: master_scroll,
            title_label,
            action_label,
            title_container,
            diagnosis_viewer,
            command_view,
            reply_entry,
            template_entry,
            accept_btn,
            reject_btn,
            ok_btn,
            reply_btn,
            icon,
            reject_file_btn,
            approve_file_btn,
            inspect_btn,
            command_frame,
            chat_box: reply_box,
            template_box,
            file_action_box,
            action_box,
            current_proposal,
            current_mode,
            stored_max_height: Rc::new(Cell::new(s.claw_popover_max_height)),
            clamp,
        }
    }

    pub fn update_dimensions(&self, width: i32, max_height: i32) {
        self.clamp.set_maximum_size(width);
        self.stored_max_height.set(max_height);
        // The effective height will be re-capped on the next pane resize event.
        // Update unconditionally so a settings change takes effect immediately
        // even before the next resize.
        self.outer_scroll.set_max_content_height(max_height);
    }

    /// Called whenever the parent pane is resized. Caps the scroll window height
    /// so the popover never overflows the visible terminal area.
    pub fn update_pane_height(&self, pane_height: i32) {
        const V_PAD: i32 = 40; // total top+bottom breathing room
        let effective = (pane_height - V_PAD)
            .max(boxxy_preferences::CLAW_HEIGHT_BOUNDS.min)
            .min(self.stored_max_height.get());
        self.outer_scroll.set_max_content_height(effective);
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.revealer
    }

    pub fn show(
        &self,
        mode: OverlayMode,
        title: &str,
        action: Option<&str>,
        diagnosis: &str,
        proposal: crate::TerminalProposal,
    ) {
        self.title_label.set_label(title);
        self.diagnosis_viewer.set_content(diagnosis);
        self.reply_entry.set_text("");
        self.template_entry.set_text("");
        *self.current_proposal.borrow_mut() = proposal.clone();
        *self.current_mode.borrow_mut() = mode;

        // Reset visibility
        self.command_frame.set_visible(false);
        self.action_box.set_visible(false);
        self.accept_btn.set_visible(false);
        self.reject_btn.set_visible(false);
        self.ok_btn.set_visible(false);
        self.file_action_box.set_visible(false);
        self.chat_box.set_visible(mode == OverlayMode::Claw);
        self.template_box.set_visible(false);
        self.inspect_btn.set_visible(mode == OverlayMode::Claw);

        match mode {
            OverlayMode::Claw => {
                self.icon.set_visible(false);
                self.title_label.remove_css_class("heading");

                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                title.hash(&mut hasher);
                let hash = hasher.finish();

                let r = (hash & 0xFF) as u8 % 150 + 50;
                let g = ((hash >> 8) & 0xFF) as u8 % 150 + 50;
                let b = ((hash >> 16) & 0xFF) as u8 % 150 + 50;
                let color = format!("rgb({}, {}, {})", r, g, b);

                let css = format!(
                    ".overlay-badge {{ background-color: {}; color: white; border-radius: 12px; padding: 4px 10px; font-weight: bold; font-size: 0.8rem; box-shadow: 0 2px 4px rgba(0,0,0,0.2); }}",
                    color
                );
                let provider = gtk::CssProvider::new();
                #[allow(deprecated)]
                provider.load_from_string(&css);
                #[allow(deprecated)]
                self.title_label
                    .style_context()
                    .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                self.title_label.add_css_class("overlay-badge");
                
                // Clear background from container so it doesn't wrap both
                self.title_container.remove_css_class("overlay-badge");

                if let Some(act) = action {
                    self.action_label.set_label(act);
                    self.action_label.set_visible(true);
                    
                    let action_css = ".action-badge { background-color: rgba(255, 255, 255, 0.1); color: @window_fg_color; border: 1px solid rgba(255,255,255,0.1); border-radius: 12px; padding: 4px 10px; font-weight: bold; font-size: 0.8rem; margin-left: 6px; }";
                    let action_provider = gtk::CssProvider::new();
                    #[allow(deprecated)]
                    action_provider.load_from_string(&action_css);
                    #[allow(deprecated)]
                    self.action_label
                        .style_context()
                        .add_provider(&action_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                    
                    self.action_label.add_css_class("action-badge");
                } else {
                    self.action_label.set_visible(false);
                }
            }
            OverlayMode::Bookmark => {
                self.icon.set_visible(true);
                self.icon
                    .set_icon_name(Some("boxxy-user-bookmarks-symbolic"));
                self.title_label.add_css_class("heading");
                self.title_label.remove_css_class("overlay-badge");
                self.title_container.remove_css_class("overlay-badge");
                self.action_label.set_visible(false);
            }
        }

        match proposal {
            crate::TerminalProposal::Command(cmd) => {
                self.command_view.buffer().set_text(&cmd);
                self.command_view.set_editable(mode == OverlayMode::Claw);
                self.command_frame.set_visible(true);
                self.action_box.set_visible(true);
                self.accept_btn.set_visible(true);
                self.reject_btn.set_visible(true);
            }
            crate::TerminalProposal::Bookmark(_filename, cmd, placeholders) => {
                let mut display_cmd = cmd.lines().take(15).collect::<Vec<_>>().join("\n");
                if cmd.lines().count() > 15 {
                    display_cmd.push_str("\n\n... (truncated for preview)");
                }
                self.command_view.buffer().set_text(&display_cmd);
                self.command_view.set_editable(false);
                self.command_frame.set_visible(true);
                self.action_box.set_visible(true);
                self.accept_btn.set_visible(true);
                self.reject_btn.set_visible(true);
                self.template_box.set_visible(true);
                self.template_entry
                    .set_placeholder_text(Some(&placeholders.join(", ")));
            }
            crate::TerminalProposal::FileWrite(_path, _content) => {
                self.file_action_box.set_visible(true);
            }
            crate::TerminalProposal::FileDelete(_path) => {
                self.file_action_box.set_visible(true);
            }
            crate::TerminalProposal::KillProcess(_pid, _name) => {
                self.file_action_box.set_visible(true);
            }
            crate::TerminalProposal::GetClipboard => {
                self.file_action_box.set_visible(true);
            }
            crate::TerminalProposal::SetClipboard(_text) => {
                self.file_action_box.set_visible(true);
            }
            crate::TerminalProposal::None => {
                self.action_box.set_visible(true);
                self.ok_btn.set_visible(true);
            }
        }

        self.revealer.set_reveal_child(true);

        let ok_btn = self.ok_btn.clone();
        let accept_btn = self.accept_btn.clone();
        let approve_file_btn = self.approve_file_btn.clone();
        let template_box = self.template_box.clone();
        let template_entry = self.template_entry.clone();
        let reply_entry = self.reply_entry.clone();

        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            if ok_btn.is_visible() {
                ok_btn.grab_focus();
            } else if accept_btn.is_visible() {
                accept_btn.grab_focus();
            } else if approve_file_btn.is_visible() {
                approve_file_btn.grab_focus();
            } else if template_box.is_visible() {
                template_entry.grab_focus();
            } else if mode == OverlayMode::Claw {
                reply_entry.grab_focus();
            }
            gtk4::glib::ControlFlow::Break
        });
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
    }

    pub fn grab_reply_focus(&self) {
        if *self.current_mode.borrow() == OverlayMode::Claw {
            self.reply_entry.grab_focus();
        } else if self.template_box.is_visible() {
            self.template_entry.grab_focus();
        }
    }

    pub fn is_visible(&self) -> bool {
        self.revealer.reveals_child()
    }
}
