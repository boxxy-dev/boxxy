//! Outer shell of the Claw sidebar ("Claw" page in the right-hand
//! sidebar stack).
//!
//! This component is window-scoped: there is exactly one per
//! `AppWindow`, and it re-targets itself at whichever pane is currently
//! active by calling `set_pane_state(agent_name, pinned, web_search_enabled)`.
//!
//! The sidebar displays active session identity/avatar, current models
//! (Claw / Dreams) and search engine, token/cost context usage, connected
//! MCP servers, pending tasks, and a temporary "Clear History" button that
//! clears the current history from the main overlay view.

use boxxy_claw_protocol::{
    ScheduledTask, SessionUsage, TaskStatus, TaskType, format_number_with_commas,
};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

pub struct ClawSidebarComponent {
    widget: gtk::Box,
    hero_icon: gtk::Image,
    hero_avatar: adw::Avatar,
    hero_title: gtk::Label,
    hero_subtitle: gtk::Label,
    badges_box: gtk::Box,
    claw_model_lbl: gtk::Label,
    dreams_model_lbl: gtk::Label,
    dream_status_lbl: gtk::Label,
    search_lbl: gtk::Label,
    context_box: gtk::Box,
    tokens_lbl: gtk::Label,
    window_row: gtk::Box,
    percent_lbl: gtk::Label,
    tools_lbl: gtk::Label,
    memories_lbl: gtk::Label,
    cost_lbl: gtk::Label,
    mcp_box: gtk::Box,
    mcp_rows_box: gtk::Box,
    last_usage: Rc<Cell<SessionUsage>>,
    web_search_enabled: Rc<Cell<bool>>,
    command_panel: gtk::Box,
    tasks_expander: gtk::Expander,
    tasks_list: gtk::ListBox,
    on_cancel_task: Rc<dyn Fn(uuid::Uuid) + 'static>,
}

impl ClawSidebarComponent {
    #[must_use]
    pub fn new<FC: Fn(uuid::Uuid) + 'static, FS: Fn() + 'static>(
        on_cancel_task: FC,
        _on_soft_clear: FS,
    ) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 12);
        widget.set_margin_top(12);
        widget.set_margin_bottom(14);
        widget.set_margin_start(14);
        widget.set_margin_end(14);

        // --- Centered Hero / Identity Section ---
        let hero_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        hero_box.set_vexpand(true);
        hero_box.set_valign(gtk::Align::Center);
        hero_box.set_halign(gtk::Align::Center);
        hero_box.set_margin_start(12);
        hero_box.set_margin_end(12);

        let hero_icon = gtk::Image::builder()
            .icon_name("boxxyclaw")
            .pixel_size(96)
            .halign(gtk::Align::Center)
            .build();
        hero_box.append(&hero_icon);

        let hero_avatar = adw::Avatar::new(84, None, true);
        hero_avatar.set_halign(gtk::Align::Center);
        hero_avatar.set_visible(false);
        hero_box.append(&hero_avatar);

        let hero_title = gtk::Label::builder()
            .label("Claw Mode")
            .css_classes(["title-2"])
            .halign(gtk::Align::Center)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .build();
        hero_box.append(&hero_title);

        let hero_subtitle = gtk::Label::builder()
            .label("System-eccentric agentic control.")
            .css_classes(["dim-label"])
            .halign(gtk::Align::Center)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        hero_box.append(&hero_subtitle);

        let badges_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        badges_box.set_halign(gtk::Align::Center);
        badges_box.set_margin_top(2);
        badges_box.set_visible(false);
        hero_box.append(&badges_box);

        let stats_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        stats_box.set_halign(gtk::Align::Center);
        stats_box.set_margin_top(2);

        let memories_lbl = gtk::Label::builder()
            .label("🧠 0 Memories")
            .css_classes(["caption", "dim-label", "monospace"])
            .build();
        stats_box.append(&memories_lbl);

        let stats_sep = gtk::Label::builder()
            .label("•")
            .css_classes(["caption", "dim-label", "monospace"])
            .build();
        stats_box.append(&stats_sep);

        let tools_lbl = gtk::Label::builder()
            .label("🛠️ 0 Tools")
            .css_classes(["caption", "dim-label", "monospace"])
            .build();
        stats_box.append(&tools_lbl);

        hero_box.append(&stats_box);

        widget.append(&hero_box);

        // --- Pending Tasks Expander ---
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

        // --- Footer Details & Actions Panel ---
        let command_panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
        command_panel.set_valign(gtk::Align::End);
        command_panel.set_hexpand(true);
        command_panel.set_halign(gtk::Align::Fill);

        let footer_info_vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        footer_info_vbox.set_hexpand(true);
        footer_info_vbox.set_halign(gtk::Align::Fill);

        let make_kv_row = |key: &str, default_val: &str| -> (gtk::Box, gtk::Label) {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_hexpand(true);

            let key_lbl = gtk::Label::builder()
                .label(key)
                .css_classes(["caption", "dim-label"])
                .halign(gtk::Align::Start)
                .xalign(0.0)
                .build();
            row.append(&key_lbl);

            let val_lbl = gtk::Label::builder()
                .label(default_val)
                .css_classes(["caption", "monospace"])
                .halign(gtk::Align::End)
                .hexpand(true)
                .xalign(1.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            row.append(&val_lbl);

            (row, val_lbl)
        };

        // 1. Models & Search Section
        let models_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        models_box.set_hexpand(true);
        models_box.set_halign(gtk::Align::Fill);

        let models_title_lbl = gtk::Label::builder()
            .label("Models")
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .build();
        models_box.append(&models_title_lbl);

        let (claw_row, claw_model_lbl) = make_kv_row("Claw", "Default");
        models_box.append(&claw_row);

        let (dreams_row, dreams_model_lbl) = make_kv_row("Dreams", "Default");
        models_box.append(&dreams_row);

        let (dream_status_row, dream_status_lbl) = make_kv_row("Dream Cycle", "Idle");
        models_box.append(&dream_status_row);

        let (search_row, search_lbl) = make_kv_row("Search", "Off");
        models_box.append(&search_row);

        footer_info_vbox.append(&models_box);

        // 2. Context Usage Section
        let context_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        context_box.set_hexpand(true);
        context_box.set_halign(gtk::Align::Fill);

        let context_title_lbl = gtk::Label::builder()
            .label("Context")
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .build();
        context_box.append(&context_title_lbl);

        let (tokens_row, tokens_lbl) = make_kv_row("Tokens", "0 tokens");
        context_box.append(&tokens_row);

        let (window_row, percent_lbl) = make_kv_row("Window", "0% used");
        window_row.set_visible(false);
        context_box.append(&window_row);

        let (cost_row, cost_lbl) = make_kv_row("Spent", "$0.00");
        context_box.append(&cost_row);

        footer_info_vbox.append(&context_box);

        // 3. MCP Servers Section
        let mcp_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
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
        widget.append(&command_panel);

        let initial_settings = boxxy_preferences::Settings::load();
        let last_usage = Rc::new(Cell::new(SessionUsage::default()));
        let web_search_enabled = Rc::new(Cell::new(initial_settings.web_search_on_by_default));

        let comp = Self {
            widget,
            hero_icon,
            hero_avatar,
            hero_title,
            hero_subtitle,
            badges_box,
            claw_model_lbl,
            dreams_model_lbl,
            dream_status_lbl,
            search_lbl,
            context_box,
            tokens_lbl,
            window_row,
            percent_lbl,
            tools_lbl,
            memories_lbl,
            cost_lbl,
            mcp_box,
            mcp_rows_box,
            last_usage,
            web_search_enabled,
            command_panel,
            tasks_expander,
            tasks_list,
            on_cancel_task: Rc::new(on_cancel_task),
        };

        comp.refresh_models_info(&initial_settings);
        comp.refresh_mcp_list(&initial_settings);
        comp.refresh_memories_count();
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
                cost_lbl_init.set_label(&initial_curr.format_from_usd(usage.cost_usd));
            });
        }

        let claw_model_sub = comp.claw_model_lbl.clone();
        let dreams_model_sub = comp.dreams_model_lbl.clone();
        let search_sub = comp.search_lbl.clone();
        let web_search_sub = comp.web_search_enabled.clone();
        let mcp_box_sub = comp.mcp_box.clone();
        let mcp_rows_sub = comp.mcp_rows_box.clone();
        let cost_lbl_sub = comp.cost_lbl.clone();
        let last_usage_sub = comp.last_usage.clone();
        let mut settings_rx = boxxy_preferences::SETTINGS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            loop {
                let settings = match settings_rx.recv().await {
                    Ok(s) => s,
                    Err(boxxy_preferences::config::BroadcastRecvError::Lagged(_)) => {
                        (*boxxy_preferences::Settings::load()).clone()
                    }
                    Err(boxxy_preferences::config::BroadcastRecvError::Closed) => break,
                };
                populate_models_info(
                    &claw_model_sub,
                    &dreams_model_sub,
                    &search_sub,
                    &settings,
                    web_search_sub.get(),
                );
                populate_mcp_rows(&mcp_box_sub, &mcp_rows_sub, &settings);
                if settings.currency != boxxy_preferences::config::Currency::Usd {
                    boxxy_sys_utils::runtime()
                        .spawn(boxxy_sys_utils::fetch_exchange_rates())
                        .await
                        .ok();
                }
                let usage = last_usage_sub.get();
                cost_lbl_sub.set_label(&settings.currency.format_from_usd(usage.cost_usd));
            }
        });

        comp
    }

    pub fn set_pane_state(&self, agent_name: &str, pinned: bool, web_search_enabled: bool) {
        while let Some(child) = self.badges_box.first_child() {
            self.badges_box.remove(&child);
        }

        if !agent_name.is_empty() {
            let cache = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
            let fallback_names = [agent_name];

            if let Some(info) =
                boxxy_claw_protocol::characters::resolve_character(&cache, "", &fallback_names)
            {
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

                self.hero_title.set_markup(&format!(
                    "<span foreground=\"{}\"><b>{}</b></span>",
                    glib::markup_escape_text(&info.config.color),
                    glib::markup_escape_text(&formatted_name)
                ));
                self.hero_subtitle.set_label(&info.config.duties);

                self.hero_avatar.set_text(Some(&formatted_name));
                self.hero_avatar.set_custom_image(None::<&gdk::Texture>);
                if info.has_avatar
                    && let Ok(dir) = boxxy_claw_protocol::character_loader::get_characters_dir()
                {
                    let avatar_path = dir.join(&info.config.name).join("AVATAR.png");
                    if let Ok(texture) = gdk::Texture::from_filename(&avatar_path) {
                        self.hero_avatar.set_custom_image(Some(&texture));
                    }
                }
                self.hero_avatar.set_visible(true);
                self.hero_icon.set_visible(false);
            } else {
                self.hero_title.set_label(agent_name);
                self.hero_subtitle
                    .set_label("System-eccentric agentic control.");
                self.hero_avatar.set_visible(false);
                self.hero_icon.set_visible(true);
            }

            if pinned {
                let pin_lbl = gtk::Label::builder()
                    .label("📌 Pinned")
                    .css_classes(["caption", "dim-label", "monospace"])
                    .build();
                self.badges_box.append(&pin_lbl);
            }
            let ws_lbl = gtk::Label::builder()
                .label(if web_search_enabled {
                    "🌐 Web Search ON"
                } else {
                    "🌐 Web Search OFF"
                })
                .css_classes(["caption", "dim-label", "monospace"])
                .build();
            self.badges_box.append(&ws_lbl);
            self.badges_box.set_visible(true);
        } else {
            self.hero_title.set_label("Claw Mode");
            self.hero_subtitle
                .set_label("System-eccentric agentic control.");
            self.hero_avatar.set_visible(false);
            self.hero_icon.set_visible(true);
            self.badges_box.set_visible(false);
        }

        self.web_search_enabled.set(web_search_enabled);
        let settings = boxxy_preferences::Settings::load();
        self.refresh_models_info(&settings);
        self.refresh_memories_count();
        self.refresh_visibility();
    }

    pub fn refresh_memories_count(&self) {
        let memories_lbl = self.memories_lbl.clone();
        glib::spawn_future_local(async move {
            if let Ok(Ok(count)) = boxxy_sys_utils::runtime()
                .spawn(async move {
                    let db = boxxy_db::Db::new().await?;
                    let store = boxxy_db::store::Store::new(db.pool());
                    store.count_memories().await
                })
                .await
            {
                let n = count.max(0) as u64;
                let word = if n == 1 { "Memory" } else { "Memories" };
                memories_lbl.set_label(&format!("🧠 {} {}", format_number_with_commas(n), word));
            }
        });
    }

    pub fn refresh_visibility(&self) {
        self.command_panel.set_visible(true);
    }

    pub fn refresh_models_info(&self, settings: &boxxy_preferences::Settings) {
        populate_models_info(
            &self.claw_model_lbl,
            &self.dreams_model_lbl,
            &self.search_lbl,
            settings,
            self.web_search_enabled.get(),
        );
    }

    pub fn refresh_mcp_list(&self, settings: &boxxy_preferences::Settings) {
        self.refresh_models_info(settings);
        populate_mcp_rows(&self.mcp_box, &self.mcp_rows_box, settings);
        self.refresh_visibility();
    }

    pub fn set_maintenance_status(&self, status: &str, on_battery: bool) {
        let formatted = match status {
            "disabled" => "Disabled",
            "paused_on_battery" => "Paused (Battery)",
            "paused_ui_attached" if on_battery => "Standby (Battery)",
            "paused_ui_attached" => "Standby (UI Open)",
            "running" => "Consolidating…",
            _ => "Idle",
        };
        self.dream_status_lbl.set_label(formatted);
    }

    pub fn set_session_usage(&self, usage: SessionUsage) {
        self.last_usage.set(usage);
        let tokens = usage.display_tokens();
        let currency = boxxy_preferences::Settings::load().currency;
        self.tokens_lbl
            .set_label(&format!("{} tokens", format_number_with_commas(tokens)));
        if let Some(pct) = usage.context_percent() {
            self.percent_lbl.set_label(&format!("{pct}% used"));
            self.window_row.set_visible(true);
        } else {
            self.window_row.set_visible(false);
        }
        let tool_word = if usage.tools_used == 1 {
            "Tool"
        } else {
            "Tools"
        };
        self.tools_lbl.set_label(&format!(
            "🛠️ {} {}",
            format_number_with_commas(usage.tools_used),
            tool_word
        ));
        self.cost_lbl
            .set_label(&currency.format_from_usd(usage.cost_usd));
        self.context_box.set_visible(true);
        self.refresh_memories_count();
    }

    pub fn set_token_usage(&self, tokens: u64) {
        self.set_session_usage(SessionUsage {
            context_tokens: tokens,
            context_window: None,
            total_tokens: tokens,
            cost_usd: 0.0,
            tools_used: 0,
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

fn resolve_active_search_label(
    settings: &boxxy_preferences::Settings,
    web_search_local_enabled: bool,
) -> &'static str {
    if !settings.enable_web_search || !web_search_local_enabled {
        return "Off";
    }

    if settings.use_native_search
        && settings
            .claw_model
            .as_ref()
            .is_some_and(|m| m.supports_native_search())
    {
        return "Native";
    }

    if !settings.get_effective_api_key("Tavily").trim().is_empty() {
        "Tavily"
    } else {
        "Off"
    }
}

fn populate_models_info(
    claw_model_lbl: &gtk::Label,
    dreams_model_lbl: &gtk::Label,
    search_lbl: &gtk::Label,
    settings: &boxxy_preferences::Settings,
    web_search_local_enabled: bool,
) {
    let claw_label = settings
        .claw_model
        .as_ref()
        .map(|m| m.format_label())
        .unwrap_or_else(|| "Default".to_string());
    let memory_label = settings
        .memory_model
        .as_ref()
        .map(|m| m.format_label())
        .unwrap_or_else(|| "Default".to_string());
    let search_label = resolve_active_search_label(settings, web_search_local_enabled);

    claw_model_lbl.set_label(&claw_label);
    dreams_model_lbl.set_label(&memory_label);
    search_lbl.set_label(search_label);
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
