use crate::config::Settings;
use crate::ui_utils::{PageFilter, PageSync};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

struct NavCategory {
    page_name: &'static str,
    widget_id: &'static str,
    header_title: &'static str,
    row: gtk::ListBoxRow,
    filter: PageFilter,
}

pub struct PreferencesComponent {
    dialog: adw::Dialog,
    stack: adw::ViewStack,
    category_list: gtk::ListBox,
    categories: Rc<Vec<NavCategory>>,
    search_entry: gtk::SearchEntry,
    reset_apis_subpage: Rc<dyn Fn()>,
    sync_hooks: Vec<PageSync>,
    handle_escape: Rc<dyn Fn()>,
    bound_parent_ptr: Cell<usize>,
}

impl std::fmt::Debug for PreferencesComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreferencesComponent").finish_non_exhaustive()
    }
}

impl PreferencesComponent {
    /// Creates the Preferences dialog and initializes all category pages.
    ///
    /// # Panics
    /// Panics if the compiled `GResource` `/dev/boxxy/BoxxyTerminal/ui/preferences.ui`
    /// is missing expected template object IDs.
    #[allow(clippy::too_many_lines)]
    pub fn new<F1: Fn() + 'static, F2: Fn() + 'static>(
        on_open_themes: F1,
        on_reload_engine: F2,
    ) -> Self {
        let builder = gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/preferences.ui");
        let dialog: adw::Dialog = builder.object("dialog").unwrap();
        let stack: adw::ViewStack = builder.object("stack").unwrap();
        let content_title: adw::WindowTitle = builder.object("content_title").unwrap();
        let apis_stack: gtk::Stack = builder.object("apis_stack").unwrap();
        let apis_back_btn: gtk::Button = builder.object("apis_back_btn").unwrap();
        let category_list: gtk::ListBox = builder.object("category_list").unwrap();
        let search_entry: gtk::SearchEntry = builder.object("search_entry").unwrap();
        let page_shortcuts: adw::PreferencesPage = builder.object("page_shortcuts").unwrap();
        let page_about: adw::PreferencesPage = builder.object("page_about").unwrap();
        let toast_overlay: adw::ToastOverlay = builder.object("toast_overlay").unwrap();

        let reset_apis_subpage: Rc<dyn Fn()> = {
            let apis_stack = apis_stack.clone();
            Rc::new(move || {
                apis_stack.set_visible_child_name("list");
                apis_back_btn.set_visible(false);
            })
        };

        let handle_escape: Rc<dyn Fn()> = {
            let dialog = dialog.clone();
            let stack = stack.clone();
            let apis_stack = apis_stack;
            let reset_apis = reset_apis_subpage.clone();
            let title = content_title.clone();
            Rc::new(move || {
                if stack.visible_child_name().as_deref() == Some("apis")
                    && apis_stack.visible_child_name().as_deref() != Some("list")
                {
                    reset_apis();
                    title.set_title("APIs");
                    return;
                }
                dialog.close();
            })
        };

        let esc_handler = gtk::EventControllerKey::new();
        esc_handler.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let handle_escape_dialog = handle_escape.clone();
            esc_handler.connect_key_pressed(move |_, keyval, _, _| {
                if keyval == gtk::gdk::Key::Escape {
                    handle_escape_dialog();
                    gtk::glib::Propagation::Stop
                } else {
                    gtk::glib::Propagation::Proceed
                }
            });
        }
        dialog.add_controller(esc_handler);

        // Initialize all pages and collect their sync + search filter hooks
        let (appearance_sync, appearance_filter) =
            crate::appearance::setup_appearance_page(&builder, Rc::new(on_open_themes));
        let (previews_sync, previews_filter) = crate::previews::setup_previews_page(&builder);
        let (themes_sync, themes_filter) = crate::themes_page::setup_themes_page(&builder);
        let (apis_sync, apis_filter) = crate::apis::setup_apis_page(&builder);
        let (characters_sync, characters_filter) =
            crate::characters::setup_characters_page(&builder);
        let (integrations_sync, integrations_filter) =
            crate::integrations::setup_integrations_page(&builder);
        let (toolbox_sync, toolbox_filter) = crate::toolbox::setup_toolbox_page(&builder);
        let (mcp_sync, mcp_filter) = crate::mcp::setup_mcp_page(&builder);
        let (skills_sync, skills_filter) = crate::skills::setup_skills_page(&builder);
        let (claw_ui_sync, claw_ui_filter) = crate::claw_ui::setup_claw_ui_page(&builder);
        let (advanced_sync, advanced_filter) =
            crate::advanced::setup_advanced_page(&builder, Rc::new(on_reload_engine));
        let shortcuts_filter = crate::shortcuts::populate_shortcuts_page(&page_shortcuts);
        let (about_sync, about_filter) =
            crate::about::populate_about_page(&page_about, &toast_overlay);

        let terminal_filter: PageFilter = Box::new(move |query: &str| {
            let app_vis = appearance_filter(query);
            let prev_vis = previews_filter(query);
            app_vis || prev_vis
        });

        let sync_hooks = vec![
            appearance_sync,
            previews_sync,
            themes_sync,
            apis_sync,
            characters_sync,
            integrations_sync,
            toolbox_sync,
            mcp_sync,
            skills_sync,
            claw_ui_sync,
            advanced_sync,
            about_sync,
        ];

        let make_cat = |page_name: &'static str,
                        widget_id: &'static str,
                        header_title: &'static str,
                        filter: PageFilter|
         -> NavCategory {
            let row: gtk::ListBoxRow = builder.object(widget_id).unwrap();
            row.set_widget_name(widget_id);
            NavCategory {
                page_name,
                widget_id,
                header_title,
                row,
                filter,
            }
        };

        let categories = Rc::new(vec![
            make_cat("appearance", "nav_appearance", "Terminal", terminal_filter),
            make_cat("themes", "nav_themes", "Themes", themes_filter),
            make_cat("apis", "nav_apis", "APIs", apis_filter),
            make_cat(
                "characters",
                "nav_characters",
                "Characters",
                characters_filter,
            ),
            make_cat(
                "integrations",
                "nav_integrations",
                "Desktop Integrations",
                integrations_filter,
            ),
            make_cat("mcp", "nav_mcp", "MCP", mcp_filter),
            make_cat("skills", "nav_skills", "Skills", skills_filter),
            make_cat(
                "claw_ui",
                "nav_claw_ui",
                "Claw UI & Sounds",
                claw_ui_filter,
            ),
            make_cat(
                "toolbox",
                "nav_toolbox",
                "Toolbox & Permissions",
                toolbox_filter,
            ),
            make_cat(
                "shortcuts",
                "nav_shortcuts",
                "Shortcuts",
                shortcuts_filter,
            ),
            make_cat("advanced", "nav_advanced", "Advanced", advanced_filter),
            make_cat("about", "nav_about", "About", about_filter),
        ]);

        {
            let stack = stack.clone();
            let title = content_title;
            let reset_apis = reset_apis_subpage.clone();
            let cats = categories.clone();
            category_list.connect_row_selected(move |_, selected| {
                let Some(row) = selected else { return };
                reset_apis();
                let row_name = row.widget_name();
                if let Some(cat) = cats.iter().find(|c| c.widget_id == row_name.as_str()) {
                    title.set_title(cat.header_title);
                    stack.set_visible_child_name(cat.page_name);
                }
            });
        }

        // Real-time search filtering across all categories
        {
            let list = category_list.clone();
            let cats = categories.clone();
            search_entry.connect_search_changed(move |entry| {
                let query = entry.text().trim().to_lowercase();

                for cat in cats.iter() {
                    let visible = (cat.filter)(&query);
                    cat.row.set_visible(visible);
                }

                if list.selected_row().is_none_or(|r| !r.is_visible())
                    && let Some(first_visible) = cats.iter().find(|c| c.row.is_visible())
                {
                    list.select_row(Some(&first_visible.row));
                }
            });
        }

        // Select the first category ("Terminal") by default on creation
        if let Some(first) = categories.first() {
            category_list.select_row(Some(&first.row));
        }

        Self {
            dialog,
            stack,
            category_list,
            categories,
            search_entry,
            reset_apis_subpage,
            sync_hooks,
            handle_escape,
            bound_parent_ptr: Cell::new(0),
        }
    }

    pub fn show(&self, parent: &impl IsA<gtk::Window>) {
        let parent_win: &gtk::Window = parent.upcast_ref();
        let parent_ptr = parent_win.as_ptr() as usize;
        if self.bound_parent_ptr.get() != parent_ptr {
            self.bound_parent_ptr.set(parent_ptr);
            // Fallback Capture-phase Escape handler on the host GtkWindow so Escape
            // always closes Preferences even if internal focus becomes `None`.
            let win_esc = gtk::EventControllerKey::new();
            win_esc.set_propagation_phase(gtk::PropagationPhase::Capture);
            let dialog_weak = self.dialog.downgrade();
            let parent_weak = parent_win.downgrade();
            let handle_escape = self.handle_escape.clone();
            win_esc.connect_key_pressed(move |_, keyval, _, _| {
                if keyval != gtk::gdk::Key::Escape {
                    return gtk::glib::Propagation::Proceed;
                }
                let Some(dialog) = dialog_weak.upgrade() else {
                    return gtk::glib::Propagation::Proceed;
                };
                let Some(win) = parent_weak.upgrade() else {
                    return gtk::glib::Propagation::Proceed;
                };
                let is_top_dialog = win
                    .downcast_ref::<adw::ApplicationWindow>()
                    .and_then(adw::prelude::AdwApplicationWindowExt::visible_dialog)
                    .is_some_and(|d| d == dialog)
                    || win
                        .downcast_ref::<adw::Window>()
                        .and_then(adw::prelude::AdwWindowExt::visible_dialog)
                        .is_some_and(|d| d == dialog);
                if !is_top_dialog {
                    return gtk::glib::Propagation::Proceed;
                }
                // Let open popovers (e.g. AdwComboRow dropdowns) dismiss themselves first.
                if gtk::prelude::GtkWindowExt::focus(&win)
                    .and_then(|w| w.ancestor(gtk::Popover::static_type()))
                    .is_some()
                {
                    return gtk::glib::Propagation::Proceed;
                }
                handle_escape();
                gtk::glib::Propagation::Stop
            });
            parent_win.add_controller(win_esc);
        }

        (self.reset_apis_subpage)();
        self.search_entry.set_text("");
        if self.category_list.selected_row().is_none()
            && let Some(first) = self.categories.first()
        {
            self.category_list.select_row(Some(&first.row));
        }

        let target_width = (parent_win.width() - 40).clamp(600, 950);
        let target_height = (parent_win.height() - 40).max(300);
        self.dialog.set_content_width(target_width);
        self.dialog.set_content_height(target_height);
        self.dialog.set_focus(Some(&self.search_entry));
        self.dialog.present(Some(parent_win));
        self.search_entry.grab_focus();
    }

    #[must_use]
    pub const fn widget(&self) -> &adw::Dialog {
        &self.dialog
    }

    pub fn show_page(&self, page_name: &str) {
        (self.reset_apis_subpage)();
        self.search_entry.set_text("");
        if let Some(cat) = self.categories.iter().find(|c| c.page_name == page_name) {
            self.category_list.select_row(Some(&cat.row));
        } else {
            self.stack.set_visible_child_name(page_name);
        }
    }

    pub fn sync_settings(&self, settings: &Settings) {
        for sync in &self.sync_hooks {
            sync(settings);
        }
    }

    pub fn hide(&self) {
        self.dialog.close();
    }
}
