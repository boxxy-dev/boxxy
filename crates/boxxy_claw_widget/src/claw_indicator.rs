use boxxy_claw_protocol::AgentStatus;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone)]
pub struct ClawIndicator {
    container: gtk::Box,       // Detailed indicator (for drawer)
    hbox: gtk::Box,            // Pill container
    badge_container: gtk::Box, // Small badge (always in terminal)
    badge_box: gtk::Box,
    badge_label: gtk::Label,
    badge_revealer: gtk::Revealer,
    clock_icon: gtk::Image,
    sleep_icon: gtk::Image,
    lock_icon: gtk::Image,
    revealer: gtk::Revealer,
    spinner: gtk::Image,
    icon: gtk::Image,
    label: gtk::Label,
    main_btn: gtk::Button,
    cancel_btn: gtk::Button,
    action_type: Rc<RefCell<u8>>, // 0 = none, 1 = lazy, 2 = proactive
    is_active: Rc<Cell<bool>>,
    is_evicted: Rc<Cell<bool>>,
    has_tasks: Rc<Cell<bool>>,
    badge_provider: gtk::CssProvider,
    pill_provider: gtk::CssProvider,
    is_drawer_open: Rc<Cell<bool>>,
    is_claw_active: Rc<Cell<bool>>,
}

impl ClawIndicator {
    pub fn new() -> Self {
        let base_css = "
            .indicator-container {
                background: transparent;
                border: none;
                box-shadow: none;
                padding: 0;
                margin: 0;
            }
            .badge-label {
                border-radius: 9999px;
                padding: 1px 8px;
                font-weight: bold;
                font-size: 0.68rem;
                color: white;
            }
            .badge-label image {
                -gtk-icon-size: 10px;
            }
            .badge-label.warning {
                color: #fff3cd;
            }
            .badge-label.evicted {
                filter: grayscale(100%);
                opacity: 0.25;
            }
            .indicator-container revealer {
                padding: 0;
                margin: 0;
            }
            .status-label {
                font-size: 0.9rem;
                font-weight: bold;
                opacity: 0.8;
            }
            .claw-status-pill {
                background-color: alpha(currentColor, 0.08);
                border: none;
                border-radius: 8px;
                padding: 0 8px 0 12px;
                min-height: 34px;
            }
            .claw-status-pill.destructive-action {
                background-color: @error_bg_color;
                color: @error_fg_color;
            }
            .claw-status-pill.destructive-action .status-label {
                color: inherit;
                opacity: 1.0;
            }
            .claw-status-pill.destructive-action image {
                color: inherit;
            }
            .claw-status-pill .flat.circular {
                min-width: 24px;
                min-height: 24px;
                padding: 0;
                margin-left: 4px;
            }
            .claw-status-pill .status-label {
                margin-left: 4px;
                margin-right: 4px;
            }
        ";
        let base_provider = gtk::CssProvider::new();
        base_provider.load_from_string(base_css);
        #[allow(deprecated)]
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().unwrap(),
            &base_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // 1. Small Badge Widget (Always in terminal)
        let badge_container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .css_classes(["indicator-container"])
            .spacing(0)
            .build();

        // 2. Detailed Indicator Widget (For drawer)
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        container.set_valign(gtk::Align::Center);

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
        revealer.set_transition_duration(280);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hbox.set_valign(gtk::Align::Center);
        hbox.add_css_class("claw-status-pill");

        let svg = gtk::Svg::from_resource("/dev/boxxy/BoxxyTerminal/icons/boxxy-spinner.gpa");
        svg.play();

        let spinner = gtk::Image::builder().paintable(&svg).pixel_size(20).build();
        spinner.add_css_class("claw-spinner");

        spinner.connect_map({
            let svg = svg.clone();
            move |widget| {
                if let Some(native) = widget.native() {
                    if let Some(surface) = native.surface() {
                        svg.set_frame_clock(&surface.frame_clock());
                    }
                }
            }
        });

        hbox.append(&spinner);

        let icon = gtk::Image::from_icon_name("boxxy-boxxyclaw-symbolic");
        icon.add_css_class("accent");
        icon.set_pixel_size(13);
        hbox.append(&icon);

        let label = gtk::Label::new(Some("Working"));
        label.add_css_class("status-label");
        hbox.append(&label);

        let main_btn = gtk::Button::builder()
            .label("Action")
            .css_classes(["flat"])
            .build();
        main_btn.set_visible(false);
        hbox.append(&main_btn);

        let cancel_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .tooltip_text("Cancel")
            .build();
        cancel_btn.set_valign(gtk::Align::Center);

        hbox.append(&cancel_btn);
        revealer.set_child(Some(&hbox));
        container.append(&revealer);

        let pill_provider = gtk::CssProvider::new();
        #[allow(deprecated)]
        hbox.style_context()
            .add_provider(&pill_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let action_type = Rc::new(RefCell::new(0));

        // Shared Badge logic. Start hidden — the badge only surfaces once
        // the daemon sends us an Identity event with a real agent name.
        // Without an identity, drawing a placeholder "CLAW" pill is noisy.
        let badge_revealer = gtk::Revealer::new();
        badge_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        badge_revealer.set_transition_duration(280);
        badge_revealer.set_reveal_child(false);

        let badge_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        badge_box.add_css_class("badge-label");
        badge_box.set_valign(gtk::Align::Center);
        badge_box.set_height_request(20);

        let badge_label = gtk::Label::builder().valign(gtk::Align::Center).build();
        badge_box.append(&badge_label);

        let clock_icon = gtk::Image::builder()
            .icon_name("boxxy-timer-symbolic")
            .visible(false)
            .build();
        badge_box.append(&clock_icon);

        let sleep_icon = gtk::Image::builder()
            .icon_name("boxxy-bedtime-symbolic")
            .visible(false)
            .css_classes(["warning"])
            .build();
        badge_box.append(&sleep_icon);

        let lock_icon = gtk::Image::builder()
            .icon_name("boxxy-lock-symbolic")
            .visible(false)
            .build();
        badge_box.append(&lock_icon);

        badge_revealer.set_child(Some(&badge_box));
        badge_container.append(&badge_revealer);

        let badge_provider = gtk::CssProvider::new();
        #[allow(deprecated)]
        badge_box
            .style_context()
            .add_provider(&badge_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let pill_provider = gtk::CssProvider::new();
        #[allow(deprecated)]
        hbox.style_context()
            .add_provider(&pill_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        Self {
            container,
            hbox,
            badge_container,
            badge_box,
            badge_label,
            badge_revealer,
            clock_icon,
            sleep_icon,
            lock_icon,
            revealer,
            spinner,
            icon,
            label,
            main_btn,
            cancel_btn,
            action_type,
            is_active: Rc::new(Cell::new(false)),
            is_evicted: Rc::new(Cell::new(false)),
            has_tasks: Rc::new(Cell::new(false)),
            badge_provider,
            pill_provider,
            is_drawer_open: Rc::new(Cell::new(false)),
            is_claw_active: Rc::new(Cell::new(false)),
        }
    }

    /// Detailed indicator for the Drawer
    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    /// Persistent badge for the Terminal Overlay
    pub fn badge(&self) -> &gtk::Box {
        &self.badge_container
    }

    pub fn set_drawer_open(&self, open: bool) {
        self.is_drawer_open.set(open);
        self.update_visibility();
    }

    pub fn set_callbacks<F1: Fn() + 'static, F2: Fn() + 'static, F3: Fn() + 'static>(
        &self,
        on_cancel: F1,
        on_lazy_click: F2,
        on_proactive_click: F3,
    ) {
        let p_clone = self.revealer.clone();
        let badge_revealer_clone = self.badge_revealer.clone();
        let clock_clone = self.clock_icon.clone();
        let has_tasks_clone = self.has_tasks.clone();

        self.cancel_btn.connect_clicked(move |_| {
            p_clone.set_reveal_child(false);
            badge_revealer_clone.set_reveal_child(true);
            clock_clone.set_visible(has_tasks_clone.get());
            on_cancel();
        });

        let on_lazy_click = Rc::new(on_lazy_click);
        let on_proactive_click = Rc::new(on_proactive_click);
        let action_type = self.action_type.clone();

        self.main_btn.connect_clicked(move |_| {
            let at = *action_type.borrow();
            if at == 1 {
                on_lazy_click();
            } else if at == 2 {
                on_proactive_click();
            }
        });
    }

    pub fn set_visible(&self, visible: bool) {
        self.is_claw_active.set(visible);
        self.update_visibility();
    }

    fn update_visibility(&self) {
        let claw_active = self.is_claw_active.get();
        let drawer_open = self.is_drawer_open.get();
        self.badge_container
            .set_visible(claw_active && !drawer_open);
        self.container.set_visible(claw_active);
    }

    pub fn set_evicted(&self, evicted: bool) {
        self.is_evicted.set(evicted);
        if evicted {
            self.badge_box.add_css_class("evicted");
            self.badge_label.set_label("EVICTED");
        } else {
            self.badge_box.remove_css_class("evicted");
        }
    }

    pub fn set_mode(&self, status: AgentStatus) {
        self.clock_icon.set_visible(false);
        self.sleep_icon.set_visible(false);
        self.lock_icon.set_visible(false);
        self.badge_box.remove_css_class("warning");

        // Status only drives the status-icon row + warning CSS class, not
        // the label text. The label is owned by `set_identity()` so the
        // badge always reads as the agent's real name (once known) rather
        // than the generic "CLAW" / "WORKING" placeholder.
        match status {
            AgentStatus::Waiting | AgentStatus::Working => {}
            AgentStatus::Sleep => {
                self.sleep_icon.set_visible(true);
                self.badge_box.add_css_class("warning");
            }
            AgentStatus::Locking { .. } => {
                self.lock_icon.set_visible(true);
            }
            AgentStatus::Faulted { .. } => {
                self.badge_box.add_css_class("warning");
            }
            AgentStatus::Off => {
                // Dormant session — retract the badge so an "off" pane has
                // no visible indicator. It comes back on the next status
                // change that isn't Off.
                self.badge_revealer.set_reveal_child(false);
                return;
            }
        }

        // Any non-Off status: make sure the badge is revealed (if we
        // already have an identity to show).
        if !self.badge_label.text().is_empty() {
            self.badge_revealer.set_reveal_child(true);
        }
    }

    pub fn show_thinking(&self, agent_name: &str, tool_name: Option<&str>) {
        self.is_active.set(true);

        let label_text = if let Some(tool) = tool_name {
            format!("{}..", self.pretty_tool_name(tool))
        } else {
            "Drinking Water..".to_string()
        };

        self.label.set_label(&label_text);
        self.icon.set_visible(false);

        self.main_btn.set_visible(false);
        self.revealer.set_reveal_child(true);
        self.spinner.set_visible(true);
        self.hbox.remove_css_class("destructive-action");

        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let color = registry
            .iter()
            .find(|c| c.config.name == agent_name || c.config.display_name == agent_name)
            .map(|c| c.config.color.clone())
            .unwrap_or_else(|| "rgba(38, 162, 105, 0.85)".to_string()); // Fallback green

        #[allow(deprecated)]
        self.pill_provider.load_from_string(&format!(
            ".status-label {{ color: {}; }} .claw-spinner {{ color: {}; }}",
            color, color
        ));
    }

    fn pretty_tool_name(&self, tool: &str) -> String {
        match tool {
            "sys_shell_exec" => "Executing command".to_string(),
            "file_read" | "read_file" => "Reading file".to_string(),
            "file_write" | "write_file" | "replace" => "Writing file".to_string(),
            "file_delete" => "Deleting file".to_string(),
            "list_directory" => "Listing directory".to_string(),
            "get_system_info" => "Fetching system info".to_string(),
            "list_processes" => "Listing processes".to_string(),
            "kill_process" => "Killing process".to_string(),
            "get_clipboard" => "Reading clipboard".to_string(),
            "set_clipboard" => "Writing clipboard".to_string(),
            "web_search" => "Searching the web".to_string(),
            "http_fetch" => "Fetching URL".to_string(),
            "memory_store" => "Saving memory".to_string(),
            "memory_delete" => "Deleting memory".to_string(),
            "read_scrollback_page" => "Reading terminal scrollback".to_string(),
            "terminal_exec" => "Executing terminal command".to_string(),
            "spawn_agent" => "Spawning agent".to_string(),
            "close_agent" => "Closing agent".to_string(),
            "delegate_task" => "Delegating task".to_string(),
            "delegate_task_async" => "Delegating task".to_string(),
            "summon_headless_worker" => "Summoning worker".to_string(),
            _ => tool.replace('_', " "),
        }
    }

    pub fn show_diagnosis_ready(&self) {
        self.is_active.set(true);
        self.label.set_label("Diagnosis Ready");
        self.main_btn.set_label("View");
        self.main_btn.set_visible(true);
        *self.action_type.borrow_mut() = 2; // proactive
        self.revealer.set_reveal_child(true);
        self.spinner.set_visible(false);
        self.icon.set_visible(true);
        self.icon.set_icon_name(Some("boxxy-bug-symbolic"));
        self.hbox.remove_css_class("destructive-action");

        #[allow(deprecated)]
        self.pill_provider
            .load_from_string(".status-label { color: rgba(255,255,255,0.8); }");
    }

    pub fn show_lazy_error(&self) {
        self.is_active.set(true);
        self.label.set_label("Error Detected");
        self.main_btn.set_label("Diagnose");
        self.main_btn.set_visible(true);
        *self.action_type.borrow_mut() = 1; // lazy
        self.revealer.set_reveal_child(true);
        self.spinner.set_visible(false);
        self.icon.set_visible(true);
        self.icon
            .set_icon_name(Some("boxxy-dialog-warning-symbolic"));
        self.hbox.add_css_class("destructive-action");

        #[allow(deprecated)]
        self.pill_provider
            .load_from_string(".status-label { color: inherit; }");
    }

    pub fn hide(&self) {
        self.is_active.set(false);
        self.revealer.set_reveal_child(false);
        self.hbox.remove_css_class("destructive-action");
        // Only surface the small badge if we actually have an identity to
        // display — otherwise hiding the detailed pill shouldn't cause an
        // empty placeholder badge to appear.
        let has_identity = !self.badge_label.text().is_empty();
        self.badge_revealer.set_reveal_child(has_identity);
        self.clock_icon.set_visible(self.has_tasks.get());
    }

    pub fn set_has_tasks(&self, has: bool) {
        self.has_tasks.set(has);
        if !self.is_active.get() {
            self.clock_icon.set_visible(has);
        }
    }

    pub fn set_identity(&self, name: &str, character_id: &str) {
        self.is_evicted.set(false);
        self.badge_box.remove_css_class("evicted");

        if name.is_empty() {
            self.badge_label.set_text("");
            self.badge_revealer.set_reveal_child(false);
            return;
        }

        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();

        // Lookup strategy: 1. ID, 2. Internal Name, 3. Display Name
        let info = registry
            .iter()
            .find(|c| c.config.id == character_id)
            .or_else(|| registry.iter().find(|c| c.config.name == name))
            .or_else(|| registry.iter().find(|c| c.config.display_name == name));

        if let Some(info) = info {
            self.badge_label.set_text(&info.config.display_name);

            #[allow(deprecated)]
            self.badge_provider.load_from_string(&format!(
                ".badge-label {{ background-color: {}; }}",
                info.config.color
            ));
        } else {
            self.badge_label.set_text(name);

            #[allow(deprecated)]
            self.badge_provider
                .load_from_string(".badge-label { background-color: #6c7086; }");
        }

        // First real identity — reveal the badge.
        self.badge_revealer.set_reveal_child(true);
    }

    pub fn update_settings(&self) {
        // No-op for now
    }
}
