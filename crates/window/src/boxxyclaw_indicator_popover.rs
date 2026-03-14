use gtk4 as gtk;
use libadwaita as adw;
use gtk::prelude::*;
use adw::prelude::*;
use std::rc::Rc;
use boxxy_preferences::Settings;

pub struct BoxxyclawIndicatorPopover {
    popover: gtk::Popover,
    enable_btn: gtk::Switch,
    proactive_btn: gtk::Switch,
    terminal_btn: gtk::Switch,
}

impl BoxxyclawIndicatorPopover {
    pub fn new<F1: Fn(bool) + 'static, F2: Fn(bool) + 'static, F3: Fn(bool) + 'static>(
        on_enable_toggled: F1,
        on_proactive_toggled: F2,
        on_terminal_toggled: F3,
    ) -> Self {
        let popover = gtk::Popover::new();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        let title = gtk::Label::builder()
            .label("Claw Agent Options")
            .css_classes(["heading"])
            .halign(gtk::Align::Start)
            .build();
        vbox.append(&title);

        let list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();

        // 1. Enable Toggle
        let enable_row = adw::ActionRow::builder()
            .title("Enable Claw Agent")
            .build();
        let enable_btn = gtk::Switch::builder().valign(gtk::Align::Center).build();
        enable_row.add_suffix(&enable_btn);
        
        let on_enable_rc = Rc::new(on_enable_toggled);
        enable_btn.connect_state_set(move |_, state| {
            on_enable_rc(state);
            gtk::glib::Propagation::Proceed
        });
        list_box.append(&enable_row);

        // 2. Proactive Toggle
        let proactive_row = adw::ActionRow::builder()
            .title("Proactive Mode")
            .subtitle("Analyze background commands")
            .build();
        let proactive_btn = gtk::Switch::builder().valign(gtk::Align::Center).build();
        proactive_row.add_suffix(&proactive_btn);

        let on_proactive_rc = Rc::new(on_proactive_toggled);
        proactive_btn.connect_state_set(move |_, state| {
            on_proactive_rc(state);
            gtk::glib::Propagation::Proceed
        });
        list_box.append(&proactive_row);

        // 3. On Terminal Toggle
        let terminal_row = adw::ActionRow::builder()
            .title("On Terminal")
            .subtitle("Show suggestions in terminal")
            .build();
        let terminal_btn = gtk::Switch::builder().valign(gtk::Align::Center).build();
        terminal_row.add_suffix(&terminal_btn);

        let on_terminal_rc = Rc::new(on_terminal_toggled);
        terminal_btn.connect_state_set(move |_, state| {
            on_terminal_rc(state);
            gtk::glib::Propagation::Proceed
        });
        list_box.append(&terminal_row);

        vbox.append(&list_box);
        popover.set_child(Some(&vbox));

        Self {
            popover,
            enable_btn,
            proactive_btn,
            terminal_btn,
        }
    }

    pub fn popover(&self) -> &gtk::Popover {
        &self.popover
    }

    pub fn show(&self, parent: &gtk::Widget) {
        self.popover.set_parent(parent);
        self.popover.popup();
    }

    pub fn update_ui(&self, active: bool, settings: &Settings) {
        self.enable_btn.set_active(active);
        self.proactive_btn.set_active(settings.claw_auto_diagnosis_mode == boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive);
        self.terminal_btn.set_active(settings.claw_terminal_suggestions);
    }
}
