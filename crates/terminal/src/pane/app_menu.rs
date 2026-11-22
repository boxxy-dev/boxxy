use gtk4::prelude::*;
use gtk4::{gdk, gio};

#[derive(Debug, Clone)]
pub struct AppMenuContext {
    pub has_splits: bool,
    pub is_maximized: bool,
    pub path_to_copy: Option<String>,
    pub has_selection: bool,
}

#[derive(Debug, Clone)]
pub struct AppMenuComponent {
    popover: gtk4::PopoverMenu,
}

fn build_menu_model(ctx: &AppMenuContext) -> gio::Menu {
    let menu_model = gio::Menu::new();

    // Helper to create menu items without inline shortcut labels
    let item = |label: &str, action: &str| {
        let i = gio::MenuItem::new(Some(label), Some(action));
        i.set_attribute_value("accel", Some(&"".to_variant()));
        i
    };

    // ── Section 1: Copy Actions ──────────────────────────────────────────────
    let copy_model = gio::Menu::new();

    let copy_action = if ctx.has_selection {
        "term.copy"
    } else {
        "term.disabled"
    };
    copy_model.append_item(&item("Copy", copy_action));

    let path_item = if let Some(ref path) = ctx.path_to_copy {
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
        item("Copy Link", "term.disabled")
    };
    copy_model.append_item(&path_item);

    let copy_section_item = gio::MenuItem::new_section(None, &copy_model);
    copy_section_item
        .set_attribute_value("display-hint", Some(&"horizontal-buttons".to_variant()));
    menu_model.append_item(&copy_section_item);

    // ── Section 2: Terminal Edit & Reset ─────────────────────────────────────
    let std_section = gio::Menu::new();
    std_section.append_item(&item("Paste", "term.paste"));
    std_section.append_item(&item("Select All", "term.select-all"));
    std_section.append_item(&item("Reset", "term.reset-terminal"));
    menu_model.append_section(None, &std_section);

    // ── Section 3: Split Pane Icons ──────────────────────────────────────────
    let icon_item = |action: &str, icon: &str| {
        let i = gio::MenuItem::new(None, Some(action));
        i.set_attribute_value("verb-icon", Some(&icon.to_variant()));
        i
    };

    let split_model = gio::Menu::new();
    split_model.append_item(&icon_item(
        "win.split-vertical",
        "boxxy-split-vertical-symbolic",
    ));
    split_model.append_item(&icon_item(
        "win.split-horizontal",
        "boxxy-split-horizontal-symbolic",
    ));
    let max_icon = if ctx.is_maximized {
        "boxxy-view-restore-symbolic"
    } else {
        "boxxy-zoom-fit-best-symbolic"
    };
    let max_action = if ctx.has_splits || ctx.is_maximized {
        "win.toggle-maximize"
    } else {
        "term.disabled"
    };
    split_model.append_item(&icon_item(max_action, max_icon));
    split_model.append_item(&icon_item("win.close-split", "boxxy-split-close-symbolic"));

    let split_section_item = gio::MenuItem::new_section(None, &split_model);
    split_section_item
        .set_attribute_value("display-hint", Some(&"horizontal-buttons".to_variant()));
    menu_model.append_item(&split_section_item);

    // ── Section 4: App & Meta Actions ────────────────────────────────────────
    let app_section = gio::Menu::new();
    app_section.append_item(&item("Open in Files", "win.open-in-files"));
    app_section.append_item(&item("Command Palette", "win.command-palette"));
    app_section.append_item(&item("Preferences", "win.preferences"));
    menu_model.append_section(None, &app_section);

    menu_model
}

impl AppMenuComponent {
    #[must_use]
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

    #[must_use]
    pub const fn widget(&self) -> &gtk4::PopoverMenu {
        &self.popover
    }

    pub fn unparent(&self) {
        if self.popover.parent().is_some() {
            self.popover.unparent();
        }
    }

    pub fn show(&self, rect: gdk::Rectangle, ctx: AppMenuContext) {
        let menu_model = build_menu_model(&ctx);
        self.popover.set_menu_model(Some(&menu_model));
        self.popover.set_pointing_to(Some(&rect));
        let popover = self.popover.clone();
        gtk4::glib::idle_add_local_once(move || {
            popover.popup();
        });
    }
}

impl Default for AppMenuComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_items(section: &gio::MenuModel) -> Vec<(Option<String>, Option<String>)> {
        (0..section.n_items())
            .map(|idx| {
                let label = section
                    .item_attribute_value(idx, gio::MENU_ATTRIBUTE_LABEL, None)
                    .and_then(|v| v.get::<String>());
                let action = section
                    .item_attribute_value(idx, gio::MENU_ATTRIBUTE_ACTION, None)
                    .and_then(|v| v.get::<String>());
                (label, action)
            })
            .collect()
    }

    #[test]
    fn context_menu_sections_match_expected_layout() {
        let menu = build_menu_model(&AppMenuContext {
            has_splits: false,
            is_maximized: false,
            path_to_copy: Some("https://example.com".to_string()),
            has_selection: true,
        });
        assert_eq!(menu.n_items(), 4);

        let edit_section = menu
            .item_link(1, gio::MENU_LINK_SECTION)
            .expect("edit section");
        assert_eq!(
            section_items(&edit_section),
            vec![
                (Some("Paste".to_string()), Some("term.paste".to_string())),
                (
                    Some("Select All".to_string()),
                    Some("term.select-all".to_string()),
                ),
                (
                    Some("Reset".to_string()),
                    Some("term.reset-terminal".to_string()),
                ),
            ]
        );

        let app_section = menu
            .item_link(3, gio::MENU_LINK_SECTION)
            .expect("app section");
        assert_eq!(
            section_items(&app_section),
            vec![
                (
                    Some("Open in Files".to_string()),
                    Some("win.open-in-files".to_string()),
                ),
                (
                    Some("Command Palette".to_string()),
                    Some("win.command-palette".to_string()),
                ),
                (
                    Some("Preferences".to_string()),
                    Some("win.preferences".to_string()),
                ),
            ]
        );
    }

    #[test]
    fn maximize_button_reflects_split_and_maximized_state() {
        let max_item_attrs = |has_splits: bool, is_maximized: bool| {
            let menu = build_menu_model(&AppMenuContext {
                has_splits,
                is_maximized,
                path_to_copy: None,
                has_selection: false,
            });
            let split_section = menu
                .item_link(2, gio::MENU_LINK_SECTION)
                .expect("split section");
            let action = split_section
                .item_attribute_value(2, gio::MENU_ATTRIBUTE_ACTION, None)
                .and_then(|v| v.get::<String>());
            let icon = split_section
                .item_attribute_value(2, "verb-icon", None)
                .and_then(|v| v.get::<String>());
            (action, icon)
        };

        assert_eq!(
            max_item_attrs(false, false),
            (
                Some("term.disabled".to_string()),
                Some("boxxy-zoom-fit-best-symbolic".to_string()),
            )
        );
        assert_eq!(
            max_item_attrs(true, false),
            (
                Some("win.toggle-maximize".to_string()),
                Some("boxxy-zoom-fit-best-symbolic".to_string()),
            )
        );
        assert_eq!(
            max_item_attrs(true, true),
            (
                Some("win.toggle-maximize".to_string()),
                Some("boxxy-view-restore-symbolic".to_string()),
            )
        );
    }
}
