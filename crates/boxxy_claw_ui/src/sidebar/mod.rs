//! Outer shell of the Claw sidebar ("Claw" page in the right-hand sidebar stack).
//!
//! Window-scoped component: one per `AppWindow`, retargeting itself at whichever
//! pane is active via `set_pane_state(agent_name, sleep, pinned, web_search_enabled, vibe_enabled)`.

mod past_sessions;
mod tasks;

use crate::character_style::{
    is_safe_hex_color, load_character_avatar_texture, load_incognito_texture,
    prettify_character_name, resolve_character_display_name,
};
use boxxy_claw_protocol::{ScheduledTask, SessionUsage, format_number_with_commas};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use past_sessions::{
    PastSessionsCtx, RenderedSessionSig, ResumeSessionCallback, populate_past_sessions,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use tasks::TasksSection;

pub struct ClawSidebarComponent {
    widget: gtk::Box,
    hero_box: gtk::Box,
    hero_icon: gtk::Image,
    hero_avatar_overlay: gtk::Overlay,
    hero_avatar: adw::Avatar,
    hero_sleep_badge: gtk::Box,
    hero_vibe_badge: gtk::Box,
    hero_title: gtk::Label,
    hero_subtitle: gtk::Label,
    badges_box: gtk::Box,
    stats_box: gtk::Box,
    sessions_box: gtk::Box,
    sessions_list: gtk::ListBox,
    is_new_session: Rc<Cell<bool>>,
    rendered_sessions_sig: Rc<RefCell<RenderedSessionSig>>,
    active_options_popover: Rc<RefCell<Option<gtk::Popover>>>,
    on_resume_session: ResumeSessionCallback,
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
    memories_fetch_in_flight: Rc<Cell<bool>>,
    cost_lbl: gtk::Label,
    mcp_box: gtk::Box,
    mcp_rows_box: gtk::Box,
    mcp_statuses: Rc<RefCell<HashMap<String, bool>>>,
    last_usage: Rc<Cell<SessionUsage>>,
    web_search_enabled: Rc<Cell<bool>>,
    tasks_section: TasksSection,
}

impl ClawSidebarComponent {
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn new<FC: Fn(uuid::Uuid) + 'static, FS: Fn() + 'static>(
        on_cancel_task: FC,
        _on_soft_clear: FS,
    ) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 12);

        // --- Centered Hero / Identity Section ---
        let hero_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        hero_box.set_vexpand(true);
        hero_box.set_valign(gtk::Align::Center);
        hero_box.set_halign(gtk::Align::Center);
        hero_box.set_margin_top(12);
        hero_box.set_margin_start(20);
        hero_box.set_margin_end(20);

        let hero_icon = gtk::Image::builder()
            .icon_name("boxxyclaw")
            .pixel_size(72)
            .halign(gtk::Align::Center)
            .build();
        hero_box.append(&hero_icon);

        let hero_avatar = adw::Avatar::new(84, None, true);
        hero_avatar.set_halign(gtk::Align::Center);
        hero_avatar.set_margin_top(4);
        hero_avatar.set_margin_bottom(4);
        hero_avatar.set_margin_start(4);
        hero_avatar.set_margin_end(4);

        let hero_sleep_icon = gtk::Image::builder()
            .icon_name("boxxy-bedtime-symbolic")
            .pixel_size(14)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let hero_sleep_badge = gtk::Box::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .css_classes(["avatar-sleep-badge", "avatar-sleep-badge-large"])
            .visible(false)
            .build();
        hero_sleep_badge.append(&hero_sleep_icon);

        let hero_vibe_icon = gtk::Image::builder()
            .icon_name("boxxy-vibe-symbolic")
            .pixel_size(14)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let hero_vibe_badge = gtk::Box::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .css_classes(["avatar-vibe-badge", "avatar-sleep-badge-large"])
            .visible(false)
            .build();
        hero_vibe_badge.append(&hero_vibe_icon);

        let hero_avatar_overlay = gtk::Overlay::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .visible(false)
            .child(&hero_avatar)
            .build();
        hero_avatar_overlay.add_overlay(&hero_vibe_badge);
        hero_avatar_overlay.add_overlay(&hero_sleep_badge);
        hero_box.append(&hero_avatar_overlay);

        let hero_title = gtk::Label::builder()
            .label("Claw Mode")
            .css_classes(["title-2"])
            .halign(gtk::Align::Center)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .visible(false)
            .build();
        hero_box.append(&hero_title);

        let hero_subtitle = gtk::Label::builder()
            .label("Conversations will appear here")
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
        stats_box.set_visible(false);

        let memories_lbl = gtk::Label::builder()
            .label("🧠 0 Memories")
            .css_classes(["caption", "dim-label"])
            .build();
        stats_box.append(&memories_lbl);

        let stats_sep = gtk::Label::builder()
            .label("•")
            .css_classes(["caption", "dim-label"])
            .build();
        stats_box.append(&stats_sep);

        let tools_lbl = gtk::Label::builder()
            .label("🛠️ 0 Tools")
            .css_classes(["caption", "dim-label"])
            .build();
        stats_box.append(&tools_lbl);

        hero_box.append(&stats_box);
        widget.append(&hero_box);

        // --- Past Conversations (Resume) Section ---
        let sessions_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        sessions_box.set_vexpand(true);
        sessions_box.set_valign(gtk::Align::Fill);
        sessions_box.set_hexpand(true);
        sessions_box.set_visible(false);

        let sessions_list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .selection_mode(gtk::SelectionMode::None)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        let sessions_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&sessions_list)
            .build();
        sessions_box.append(&sessions_scroll);
        widget.append(&sessions_box);

        // --- Prominent Pending Tasks Section ---
        let on_cancel_rc: Rc<dyn Fn(uuid::Uuid) + 'static> = Rc::new(on_cancel_task);
        let tasks_section = TasksSection::new(on_cancel_rc);
        widget.append(tasks_section.widget());

        // --- Footer Details Panel ---
        let command_panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
        command_panel.set_valign(gtk::Align::End);
        command_panel.set_hexpand(true);
        command_panel.set_halign(gtk::Align::Fill);
        command_panel.set_margin_start(14);
        command_panel.set_margin_end(14);
        command_panel.set_margin_bottom(14);

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
                .css_classes(["caption"])
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

        let models_header_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        models_header_row.set_hexpand(true);

        let models_title_lbl = gtk::Label::builder()
            .label("Models")
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .hexpand(true)
            .xalign(0.0)
            .build();
        models_header_row.append(&models_title_lbl);

        let change_models_lbl = gtk::Label::builder()
            .label("Change…")
            .css_classes(["caption", "accent", "boxxy-link"])
            .halign(gtk::Align::End)
            .tooltip_text("Select AI Models")
            .build();
        attach_window_action_click(&change_models_lbl, "win.model-selection");
        models_header_row.append(&change_models_lbl);
        models_box.append(&models_header_row);

        let (claw_row, claw_model_lbl) = make_kv_row("Claw", "Default");
        claw_row.set_tooltip_text(Some("Select AI Models"));
        attach_window_action_click(&claw_row, "win.model-selection");
        models_box.append(&claw_row);

        let (dreams_row, dreams_model_lbl) = make_kv_row("Dreams", "Default");
        dreams_row.set_tooltip_text(Some("Select AI Models"));
        attach_window_action_click(&dreams_row, "win.model-selection");
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
        context_box.set_visible(false);

        footer_info_vbox.append(&context_box);

        // 3. MCP Servers Section
        let mcp_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        mcp_box.set_hexpand(true);
        mcp_box.set_halign(gtk::Align::Fill);

        let mcp_header_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        mcp_header_row.set_hexpand(true);

        let mcp_title_lbl = gtk::Label::builder()
            .label("MCP")
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .hexpand(true)
            .xalign(0.0)
            .build();
        mcp_header_row.append(&mcp_title_lbl);

        let configure_mcp_lbl = gtk::Label::builder()
            .label("Configure…")
            .css_classes(["caption", "accent", "boxxy-link"])
            .halign(gtk::Align::End)
            .tooltip_text("Configure MCP Servers")
            .build();
        attach_window_action_click(&configure_mcp_lbl, "win.open-mcp");
        mcp_header_row.append(&configure_mcp_lbl);
        mcp_box.append(&mcp_header_row);

        let mcp_rows_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        mcp_rows_box.set_hexpand(true);
        mcp_rows_box.set_halign(gtk::Align::Fill);
        mcp_rows_box.set_visible(false);
        mcp_box.append(&mcp_rows_box);

        footer_info_vbox.append(&mcp_box);
        command_panel.append(&footer_info_vbox);
        widget.append(&command_panel);

        let initial_settings = boxxy_preferences::Settings::load();
        let last_usage = Rc::new(Cell::new(SessionUsage::default()));
        let web_search_enabled = Rc::new(Cell::new(initial_settings.web_search_on_by_default));

        let initial_mcp_map: HashMap<String, bool> =
            boxxy_claw_protocol::characters::MCP_STATUS_CACHE
                .load()
                .iter()
                .cloned()
                .collect();

        let comp = Self {
            widget,
            hero_box,
            hero_icon,
            hero_avatar_overlay,
            hero_avatar,
            hero_sleep_badge,
            hero_vibe_badge,
            hero_title,
            hero_subtitle,
            badges_box,
            stats_box,
            sessions_box,
            sessions_list,
            is_new_session: Rc::new(Cell::new(true)),
            rendered_sessions_sig: Rc::new(RefCell::new((Vec::new(), Vec::new()))),
            active_options_popover: Rc::new(RefCell::new(None)),
            on_resume_session: Rc::new(RefCell::new(None)),
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
            memories_fetch_in_flight: Rc::new(Cell::new(false)),
            cost_lbl,
            mcp_box,
            mcp_rows_box,
            mcp_statuses: Rc::new(RefCell::new(initial_mcp_map)),
            last_usage,
            web_search_enabled,
            tasks_section,
        };

        comp.refresh_models_info(&initial_settings);
        populate_mcp_rows(
            &comp.mcp_box,
            &comp.mcp_rows_box,
            &initial_settings,
            &comp.mcp_statuses.borrow(),
        );
        comp.refresh_memories_count();
        comp.refresh_past_sessions();

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

        // Subscribe to live Preferences updates.
        let claw_model_sub = comp.claw_model_lbl.clone();
        let dreams_model_sub = comp.dreams_model_lbl.clone();
        let search_sub = comp.search_lbl.clone();
        let web_search_sub = comp.web_search_enabled.clone();
        let mcp_box_sub = comp.mcp_box.clone();
        let mcp_rows_sub = comp.mcp_rows_box.clone();
        let mcp_statuses_sub = comp.mcp_statuses.clone();
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
                populate_mcp_rows(
                    &mcp_box_sub,
                    &mcp_rows_sub,
                    &settings,
                    &mcp_statuses_sub.borrow(),
                );
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

        // Subscribe to CharacterRegistry claim updates for Past Conversations state.
        let past_ctx_reg = comp.past_sessions_ctx();
        let mut reg_rx = boxxy_claw_protocol::characters::subscribe_registry_changes();
        glib::spawn_future_local(async move {
            while reg_rx.changed().await.is_ok() {
                *past_ctx_reg.rendered_sessions_sig.borrow_mut() = (Vec::new(), Vec::new());
                if past_ctx_reg.is_new_session.get() {
                    populate_past_sessions(past_ctx_reg.clone(), true);
                }
            }
        });

        // Subscribe to live MCP connection statuses.
        let mcp_box_watch = comp.mcp_box.clone();
        let mcp_rows_watch = comp.mcp_rows_box.clone();
        let mcp_statuses_watch = comp.mcp_statuses.clone();
        let mut mcp_rx = boxxy_claw_protocol::characters::subscribe_mcp_status_changes();
        glib::spawn_future_local(async move {
            while mcp_rx.changed().await.is_ok() {
                let latest = boxxy_claw_protocol::characters::MCP_STATUS_CACHE.load();
                let map: HashMap<String, bool> = latest.iter().cloned().collect();
                if *mcp_statuses_watch.borrow() != map {
                    *mcp_statuses_watch.borrow_mut() = map;
                    let settings = boxxy_preferences::Settings::load();
                    populate_mcp_rows(
                        &mcp_box_watch,
                        &mcp_rows_watch,
                        &settings,
                        &mcp_statuses_watch.borrow(),
                    );
                }
            }
        });

        comp
    }

    fn past_sessions_ctx(&self) -> PastSessionsCtx {
        PastSessionsCtx {
            sessions_box: self.sessions_box.clone(),
            sessions_list: self.sessions_list.clone(),
            hero_box: self.hero_box.clone(),
            hero_icon: self.hero_icon.clone(),
            is_new_session: self.is_new_session.clone(),
            rendered_sessions_sig: self.rendered_sessions_sig.clone(),
            active_options_popover: self.active_options_popover.clone(),
            on_resume_session: self.on_resume_session.clone(),
        }
    }

    pub fn set_pane_state(
        &self,
        agent_name: &str,
        sleep: bool,
        pinned: bool,
        web_search_enabled: bool,
        vibe_enabled: bool,
    ) {
        while let Some(child) = self.badges_box.first_child() {
            self.badges_box.remove(&child);
        }

        if !agent_name.is_empty() {
            let is_incognito = boxxy_claw_protocol::characters::is_incognito_character(agent_name);
            let cache = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
            let fallback_names = [agent_name];
            let resolved_char_display: String;

            if is_incognito {
                let disp = boxxy_claw_protocol::characters::INCOGNITO_DISPLAY_NAME;
                let color = boxxy_claw_protocol::characters::INCOGNITO_COLOR;
                resolved_char_display = disp.to_string();
                self.set_hero_title_colored(disp, color);
                self.hero_subtitle
                    .set_label(boxxy_claw_protocol::characters::INCOGNITO_DUTIES);

                self.hero_avatar.set_text(Some(disp));
                self.hero_avatar.set_custom_image(None::<&gdk::Texture>);
                if let Some(texture) = load_incognito_texture() {
                    self.hero_avatar.set_custom_image(Some(&texture));
                }
                self.hero_avatar_overlay.set_visible(true);
                self.hero_sleep_badge.set_visible(false);
                self.hero_vibe_badge.set_visible(false);
                self.hero_icon.set_visible(false);
            } else if let Some(info) =
                boxxy_claw_protocol::characters::resolve_character(&cache, "", &fallback_names)
            {
                let formatted_name = resolve_character_display_name(info);
                resolved_char_display = formatted_name.clone();
                self.set_hero_title_colored(&formatted_name, &info.config.color);
                self.hero_subtitle.set_label(&info.config.duties);

                self.hero_avatar.set_text(Some(&formatted_name));
                self.hero_avatar.set_custom_image(None::<&gdk::Texture>);
                if let Some(texture) = load_character_avatar_texture(info) {
                    self.hero_avatar.set_custom_image(Some(&texture));
                }
                self.hero_avatar_overlay.set_visible(true);
                self.hero_sleep_badge.set_visible(sleep);
                self.hero_vibe_badge.set_visible(vibe_enabled && !sleep);
                self.hero_icon.set_visible(false);
            } else {
                resolved_char_display = prettify_character_name(agent_name);
                self.hero_title.set_label(&resolved_char_display);
                self.hero_subtitle
                    .set_label("System-eccentric agentic control.");
                self.hero_avatar_overlay.set_visible(false);
                self.hero_sleep_badge.set_visible(false);
                self.hero_vibe_badge.set_visible(false);
                self.hero_icon.set_visible(true);
            }

            if is_incognito {
                let status_line = gtk::Label::builder()
                    .label("Incognito Mode  •  Ephemeral")
                    .css_classes(["caption", "dim-label"])
                    .build();
                self.badges_box.append(&status_line);
                self.stats_box.set_visible(false);
                self.tasks_section.set_character_context(None, false);
                self.hero_box.set_vexpand(true);
                self.hero_box.set_valign(gtk::Align::Center);
                self.hero_box.set_margin_top(12);
                self.hero_box.set_margin_bottom(0);
            } else {
                let vibe_str = if vibe_enabled { "Vibe On" } else { "Vibe Off" };
                let sleep_str = if sleep { "Sleep On" } else { "Sleep Off" };
                let pin_str = if pinned { "Pin On" } else { "Pin Off" };
                let web_str = if web_search_enabled {
                    "Web On"
                } else {
                    "Web Off"
                };
                let status_line = gtk::Label::builder()
                    .label(format!(
                        "{vibe_str}  •  {sleep_str}  •  {pin_str}  •  {web_str}"
                    ))
                    .css_classes(["caption", "dim-label"])
                    .build();
                self.badges_box.append(&status_line);
                self.stats_box.set_visible(true);
                // 3-Row Active Character Layout:
                // Row 1 (Top): Character avatar + info (`vexpand = false`)
                // Row 2 (Mid): Pending Tasks & Interval Sessions (`vexpand = true`)
                // Row 3 (Bot): Models / Context / MCP (`vexpand = false`)
                self.hero_box.set_vexpand(false);
                self.hero_box.set_valign(gtk::Align::Start);
                self.hero_box.set_margin_top(16);
                self.hero_box.set_margin_bottom(4);
                self.tasks_section
                    .set_character_context(Some(&resolved_char_display), true);
            }

            self.hero_title.set_visible(true);
            self.badges_box.set_visible(true);
            self.is_new_session.set(false);
            self.sessions_box.set_visible(false);
            self.hero_box.set_visible(true);
            self.hero_icon.set_pixel_size(96);
            self.context_box.set_visible(true);
        } else {
            self.hero_title.set_visible(false);
            self.hero_subtitle
                .set_label("Conversations will appear here");
            self.hero_avatar_overlay.set_visible(false);
            self.hero_sleep_badge.set_visible(false);
            self.hero_vibe_badge.set_visible(false);
            self.hero_icon.set_pixel_size(72);
            self.hero_icon.set_visible(true);
            self.hero_box.set_vexpand(true);
            self.hero_box.set_valign(gtk::Align::Center);
            self.hero_box.set_margin_top(12);
            self.hero_box.set_margin_bottom(0);
            self.badges_box.set_visible(false);
            self.stats_box.set_visible(false);
            self.is_new_session.set(true);
            self.context_box.set_visible(false);
            self.tasks_section.set_character_context(None, false);
            self.refresh_past_sessions();
        }

        self.web_search_enabled.set(web_search_enabled);
        let settings = boxxy_preferences::Settings::load();
        self.refresh_models_info(&settings);
        self.refresh_memories_count();
    }

    fn set_hero_title_colored(&self, name: &str, raw_color: &str) {
        let escaped = glib::markup_escape_text(name);
        if is_safe_hex_color(raw_color) {
            self.hero_title.set_markup(&format!(
                "<span foreground=\"{raw_color}\"><b>{escaped}</b></span>"
            ));
        } else {
            self.hero_title.set_markup(&format!("<b>{escaped}</b>"));
        }
    }

    pub fn set_on_resume_session<FR: Fn(String, Option<String>) + 'static>(&self, cb: FR) {
        *self.on_resume_session.borrow_mut() = Some(Rc::new(cb));
    }

    pub fn set_on_cancel_all_tasks<FA: Fn() + 'static>(&self, cb: FA) {
        self.tasks_section.set_on_cancel_all(cb);
    }

    pub fn set_on_toggle_pause_task<FP: Fn(uuid::Uuid) + 'static>(&self, cb: FP) {
        self.tasks_section.set_on_toggle_pause(cb);
    }

    #[must_use]
    pub fn is_options_popover_open(&self) -> bool {
        self.active_options_popover
            .borrow()
            .as_ref()
            .is_some_and(|p| p.is_visible())
    }

    pub fn refresh_past_sessions(&self) {
        if self.is_options_popover_open() {
            return;
        }
        populate_past_sessions(self.past_sessions_ctx(), false);
    }

    pub fn refresh_memories_count(&self) {
        if self.memories_fetch_in_flight.replace(true) {
            return;
        }
        let memories_lbl = self.memories_lbl.clone();
        let in_flight = self.memories_fetch_in_flight.clone();
        glib::spawn_future_local(async move {
            let res = boxxy_sys_utils::runtime()
                .spawn(async move {
                    let db = boxxy_db::Db::new().await?;
                    let store = boxxy_db::store::Store::new(db.pool());
                    store.count_memories().await
                })
                .await;
            in_flight.set(false);
            if let Ok(Ok(count)) = res {
                let n = u64::try_from(count.max(0)).unwrap_or(0);
                let word = if n == 1 { "Memory" } else { "Memories" };
                memories_lbl.set_label(&format!("🧠 {} {}", format_number_with_commas(n), word));
            }
        });
    }

    pub const fn refresh_visibility(&self) {}

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
        populate_mcp_rows(
            &self.mcp_box,
            &self.mcp_rows_box,
            settings,
            &self.mcp_statuses.borrow(),
        );
    }

    pub fn set_mcp_statuses(&self, statuses: Vec<(String, bool)>) {
        boxxy_claw_protocol::characters::update_mcp_status_cache(statuses.clone());
        let map: HashMap<String, bool> = statuses.into_iter().collect();
        if *self.mcp_statuses.borrow() != map {
            *self.mcp_statuses.borrow_mut() = map;
            let settings = boxxy_preferences::Settings::load();
            populate_mcp_rows(
                &self.mcp_box,
                &self.mcp_rows_box,
                &settings,
                &self.mcp_statuses.borrow(),
            );
        }
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
        self.context_box.set_visible(!self.is_new_session.get());
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
        self.tasks_section.update_tasks(tasks);
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

fn attach_window_action_click(target: &impl IsA<gtk::Widget>, action_name: &'static str) {
    let widget = target.as_ref();
    widget.set_cursor_from_name(Some("pointer"));
    let gesture = gtk::GestureClick::new();
    let weak = widget.downgrade();
    gesture.connect_released(move |_, _, _, _| {
        if let Some(w) = weak.upgrade()
            && let Some(root) = w.root()
        {
            let _ = gtk::prelude::WidgetExt::activate_action(&root, action_name, None);
        }
    });
    widget.add_controller(gesture);
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

    if settings.get_effective_api_key("Tavily").trim().is_empty() {
        "Off"
    } else {
        "Tavily"
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
        .map_or_else(|| "Default".to_string(), |m| m.format_label());
    let memory_label = settings
        .memory_model
        .as_ref()
        .map_or_else(|| "Default".to_string(), |m| m.format_label());
    let search_label = resolve_active_search_label(settings, web_search_local_enabled);

    claw_model_lbl.set_label(&claw_label);
    dreams_model_lbl.set_label(&memory_label);
    search_lbl.set_label(search_label);
}

fn populate_mcp_rows(
    mcp_box: &gtk::Box,
    mcp_rows_box: &gtk::Box,
    settings: &boxxy_preferences::Settings,
    statuses: &HashMap<String, bool>,
) {
    while let Some(child) = mcp_rows_box.first_child() {
        mcp_rows_box.remove(&child);
    }

    let active_servers: Vec<_> = settings.mcp_servers.iter().filter(|s| s.enabled).collect();

    if active_servers.is_empty() {
        mcp_rows_box.set_visible(false);
        mcp_box.set_visible(true);
        return;
    }

    for srv in active_servers {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_hexpand(true);

        let is_connected = statuses.get(&srv.name).copied().unwrap_or(false);
        let dot_class = if is_connected { "success" } else { "error" };
        let status_text = if is_connected {
            "Connected"
        } else {
            "Not connected"
        };

        let dot_lbl = gtk::Label::builder()
            .label("•")
            .css_classes(["caption", dot_class])
            .halign(gtk::Align::Start)
            .build();
        row.append(&dot_lbl);

        let name_lbl = gtk::Label::builder()
            .label(&srv.name)
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .hexpand(true)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        row.append(&name_lbl);

        let status_lbl = gtk::Label::builder()
            .label(status_text)
            .css_classes(["caption", "dim-label"])
            .halign(gtk::Align::End)
            .build();
        row.append(&status_lbl);

        mcp_rows_box.append(&row);
    }

    mcp_rows_box.set_visible(true);
    mcp_box.set_visible(true);
}
