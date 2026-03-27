use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone)]
pub struct AgentBadge {
    container: gtk::Box,
    label: gtk::Label,
    is_active: Rc<Cell<bool>>,
    is_evicted: Rc<Cell<bool>>,
}

impl AgentBadge {
    pub fn new(overlay: &gtk::Overlay) -> Self {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .css_classes(["agent-badge-container"])
            .spacing(6)
            .visible(false)
            .build();

        let label = gtk::Label::builder()
            .css_classes(["agent-badge-label"])
            .build();

        container.append(&label);

        overlay.add_overlay(&container);

        Self {
            container,
            label,
            is_active: Rc::new(Cell::new(false)),
            is_evicted: Rc::new(Cell::new(false)),
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
        self.container.remove_css_class("evicted");
        self.label.set_text(name);

        let color = self.generate_color(name);

        // Apply custom styling via CSS for the specific background color
        let css = format!(
            ".agent-badge-container {{ background-color: {}; color: white; border-radius: 12px; padding: 4px 10px; opacity: 0.7; font-weight: bold; font-size: 0.8rem; box-shadow: 0 2px 4px rgba(0,0,0,0.2); transition: opacity 0.3s ease; }} \
             .agent-badge-container.evicted {{ opacity: 0.2; filter: grayscale(100%); }}",
            color
        );

        let provider = gtk::CssProvider::new();
        provider.load_from_data(&css);
        #[allow(deprecated)]
        self.container
            .style_context()
            .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

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
        let has_name = !self.label.text().is_empty();

        if settings.hide_agent_badge
            || (!self.is_active.get() && !self.is_evicted.get())
            || !has_name
        {
            self.container.set_visible(false);
        } else {
            self.container.set_visible(true);
        }
    }

    fn generate_color(&self, name: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        let hash = hasher.finish();

        // Use the hash to pick a pleasant, fairly dark color (to ensure white text contrast)
        let r = (hash & 0xFF) as u8 % 150 + 50;
        let g = ((hash >> 8) & 0xFF) as u8 % 150 + 50;
        let b = ((hash >> 16) & 0xFF) as u8 % 150 + 50;

        format!("rgb({}, {}, {})", r, g, b)
    }
}
