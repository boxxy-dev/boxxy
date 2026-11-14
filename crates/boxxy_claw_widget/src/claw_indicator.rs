use boxxy_claw_protocol::AgentStatus;
use boxxy_claw_ui::{
    claw_badge_bg_color_class, claw_pill_spinner_color_class, resolve_character_display_name,
};
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone)]
pub struct ClawIndicator {
    container: gtk::Box,       // Detailed thinking pill (inside drawer)
    hbox: gtk::Box,            // Drawer pill container
    badge_container: gtk::Box, // Floating top-right overlay (always over terminal)
    badge_box: gtk::Box,
    badge_label: gtk::Label,
    badge_revealer: gtk::Revealer,
    alert_revealer: gtk::Revealer,
    alert_hbox: gtk::Box,
    alert_icon: gtk::Image,
    alert_label: gtk::Label,
    alert_action_btn: gtk::Button,
    alert_dismiss_btn: gtk::Button,
    clock_icon: gtk::Image,
    sleep_icon: gtk::Image,
    vibe_icon: gtk::Image,
    lock_icon: gtk::Image,
    revealer: gtk::Revealer,
    spinner_svg: gtk::Svg,
    spinner_playing: Rc<Cell<bool>>,
    spinner: gtk::Image,
    label: gtk::Label,
    cancel_btn: gtk::Button,
    alert_type: Rc<Cell<u8>>, // 0 = none, 1 = lazy error, 2 = proactive diagnosis
    is_active: Rc<Cell<bool>>,
    is_evicted: Rc<Cell<bool>>,
    is_off: Rc<Cell<bool>>,
    has_tasks: Rc<Cell<bool>>,
    host_display_name: Rc<RefCell<String>>,
    host_slug: Rc<RefCell<String>>,
    host_character_id: Rc<RefCell<String>>,
    active_badge_class: Rc<RefCell<String>>,
    active_pill_class: Rc<RefCell<String>>,
    is_drawer_open: Rc<Cell<bool>>,
    is_claw_active: Rc<Cell<bool>>,
    is_busy_hidden: Rc<Cell<bool>>,
    hide_character_badge: Rc<Cell<bool>>,
}

impl Default for ClawIndicator {
    fn default() -> Self {
        Self::new()
    }
}

impl ClawIndicator {
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn new() -> Self {
        // 1. Floating Terminal Overlay Container (hosts both the passive character badge
        //    and the interactive "Error Detected / Diagnose" lazy-error pill).
        let badge_container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .css_classes(["indicator-container"])
            .spacing(6)
            .can_target(false)
            .build();

        // 2. Detailed Thinking Indicator Widget (embedded inside the drawer's bottom row)
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        container.set_valign(gtk::Align::Center);

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
        revealer.set_transition_duration(280);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hbox.set_valign(gtk::Align::Center);
        hbox.add_css_class("claw-status-pill");

        let spinner_svg =
            gtk::Svg::from_resource("/dev/boxxy/BoxxyTerminal/icons/boxxy-spinner.gpa");
        spinner_svg.pause();
        let spinner_playing = Rc::new(Cell::new(false));

        let spinner = gtk::Image::builder()
            .paintable(&spinner_svg)
            .pixel_size(20)
            .build();
        spinner.add_css_class("claw-spinner");

        spinner.connect_map({
            let svg = spinner_svg.clone();
            let playing = spinner_playing.clone();
            move |widget| {
                if let Some(native) = widget.native()
                    && let Some(surface) = native.surface()
                {
                    svg.set_frame_clock(&surface.frame_clock());
                }
                if playing.get() {
                    svg.play();
                } else {
                    svg.pause();
                }
            }
        });
        spinner.connect_unmap({
            let svg = spinner_svg.clone();
            move |_| {
                svg.pause();
            }
        });

        hbox.append(&spinner);

        let label = gtk::Label::new(Some("Working"));
        label.add_css_class("status-label");
        hbox.append(&label);

        let cancel_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .tooltip_text("Cancel")
            .valign(gtk::Align::Center)
            .build();
        hbox.append(&cancel_btn);

        revealer.set_child(Some(&hbox));
        container.append(&revealer);

        // 3. Floating Lazy-Error / Proactive Diagnosis Alert Pill (over terminal corner)
        let alert_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .transition_duration(220)
            .reveal_child(false)
            .can_target(false)
            .build();

        let alert_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        alert_hbox.set_valign(gtk::Align::Center);
        alert_hbox.add_css_class("claw-status-pill");

        let alert_icon = gtk::Image::from_icon_name("boxxy-dialog-warning-symbolic");
        alert_icon.set_pixel_size(13);
        alert_hbox.append(&alert_icon);

        let alert_label = gtk::Label::new(Some("Error Detected"));
        alert_label.add_css_class("status-label");
        alert_hbox.append(&alert_label);

        let alert_action_btn = gtk::Button::builder()
            .label("Diagnose")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build();
        alert_hbox.append(&alert_action_btn);

        let alert_dismiss_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .tooltip_text("Dismiss")
            .valign(gtk::Align::Center)
            .build();
        alert_hbox.append(&alert_dismiss_btn);

        alert_revealer.set_child(Some(&alert_hbox));
        badge_container.append(&alert_revealer);

        // 4. Passive Floating Character Badge. Starts hidden — only surfaces once
        //    the daemon sends an Identity event with a real agent name.
        let badge_revealer = gtk::Revealer::new();
        badge_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        badge_revealer.set_transition_duration(280);
        badge_revealer.set_reveal_child(false);
        badge_revealer.set_can_target(false);

        let badge_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        badge_box.add_css_class("badge-label");
        badge_box.set_valign(gtk::Align::Center);
        badge_box.set_height_request(20);
        badge_box.set_can_target(false);

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

        let vibe_icon = gtk::Image::builder()
            .icon_name("boxxy-vibe-symbolic")
            .visible(false)
            .build();
        badge_box.append(&vibe_icon);

        let lock_icon = gtk::Image::builder()
            .icon_name("boxxy-lock-symbolic")
            .visible(false)
            .build();
        badge_box.append(&lock_icon);

        badge_revealer.set_child(Some(&badge_box));
        badge_container.append(&badge_revealer);

        let ind = Self {
            container,
            hbox,
            badge_container,
            badge_box,
            badge_label,
            badge_revealer,
            alert_revealer,
            alert_hbox,
            alert_icon,
            alert_label,
            alert_action_btn,
            alert_dismiss_btn,
            clock_icon,
            sleep_icon,
            vibe_icon,
            lock_icon,
            revealer,
            spinner_svg,
            spinner_playing,
            spinner,
            label,
            cancel_btn,
            alert_type: Rc::new(Cell::new(0)),
            is_active: Rc::new(Cell::new(false)),
            is_evicted: Rc::new(Cell::new(false)),
            is_off: Rc::new(Cell::new(false)),
            has_tasks: Rc::new(Cell::new(false)),
            host_display_name: Rc::new(RefCell::new(String::new())),
            host_slug: Rc::new(RefCell::new(String::new())),
            host_character_id: Rc::new(RefCell::new(String::new())),
            active_badge_class: Rc::new(RefCell::new(String::new())),
            active_pill_class: Rc::new(RefCell::new(String::new())),
            is_drawer_open: Rc::new(Cell::new(false)),
            is_claw_active: Rc::new(Cell::new(false)),
            is_busy_hidden: Rc::new(Cell::new(false)),
            hide_character_badge: Rc::new(Cell::new(false)),
        };

        {
            let ind_dismiss = ind.clone();
            ind.alert_dismiss_btn.connect_clicked(move |_| {
                ind_dismiss.dismiss_alert();
            });
        }

        ind
    }

    /// Detailed thinking indicator for the Drawer.
    #[must_use]
    pub const fn widget(&self) -> &gtk::Box {
        &self.container
    }

    /// Persistent floating badge & lazy-error pill for the Terminal Overlay.
    #[must_use]
    pub const fn badge(&self) -> &gtk::Box {
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
        let ind_cancel = self.clone();
        self.cancel_btn.connect_clicked(move |_| {
            ind_cancel.hide();
            on_cancel();
        });

        let ind_action = self.clone();
        self.alert_action_btn.connect_clicked(move |_| {
            let at = ind_action.alert_type.get();
            ind_action.dismiss_alert();
            if at == 1 {
                on_lazy_click();
            } else if at == 2 {
                on_proactive_click();
            }
        });
    }

    pub fn trigger_cancel(&self) {
        self.cancel_btn.emit_clicked();
    }

    pub fn connect_cancelled<F: Fn() + 'static>(&self, f: F) {
        self.cancel_btn.connect_clicked(move |_| {
            f();
        });
    }

    pub fn grab_cancel_focus(&self) {
        self.cancel_btn.grab_focus();
    }

    pub fn set_visible(&self, visible: bool) {
        self.is_claw_active.set(visible);
        if !visible {
            self.is_busy_hidden.set(false);
            self.dismiss_alert();
        }
        self.update_visibility();
    }

    /// Temporarily suppresses the floating terminal corner badge while a full-screen
    /// TUI owns the alternate screen, without disabling the drawer's thinking indicator.
    pub fn set_busy_hidden(&self, busy: bool) {
        if self.is_busy_hidden.replace(busy) != busy {
            self.update_visibility();
        }
    }

    fn update_visibility(&self) {
        let claw_active = self.is_claw_active.get();
        let drawer_open = self.is_drawer_open.get();
        let busy_hidden = self.is_busy_hidden.get();
        let has_alert = self.alert_type.get() != 0;
        let hide_badge = self.hide_character_badge.get() && !has_alert;

        let show_floating = claw_active && !drawer_open && !busy_hidden && !hide_badge;
        self.badge_container.set_visible(show_floating);
        self.badge_container
            .set_can_target(show_floating && has_alert);
        self.container.set_visible(claw_active);
    }

    fn sync_badge_revealer(&self) {
        let has_alert = self.alert_type.get() != 0;
        let has_identity = !self.badge_label.text().is_empty();
        let should_reveal =
            !has_alert && (self.is_evicted.get() || (!self.is_off.get() && has_identity));
        self.badge_revealer.set_reveal_child(should_reveal);
    }

    pub fn dismiss_alert(&self) {
        if self.alert_type.replace(0) != 0 {
            self.alert_revealer.set_reveal_child(false);
            self.alert_revealer.set_can_target(false);
            self.alert_hbox.remove_css_class("destructive-action");
            self.sync_badge_revealer();
            self.update_visibility();
        }
    }

    pub fn set_evicted(&self, evicted: bool) {
        self.is_evicted.set(evicted);
        if evicted {
            self.dismiss_alert();
            self.badge_box.add_css_class("evicted");
            self.badge_label.set_label("EVICTED");
        } else {
            self.badge_box.remove_css_class("evicted");
            let saved_name = self.host_display_name.borrow();
            self.badge_label.set_label(&saved_name);
        }
        self.sync_badge_revealer();
    }

    pub fn set_vibe(&self, vibe: bool) {
        self.vibe_icon.set_visible(vibe);
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn set_mode(&self, status: AgentStatus) {
        self.is_busy_hidden.set(false);
        self.clock_icon.set_visible(false);
        self.sleep_icon.set_visible(false);
        self.lock_icon.set_visible(false);
        self.badge_box.remove_css_class("warning");

        match status {
            AgentStatus::Waiting | AgentStatus::Working => {
                self.is_off.set(false);
            }
            AgentStatus::Sleep => {
                self.is_off.set(false);
                self.sleep_icon.set_visible(true);
                self.badge_box.add_css_class("warning");
            }
            AgentStatus::Locking { .. } => {
                self.is_off.set(false);
                self.lock_icon.set_visible(true);
            }
            AgentStatus::Faulted { .. } => {
                self.is_off.set(false);
                self.badge_box.add_css_class("warning");
            }
            AgentStatus::Off => {
                self.is_off.set(true);
                self.dismiss_alert();
                self.sync_badge_revealer();
                return;
            }
        }

        if !self.is_active.get() {
            self.clock_icon.set_visible(self.has_tasks.get());
        }
        self.sync_badge_revealer();
        self.update_visibility();
    }

    pub fn show_thinking(&self, agent_name: &str, tool_name: Option<&str>) {
        self.dismiss_alert();
        self.is_active.set(true);

        let host_disp = self.host_display_name.borrow();
        let host_slug = self.host_slug.borrow();
        let is_guest = !agent_name.is_empty()
            && (!host_disp.is_empty() || !host_slug.is_empty())
            && !host_disp.eq_ignore_ascii_case(agent_name)
            && !host_slug.eq_ignore_ascii_case(agent_name);

        let label_text = match (is_guest, tool_name) {
            (true, Some(tool)) => format!("{agent_name}: {}..", Self::pretty_tool_name(tool)),
            (true, None) => format!("{agent_name} is thinking.."),
            (false, Some(tool)) => format!("{}..", Self::pretty_tool_name(tool)),
            (false, None) => "Drinking Water..".to_string(),
        };

        self.label.set_label(&label_text);
        self.revealer.set_reveal_child(true);
        self.spinner.set_visible(true);
        self.spinner_playing.set(true);
        if self.spinner.is_mapped() {
            self.spinner_svg.play();
        }

        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let host_cid = self.host_character_id.borrow();
        let fallback_names = [agent_name];
        let color = if boxxy_claw_protocol::characters::is_incognito_character(agent_name)
            || (!is_guest && boxxy_claw_protocol::characters::is_incognito_character(&host_cid))
        {
            boxxy_claw_protocol::characters::INCOGNITO_COLOR.to_string()
        } else {
            let cid_lookup = if is_guest { "" } else { host_cid.as_str() };
            boxxy_claw_protocol::characters::find_character_exact(
                &registry,
                cid_lookup,
                &fallback_names,
            )
            .map(|c| c.config.color.clone())
            .unwrap_or_default()
        };

        self.apply_pill_color(&color);
    }

    fn apply_pill_color(&self, raw_color: &str) {
        let next_cls = claw_pill_spinner_color_class(raw_color);
        let mut cur = self.active_pill_class.borrow_mut();
        if *cur != next_cls {
            if !cur.is_empty() {
                self.hbox.remove_css_class(&cur);
            }
            self.hbox.add_css_class(&next_cls);
            *cur = next_cls;
        }
    }

    fn apply_badge_color(&self, raw_color: &str) {
        let next_cls = claw_badge_bg_color_class(raw_color);
        let mut cur = self.active_badge_class.borrow_mut();
        if *cur != next_cls {
            if !cur.is_empty() {
                self.badge_box.remove_css_class(&cur);
            }
            self.badge_box.add_css_class(&next_cls);
            *cur = next_cls;
        }
    }

    fn pretty_tool_name(tool: &str) -> String {
        match tool {
            "sys_shell_exec" => "Executing command".to_string(),
            "file_read" | "read_file" => "Reading file".to_string(),
            "file_write" | "write_file" => "Writing file".to_string(),
            "replace_in_file" | "replace" => "Editing file".to_string(),
            "file_delete" => "Deleting file".to_string(),
            "list_directory" => "Listing directory".to_string(),
            "glob_search" => "Finding files".to_string(),
            "grep_search" => "Searching code".to_string(),
            "get_system_info" => "Fetching system info".to_string(),
            "list_processes" => "Listing processes".to_string(),
            "kill_process" => "Killing process".to_string(),
            "run_background_command" => "Launching background app".to_string(),
            "get_clipboard" => "Reading clipboard".to_string(),
            "set_clipboard" => "Writing clipboard".to_string(),
            "web_search" | "native_web_search" => "Searching the web".to_string(),
            "http_fetch" => "Fetching URL".to_string(),
            "memory_store" => "Saving memory".to_string(),
            "memory_delete" => "Deleting memory".to_string(),
            "read_scrollback_page" => "Reading terminal scrollback".to_string(),
            "terminal_exec" => "Executing terminal command".to_string(),
            "activate_skill" => "Activating skill".to_string(),
            "schedule_task" => "Scheduling task".to_string(),
            "list_my_tasks" => "Listing tasks".to_string(),
            "cancel_task" => "Cancelling task".to_string(),
            "spawn_agent" => "Spawning agent".to_string(),
            "close_agent" => "Closing agent".to_string(),
            "restore_background_shell" => "Restoring background shell".to_string(),
            "read_pane_buffer" => "Reading peer pane".to_string(),
            "list_active_agents" => "Checking active agents".to_string(),
            "set_global_intent" => "Updating global intent".to_string(),
            "abort_agent_task" => "Aborting peer task".to_string(),
            "send_keystrokes_to_pane" => "Sending keystrokes".to_string(),
            "delegate_task" | "delegate_task_async" => "Delegating task".to_string(),
            "await_tasks" => "Awaiting delegated tasks".to_string(),
            "subscribe_to_pane" => "Subscribing to pane".to_string(),
            "acquire_lock" => "Acquiring lock".to_string(),
            "release_lock" => "Releasing lock".to_string(),
            "publish_custom_event" => "Publishing event".to_string(),
            "orchestrate_agent" => "Orchestrating peer".to_string(),
            "invite_character" => "Inviting character".to_string(),
            "set_agent_state" => "Updating agent state".to_string(),
            "summon_headless_worker" => "Summoning worker".to_string(),
            _ => tool.replace('_', " "),
        }
    }

    pub fn show_diagnosis_ready(&self) {
        self.alert_type.set(2);
        self.alert_label.set_label("Diagnosis Ready");
        self.alert_action_btn.set_label("View");
        self.alert_icon.set_icon_name(Some("boxxy-bug-symbolic"));
        self.alert_hbox.remove_css_class("destructive-action");
        self.badge_revealer.set_reveal_child(false);
        self.alert_revealer.set_can_target(true);
        self.alert_revealer.set_reveal_child(true);
        self.update_visibility();
    }

    pub fn show_lazy_error(&self) {
        self.alert_type.set(1);
        self.alert_label.set_label("Error Detected");
        self.alert_action_btn.set_label("Diagnose");
        self.alert_icon
            .set_icon_name(Some("boxxy-dialog-warning-symbolic"));
        self.alert_hbox.add_css_class("destructive-action");
        self.badge_revealer.set_reveal_child(false);
        self.alert_revealer.set_can_target(true);
        self.alert_revealer.set_reveal_child(true);
        self.update_visibility();
    }

    pub fn hide(&self) {
        self.is_active.set(false);
        self.spinner_playing.set(false);
        self.spinner_svg.pause();
        self.revealer.set_reveal_child(false);
        self.clock_icon.set_visible(self.has_tasks.get());
        self.sync_badge_revealer();
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

        if name.is_empty() && character_id.is_empty() {
            self.host_display_name.borrow_mut().clear();
            self.host_slug.borrow_mut().clear();
            self.host_character_id.borrow_mut().clear();
            self.badge_label.set_text("");
            self.sync_badge_revealer();
            return;
        }

        self.is_off.set(false);
        *self.host_character_id.borrow_mut() = character_id.to_string();

        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let info = registry.iter().find(|c| c.config.id == character_id);

        if boxxy_claw_protocol::characters::is_incognito_character(character_id)
            || boxxy_claw_protocol::characters::is_incognito_character(name)
        {
            let disp = boxxy_claw_protocol::characters::INCOGNITO_DISPLAY_NAME;
            *self.host_display_name.borrow_mut() = disp.to_string();
            *self.host_slug.borrow_mut() = name.to_string();
            self.badge_label.set_text(disp);
            self.apply_badge_color(boxxy_claw_protocol::characters::INCOGNITO_COLOR);
        } else if let Some(info) = info {
            let disp = resolve_character_display_name(info);
            self.host_display_name.borrow_mut().clone_from(&disp);
            self.host_slug.borrow_mut().clone_from(&info.config.name);
            self.badge_label.set_text(&disp);
            self.apply_badge_color(&info.config.color);
        } else {
            *self.host_display_name.borrow_mut() = name.to_string();
            *self.host_slug.borrow_mut() = name.to_string();
            self.badge_label.set_text(name);
            self.apply_badge_color("");
        }

        self.sync_badge_revealer();
        self.update_visibility();
    }

    pub fn update_settings(&self, hide_character_badge: bool) {
        if self.hide_character_badge.replace(hide_character_badge) != hide_character_badge {
            self.update_visibility();
        }
    }
}
