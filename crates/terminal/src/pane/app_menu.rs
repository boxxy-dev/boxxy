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

fn menu_item(label: &str, action: &str) -> gio::MenuItem {
    let i = gio::MenuItem::new(Some(label), Some(action));
    i.set_attribute_value("accel", Some(&"".to_variant()));
    i
}

fn append_app_section(menu_model: &gio::Menu, include_open_in_files: bool) {
    let app_section = gio::Menu::new();
    if include_open_in_files {
        app_section.append_item(&menu_item("Open in Files", "win.open-in-files"));
    }
    app_section.append_item(&menu_item("Command Palette", "win.command-palette"));
    app_section.append_item(&menu_item("Preferences", "win.preferences"));
    menu_model.append_section(None, &app_section);
}

fn build_app_only_menu_model(include_open_in_files: bool) -> gio::Menu {
    let menu_model = gio::Menu::new();
    append_app_section(&menu_model, include_open_in_files);
    menu_model
}

fn build_menu_model(ctx: &AppMenuContext) -> gio::Menu {
    let menu_model = gio::Menu::new();

    // ── Section 1: Copy Actions ──────────────────────────────────────────────
    let copy_model = gio::Menu::new();

    let copy_action = if ctx.has_selection {
        "term.copy"
    } else {
        "term.disabled"
    };
    copy_model.append_item(&menu_item("Copy", copy_action));

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
        menu_item("Copy Link", "term.disabled")
    };
    copy_model.append_item(&path_item);

    let copy_section_item = gio::MenuItem::new_section(None, &copy_model);
    copy_section_item
        .set_attribute_value("display-hint", Some(&"horizontal-buttons".to_variant()));
    menu_model.append_item(&copy_section_item);

    // ── Section 2: Terminal Edit & Reset ─────────────────────────────────────
    let std_section = gio::Menu::new();
    std_section.append_item(&menu_item("Paste", "term.paste"));
    std_section.append_item(&menu_item("Select All", "term.select-all"));
    std_section.append_item(&menu_item("Reset", "term.reset-terminal"));
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
        "boxxy-view-fullscreen-symbolic"
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
    append_app_section(&menu_model, true);

    menu_model
}

/// Returns `true` when a secondary click at `(x, y)` inside `surface` should be left to
/// a descendant widget's native context menu (text inputs, selected/linked labels, child
/// popovers, or inline media preview cards).
fn should_defer_secondary_click(surface: &gtk4::Widget, x: f64, y: f64) -> bool {
    let mut current = surface.pick(x, y, gtk4::PickFlags::DEFAULT);
    while let Some(widget) = current {
        if &widget == surface {
            break;
        }
        if widget.is::<gtk4::Popover>()
            || widget.is::<gtk4::Text>()
            || widget.is::<gtk4::TextView>()
            || widget.is::<gtk4::Editable>()
            || widget.has_css_class("viewer-image-card")
            || widget.has_css_class("sidebar-resize-handle")
        {
            return true;
        }
        if let Ok(label) = widget.clone().downcast::<gtk4::Label>()
            && label.is_selectable()
            && (label.selection_bounds().is_some() || label.current_uri().is_some())
        {
            return true;
        }
        current = widget.parent();
    }
    false
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
        self.popup_with_model(rect, &build_menu_model(&ctx));
    }

    pub fn show_app_only(&self, rect: gdk::Rectangle, include_open_in_files: bool) {
        self.popup_with_model(rect, &build_app_only_menu_model(include_open_in_files));
    }

    fn popup_with_model(&self, rect: gdk::Rectangle, menu_model: &gio::Menu) {
        self.popover.set_menu_model(Some(menu_model));
        self.popover.set_pointing_to(Some(&rect));
        let popover = self.popover.clone();
        gtk4::glib::idle_add_local_once(move || {
            popover.popup();
        });
    }

    /// Attaches a right-click gesture onto `surface` that opens the compact app-only
    /// menu (`Command Palette`, `Preferences`, plus optional `Open in Files`), deferring
    /// to native context menus on text inputs, selected/linked labels, child popovers,
    /// and inline media cards.
    pub fn bind_app_only_surface<F: Fn() + 'static>(
        &self,
        surface: &impl IsA<gtk4::Widget>,
        include_open_in_files: bool,
        on_before_open: F,
    ) {
        let gesture = gtk4::GestureClick::new();
        gesture.set_button(gdk::BUTTON_SECONDARY);
        gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);

        let surface_weak = surface.upcast_ref().downgrade();
        let menu = self.clone();
        gesture.connect_pressed(move |gesture, _n_press, x, y| {
            let Some(surface_widget) = surface_weak.upgrade() else {
                return;
            };
            if should_defer_secondary_click(&surface_widget, x, y) {
                return;
            }
            let Some(popover_parent) = menu.widget().parent() else {
                return;
            };
            let pt = surface_widget
                .compute_point(
                    &popover_parent,
                    &gtk4::graphene::Point::new(x as f32, y as f32),
                )
                .unwrap_or_else(|| gtk4::graphene::Point::new(x as f32, y as f32));

            gesture.set_state(gtk4::EventSequenceState::Claimed);
            on_before_open();
            menu.show_app_only(
                gdk::Rectangle::new(pt.x() as i32, pt.y() as i32, 1, 1),
                include_open_in_files,
            );
        });

        surface.add_controller(gesture);
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
                Some("boxxy-view-fullscreen-symbolic".to_string()),
            )
        );
        assert_eq!(
            max_item_attrs(true, false),
            (
                Some("win.toggle-maximize".to_string()),
                Some("boxxy-view-fullscreen-symbolic".to_string()),
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

    #[test]
    fn app_only_context_menu_respects_open_in_files_flag() {
        let overlay_menu = build_app_only_menu_model(true);
        assert_eq!(overlay_menu.n_items(), 1);
        let overlay_section = overlay_menu
            .item_link(0, gio::MENU_LINK_SECTION)
            .expect("overlay app section");
        assert_eq!(
            section_items(&overlay_section),
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

        let sidebar_menu = build_app_only_menu_model(false);
        assert_eq!(sidebar_menu.n_items(), 1);
        let sidebar_section = sidebar_menu
            .item_link(0, gio::MENU_LINK_SECTION)
            .expect("sidebar app section");
        assert_eq!(
            section_items(&sidebar_section),
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
