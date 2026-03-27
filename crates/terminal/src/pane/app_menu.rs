use gtk4::prelude::*;
use gtk4::{gdk, gio};

#[derive(Debug, Clone)]
pub struct AppMenuContext {
    pub is_maximized: bool,
    pub path_to_copy: Option<String>,
    pub has_selection: bool,
}

#[derive(Debug, Clone)]
pub struct AppMenuComponent {
    popover: gtk4::PopoverMenu,
}

impl AppMenuComponent {
    pub fn new() -> Self {
        let popover = gtk4::PopoverMenu::builder()
            .has_arrow(false)
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Start)
            .autohide(true)
            .build();

        popover.add_css_class("terminal-menu");

        Self { popover }
    }

    pub fn widget(&self) -> &gtk4::PopoverMenu {
        &self.popover
    }

    pub fn show(&self, rect: gdk::Rectangle, ctx: AppMenuContext) {
        let menu_model = gio::Menu::new();

        // Helper to create menu items without shortcut labels
        let item = |label: &str, action: &str| {
            let i = gio::MenuItem::new(Some(label), Some(action));
            i.set_attribute_value("accel", Some(&"".to_variant()));
            i
        };

        // ── Section 1: Copy Actions ──────────────────────────────────────────
        let copy_model = gio::Menu::new();

        let copy_item = if ctx.has_selection {
            gio::MenuItem::new(Some("Copy"), Some("term.copy"))
        } else {
            gio::MenuItem::new(Some("Copy"), Some("term.disabled"))
        };
        copy_item.set_attribute_value("accel", Some(&"".to_variant()));
        copy_model.append_item(&copy_item);

        let path_item = if let Some(path) = ctx.path_to_copy {
            let label = if path.starts_with("http://") || path.starts_with("https://") {
                "Copy Link"
            } else {
                "Copy Path"
            };
            let i = gio::MenuItem::new(Some(label), None);
            i.set_action_and_target_value(Some("term.copy-path"), Some(&path.to_variant()));
            i.set_attribute_value("accel", Some(&"".to_variant()));
            i
        } else {
            let i = gio::MenuItem::new(Some("Copy Link"), Some("term.disabled"));
            i.set_attribute_value("accel", Some(&"".to_variant()));
            i
        };
        copy_model.append_item(&path_item);

        let copy_section_item = gio::MenuItem::new_section(None, &copy_model);
        copy_section_item
            .set_attribute_value("display-hint", Some(&"horizontal-buttons".to_variant()));
        menu_model.append_item(&copy_section_item);

        // ── Section 2: Standard Edit Actions ─────────────────────────────────
        let std_section = gio::Menu::new();
        std_section.append_item(&item("Paste", "term.paste"));
        std_section.append_item(&item("Select All", "term.select-all"));
        menu_model.append_section(None, &std_section);

        // ── Section 3: Split Pane Icons ──────────────────────────────────────
        let split_model = gio::Menu::new();

        let split_v = gio::MenuItem::new(None, Some("win.split-vertical"));
        split_v.set_attribute_value(
            "verb-icon",
            Some(&"boxxy-split-vertical-symbolic".to_variant()),
        );
        split_model.append_item(&split_v);

        let split_h = gio::MenuItem::new(None, Some("win.split-horizontal"));
        split_h.set_attribute_value(
            "verb-icon",
            Some(&"boxxy-split-horizontal-symbolic".to_variant()),
        );
        split_model.append_item(&split_h);

        let toggle_max = gio::MenuItem::new(None, Some("win.toggle-maximize"));
        let max_icon = if ctx.is_maximized {
            "boxxy-split-unmaximize-symbolic"
        } else {
            "boxxy-split-maximize-symbolic"
        };
        toggle_max.set_attribute_value("verb-icon", Some(&max_icon.to_variant()));
        split_model.append_item(&toggle_max);

        let close_split = gio::MenuItem::new(None, Some("win.close-split"));
        close_split.set_attribute_value(
            "verb-icon",
            Some(&"boxxy-split-close-symbolic".to_variant()),
        );
        split_model.append_item(&close_split);

        let split_section_item = gio::MenuItem::new_section(None, &split_model);
        split_section_item
            .set_attribute_value("display-hint", Some(&"horizontal-buttons".to_variant()));
        menu_model.append_item(&split_section_item);

        // ── Section 4: New Tab / New Window (Text Items) ─────────────────────
        let tab_model = gio::Menu::new();
        tab_model.append_item(&item("New Tab", "win.new-tab"));
        tab_model.append_item(&item("New Window", "win.new-window"));
        menu_model.append_section(None, &tab_model);

        // ── Section 5: App Features ──────────────────────────────────────────
        let app_section = gio::Menu::new();
        app_section.append_item(&item("Open in Files", "win.open-in-files"));
        app_section.append_item(&item("Bookmarks", "win.bookmarks"));
        menu_model.append_section(None, &app_section);

        // ── Section 6: Meta Actions ──────────────────────────────────────────
        let meta_section = gio::Menu::new();
        meta_section.append_item(&item("Preferences", "win.preferences"));
        menu_model.append_section(None, &meta_section);

        self.popover.set_menu_model(Some(&menu_model));
        self.popover.set_pointing_to(Some(&rect));
        self.popover.popup();
    }

    pub fn hide(&self) {
        self.popover.popdown();
    }
}

impl Default for AppMenuComponent {
    fn default() -> Self {
        Self::new()
    }
}
