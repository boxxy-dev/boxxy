use crate::engine::{ScheduledTask, TaskStatus, TaskType};
use boxxy_core_widgets::ObjectExtSafe;
use boxxy_viewer::{BlockRenderer, ContentBlock, StructuredViewer, ViewerRegistry};
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::rc::Rc;

pub struct ProcessListRenderer;

impl BlockRenderer for ProcessListRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Custom { schema, .. } if schema == "list_processes")
    }

    fn render(
        &self,
        block: &ContentBlock,
        _registry: &boxxy_viewer::ViewerRegistry,
    ) -> gtk::Widget {
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
    usage_lbl: gtk::Label,
    command_panel: gtk::Box,
    current_list: Rc<std::cell::RefCell<Option<gtk::ListView>>>,
    tasks_expander: gtk::Expander,
    tasks_list: gtk::ListBox,
    on_cancel_task: Rc<dyn Fn(uuid::Uuid) + 'static>,
}

impl ClawSidebarComponent {
    #[must_use]
    pub fn new<F3: Fn(uuid::Uuid) + 'static, F4: Fn() + 'static>(
        on_cancel_task: F3,
        on_soft_clear: F4,
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

        let current_list: Rc<std::cell::RefCell<Option<gtk::ListView>>> =
            Rc::new(std::cell::RefCell::new(None));

        // Command panel (Usage + Clear Button)
        let command_panel = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        command_panel.set_valign(gtk::Align::End);
        command_panel.set_margin_top(4);

        let usage_lbl = gtk::Label::builder()
            .label("Context: 0 tokens")
            .css_classes(["caption", "dim-label"])
            .hexpand(true)
            .halign(gtk::Align::Center)
            .visible(false)
            .build();
        command_panel.append(&usage_lbl);

        let clear_btn = gtk::Button::builder()
            .icon_name("boxxy-edit-clear-symbolic")
            .css_classes(["flat", "image-button"])
            .tooltip_text("Clear Screen")
            .halign(gtk::Align::End)
            .build();

        let current_list_clear = current_list.clone();
        let status_page_clone = status_page.clone();
        let scroll_clone = scroll.clone();
        clear_btn.connect_clicked(move |_| {
            if let Some(list) = current_list_clear.borrow().as_ref() {
                if let Some(model) = list.model() {
                    if let Some(store) = model
                        .downcast_ref::<gtk::NoSelection>()
                        .and_then(|s| s.model())
                        .and_then(|m| m.downcast::<gtk::gio::ListStore>().ok())
                    {
                        store.remove_all();
                    }
                }
            }
            status_page_clone.set_visible(true);
            scroll_clone.set_visible(false);
            on_soft_clear();
        });

        command_panel.append(&clear_btn);
        widget.append(&command_panel);

        let tasks_list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();

        let tasks_expander = gtk::Expander::builder()
            .label("Pending Tasks")
            .child(&tasks_list)
            .visible(false)
            .build();

        widget.append(&tasks_expander);

        Self {
            widget,
            status_page,
            scroll,
            usage_lbl,
            command_panel,
            current_list,
            tasks_expander,
            tasks_list,
            on_cancel_task: Rc::new(on_cancel_task),
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        // This is now purely tracked in the window state/msgbar
        false
    }

    pub fn set_history_widget(
        &self,
        list: &gtk::ListView,
        agent_name: &str,
        pinned: bool,
        web_search_enabled: bool,
    ) {
        if !agent_name.is_empty() {
            self.status_page.set_title(&format!("Claw: {}", agent_name));
            let mut desc = format!("System-eccentric agentic control.");
            if pinned {
                desc.push_str(" (Pinned)");
            }
            if web_search_enabled {
                desc.push_str(" [Web Search ON]");
            }
            self.status_page.set_description(Some(&desc));
        }

        if let Some(old) = self.current_list.borrow().as_ref()
            && old == list
        {
            self.refresh_visibility();
            return;
        }

        if list.parent().is_some() {
            list.unparent();
        }

        // Auto-scroll logic for virtual list
        let adj = self.scroll.vadjustment();
        let list_clone = list.clone();
        if let Some(model) = list.model() {
            if let Some(store) = model
                .downcast_ref::<gtk::NoSelection>()
                .and_then(|s| s.model())
                .and_then(|m| m.downcast::<gtk::gio::ListStore>().ok())
            {
                let adj_clone = adj.clone();
                let lv = list_clone.clone();
                store.connect_items_changed(move |s, _, _, _| {
                    let a = adj_clone.clone();
                    let list_v = lv.clone();
                    let n_items = s.n_items();
                    gtk::glib::idle_add_local_once(move || {
                        // User guard: only scroll if the user is already at the bottom
                        let is_at_bottom = a.value() + a.page_size() >= a.upper() - 100.0;
                        if is_at_bottom && n_items > 0 {
                            list_v.scroll_to(n_items - 1, gtk::ListScrollFlags::FOCUS, None);
                        }
                    });
                });
            }
        }

        self.scroll.set_child(Some(list));
        *self.current_list.borrow_mut() = Some(list.clone());
        self.refresh_visibility();
    }

    pub fn refresh_visibility(&self) {
        if let Some(list) = self.current_list.borrow().as_ref() {
            let has_items = list.model().map(|m| m.n_items() > 0).unwrap_or(false);
            self.status_page.set_visible(!has_items);
            self.scroll.set_visible(has_items);
            self.command_panel.set_visible(has_items);
        }
    }

    pub fn update_active(&self, _active: bool) {
        // No longer managed in sidebar UI
    }

    pub fn update_ui(&self, _active: bool, _proactive: bool) {
        // No longer managed in sidebar UI
    }

    pub fn set_token_usage(&self, tokens: u64) {
        if tokens > 0 {
            self.usage_lbl
                .set_label(&format!("Context: {tokens} tokens"));
            self.usage_lbl.set_visible(true);
        } else {
            self.usage_lbl.set_visible(false);
        }
    }

    pub fn update_tasks(&self, tasks: Vec<ScheduledTask>) {
        // Clear old tasks
        while let Some(row) = self.tasks_list.row_at_index(0) {
            self.tasks_list.remove(&row);
        }

        let pending_count = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .count();
        if pending_count > 0 {
            self.tasks_expander
                .set_label(Some(&format!("Pending Tasks ({})", pending_count)));
            self.tasks_expander.set_visible(true);

            let on_cancel_rc = self.on_cancel_task.clone();
            for task in tasks {
                if task.status == TaskStatus::Pending {
                    let cancel_cb = on_cancel_rc.clone();
                    add_task_row(&self.tasks_list, task, move |id| cancel_cb(id));
                }
            }
        } else {
            self.tasks_expander.set_visible(false);
        }
    }

    #[must_use]
    pub const fn widget(&self) -> &gtk::Box {
        &self.widget
    }
}

pub fn add_task_row<F: Fn(uuid::Uuid) + 'static>(
    list: &gtk::ListBox,
    task: ScheduledTask,
    on_cancel: F,
) {
    let row = gtk::ListBoxRow::new();
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vbox.set_margin_top(6);
    vbox.set_margin_bottom(6);
    vbox.set_margin_start(6);
    vbox.set_margin_end(6);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let (icon_name, title) = match task.task_type {
        TaskType::Notification => ("boxxy-chat-symbolic", "Reminder"),
        TaskType::Command => ("utilities-terminal-symbolic", "Command"),
        TaskType::Query => ("help-browser-symbolic", "Query"),
    };

    let icon = gtk::Image::from_icon_name(icon_name);
    header.append(&icon);

    let title_lbl = gtk::Label::builder()
        .label(title)
        .css_classes(["heading"])
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    header.append(&title_lbl);

    let cancel_btn = gtk::Button::builder()
        .icon_name("boxxy-cross-small-symbolic")
        .css_classes(["flat", "circular"])
        .tooltip_text("Cancel Task")
        .build();

    let task_id = task.id;
    cancel_btn.connect_clicked(move |_| {
        on_cancel(task_id);
    });
    header.append(&cancel_btn);

    vbox.append(&header);

    let payload_lbl = gtk::Label::builder()
        .label(&task.payload)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .css_classes(["caption"])
        .build();
    vbox.append(&payload_lbl);

    let due_str = format!(
        "Due at: {}",
        task.due_at.with_timezone(&chrono::Local).format("%H:%M:%S")
    );
    let due_lbl = gtk::Label::builder()
        .label(&due_str)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .css_classes(["caption", "dim-label"])
        .build();
    vbox.append(&due_lbl);

    row.set_child(Some(&vbox));
    list.append(&row);
}

impl Default for ClawSidebarComponent {
    fn default() -> Self {
        Self::new(|_| {}, || {})
    }
}

pub fn create_claw_message_list() -> (gtk::ListView, gtk::gio::ListStore) {
    let list_store = gtk::gio::ListStore::new::<crate::engine::ClawRowObject>();
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
        vbox.add_css_class("claw-virtual-row");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::new();
        header.append(&icon);

        let title = gtk::Label::new(None);
        title.add_css_class("heading");
        title.set_halign(gtk::Align::Start);
        header.append(&title);

        let pane_lbl = gtk::Label::new(None);
        pane_lbl.add_css_class("caption");
        pane_lbl.add_css_class("dim-label");
        header.append(&pane_lbl);

        vbox.append(&header);

        let registry = get_claw_viewer_registry();
        let viewer = StructuredViewer::new(registry);
        vbox.append(viewer.widget());

        let cmd_label = gtk::Label::new(None);
        cmd_label.set_halign(gtk::Align::Start);
        cmd_label.set_wrap(true);
        cmd_label.set_selectable(true);
        cmd_label.add_css_class("monospace");
        cmd_label.add_css_class("dim-label");
        cmd_label.set_visible(false);
        vbox.append(&cmd_label);

        vbox.set_safe_data("icon", icon);
        vbox.set_safe_data("title", title);
        vbox.set_safe_data("pane_lbl", pane_lbl);
        vbox.set_safe_data("viewer", viewer);
        vbox.set_safe_data("cmd_label", cmd_label);

        list_item.set_child(Some(&vbox));
    });

    factory.connect_bind(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let vbox = list_item.child().and_downcast::<gtk::Box>().unwrap();

        let icon = vbox.get_safe_data::<gtk::Image>("icon").unwrap();
        let title = vbox.get_safe_data::<gtk::Label>("title").unwrap();
        let pane_lbl = vbox.get_safe_data::<gtk::Label>("pane_lbl").unwrap();
        let viewer = vbox.get_safe_data::<StructuredViewer>("viewer").unwrap();
        let cmd_label = vbox.get_safe_data::<gtk::Label>("cmd_label").unwrap();

        if let Some(obj) = list_item
            .item()
            .and_downcast::<crate::engine::ClawRowObject>()
        {
            let row = obj.get_row();
            match row {
                crate::engine::PersistentClawRow::Diagnosis {
                    pane_id,
                    agent_name,
                    content,
                    ..
                } => {
                    icon.set_icon_name(Some("boxxyclaw"));
                    icon.add_css_class("accent");
                    icon.remove_css_class("warning");
                    title.set_label("Diagnosis");

                    let pane_text = if let Some(name) = agent_name {
                        name
                    } else {
                        "Unknown Agent".to_string()
                    };
                    pane_lbl.set_label(&pane_text);

                    viewer.set_content(&content);
                    viewer.widget().set_visible(true);
                    cmd_label.set_visible(false);
                }
                crate::engine::PersistentClawRow::User { pane_id: _, content } => {
                    icon.set_icon_name(Some("boxxy-comic-bubble-symbolic"));
                    icon.remove_css_class("accent");
                    icon.remove_css_class("warning");
                    title.set_label("User Message");

                    pane_lbl.set_label("User");

                    viewer.set_content(&content);
                    viewer.widget().set_visible(true);
                    cmd_label.set_visible(false);
                }
                crate::engine::PersistentClawRow::Suggested {
                    pane_id,
                    agent_name,
                    diagnosis,
                    command,
                    ..
                } => {
                    icon.set_icon_name(Some("boxxy-dialog-warning-symbolic"));
                    icon.add_css_class("warning");
                    icon.remove_css_class("accent");
                    title.set_label("Suggested Action");

                    let pane_text = if let Some(name) = agent_name {
                        name
                    } else {
                        "Unknown Agent".to_string()
                    };
                    pane_lbl.set_label(&pane_text);

                    if !diagnosis.is_empty() {
                        viewer.set_content(&diagnosis);
                        viewer.widget().set_visible(true);
                    } else {
                        viewer.widget().set_visible(false);
                    }

                    cmd_label.set_label(&command);
                    cmd_label.set_visible(true);
                }
                crate::engine::PersistentClawRow::ProcessList {
                    pane_id,
                    agent_name,
                    result_json,
                    ..
                } => {
                    icon.set_icon_name(Some("boxxyclaw"));
                    icon.add_css_class("accent");
                    icon.remove_css_class("warning");
                    title.set_label("Process List");

                    let pane_text = if let Some(name) = agent_name {
                        name
                    } else {
                        "Unknown Agent".to_string()
                    };
                    pane_lbl.set_label(&pane_text);

                    viewer.clear();
                    viewer.append_custom_block("list_processes", &result_json);
                    viewer.widget().set_visible(true);
                    cmd_label.set_visible(false);
                }
                crate::engine::PersistentClawRow::ToolCall {
                    pane_id,
                    agent_name,
                    tool_name,
                    .. // Ignore result!
                } => {
                    icon.set_icon_name(Some("boxxy-build-circle-symbolic"));
                    icon.add_css_class("accent");
                    icon.remove_css_class("warning");
                    title.set_label(&format!("Used tool: {tool_name}"));

                    let pane_text = if let Some(name) = agent_name {
                        name
                    } else {
                        "Unknown Agent".to_string()
                    };
                    pane_lbl.set_label(&pane_text);

                    // We intentionally hide the viewer here so the tool call is just a single compact row.
                    viewer.clear();
                    viewer.widget().set_visible(false);
                    cmd_label.set_visible(false);
                }
                crate::engine::PersistentClawRow::Command { command, exit_code } => {
                    icon.set_icon_name(Some("utilities-terminal-symbolic"));
                    icon.remove_css_class("accent");
                    icon.remove_css_class("warning");
                    if exit_code == 0 {
                        title.set_label("Command Execution");
                    } else {
                        title.set_label(&format!("Command Failed (Exit {})", exit_code));
                        icon.add_css_class("error");
                    }

                    pane_lbl.set_label("User");

                    viewer.set_content(&command);
                    viewer.widget().set_visible(true);
                    cmd_label.set_visible(false);
                }
            }
        }
    });

    factory.connect_unbind(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let vbox = list_item.child().and_downcast::<gtk::Box>().unwrap();
        let viewer = vbox.get_safe_data::<StructuredViewer>("viewer").unwrap();
        viewer.clear();
    });

    let selection_model = gtk::NoSelection::new(Some(list_store.clone()));
    let list_view = gtk::ListView::new(Some(selection_model), Some(factory));
    list_view.set_show_separators(false);
    list_view.add_css_class("virtual-history");

    (list_view, list_store)
}

pub fn add_diagnosis_row(
    list: &gtk::gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    diagnosis: &str,
) {
    list.append(&crate::engine::ClawRowObject::new(
        crate::engine::PersistentClawRow::Diagnosis {
            pane_id,
            agent_name,
            content: diagnosis.to_string(),
            usage: None,
        },
    ));
}

pub fn add_user_row(list: &gtk::gio::ListStore, pane_id: String, content: &str) {
    list.append(&crate::engine::ClawRowObject::new(
        crate::engine::PersistentClawRow::User {
            pane_id,
            content: content.to_string(),
        },
    ));
}

pub fn add_suggested_row(
    list: &gtk::gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    diagnosis: &str,
    command: &str,
) {
    list.append(&crate::engine::ClawRowObject::new(
        crate::engine::PersistentClawRow::Suggested {
            pane_id,
            agent_name,
            diagnosis: diagnosis.to_string(),
            command: command.to_string(),
            usage: None,
        },
    ));
}

pub fn add_tool_call_row(
    list: &gtk::gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    tool_name: &str,
    result: &str,
) {
    list.append(&crate::engine::ClawRowObject::new(
        crate::engine::PersistentClawRow::ToolCall {
            pane_id,
            agent_name,
            tool_name: tool_name.to_string(),
            result: result.to_string(),
            usage: None,
        },
    ));
}

pub fn add_approval_row(
    list: &gtk::gio::ListStore,
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
    list: &gtk::gio::ListStore,
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
    list: &gtk::gio::ListStore,
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
    list: &gtk::gio::ListStore,
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
    list: &gtk::gio::ListStore,
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
    list: &gtk::gio::ListStore,
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
    list: &gtk::gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    result_json: &str,
    _on_kill_request: impl Fn(u32, String) + 'static,
) {
    list.append(&crate::engine::ClawRowObject::new(
        crate::engine::PersistentClawRow::ProcessList {
            pane_id,
            agent_name,
            result_json: result_json.to_string(),
            usage: None,
        },
    ));
}
