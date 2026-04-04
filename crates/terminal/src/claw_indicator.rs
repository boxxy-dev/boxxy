use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone)]
pub struct ClawIndicator {
    container: gtk::Box,
    badge_label: gtk::Label,
    badge_revealer: gtk::Revealer,
    clock_icon: gtk::Image,
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
}

impl ClawIndicator {
    pub fn new(overlay: &gtk::Overlay) -> Self {
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
                padding: 2px 8px;
                font-weight: bold;
                font-size: 0.70rem;
                color: white;
            }
            .badge-label.evicted {
                filter: grayscale(100%);
                opacity: 0.25;
            }
            .indicator-container revealer {
                padding: 0;
                margin: 0;
            }
            .indicator-pill {
                border-radius: 9999px;
                border: 1px solid rgba(255,255,255,0.12);
                box-shadow: 0 4px 18px rgba(0,0,0,0.5);
                padding: 4px 6px 4px 10px;
            }
            .indicator-pill button.flat {
                border-radius: 9999px;
                padding: 2px 6px;
                min-height: 0;
                min-width: 0;
                color: white;
            }
            .indicator-pill button.flat:hover {
                background-color: rgba(255,255,255,0.15);
            }
            .indicator-pill button.circular {
                border-radius: 9999px;
                padding: 2px;
                min-height: 0;
                min-width: 0;
                opacity: 0.75;
                color: white;
            }
            .indicator-pill button.circular:hover { opacity: 1.0; }
            .indicator-pill image.accent  { color: rgba(255,255,255,0.90); }
            .indicator-pill image.warning { color: #fff3cd; }
            .indicator-pill image.success { color: #d4f7d4; }
            .status-label {
                font-size: 0.82rem;
                font-weight: bold;
                color: white;
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

        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .css_classes(["indicator-container"])
            .spacing(0)
            .visible(false)
            .build();
        container.set_size_request(-1, 28);

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
        revealer.set_transition_duration(280);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hbox.add_css_class("indicator-pill");
        hbox.set_valign(gtk::Align::Center);

        let main_btn = gtk::Button::builder().css_classes(["flat"]).build();

        let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        btn_box.set_valign(gtk::Align::Center);

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

        if let Some(clock) = spinner.frame_clock() {
            svg.set_frame_clock(&clock);
        }
        btn_box.append(&spinner);

        let icon = gtk::Image::from_icon_name("boxxy-boxxyclaw-symbolic");
        icon.add_css_class("accent");
        icon.set_pixel_size(13);
        btn_box.append(&icon);

        let label = gtk::Label::new(Some("Thinking.."));
        label.add_css_class("status-label");
        btn_box.append(&label);

        main_btn.set_child(Some(&btn_box));
        hbox.append(&main_btn);

        let cancel_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .tooltip_text("Cancel")
            .build();
        cancel_btn.set_valign(gtk::Align::Center);

        hbox.append(&cancel_btn);
        revealer.set_child(Some(&hbox));

        let action_type = Rc::new(RefCell::new(0));

        let badge_revealer = gtk::Revealer::new();
        badge_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        badge_revealer.set_transition_duration(280);
        badge_revealer.set_reveal_child(true);

        let badge_label = gtk::Label::builder()
            .css_classes(["badge-label"])
            .valign(gtk::Align::Center)
            .build();
        badge_revealer.set_child(Some(&badge_label));
        container.append(&badge_revealer);

        let clock_icon = gtk::Image::builder()
            .icon_name("boxxy-timer-symbolic")
            .visible(false)
            .build();
        container.append(&clock_icon);

        container.append(&revealer);

        overlay.add_overlay(&container);

        let badge_provider = gtk::CssProvider::new();
        #[allow(deprecated)]
        badge_label
            .style_context()
            .add_provider(&badge_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let pill_provider = gtk::CssProvider::new();
        #[allow(deprecated)]
        hbox.style_context()
            .add_provider(&pill_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        Self {
            container,
            badge_label,
            badge_revealer,
            clock_icon,
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
        }
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

        let action_clone = self.action_type.clone();
        self.main_btn.connect_clicked(move |_| {
            let action = *action_clone.borrow();
            if action == 1 {
                on_lazy_click();
            } else if action == 2 {
                on_proactive_click();
            }
        });
    }

    pub fn set_has_tasks(&self, has_tasks: bool) {
        self.has_tasks.set(has_tasks);
        if !self.revealer.reveals_child() {
            self.clock_icon.set_visible(has_tasks);
        }
    }

    pub fn set_evicted(&self, evicted: bool) {
        self.is_evicted.set(evicted);
        if evicted {
            self.container.add_css_class("evicted");
        } else {
            self.container.remove_css_class("evicted");
        }
        self.refresh_visibility();
    }

    pub fn set_identity(&self, name: &str) {
        self.is_evicted.set(false);
        self.badge_label.remove_css_class("evicted");
        self.badge_label.set_text(name);

        let color = self.generate_color(name);

        self.badge_provider
            .load_from_string(&format!(".badge-label {{ background-color: {}; }}", color));
        self.pill_provider.load_from_string(&format!(
            ".indicator-pill {{ background-color: {}; }}",
            color
        ));

        self.refresh_visibility();
    }

    pub fn set_visible(&self, visible: bool) {
        self.is_active.set(visible);
        self.refresh_visibility();
    }

    pub fn update_settings(&self) {
        self.refresh_visibility();
    }

    fn refresh_visibility(&self) {
        let settings = boxxy_preferences::Settings::load();
        let has_name = !self.badge_label.text().is_empty();

        if settings.hide_agent_badge
            || (!self.is_active.get() && !self.is_evicted.get())
            || !has_name
        {
            self.container.set_visible(false);
        } else {
            self.container.set_visible(true);
            self.badge_label.set_visible(true); // Explicitly ensure the label is visible inside the revealer
        }
    }

    fn generate_color(&self, name: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        let hash = hasher.finish();

        let r = (hash & 0xFF) as u8 % 150 + 50;
        let g = ((hash >> 8) & 0xFF) as u8 % 150 + 50;
        let b = ((hash >> 16) & 0xFF) as u8 % 150 + 50;

        format!("rgb({}, {}, {})", r, g, b)
    }

    pub fn show_thinking(&self) {
        *self.action_type.borrow_mut() = 0;
        self.spinner.set_visible(true);
        self.icon.set_visible(false);
        self.label.set_text("Thinking..");
        self.main_btn.set_can_focus(false);
        self.revealer.set_reveal_child(true);
        self.badge_revealer.set_reveal_child(false);
        self.clock_icon.set_visible(false);
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
        self.badge_revealer.set_reveal_child(false);
        self.clock_icon.set_visible(false);

        let badge_rev_hide = self.badge_revealer.clone();
        let clock_hide = self.clock_icon.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            badge_rev_hide.set_visible(false);
            clock_hide.set_visible(false);
        });

        let rev_clone = self.revealer.clone();
        let badge_revealer_clone = self.badge_revealer.clone();
        let clock_clone = self.clock_icon.clone();
        let has_tasks_clone = self.has_tasks.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(5000), move || {
            if rev_clone.reveals_child() {
                rev_clone.set_reveal_child(false);
                badge_revealer_clone.set_visible(true);
                badge_revealer_clone.set_reveal_child(true);
                clock_clone.set_visible(has_tasks_clone.get());
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
        self.main_btn.set_sensitive(true);

        self.badge_revealer.set_reveal_child(false);
        let badge_rev_hide = self.badge_revealer.clone();
        let clock_hide = self.clock_icon.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(280), move || {
            badge_rev_hide.set_visible(false);
            clock_hide.set_visible(false);
        });
        self.revealer.set_reveal_child(true);

        let rev_clone = self.revealer.clone();
        let badge_revealer_clone = self.badge_revealer.clone();
        let clock_clone = self.clock_icon.clone();
        let has_tasks_clone = self.has_tasks.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(5000), move || {
            if rev_clone.reveals_child() {
                rev_clone.set_reveal_child(false);
                badge_revealer_clone.set_visible(true);
                badge_revealer_clone.set_reveal_child(true);
                clock_clone.set_visible(has_tasks_clone.get());
            }
        });
    }

    pub fn hide(&self) {
        self.badge_revealer.set_visible(true);
        self.revealer.set_reveal_child(false);
        self.badge_revealer.set_reveal_child(true);
        self.clock_icon.set_visible(self.has_tasks.get());
    }
}
