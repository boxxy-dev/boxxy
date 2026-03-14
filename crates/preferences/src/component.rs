use crate::config::{CursorShape, Settings};
use libadwaita::prelude::*;
use libadwaita as adw;
use gtk4 as gtk;
use gtk::{gdk, pango};
use std::fs;
use std::rc::Rc;
use std::cell::RefCell;

#[derive(Clone)]
pub struct PreferencesComponent {
    dialog: adw::Dialog,
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
    pub fn new<F1: Fn(Settings) + 'static, F2: Fn() + 'static, F3: Fn() + 'static>(on_settings_changed: F1, on_open_themes: F2, on_reload_engine: F3) -> Self {
        let settings_rc = Rc::new(RefCell::new(Settings::load()));
        let cb_rc = Rc::new(on_settings_changed);
        let reload_cb_rc = Rc::new(on_reload_engine);

        let builder = gtk::Builder::from_resource("/play/mii/Boxxy/ui/preferences.ui");
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
        let nav_advanced: gtk::ListBoxRow = builder.object("nav_advanced").unwrap();
        nav_advanced.set_widget_name("nav_advanced");
        
        let stack_clone = stack.clone();
        let title_clone = content_title.clone();
        category_list.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                let name = match r.widget_name().as_str() {
                    "nav_appearance" => { title_clone.set_title("Appearance"); "appearance" },
                    "nav_previews" => { title_clone.set_title("Previews"); "previews" },
                    "nav_apis" => { title_clone.set_title("APIs"); "apis" },
                    "nav_advanced" => { title_clone.set_title("Advanced"); "advanced" },
                    _ => "appearance"
                };
                stack_clone.set_visible_child_name(name);
            }
        });
        category_list.select_row(Some(&nav_appearance));

        // Get references to widgets
        let font_row: adw::ActionRow = builder.object("font_row").unwrap();
        let theme_row: adw::ActionRow = builder.object("theme_row").unwrap();
        let padding_spin: adw::SpinRow = builder.object("padding_spin").unwrap();
        let line_spacing_spin: adw::SpinRow = builder.object("line_spacing_spin").unwrap();
        let col_spacing_spin: adw::SpinRow = builder.object("col_spacing_spin").unwrap();
        let preserve_cwd_switch: adw::SwitchRow = builder.object("preserve_cwd_switch").unwrap();
        let cursor_shape_combo: adw::ComboRow = builder.object("cursor_shape_combo").unwrap();
        let cursor_color_switch: adw::SwitchRow = builder.object("cursor_color_switch").unwrap();
        let cursor_color_row: adw::ActionRow = builder.object("cursor_color_row").unwrap();
        let cursor_blinking_switch: adw::SwitchRow = builder.object("cursor_blinking_switch").unwrap();
        let hide_scrollbars_switch: adw::SwitchRow = builder.object("hide_scrollbars_switch").unwrap();
        let invert_scroll_switch: adw::SwitchRow = builder.object("invert_scroll_switch").unwrap();
        let dim_inactive_switch: adw::SwitchRow = builder.object("dim_inactive_switch").unwrap();
        let always_show_tabs_switch: adw::SwitchRow = builder.object("always_show_tabs_switch").unwrap();
        let fixed_width_tabs_switch: adw::SwitchRow = builder.object("fixed_width_tabs_switch").unwrap();
        let chat_width_spin: adw::SpinRow = builder.object("chat_width_spin").unwrap();
        let open_config_btn: gtk::Button = builder.object("open_config_btn").unwrap();
        let reset_config_btn: gtk::Button = builder.object("reset_config_btn").unwrap();
        let api_key_entry: adw::PasswordEntryRow = builder.object("api_key_entry").unwrap();
        let ollama_base_url_entry: adw::EntryRow = builder.object("ollama_base_url_entry").unwrap();
        let login_shell_switch: adw::SwitchRow = builder.object("login_shell_switch").unwrap();
        let show_vte_grid_switch: adw::SwitchRow = builder.object("show_vte_grid_switch").unwrap();
        let image_preview_trigger_combo: adw::ComboRow = builder.object("image_preview_trigger_combo").unwrap();
        let preview_max_width_spin: adw::SpinRow = builder.object("preview_max_width_spin").unwrap();
        let preview_max_height_spin: adw::SpinRow = builder.object("preview_max_height_spin").unwrap();
        let custom_regex_entry: adw::EntryRow = builder.object("custom_regex_entry").unwrap();
        let reset_regex_btn: gtk::Button = builder.object("reset_regex_btn").unwrap();

        let row_hyperlink_warning: adw::ActionRow = builder.object("row_hyperlink_warning").unwrap();
        let row_reset_regex: adw::ActionRow = builder.object("row_reset_regex").unwrap();
        let row_open_config: adw::ActionRow = builder.object("row_open_config").unwrap();
        let row_reset_config: adw::ActionRow = builder.object("row_reset_config").unwrap();

        // 1. Font
        let font_dialog = gtk::FontDialog::new();
        let font_button = gtk::FontDialogButton::new(Some(font_dialog));
        font_button.set_valign(gtk::Align::Center);
        let mut initial_desc = pango::FontDescription::new();
        initial_desc.set_family(&settings_rc.borrow().font_name);
        initial_desc.set_size(settings_rc.borrow().font_size as i32 * pango::SCALE);
        font_button.set_font_desc(&initial_desc);
        font_row.add_suffix(&font_button);

        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        font_button.connect_font_desc_notify(move |btn| {
            if let Some(desc) = btn.font_desc()
                && let Some(family) = desc.family() {
                    let size_pt = (desc.size() / pango::SCALE).max(6) as u16;
                    let mut s = s_rc.borrow_mut();
                    if s.font_name != family || s.font_size != size_pt {
                        s.font_name = family.to_string();
                        s.font_size = size_pt;
                        s.save();
                        cb(s.clone());
                    }
                }
        });

        // 2. Adjustments
        let padding_adj = gtk::Adjustment::new(settings_rc.borrow().padding as f64, 0.0, 60.0, 2.0, 8.0, 0.0);
        padding_spin.set_adjustment(Some(&padding_adj));
        padding_spin.set_value(settings_rc.borrow().padding as f64);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        padding_spin.connect_value_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            let v = row.value() as i32;
            if s.padding != v {
                s.padding = v;
                s.save();
                cb(s.clone());
            }
        });

        let line_spacing_adj = gtk::Adjustment::new(settings_rc.borrow().cell_height_scale, 1.0, 3.0, 0.1, 0.5, 0.0);
        line_spacing_spin.set_adjustment(Some(&line_spacing_adj));
        line_spacing_spin.set_value(settings_rc.borrow().cell_height_scale);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        line_spacing_spin.connect_value_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if (s.cell_height_scale - row.value()).abs() > 1e-6 {
                s.cell_height_scale = row.value();
                s.save();
                cb(s.clone());
            }
        });

        let col_spacing_adj = gtk::Adjustment::new(settings_rc.borrow().cell_width_scale, 1.0, 3.0, 0.1, 0.5, 0.0);
        col_spacing_spin.set_adjustment(Some(&col_spacing_adj));
        col_spacing_spin.set_value(settings_rc.borrow().cell_width_scale);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        col_spacing_spin.connect_value_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if (s.cell_width_scale - row.value()).abs() > 1e-6 {
                s.cell_width_scale = row.value();
                s.save();
                cb(s.clone());
            }
        });

        let chat_width_adj = gtk::Adjustment::new(settings_rc.borrow().ai_chat_width as f64, 360.0, 800.0, 10.0, 50.0, 0.0);
        chat_width_spin.set_adjustment(Some(&chat_width_adj));
        chat_width_spin.set_value(settings_rc.borrow().ai_chat_width as f64);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        chat_width_spin.connect_value_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            let v = row.value() as i32;
            if s.ai_chat_width != v {
                s.ai_chat_width = v;
                s.save();
                cb(s.clone());
            }
        });

        // 3. Theme ActionRow
        theme_row.set_subtitle(&settings_rc.borrow().theme);

        let dialog_clone = dialog.clone();
        theme_row.connect_activated(move |_| {
            dialog_clone.close();
            on_open_themes();
        });

        // 4. Switches
        preserve_cwd_switch.set_active(settings_rc.borrow().preserve_working_dir);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        preserve_cwd_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.preserve_working_dir != row.is_active() {
                s.preserve_working_dir = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        hide_scrollbars_switch.set_active(settings_rc.borrow().hide_scrollbars);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        hide_scrollbars_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.hide_scrollbars != row.is_active() {
                s.hide_scrollbars = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        invert_scroll_switch.set_active(settings_rc.borrow().invert_scroll);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        invert_scroll_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.invert_scroll != row.is_active() {
                s.invert_scroll = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        dim_inactive_switch.set_active(settings_rc.borrow().dim_inactive_panes);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        dim_inactive_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.dim_inactive_panes != row.is_active() {
                s.dim_inactive_panes = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        always_show_tabs_switch.set_active(settings_rc.borrow().always_show_tabs);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        always_show_tabs_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.always_show_tabs != row.is_active() {
                s.always_show_tabs = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        fixed_width_tabs_switch.set_active(settings_rc.borrow().fixed_width_tabs);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        fixed_width_tabs_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.fixed_width_tabs != row.is_active() {
                s.fixed_width_tabs = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        // 5. Cursor settings
        let cursor_shapes_list = gtk::StringList::new(&["Block", "I-Beam", "Underline"]);
        cursor_shape_combo.set_model(Some(&cursor_shapes_list));
        let shape_idx = match settings_rc.borrow().cursor_shape {
            CursorShape::Block => 0,
            CursorShape::IBeam => 1,
            CursorShape::Underline => 2,
        };
        cursor_shape_combo.set_selected(shape_idx);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        cursor_shape_combo.connect_selected_notify(move |row| {
            let shape = match row.selected() {
                0 => CursorShape::Block,
                1 => CursorShape::IBeam,
                2 => CursorShape::Underline,
                _ => return,
            };
            let mut s = s_rc.borrow_mut();
            if s.cursor_shape != shape {
                s.cursor_shape = shape;
                s.save();
                cb(s.clone());
            }
        });

        cursor_color_switch.set_active(settings_rc.borrow().cursor_color_override);
        cursor_color_row.set_sensitive(settings_rc.borrow().cursor_color_override);
        let ccr_clone = cursor_color_row.clone();
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        cursor_color_switch.connect_active_notify(move |row| {
            ccr_clone.set_sensitive(row.is_active());
            let mut s = s_rc.borrow_mut();
            if s.cursor_color_override != row.is_active() {
                s.cursor_color_override = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        let cursor_color_dialog = gtk::ColorDialog::builder().with_alpha(false).build();
        let cursor_color_button = gtk::ColorDialogButton::new(Some(cursor_color_dialog));
        cursor_color_button.set_valign(gtk::Align::Center);
        if let Ok(rgba) = gdk::RGBA::parse(&settings_rc.borrow().cursor_color) {
            cursor_color_button.set_rgba(&rgba);
        }
        cursor_color_row.add_suffix(&cursor_color_button);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        cursor_color_button.connect_rgba_notify(move |btn| {
            let rgba = btn.rgba();
            let current = rgba.to_str().to_string();
            let mut s = s_rc.borrow_mut();
            if s.cursor_color != current {
                s.cursor_color = current;
                s.save();
                cb(s.clone());
            }
        });

        cursor_blinking_switch.set_active(settings_rc.borrow().cursor_blinking);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        cursor_blinking_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.cursor_blinking != row.is_active() {
                s.cursor_blinking = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        // 6. Config buttons
        open_config_btn.connect_clicked(|_| {
            if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
                let config_dir = dirs.config_dir();
                if !config_dir.exists() {
                    let _ = fs::create_dir_all(config_dir);
                }
                let uri = format!("file://{}", config_dir.display());
                let _ = gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);
            }
        });

        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        let dialog_clone = dialog.clone();
        reset_config_btn.connect_clicked(move |_| {
            let confirm = libadwaita::AlertDialog::builder()
                .heading("Reset Everything?")
                .body("This will delete all settings and permanently remove all installed Boxxy apps. This action cannot be undone.")
                .build();
            confirm.add_response("cancel", "Cancel");
            confirm.add_response("reset", "Reset");
            confirm.set_response_appearance("reset", libadwaita::ResponseAppearance::Destructive);

            let s_rc2 = s_rc.clone();
            let cb2 = cb.clone();
            let reload_cb2 = reload_cb_rc.clone();
            confirm.connect_response(None, move |_, response| {
                if response == "reset" {
                    if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
                        let _ = fs::remove_dir_all(dirs.config_dir());
                        Settings::ensure_claw_skills();
                    }
                    let updated_settings = Settings::default();
                    {
                        let mut s = s_rc2.borrow_mut();
                        *s = updated_settings.clone();
                        s.save();
                    }
                    cb2(updated_settings);
                    reload_cb2();
                }
            });
            confirm.present(Some(&dialog_clone));
        });

        // 7. APIs
        api_key_entry.set_text(&settings_rc.borrow().gemini_api_key);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        api_key_entry.connect_changed(move |entry| {
            let mut s = s_rc.borrow_mut();
            if s.gemini_api_key != entry.text().as_str() {
                s.gemini_api_key = entry.text().to_string();
                s.save();
                cb(s.clone());
            }
        });

        ollama_base_url_entry.set_text(&settings_rc.borrow().ollama_base_url);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        ollama_base_url_entry.connect_changed(move |entry| {
            let mut s = s_rc.borrow_mut();
            if s.ollama_base_url != entry.text().as_str() {
                s.ollama_base_url = entry.text().to_string();
                s.save();
                cb(s.clone());
            }
        });

        // 8. Advanced
        login_shell_switch.set_active(settings_rc.borrow().login_shell);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        login_shell_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.login_shell != row.is_active() {
                s.login_shell = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        show_vte_grid_switch.set_active(settings_rc.borrow().show_vte_grid);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        show_vte_grid_switch.connect_active_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            if s.show_vte_grid != row.is_active() {
                s.show_vte_grid = row.is_active();
                s.save();
                cb(s.clone());
            }
        });

        let preview_triggers_list = gtk::StringList::new(&["Disabled", "On Click (Shift+Click)", "On Hover"]);
        image_preview_trigger_combo.set_model(Some(&preview_triggers_list));
        let trigger_idx = match settings_rc.borrow().image_preview_trigger {
            crate::config::ImagePreviewTrigger::None => 0,
            crate::config::ImagePreviewTrigger::Click => 1,
            crate::config::ImagePreviewTrigger::Hover => 2,
        };
        image_preview_trigger_combo.set_selected(trigger_idx);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        image_preview_trigger_combo.connect_selected_notify(move |row| {
            let trigger = match row.selected() {
                0 => crate::config::ImagePreviewTrigger::None,
                1 => crate::config::ImagePreviewTrigger::Click,
                2 => crate::config::ImagePreviewTrigger::Hover,
                _ => return,
            };
            let mut s = s_rc.borrow_mut();
            if s.image_preview_trigger != trigger {
                s.image_preview_trigger = trigger;
                s.save();
                cb(s.clone());
            }
        });

        let preview_max_width_adj = gtk::Adjustment::new(settings_rc.borrow().preview_max_width as f64, 90.0, 600.0, 10.0, 50.0, 0.0);
        preview_max_width_spin.set_adjustment(Some(&preview_max_width_adj));
        preview_max_width_spin.set_value(settings_rc.borrow().preview_max_width as f64);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        preview_max_width_spin.connect_value_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            let v = row.value() as i32;
            if s.preview_max_width != v {
                s.preview_max_width = v;
                s.save();
                cb(s.clone());
            }
        });

        let preview_max_height_adj = gtk::Adjustment::new(settings_rc.borrow().preview_max_height as f64, 90.0, 600.0, 10.0, 50.0, 0.0);
        preview_max_height_spin.set_adjustment(Some(&preview_max_height_adj));
        preview_max_height_spin.set_value(settings_rc.borrow().preview_max_height as f64);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        preview_max_height_spin.connect_value_notify(move |row| {
            let mut s = s_rc.borrow_mut();
            let v = row.value() as i32;
            if s.preview_max_height != v {
                s.preview_max_height = v;
                s.save();
                cb(s.clone());
            }
        });

        custom_regex_entry.set_text(&settings_rc.borrow().custom_regex);
        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        custom_regex_entry.connect_changed(move |entry| {
            let mut s = s_rc.borrow_mut();
            if s.custom_regex != entry.text().as_str() {
                s.custom_regex = entry.text().to_string();
                s.save();
                cb(s.clone());
            }
        });

        let s_rc = settings_rc.clone();
        let cb = cb_rc.clone();
        let regex_entry_clone = custom_regex_entry.clone();
        reset_regex_btn.connect_clicked(move |_| {
            regex_entry_clone.set_text(crate::config::DEFAULT_FILE_REGEX);
            let mut s = s_rc.borrow_mut();
            if s.custom_regex != crate::config::DEFAULT_FILE_REGEX {
                s.custom_regex = crate::config::DEFAULT_FILE_REGEX.to_string();
                s.save();
                cb(s.clone());
            }
        });

        // Search filtering logic
        let search_entry: gtk::SearchEntry = builder.object("search_entry").unwrap();
        
        let group_font: adw::PreferencesGroup = builder.object("group_font").unwrap();
        let group_terminal: adw::PreferencesGroup = builder.object("group_terminal").unwrap();
        let group_cursor: adw::PreferencesGroup = builder.object("group_cursor").unwrap();
        let group_layout: adw::PreferencesGroup = builder.object("group_layout").unwrap();
        let group_image_previews: adw::PreferencesGroup = builder.object("group_image_previews").unwrap();
        let group_gemini_api: adw::PreferencesGroup = builder.object("group_gemini_api").unwrap();
        let group_ollama_api: adw::PreferencesGroup = builder.object("group_ollama_api").unwrap();
        let group_shell: adw::PreferencesGroup = builder.object("group_shell").unwrap();
        let group_terminal_interaction: adw::PreferencesGroup = builder.object("group_terminal_interaction").unwrap();
        let group_config: adw::PreferencesGroup = builder.object("group_config").unwrap();

        let list_clone = category_list.clone();
        let font_row_clone = font_row.clone();
        let theme_row_clone = theme_row.clone();
        let padding_spin_clone = padding_spin.clone();
        let line_spacing_spin_clone = line_spacing_spin.clone();
        let col_spacing_spin_clone = col_spacing_spin.clone();
        let preserve_cwd_switch_clone = preserve_cwd_switch.clone();
        let cursor_shape_combo_clone = cursor_shape_combo.clone();
        let cursor_color_switch_clone = cursor_color_switch.clone();
        let cursor_color_row_clone = cursor_color_row.clone();
        let cursor_blinking_switch_clone = cursor_blinking_switch.clone();
        let hide_scrollbars_switch_clone = hide_scrollbars_switch.clone();
        let invert_scroll_switch_clone = invert_scroll_switch.clone();
        let dim_inactive_switch_clone = dim_inactive_switch.clone();
        let always_show_tabs_switch_clone = always_show_tabs_switch.clone();
        let fixed_width_tabs_switch_clone = fixed_width_tabs_switch.clone();
        let chat_width_spin_clone = chat_width_spin.clone();
        let row_hyperlink_warning_clone = row_hyperlink_warning.clone();
        let image_preview_trigger_combo_clone = image_preview_trigger_combo.clone();
        let preview_max_width_spin_clone = preview_max_width_spin.clone();
        let preview_max_height_spin_clone = preview_max_height_spin.clone();
        let api_key_entry_clone = api_key_entry.clone();
        let ollama_base_url_entry_clone = ollama_base_url_entry.clone();
        let login_shell_switch_clone = login_shell_switch.clone();
        let show_vte_grid_switch_clone = show_vte_grid_switch.clone();
        let custom_regex_entry_clone = custom_regex_entry.clone();
        let row_reset_regex_clone = row_reset_regex.clone();
        let row_open_config_clone = row_open_config.clone();
        let row_reset_config_clone = row_reset_config.clone();

        search_entry.connect_search_changed(move |entry| {
            let query = entry.text().to_lowercase();
            
            let match_row = |r: &gtk::Widget, text: &str| {
                let m = query.is_empty() || text.to_lowercase().contains(&query);
                r.set_visible(m);
                m
            };

            let f1 = match_row(font_row_clone.upcast_ref(), "font family size");
            let t1 = match_row(theme_row_clone.upcast_ref(), "theme");
            let t2 = match_row(padding_spin_clone.upcast_ref(), "padding px");
            let t3 = match_row(line_spacing_spin_clone.upcast_ref(), "line spacing");
            let t4 = match_row(col_spacing_spin_clone.upcast_ref(), "column spacing");
            let t5 = match_row(preserve_cwd_switch_clone.upcast_ref(), "preserve working directory new tabs open in active tab directory");
            let c1 = match_row(cursor_shape_combo_clone.upcast_ref(), "cursor shape");
            let c2 = match_row(cursor_color_switch_clone.upcast_ref(), "cursor custom color");
            let c3 = match_row(cursor_color_row_clone.upcast_ref(), "cursor color");
            let c4 = match_row(cursor_blinking_switch_clone.upcast_ref(), "blinking cursor fades in out");
            let l1 = match_row(hide_scrollbars_switch_clone.upcast_ref(), "hide scrollbars do not show vertical");
            let l2 = match_row(invert_scroll_switch_clone.upcast_ref(), "invert scroll direction reverse mouse wheel");
            let l3 = match_row(dim_inactive_switch_clone.upcast_ref(), "dim inactive panes terminal splits slightly");
            let l4 = match_row(always_show_tabs_switch_clone.upcast_ref(), "always show tab bar");
            let l5 = match_row(fixed_width_tabs_switch_clone.upcast_ref(), "fixed width tabs do not expand");
            let l6 = match_row(chat_width_spin_clone.upcast_ref(), "sidebar width px hacky mouse resize overlay split view");

            group_font.set_visible(f1);
            group_terminal.set_visible(t1 || t2 || t3 || t4 || t5);
            group_cursor.set_visible(c1 || c2 || c3 || c4);
            group_layout.set_visible(l1 || l2 || l3 || l4 || l5 || l6);
            nav_appearance.set_visible(group_font.is_visible() || group_terminal.is_visible() || group_cursor.is_visible() || group_layout.is_visible());

            let p1 = match_row(row_hyperlink_warning_clone.upcast_ref(), "use --hyperlink for file previews ensure cli tools eza ls path copying");
            let p2 = match_row(image_preview_trigger_combo_clone.upcast_ref(), "image previews display small popup");
            let p3 = match_row(preview_max_width_spin_clone.upcast_ref(), "maximum width px");
            let p4 = match_row(preview_max_height_spin_clone.upcast_ref(), "maximum height px");
            
            group_image_previews.set_visible(p1 || p2 || p3 || p4);
            nav_previews.set_visible(group_image_previews.is_visible());

            let a1 = match_row(api_key_entry_clone.upcast_ref(), "api key gemini");
            let a2 = match_row(ollama_base_url_entry_clone.upcast_ref(), "base url ollama");
            
            group_gemini_api.set_visible(a1);
            group_ollama_api.set_visible(a2);
            nav_apis.set_visible(group_gemini_api.is_visible() || group_ollama_api.is_visible());

            let ad1 = match_row(login_shell_switch_clone.upcast_ref(), "login shell spawn terminal");
            let ad2 = match_row(show_vte_grid_switch_clone.upcast_ref(), "show vte grid lines representing cells");
            let ad3 = match_row(custom_regex_entry_clone.upcast_ref(), "file path regex ctrl+click freezes");
            let ad4 = match_row(row_reset_regex_clone.upcast_ref(), "reset to default");
            let ad5 = match_row(row_open_config_clone.upcast_ref(), "open config folder manage settings");
            let ad6 = match_row(row_reset_config_clone.upcast_ref(), "reset everything delete all apps destructive");

            group_shell.set_visible(ad1 || ad2);
            group_terminal_interaction.set_visible(ad3 || ad4);
            group_config.set_visible(ad5 || ad6);
            nav_advanced.set_visible(group_shell.is_visible() || group_terminal_interaction.is_visible() || group_config.is_visible());

            if let Some(selected) = list_clone.selected_row()
                && !selected.is_visible() {
                    for i in 0..4 {
                        if let Some(row) = list_clone.row_at_index(i)
                            && row.is_visible() {
                                list_clone.select_row(Some(&row));
                                break;
                            }
                    }
                }
        });

        Self { dialog, theme_row, chat_width_spin, settings_rc }
    }
    pub fn show(&self, parent: &gtk::Window) {
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

    pub fn set_theme(&self, theme: &str) {
        self.theme_row.set_subtitle(theme);
        self.settings_rc.borrow_mut().theme = theme.to_string();
    }

    pub fn sync_settings(&self, settings: &Settings) {
        *self.settings_rc.borrow_mut() = settings.clone();
        self.theme_row.set_subtitle(&settings.theme);
        if (self.chat_width_spin.value() - settings.ai_chat_width as f64).abs() > 1e-6 {
            self.chat_width_spin.set_value(settings.ai_chat_width as f64);
        }
    }

    pub fn hide(&self) {
        self.dialog.close();
    }
}
