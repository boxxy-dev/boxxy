use boxxy_viewer::{BlockRenderer, ContentBlock, StructuredViewer, ViewerRegistry};
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

pub struct ProcessListRenderer;

impl BlockRenderer for ProcessListRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Custom { schema, .. } if schema == "list_processes")
    }

    fn render(&self, block: &ContentBlock) -> gtk::Widget {
        if let ContentBlock::Custom { raw_payload, .. } = block {
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            if let Ok(processes) =
                serde_json::from_str::<Vec<(u32, String, f64, u64, u64, u64)>>(raw_payload)
            {
                let list_box = gtk::ListBox::new();
                list_box.add_css_class("boxed-list");
                list_box.set_selection_mode(gtk::SelectionMode::None);

                for (pid, name, cpu, mem, read, write) in processes {
                    let item_row = gtk::ListBoxRow::new();
                    let item_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    item_box.set_margin_top(4);
                    item_box.set_margin_bottom(4);
                    item_box.set_margin_start(4);
                    item_box.set_margin_end(4);

                    let pid_lbl = gtk::Label::new(Some(&format!("{pid}")));
                    pid_lbl.set_width_chars(6);
                    item_box.append(&pid_lbl);

                    let name_lbl = gtk::Label::new(Some(&name));
                    name_lbl.set_hexpand(true);
                    name_lbl.set_halign(gtk::Align::Start);
                    name_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    item_box.append(&name_lbl);

                    // Only show disk I/O if there is any activity to avoid clutter
                    if read > 0 || write > 0 {
                        let io_str = format!("R:{}MB W:{}MB", read / 1_048_576, write / 1_048_576);
                        let io_lbl = gtk::Label::new(Some(&io_str));
                        io_lbl.add_css_class("caption");
                        io_lbl.add_css_class("dim-label");
                        io_lbl.set_margin_end(6);
                        item_box.append(&io_lbl);
                    }

                    let cpu_lbl = gtk::Label::new(Some(&format!("{cpu:.1}%")));
                    cpu_lbl.add_css_class("caption");
                    cpu_lbl.add_css_class("dim-label");
                    item_box.append(&cpu_lbl);

                    let mem_mb = mem / (1024 * 1024);
                    let mem_lbl = gtk::Label::new(Some(&format!("{mem_mb}MB")));
                    mem_lbl.add_css_class("caption");
                    mem_lbl.add_css_class("dim-label");
                    item_box.append(&mem_lbl);

                    item_row.set_child(Some(&item_box));
                    list_box.append(&item_row);
                }
                vbox.append(&list_box);
            } else {
                let error_lbl = gtk::Label::new(Some("Failed to parse process list."));
                vbox.append(&error_lbl);
            }
            vbox.upcast()
        } else {
            unreachable!()
        }
    }
}

pub fn get_claw_viewer_registry() -> Rc<ViewerRegistry> {
    let mut registry = ViewerRegistry::new_with_defaults();
    registry.register(Box::new(ProcessListRenderer));
    Rc::new(registry)
}

pub struct ClawSidebarComponent {
    widget: gtk::Box,
    status_page: adw::StatusPage,
    scroll: gtk::ScrolledWindow,
    is_active: Rc<Cell<bool>>,
    is_proactive: Rc<Cell<bool>>,
    mode_toggle_btn: gtk::Button,
    toggle_btn: gtk::Button,
    current_list: Rc<std::cell::RefCell<Option<gtk::ListBox>>>,
}

impl ClawSidebarComponent {
    #[must_use]
    pub fn new<F1: Fn(bool) + 'static, F2: Fn(bool) + 'static>(
        on_active_toggled: F1,
        on_proactive_toggled: F2,
    ) -> Self {
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
        let is_proactive = Rc::new(Cell::new(false));

        let current_list: Rc<std::cell::RefCell<Option<gtk::ListBox>>> =
            Rc::new(std::cell::RefCell::new(None));

        // Command panel
        let command_panel = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        command_panel.set_halign(gtk::Align::Center);

        // 1. Clear Button
        let clear_btn = gtk::Button::builder()
            .icon_name("boxxy-edit-clear-all-symbolic")
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

        // 2. Play/Stop Button
        let toggle_btn = gtk::Button::builder()
            .icon_name("boxxy-media-playback-start-symbolic")
            .css_classes(["flat", "suggested-action"])
            .tooltip_text("Activate Claw")
            .build();

        let is_active_clone = is_active.clone();
        let on_toggled_rc = std::rc::Rc::new(on_active_toggled);
        toggle_btn.connect_clicked(move |_| {
            let next_state = !is_active_clone.get();
            on_toggled_rc(next_state);
        });

        // 3. Proactive Mode Button
        let mode_toggle_btn = gtk::Button::builder()
            .icon_name("boxxy-walking2-symbolic")
            .css_classes(["flat"])
            .tooltip_text("Lazy Diagnosis Mode")
            .build();

        let is_proactive_clone = is_proactive.clone();
        let on_proactive_rc = std::rc::Rc::new(on_proactive_toggled);
        mode_toggle_btn.connect_clicked(move |_| {
            let next_state = !is_proactive_clone.get();
            on_proactive_rc(next_state);
        });

        command_panel.append(&clear_btn);
        command_panel.append(&mode_toggle_btn);
        command_panel.append(&toggle_btn);
        widget.append(&command_panel);

        Self {
            widget,
            status_page,
            scroll,
            is_active,
            is_proactive,
            mode_toggle_btn,
            toggle_btn,
            current_list,
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.is_active.get()
    }

    pub fn set_history_widget(&self, list: &gtk::ListBox) {
        if let Some(old) = self.current_list.borrow().as_ref()
            && old == list
        {
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
        gtk::glib::idle_add_local_once(move || {
            let value = adj.value();
            let upper = adj.upper();
            let page_size = adj.page_size();

            // If we are close to the bottom (within 100 pixels), keep scrolling
            if value > upper - page_size - 100.0 || value < 1.0 {
                adj.set_value(upper - page_size);
            }
        });
    }

    pub fn update_diagnosis_mode(&self, mode: &boxxy_preferences::config::ClawAutoDiagnosisMode) {
        match mode {
            boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive => {
                self.mode_toggle_btn.set_icon_name("boxxy-running-symbolic");
                self.mode_toggle_btn
                    .set_tooltip_text(Some("Proactive Diagnosis Mode"));
                self.mode_toggle_btn.add_css_class("accent");
            }
            boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy => {
                self.mode_toggle_btn.set_icon_name("boxxy-walking2-symbolic");
                self.mode_toggle_btn
                    .set_tooltip_text(Some("Lazy Diagnosis Mode"));
                self.mode_toggle_btn.remove_css_class("accent");
            }
        }
    }

    pub fn update_active(&self, active: bool) {
        self.is_active.set(active);
        if active {
            self.toggle_btn
                .set_icon_name("boxxy-media-playback-stop-symbolic");
            self.toggle_btn.set_tooltip_text(Some("Deactivate Claw"));
            self.toggle_btn.remove_css_class("suggested-action");
            self.toggle_btn.add_css_class("destructive-action");
        } else {
            self.toggle_btn
                .set_icon_name("boxxy-media-playback-start-symbolic");
            self.toggle_btn.set_tooltip_text(Some("Activate Claw"));
            self.toggle_btn.remove_css_class("destructive-action");
            self.toggle_btn.add_css_class("suggested-action");
        }
    }

    pub fn update_ui(&self, active: bool, proactive: bool) {
        self.is_active.set(active);
        self.is_proactive.set(proactive);
        self.update_active(active);
        let mode = if proactive {
            boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
        } else {
            boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
        };
        self.update_diagnosis_mode(&mode);
    }

    #[must_use]
    pub const fn widget(&self) -> &gtk::Box {
        &self.widget
    }
}

impl Default for ClawSidebarComponent {
    fn default() -> Self {
        Self::new(|_| {}, |_| {})
    }
}

pub fn create_claw_message_list() -> gtk::ListBox {
    gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build()
}

pub fn add_diagnosis_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    diagnosis: &str,
) {
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

    let id_short = if pane_id.len() >= 7 {
        &pane_id[..7]
    } else {
        &pane_id
    };

    let pane_text = if let Some(name) = agent_name {
        format!("{} ({})", name, id_short)
    } else {
        format!("Pane {}", id_short)
    };

    let pane_lbl = gtk::Label::new(Some(&pane_text));
    pane_lbl.add_css_class("caption");
    pane_lbl.add_css_class("dim-label");
    header.append(&pane_lbl);

    vbox.append(&header);

    let viewer = StructuredViewer::new(get_claw_viewer_registry());
    viewer.set_content(diagnosis);
    vbox.append(viewer.widget());

    row.set_child(Some(&vbox));

    list.append(&row);
}

pub fn add_suggested_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    diagnosis: &str,
    command: &str,
) {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(8);
    vbox.set_margin_start(8);
    vbox.set_margin_end(8);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name("boxxy-dialog-warning-symbolic");
    icon.add_css_class("warning");
    header.append(&icon);

    let title = gtk::Label::new(Some("Suggested Action"));
    title.add_css_class("heading");
    title.set_halign(gtk::Align::Start);
    header.append(&title);

    let id_short = if pane_id.len() >= 7 {
        &pane_id[..7]
    } else {
        &pane_id
    };

    let pane_text = if let Some(name) = agent_name {
        format!("{} ({})", name, id_short)
    } else {
        format!("Pane {}", id_short)
    };

    let pane_lbl = gtk::Label::new(Some(&pane_text));
    pane_lbl.add_css_class("caption");
    pane_lbl.add_css_class("dim-label");
    header.append(&pane_lbl);

    vbox.append(&header);

    if !diagnosis.is_empty() {
        let viewer = StructuredViewer::new(get_claw_viewer_registry());
        viewer.set_content(diagnosis);
        vbox.append(viewer.widget());
    }

    let cmd_label = gtk::Label::new(Some(command));
    cmd_label.set_halign(gtk::Align::Start);
    cmd_label.set_wrap(true);
    cmd_label.set_selectable(true);
    cmd_label.add_css_class("monospace");
    cmd_label.add_css_class("dim-label");
    vbox.append(&cmd_label);

    row.set_child(Some(&vbox));
    list.append(&row);
}

pub fn add_approval_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    command: &str,
    _on_text_reply: impl Fn(String) + 'static,
) -> gtk::Box {
    add_diagnosis_row(
        list,
        pane_id,
        agent_name,
        &format!("Proposed command:\n```bash\n{command}\n```"),
    );
    gtk::Box::new(gtk::Orientation::Horizontal, 0)
}

pub fn add_file_write_approval_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    path: &str,
    content: &str,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) -> gtk::Box {
    add_diagnosis_row(
        list,
        pane_id,
        agent_name,
        &format!("Proposed file write to `{path}`:\n```\n{content}\n```"),
    );
    gtk::Box::new(gtk::Orientation::Horizontal, 0)
}

pub fn add_file_delete_approval_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    path: &str,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) -> gtk::Box {
    add_diagnosis_row(
        list,
        pane_id,
        agent_name,
        &format!("Proposed file deletion: `{path}`"),
    );
    gtk::Box::new(gtk::Orientation::Horizontal, 0)
}

pub fn add_kill_process_approval_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    pid: u32,
    process_name: &str,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) -> gtk::Box {
    add_diagnosis_row(
        list,
        pane_id,
        agent_name,
        &format!("Proposed killing process: {process_name} (PID: {pid})"),
    );
    gtk::Box::new(gtk::Orientation::Horizontal, 0)
}

pub fn add_clipboard_get_approval_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) -> gtk::Box {
    add_diagnosis_row(
        list,
        pane_id,
        agent_name,
        "Proposed reading from clipboard.",
    );
    gtk::Box::new(gtk::Orientation::Horizontal, 0)
}

pub fn add_clipboard_set_approval_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    text: &str,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) -> gtk::Box {
    add_diagnosis_row(
        list,
        pane_id,
        agent_name,
        &format!("Proposed writing to clipboard:\n```\n{text}\n```"),
    );
    gtk::Box::new(gtk::Orientation::Horizontal, 0)
}

pub fn add_process_list_row(
    list: &gtk::ListBox,
    pane_id: String,
    agent_name: Option<String>,
    result_json: &str,
    _on_kill_request: impl Fn(u32, String) + 'static,
) {
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

    let title = gtk::Label::new(Some("Process List"));
    title.add_css_class("heading");
    title.set_halign(gtk::Align::Start);
    header.append(&title);

    let id_short = if pane_id.len() >= 7 {
        &pane_id[..7]
    } else {
        &pane_id
    };
    let pane_text = if let Some(name) = agent_name {
        format!("{name} ({id_short})")
    } else {
        format!("Pane {id_short}")
    };

    let pane_lbl = gtk::Label::new(Some(&pane_text));
    pane_lbl.add_css_class("caption");
    pane_lbl.add_css_class("dim-label");
    header.append(&pane_lbl);

    vbox.append(&header);

    let viewer = StructuredViewer::new(get_claw_viewer_registry());
    viewer.append_custom_block("list_processes", result_json);
    vbox.append(viewer.widget());

    row.set_child(Some(&vbox));
    list.append(&row);
}
