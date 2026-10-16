//! Outer shell of the Claw sidebar ("Claw" page in the right-hand
//! sidebar stack).
//!
//! This component is window-scoped: there is exactly one per
//! `AppWindow`, and it re-targets itself at whichever pane is currently
//! active by calling `set_history_widget(list_view, ...)`. The per-pane
//! list itself is built by `create_claw_message_list()` and owned by
//! the pane.
//!
//! The sidebar is a strictly read-only debug log. It renders identity,
//! token usage, a pending-tasks drawer, and a "Clear" button that
//! empties the visible list and emits a soft-clear to the engine.

use boxxy_claw_protocol::{
    ScheduledTask, SessionUsage, TaskStatus, TaskType, format_number_with_commas,
};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct ClawSidebarComponent {
    widget: gtk::Box,
    status_page: adw::StatusPage,
    scroll: gtk::ScrolledWindow,
    context_box: gtk::Box,
    tokens_lbl: gtk::Label,
    percent_lbl: gtk::Label,
    cost_lbl: gtk::Label,
    mcp_box: gtk::Box,
    mcp_rows_box: gtk::Box,
    last_usage: Rc<Cell<SessionUsage>>,
    clear_btn: gtk::Button,
    command_panel: gtk::Box,
    current_list: Rc<RefCell<Option<gtk::ListView>>>,
    tasks_expander: gtk::Expander,
    tasks_list: gtk::ListBox,
    on_cancel_task: Rc<dyn Fn(uuid::Uuid) + 'static>,
}

impl ClawSidebarComponent {
    #[must_use]
    pub fn new<FC: Fn(uuid::Uuid) + 'static, FS: Fn() + 'static>(
        on_cancel_task: FC,
        on_soft_clear: FS,
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

        let current_list: Rc<RefCell<Option<gtk::ListView>>> = Rc::new(RefCell::new(None));

        let command_panel = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        command_panel.set_valign(gtk::Align::End);
        command_panel.set_margin_top(4);
        command_panel.set_margin_start(6);
        command_panel.set_margin_end(2);
        command_panel.set_margin_bottom(2);

        let footer_info_vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
        footer_info_vbox.set_hexpand(true);
        footer_info_vbox.set_halign(gtk::Align::Fill);

        let context_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        context_box.set_hexpand(true);
        context_box.set_halign(gtk::Align::Start);
        context_box.set_visible(false);

        let context_title_lbl = gtk::Label::builder()
            .label("Context")
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .build();
        context_box.append(&context_title_lbl);

        let tokens_lbl = gtk::Label::builder()
            .label("0 tokens")
            .css_classes(["caption", "dim-label", "monospace"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .build();
        context_box.append(&tokens_lbl);

        let percent_lbl = gtk::Label::builder()
            .label("0% used")
            .css_classes(["caption", "dim-label", "monospace"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .visible(false)
            .build();
        context_box.append(&percent_lbl);

        let cost_lbl = gtk::Label::builder()
            .label("$0.00 spent")
            .css_classes(["caption", "dim-label", "monospace"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .build();
        context_box.append(&cost_lbl);

        footer_info_vbox.append(&context_box);

        let mcp_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        mcp_box.set_hexpand(true);
        mcp_box.set_halign(gtk::Align::Fill);
        mcp_box.set_visible(false);

        let mcp_title_lbl = gtk::Label::builder()
            .label("MCP")
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .build();
        mcp_box.append(&mcp_title_lbl);

        let mcp_rows_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        mcp_rows_box.set_hexpand(true);
        mcp_rows_box.set_halign(gtk::Align::Fill);
        mcp_box.append(&mcp_rows_box);

        footer_info_vbox.append(&mcp_box);
        command_panel.append(&footer_info_vbox);

        let clear_btn = gtk::Button::builder()
            .icon_name("boxxy-edit-clear-symbolic")
            .css_classes(["flat", "image-button"])
            .tooltip_text("Clear Screen")
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .build();

        let current_list_clear = current_list.clone();
        let status_page_clone = status_page.clone();
        let scroll_clone = scroll.clone();
        let clear_btn_for_click = clear_btn.clone();
        let mcp_box_for_click = mcp_box.clone();
        let command_panel_for_click = command_panel.clone();
        clear_btn.connect_clicked(move |_| {
            // Drop visible rows. Engine still has the underlying history —
            // soft-clear marks a timestamp in the DB so restored sessions
            // skip pre-clear events.
            if let Some(list) = current_list_clear.borrow().as_ref() {
                if let Some(store) = list
                    .model()
                    .and_then(|m| m.downcast::<gtk::NoSelection>().ok())
                    .and_then(|s| s.model())
                    .and_then(|m| m.downcast::<gtk::gio::ListStore>().ok())
                {
                    store.remove_all();
                }
            }
            status_page_clone.set_visible(true);
            scroll_clone.set_visible(false);
            clear_btn_for_click.set_visible(false);
            command_panel_for_click.set_visible(mcp_box_for_click.is_visible());
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

        let last_usage = Rc::new(Cell::new(SessionUsage::default()));

        let comp = Self {
            widget,
            status_page,
            scroll,
            context_box,
            tokens_lbl,
            percent_lbl,
            cost_lbl,
            mcp_box,
            mcp_rows_box,
            last_usage,
            clear_btn: clear_btn.clone(),
            command_panel,
            current_list,
            tasks_expander,
            tasks_list,
            on_cancel_task: Rc::new(on_cancel_task),
        };

        let initial_settings = boxxy_preferences::Settings::load();
        comp.refresh_mcp_list(&initial_settings);
        if initial_settings.currency != boxxy_preferences::config::Currency::Usd {
            let cost_lbl_init = comp.cost_lbl.clone();
            let last_usage_init = comp.last_usage.clone();
            let initial_curr = initial_settings.currency;
            glib::spawn_future_local(async move {
                boxxy_sys_utils::runtime()
                    .spawn(boxxy_sys_utils::fetch_exchange_rates())
                    .await
                    .ok();
                let usage = last_usage_init.get();
                cost_lbl_init.set_label(&format!(
                    "{} spent",
                    initial_curr.format_from_usd(usage.cost_usd)
                ));
            });
        }

        let mcp_box_sub = comp.mcp_box.clone();
        let mcp_rows_sub = comp.mcp_rows_box.clone();
        let cost_lbl_sub = comp.cost_lbl.clone();
        let last_usage_sub = comp.last_usage.clone();
        let current_list_sub = comp.current_list.clone();
        let status_page_sub = comp.status_page.clone();
        let scroll_sub = comp.scroll.clone();
        let command_panel_sub = comp.command_panel.clone();
        let clear_btn_sub = clear_btn.clone();
        let mut settings_rx = boxxy_preferences::SETTINGS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            while let Ok(settings) = settings_rx.recv().await {
                populate_mcp_rows(&mcp_box_sub, &mcp_rows_sub, &settings);
                if settings.currency != boxxy_preferences::config::Currency::Usd {
                    boxxy_sys_utils::runtime()
                        .spawn(boxxy_sys_utils::fetch_exchange_rates())
                        .await
                        .ok();
                }
                let usage = last_usage_sub.get();
                cost_lbl_sub.set_label(&format!(
                    "{} spent",
                    settings.currency.format_from_usd(usage.cost_usd)
                ));
                let has_items = current_list_sub
                    .borrow()
                    .as_ref()
                    .and_then(|l| l.model())
                    .is_some_and(|m| m.n_items() > 0);
                let has_mcp = mcp_box_sub.is_visible();
                status_page_sub.set_visible(!has_items);
                scroll_sub.set_visible(has_items);
                clear_btn_sub.set_visible(has_items);
                command_panel_sub.set_visible(has_items || has_mcp);
            }
        });

        comp
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
            let mut desc = String::from("System-eccentric agentic control.");
            if pinned {
                desc.push_str(" (Pinned)");
            }
            if web_search_enabled {
                desc.push_str(" [Web Search ON]");
            }
            self.status_page.set_description(Some(&desc));
        }

        // Reuse path: same pane — nothing to re-parent.
        if let Some(old) = self.current_list.borrow().as_ref()
            && old == list
        {
            self.refresh_visibility();
            return;
        }

        if list.parent().is_some() {
            list.unparent();
        }

        // Virtual-list auto-scroll: only scroll if the user is already at
        // the bottom, so we don't yank them away from reading history.
        let adj = self.scroll.vadjustment();
        if let Some(store) = list
            .model()
            .and_then(|m| m.downcast::<gtk::NoSelection>().ok())
            .and_then(|s| s.model())
            .and_then(|m| m.downcast::<gtk::gio::ListStore>().ok())
        {
            let adj_clone = adj.clone();
            let list_v = list.clone();
            store.connect_items_changed(move |s, _, _, _| {
                let a = adj_clone.clone();
                let lv = list_v.clone();
                let n_items = s.n_items();
                gtk::glib::idle_add_local_once(move || {
                    let at_bottom = a.value() + a.page_size() >= a.upper() - 100.0;
                    if at_bottom && n_items > 0 {
                        lv.scroll_to(n_items - 1, gtk::ListScrollFlags::FOCUS, None);
                    }
                });
            });
        }

        self.scroll.set_child(Some(list));
        *self.current_list.borrow_mut() = Some(list.clone());
        self.refresh_visibility();
    }

    pub fn refresh_visibility(&self) {
        let has_items = self
            .current_list
            .borrow()
            .as_ref()
            .and_then(|l| l.model())
            .is_some_and(|m| m.n_items() > 0);
        let has_mcp = self.mcp_box.is_visible();
        self.status_page.set_visible(!has_items);
        self.scroll.set_visible(has_items);
        self.clear_btn.set_visible(has_items);
        self.command_panel.set_visible(has_items || has_mcp);
    }

    pub fn refresh_mcp_list(&self, settings: &boxxy_preferences::Settings) {
        populate_mcp_rows(&self.mcp_box, &self.mcp_rows_box, settings);
        self.refresh_visibility();
    }

    pub fn set_session_usage(&self, usage: SessionUsage) {
        self.last_usage.set(usage);
        let tokens = usage.display_tokens();
        if tokens > 0 || usage.cost_usd > 0.0 {
            let currency = boxxy_preferences::Settings::load().currency;
            self.tokens_lbl
                .set_label(&format!("{} tokens", format_number_with_commas(tokens)));
            if let Some(pct) = usage.context_percent() {
                self.percent_lbl.set_label(&format!("{pct}% used"));
                self.percent_lbl.set_visible(true);
            } else {
                self.percent_lbl.set_visible(false);
            }
            self.cost_lbl
                .set_label(&format!("{} spent", currency.format_from_usd(usage.cost_usd)));
            self.context_box.set_visible(true);
        } else {
            self.context_box.set_visible(false);
        }
    }

    pub fn set_token_usage(&self, tokens: u64) {
        self.set_session_usage(SessionUsage {
            context_tokens: tokens,
            context_window: None,
            total_tokens: tokens,
            cost_usd: 0.0,
        });
    }

    pub fn update_tasks(&self, tasks: Vec<ScheduledTask>) {
        while let Some(row) = self.tasks_list.row_at_index(0) {
            self.tasks_list.remove(&row);
        }

        let pending_count = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .count();
        if pending_count == 0 {
            self.tasks_expander.set_visible(false);
            return;
        }

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
    }

    #[must_use]
    pub const fn widget(&self) -> &gtk::Box {
        &self.widget
    }
}

impl Default for ClawSidebarComponent {
    fn default() -> Self {
        Self::new(|_| {}, || {})
    }
}

fn populate_mcp_rows(
    mcp_box: &gtk::Box,
    mcp_rows_box: &gtk::Box,
    settings: &boxxy_preferences::Settings,
) {
    while let Some(child) = mcp_rows_box.first_child() {
        mcp_rows_box.remove(&child);
    }

    let active_servers: Vec<_> = settings
        .mcp_servers
        .iter()
        .filter(|s| s.enabled)
        .collect();

    if active_servers.is_empty() {
        mcp_box.set_visible(false);
        return;
    }

    for srv in active_servers {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_hexpand(true);

        let dot_lbl = gtk::Label::builder()
            .label("•")
            .css_classes(["caption", "success", "monospace"])
            .halign(gtk::Align::Start)
            .build();
        row.append(&dot_lbl);

        let name_lbl = gtk::Label::builder()
            .label(&srv.name)
            .css_classes(["caption", "heading", "monospace"])
            .halign(gtk::Align::Start)
            .hexpand(true)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        row.append(&name_lbl);

        let status_lbl = gtk::Label::builder()
            .label("Connected")
            .css_classes(["caption", "dim-label", "monospace"])
            .halign(gtk::Align::End)
            .build();
        row.append(&status_lbl);

        mcp_rows_box.append(&row);
    }

    mcp_box.set_visible(true);
}

fn add_task_row<F: Fn(uuid::Uuid) + 'static>(
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

    header.append(&gtk::Image::from_icon_name(icon_name));

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
    cancel_btn.connect_clicked(move |_| on_cancel(task_id));
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
