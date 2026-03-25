use crate::config::Settings;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct PreferencesComponent {
    dialog: adw::Dialog,
    stack: adw::ViewStack,
    nav_shortcuts: gtk::ListBoxRow,
    nav_about: gtk::ListBoxRow,
    search_entry: gtk::SearchEntry,
    theme_row: adw::ActionRow,
    chat_width_spin: adw::SpinRow,
    settings_rc: Rc<RefCell<Settings>>,
}

impl std::fmt::Debug for PreferencesComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreferencesComponent").finish()
    }
}

impl PreferencesComponent {
    pub fn new<F1: Fn(Settings) + 'static, F2: Fn() + 'static, F3: Fn() + 'static>(
        on_settings_changed: F1,
        on_open_themes: F2,
        on_reload_engine: F3,
    ) -> Self {
        let settings_rc = Rc::new(RefCell::new(Settings::load()));
        let cb_rc = Rc::new(on_settings_changed);

        let builder = gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/preferences.ui");
        let dialog: adw::Dialog = builder.object("dialog").unwrap();
        let stack: adw::ViewStack = builder.object("stack").unwrap();
        let content_title: adw::WindowTitle = builder.object("content_title").unwrap();

        let esc_handler = gtk::EventControllerKey::new();
        esc_handler.set_propagation_phase(gtk::PropagationPhase::Capture);
        let dialog_esc_clone = dialog.clone();
        esc_handler.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                dialog_esc_clone.close();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        dialog.add_controller(esc_handler);

        // Left sidebar category list
        let category_list: gtk::ListBox = builder.object("category_list").unwrap();
        let nav_appearance: gtk::ListBoxRow = builder.object("nav_appearance").unwrap();
        nav_appearance.set_widget_name("nav_appearance");
        let nav_previews: gtk::ListBoxRow = builder.object("nav_previews").unwrap();
        nav_previews.set_widget_name("nav_previews");
        let nav_apis: gtk::ListBoxRow = builder.object("nav_apis").unwrap();
        nav_apis.set_widget_name("nav_apis");
        let nav_agents: gtk::ListBoxRow = builder.object("nav_agents").unwrap();
        nav_agents.set_widget_name("nav_agents");
        let nav_shortcuts: gtk::ListBoxRow = builder.object("nav_shortcuts").unwrap();
        nav_shortcuts.set_widget_name("nav_shortcuts");
        let nav_advanced: gtk::ListBoxRow = builder.object("nav_advanced").unwrap();
        nav_advanced.set_widget_name("nav_advanced");
        let nav_about: gtk::ListBoxRow = builder.object("nav_about").unwrap();
        nav_about.set_widget_name("nav_about");

        let stack_clone = stack.clone();
        let title_clone = content_title.clone();
        category_list.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                let name = match r.widget_name().as_str() {
                    "nav_appearance" => {
                        title_clone.set_title("Appearance");
                        "appearance"
                    }
                    "nav_previews" => {
                        title_clone.set_title("Previews");
                        "previews"
                    }
                    "nav_apis" => {
                        title_clone.set_title("APIs");
                        "apis"
                    }
                    "nav_agents" => {
                        title_clone.set_title("Agents");
                        "agents"
                    }
                    "nav_shortcuts" => {
                        title_clone.set_title("Shortcuts");
                        "shortcuts"
                    }
                    "nav_advanced" => {
                        title_clone.set_title("Advanced");
                        "advanced"
                    }
                    "nav_about" => {
                        title_clone.set_title("About");
                        "about"
                    }
                    _ => "appearance",
                };
                stack_clone.set_visible_child_name(name);
            }
        });
        category_list.select_row(Some(&nav_appearance));

        let page_shortcuts: adw::PreferencesPage = builder.object("page_shortcuts").unwrap();
        let page_about: adw::PreferencesPage = builder.object("page_about").unwrap();

        // Initialize submodules
        let (appearance_widgets, appearance_filter) = crate::appearance::setup_appearance_page(
            &builder,
            settings_rc.clone(),
            cb_rc.clone(),
            Rc::new(on_open_themes),
        );
        let previews_filter =
            crate::previews::setup_previews_page(&builder, settings_rc.clone(), cb_rc.clone());
        let apis_filter =
            crate::apis::setup_apis_page(&builder, settings_rc.clone(), cb_rc.clone());
        let agents_filter =
            crate::agents::setup_agents_page(&builder, settings_rc.clone(), cb_rc.clone());
        let advanced_filter = crate::advanced::setup_advanced_page(
            &builder,
            settings_rc.clone(),
            cb_rc.clone(),
            Rc::new(on_reload_engine),
        );
        let shortcuts_filter = crate::shortcuts::populate_shortcuts_page(&page_shortcuts);
        let about_filter = crate::about::populate_about_page(&page_about);

        let theme_row = appearance_widgets.theme_row;
        let chat_width_spin = appearance_widgets.chat_width_spin;

        // Search filtering logic
        let search_entry: gtk::SearchEntry = builder.object("search_entry").unwrap();
        let list_clone = category_list.clone();
        let nav_appearance_clone = nav_appearance.clone();
        let nav_previews_clone = nav_previews.clone();
        let nav_apis_clone = nav_apis.clone();
        let nav_agents_clone = nav_agents.clone();
        let nav_advanced_clone = nav_advanced.clone();
        let nav_shortcuts_clone = nav_shortcuts.clone();
        let nav_about_clone = nav_about.clone();

        search_entry.connect_search_changed(move |entry| {
            let query = entry.text().to_lowercase();

            nav_appearance_clone.set_visible(appearance_filter(&query));
            nav_previews_clone.set_visible(previews_filter(&query));
            nav_apis_clone.set_visible(apis_filter(&query));
            nav_agents_clone.set_visible(agents_filter(&query));
            nav_advanced_clone.set_visible(advanced_filter(&query));
            nav_shortcuts_clone.set_visible(shortcuts_filter(&query));
            nav_about_clone.set_visible(about_filter(&query));

            if let Some(selected) = list_clone.selected_row()
                && !selected.is_visible()
            {
                for i in 0..6 {
                    if let Some(row) = list_clone.row_at_index(i)
                        && row.is_visible()
                    {
                        list_clone.select_row(Some(&row));
                        break;
                    }
                }
            }
        });

        Self {
            dialog,
            stack,
            nav_shortcuts,
            nav_about,
            search_entry,
            theme_row,
            chat_width_spin,
            settings_rc,
        }
    }

    pub fn show(&self, parent: &gtk::Window) {
        self.search_entry.set_text("");
        let width = parent.width();
        let height = parent.height();
        let target_width = (width - 40).clamp(600, 950);
        let target_height = (height - 40).max(300);
        self.dialog.set_content_width(target_width);
        self.dialog.set_content_height(target_height);
        self.dialog.present(Some(parent));
    }

    pub fn widget(&self) -> &adw::Dialog {
        &self.dialog
    }

    pub fn show_page(&self, page_name: &str) {
        if let Some(list_box) = self
            .nav_shortcuts
            .parent()
            .and_then(|p| p.downcast::<gtk::ListBox>().ok())
        {
            if page_name == "shortcuts" {
                list_box.select_row(Some(&self.nav_shortcuts));
            } else if page_name == "about" {
                list_box.select_row(Some(&self.nav_about));
            }
        }
        self.stack.set_visible_child_name(page_name);
    }

    pub fn set_theme(&self, theme: &str) {
        self.theme_row.set_subtitle(theme);
        self.settings_rc.borrow_mut().theme = theme.to_string();
    }

    pub fn sync_settings(&self, settings: &Settings) {
        *self.settings_rc.borrow_mut() = settings.clone();
        self.theme_row.set_subtitle(&settings.theme);
        if (self.chat_width_spin.value() - settings.ai_chat_width as f64).abs() > 1e-6 {
            self.chat_width_spin
                .set_value(settings.ai_chat_width as f64);
        }
    }

    pub fn hide(&self) {
        self.dialog.close();
    }
}
