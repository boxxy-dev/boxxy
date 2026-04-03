use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct ClawIndicator {
    revealer: gtk::Revealer,
    spinner: gtk::Image,
    icon: gtk::Image,
    label: gtk::Label,
    main_btn: gtk::Button,
    action_type: Rc<RefCell<u8>>, // 0 = none, 1 = lazy, 2 = proactive
}

impl ClawIndicator {
    pub fn new<F1: Fn() + 'static, F2: Fn() + 'static, F3: Fn() + 'static>(
        on_cancel: F1,
        on_lazy_click: F2,
        on_proactive_click: F3,
    ) -> Self {
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        revealer.set_halign(gtk::Align::End);
        revealer.set_valign(gtk::Align::Start); // Top right
        revealer.set_margin_top(8);
        revealer.set_margin_end(8);

        let frame = gtk::Frame::new(None);
        frame.add_css_class("claw-indicator");
        frame.add_css_class("background");
        frame.set_margin_bottom(0);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hbox.set_margin_top(4);
        hbox.set_margin_bottom(4);
        hbox.set_margin_start(8);
        hbox.set_margin_end(4);

        // We wrap the main content in a flat button so it's clickable
        let main_btn = gtk::Button::builder().css_classes(["flat"]).build();

        let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        // 1. Load the animated SVG from the compiled GResource blob.
        // Make sure the file is registered in `crates/app/build.rs` or it will silently load empty!
        let svg = gtk::Svg::from_resource("/dev/boxxy/BoxxyTerminal/icons/boxxy-spinner.gpa");
        svg.play();

        let spinner = gtk::Image::builder().paintable(&svg).pixel_size(16).build();
        spinner.add_css_class("claw-spinner");

        // 2. Connect the SVG to the widget's native frame clock so it can animate.
        // Using `connect_map` ensures the widget is fully mapped to a surface.
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

        if let Some(clock) = spinner.frame_clock() {
            svg.set_frame_clock(&clock);
        }
        btn_box.append(&spinner);

        let icon = gtk::Image::from_icon_name("boxxy-boxxyclaw-symbolic");
        icon.add_css_class("accent");
        icon.set_pixel_size(16);
        btn_box.append(&icon);

        let label = gtk::Label::new(Some("Thinking.."));
        label.add_css_class("caption");
        btn_box.append(&label);

        main_btn.set_child(Some(&btn_box));
        hbox.append(&main_btn);

        let cancel_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .tooltip_text("Cancel")
            .build();
        cancel_btn.set_valign(gtk::Align::Center);

        let p_clone = revealer.clone();
        cancel_btn.connect_clicked(move |_| {
            p_clone.set_reveal_child(false);
            on_cancel();
        });

        hbox.append(&cancel_btn);

        frame.set_child(Some(&hbox));
        revealer.set_child(Some(&frame));

        let action_type = Rc::new(RefCell::new(0));

        let action_clone = action_type.clone();
        main_btn.connect_clicked(move |_| {
            let action = *action_clone.borrow();
            if action == 1 {
                on_lazy_click();
            } else if action == 2 {
                on_proactive_click();
            }
        });

        Self {
            revealer,
            spinner,
            icon,
            label,
            main_btn,
            action_type,
        }
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.revealer
    }

    pub fn show_thinking(&self) {
        *self.action_type.borrow_mut() = 0;
        self.spinner.set_visible(true);
        self.icon.set_visible(false);
        self.label.set_text("Thinking..");
        self.main_btn.set_can_focus(false);
        self.revealer.set_reveal_child(true);
    }

    pub fn show_lazy_error(&self) {
        *self.action_type.borrow_mut() = 1;
        self.spinner.set_visible(false);
        self.icon.set_visible(true);
        self.icon
            .set_icon_name(Some("boxxy-dialog-warning-symbolic"));
        self.icon.set_css_classes(&["warning"]);
        self.label.set_text("Fix Available");
        self.main_btn.set_can_focus(true);
        self.revealer.set_reveal_child(true);

        // Auto-hide after 5 seconds
        let rev_clone = self.revealer.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(5000), move || {
            if rev_clone.reveals_child() {
                rev_clone.set_reveal_child(false);
            }
        });
    }

    pub fn show_diagnosis_ready(&self) {
        *self.action_type.borrow_mut() = 2;
        self.spinner.set_visible(false);
        self.icon.set_visible(true);
        self.icon.set_icon_name(Some("boxxy-boxxyclaw-symbolic"));
        self.icon.set_css_classes(&["success"]);
        self.label.set_text("Solution Ready");
        self.main_btn.set_can_focus(true);
        self.revealer.set_reveal_child(true);

        // Auto-hide after 5 seconds
        let rev_clone = self.revealer.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(5000), move || {
            if rev_clone.reveals_child() {
                rev_clone.set_reveal_child(false);
            }
        });
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
    }
}
