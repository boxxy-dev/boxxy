use gtk4::prelude::*;
use gtk4::{gdk, gio};

#[derive(Debug, Clone)]
pub struct TerminalEditContext {
    pub path_to_copy: Option<String>,
    pub has_selection: bool,
}

#[derive(Debug, Clone)]
pub struct AppMenuContext {
    pub is_maximized: bool,
    /// Present when opened on a `TerminalWidget`; `None` for window-level fallback surfaces.
    pub terminal_edit: Option<TerminalEditContext>,
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

    let reset_action = if let Some(ref edit) = ctx.terminal_edit {
        // ── Copy Actions ─────────────────────────────────────────────────────
        let copy_model = gio::Menu::new();

        let copy_action = if edit.has_selection {
            "term.copy"
        } else {
            "term.disabled"
        };
        copy_model.append_item(&item("Copy", copy_action));

        let path_item = if let Some(ref path) = edit.path_to_copy {
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

        // ── Standard Edit Actions ────────────────────────────────────────────
        let std_section = gio::Menu::new();
        std_section.append_item(&item("Paste", "term.paste"));
        std_section.append_item(&item("Select All", "term.select-all"));
        menu_model.append_section(None, &std_section);

        "term.reset-terminal"
    } else {
        "win.reset-terminal"
    };

    // ── Split Pane Icons ─────────────────────────────────────────────────────
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
        "boxxy-split-unmaximize-symbolic"
    } else {
        "boxxy-split-maximize-symbolic"
    };
    split_model.append_item(&icon_item("win.toggle-maximize", max_icon));
    split_model.append_item(&icon_item("win.close-split", "boxxy-split-close-symbolic"));

    let split_section_item = gio::MenuItem::new_section(None, &split_model);
    split_section_item
        .set_attribute_value("display-hint", Some(&"horizontal-buttons".to_variant()));
    menu_model.append_item(&split_section_item);

    // ── New Tab / New Window ─────────────────────────────────────────────────
    let tab_model = gio::Menu::new();
    tab_model.append_item(&item("New Tab", "win.new-tab"));
    tab_model.append_item(&item("New Window", "win.new-window"));
    menu_model.append_section(None, &tab_model);

    // ── App Features ─────────────────────────────────────────────────────────
    let app_section = gio::Menu::new();
    app_section.append_item(&item("Open in Files", "win.open-in-files"));
    app_section.append_item(&item("Reset", reset_action));
    menu_model.append_section(None, &app_section);

    // ── Meta Actions ─────────────────────────────────────────────────────────
    let meta_section = gio::Menu::new();
    meta_section.append_item(&item("Command Palette", "win.command-palette"));
    meta_section.append_item(&item("Preferences", "win.preferences"));
    menu_model.append_section(None, &meta_section);

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
    fn context_menu_groups_command_palette_right_above_preferences() {
        let term_menu = build_menu_model(&AppMenuContext {
            is_maximized: false,
            terminal_edit: Some(TerminalEditContext {
                path_to_copy: Some("https://example.com".to_string()),
                has_selection: true,
            }),
        });
        assert_eq!(term_menu.n_items(), 6);

        let term_meta = term_menu
            .item_link(5, gio::MENU_LINK_SECTION)
            .expect("terminal meta section");
        assert_eq!(
            section_items(&term_meta),
            vec![
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

        let win_menu = build_menu_model(&AppMenuContext {
            is_maximized: true,
            terminal_edit: None,
        });
        assert_eq!(win_menu.n_items(), 4);

        let win_meta = win_menu
            .item_link(3, gio::MENU_LINK_SECTION)
            .expect("window meta section");
        assert_eq!(
            section_items(&win_meta),
            vec![
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
}
