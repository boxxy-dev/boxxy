use crate::config::{CursorShape, Settings};
use adw::prelude::*;
use gtk::{gdk, pango};
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

pub struct AppearanceWidgets {
    pub theme_row: adw::ActionRow,
    pub chat_width_spin: adw::SpinRow,
}

pub fn setup_appearance_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
    on_open_themes: Rc<dyn Fn() + 'static>,
) -> (AppearanceWidgets, Box<dyn Fn(&str) -> bool>) {
    let color_scheme_combo: adw::ComboRow = builder.object("color_scheme_combo").unwrap();
    let select_bg_image_btn: gtk::Button = builder.object("select_bg_image_btn").unwrap();
    let clear_bg_image_btn: gtk::Button = builder.object("clear_bg_image_btn").unwrap();
    let background_image_row: adw::ActionRow = builder.object("background_image_row").unwrap();
    let font_row: adw::ActionRow = builder.object("font_row").unwrap();
    let theme_row: adw::ActionRow = builder.object("theme_row").unwrap();
    let opacity_row: adw::ActionRow = builder.object("opacity_row").unwrap();
    let opacity_scale: gtk::Scale = builder.object("opacity_scale").unwrap();
    let padding_spin: adw::SpinRow = builder.object("padding_spin").unwrap();
    let line_spacing_spin: adw::SpinRow = builder.object("line_spacing_spin").unwrap();
    let col_spacing_spin: adw::SpinRow = builder.object("col_spacing_spin").unwrap();
    let preserve_cwd_switch: adw::SwitchRow = builder.object("preserve_cwd_switch").unwrap();
    let cursor_shape_combo: adw::ComboRow = builder.object("cursor_shape_combo").unwrap();
    let cursor_color_switch: gtk::Switch = builder.object("cursor_color_switch").unwrap();
    let cursor_color_row: adw::ActionRow = builder.object("cursor_color_row").unwrap();
    let cursor_blinking_switch: adw::SwitchRow = builder.object("cursor_blinking_switch").unwrap();
    let enable_progress_bar_switch: adw::SwitchRow =
        builder.object("enable_progress_bar_switch").unwrap();
    let hide_scrollbars_switch: adw::SwitchRow = builder.object("hide_scrollbars_switch").unwrap();
    let invert_scroll_switch: adw::SwitchRow = builder.object("invert_scroll_switch").unwrap();
    let dim_inactive_switch: adw::SwitchRow = builder.object("dim_inactive_switch").unwrap();
    let fixed_width_tabs_switch: adw::SwitchRow =
        builder.object("fixed_width_tabs_switch").unwrap();
    let colored_tabs_switch: adw::SwitchRow = builder.object("colored_tabs_switch").unwrap();
    let chat_width_spin: adw::SpinRow = builder.object("chat_width_spin").unwrap();

    let group_appearance: adw::PreferencesGroup = builder.object("group_appearance").unwrap();
    let group_terminal: adw::PreferencesGroup = builder.object("group_terminal").unwrap();
    let group_cursor: adw::PreferencesGroup = builder.object("group_cursor").unwrap();
    let group_layout: adw::PreferencesGroup = builder.object("group_layout").unwrap();

    // 0. Color Scheme
    let color_schemes_list = gtk::StringList::new(&["Follow System", "Light", "Dark"]);
    color_scheme_combo.set_model(Some(&color_schemes_list));
    let scheme_idx = match settings_rc.borrow().color_scheme {
        crate::config::ColorScheme::Default => 0,
        crate::config::ColorScheme::Light => 1,
        crate::config::ColorScheme::Dark => 2,
    };
    color_scheme_combo.set_selected(scheme_idx);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    color_scheme_combo.connect_selected_notify(move |row| {
        let scheme = match row.selected() {
            0 => crate::config::ColorScheme::Default,
            1 => crate::config::ColorScheme::Light,
            2 => crate::config::ColorScheme::Dark,
            _ => return,
        };
        let mut s = s_rc.borrow_mut();
        if s.color_scheme != scheme {
            s.color_scheme = scheme;
            s.save();
            cb(s.clone());
        }
    });

    // 0.1 Background Image
    let update_bg_subtitle = {
        let row = background_image_row.clone();
        let settings = settings_rc.clone();
        move || {
            if let Some(path) = &settings.borrow().background_image_path {
                let filename = std::path::Path::new(path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown");
                row.set_subtitle(filename);
            } else {
                row.set_subtitle("No background image selected");
            }
        }
    };
    update_bg_subtitle();

    let dialog: adw::Dialog = builder.object("dialog").unwrap();
    let file_dialog = gtk::FileDialog::new();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Images"));
    filter.add_mime_type("image/png");
    filter.add_mime_type("image/jpeg");
    filter.add_mime_type("image/webp");
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    file_dialog.set_filters(Some(&filters));

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let update_subtitle = update_bg_subtitle.clone();
    select_bg_image_btn.connect_clicked(move |_| {
        let s_rc = s_rc.clone();
        let cb = cb.clone();
        let update_subtitle = update_subtitle.clone();
        file_dialog.open(
            None::<&gtk::Window>,
            gtk::gio::Cancellable::NONE,
            move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                        && let Some(new_path) = boxxy_themes::copy_background_image(&path) {
                            let mut s = s_rc.borrow_mut();
                            s.background_image_path = Some(new_path);
                            s.save();
                            let s_clone = s.clone();
                            drop(s);
                            cb(s_clone);
                            update_subtitle();
                        }
            },
        );
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let update_subtitle = update_bg_subtitle.clone();
    clear_bg_image_btn.connect_clicked(move |_| {
        let mut s = s_rc.borrow_mut();
        s.background_image_path = None;
        s.save();
        let s_clone = s.clone();
        drop(s);
        cb(s_clone);
        update_subtitle();
    });

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
    let cb = on_change.clone();
    font_button.connect_font_desc_notify(move |btn| {
        if let Some(desc) = btn.font_desc()
            && let Some(family) = desc.family()
        {
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
    let opacity_adj = gtk::Adjustment::new(settings_rc.borrow().opacity, 0.1, 1.0, 0.05, 0.1, 0.0);
    opacity_scale.set_adjustment(&opacity_adj);

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    opacity_scale.connect_value_changed(move |scale| {
        let mut s = s_rc.borrow_mut();
        let val = scale.value();
        if (s.opacity - val).abs() > 1e-4 {
            s.opacity = val;
            s.save();
            cb(s.clone());
        }
    });

    let padding_adj = gtk::Adjustment::new(
        settings_rc.borrow().padding as f64,
        0.0,
        60.0,
        2.0,
        8.0,
        0.0,
    );
    padding_spin.set_adjustment(Some(&padding_adj));
    padding_spin.set_value(settings_rc.borrow().padding as f64);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    padding_spin.connect_value_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        let v = row.value() as i32;
        if s.padding != v {
            s.padding = v;
            s.save();
            cb(s.clone());
        }
    });

    let line_spacing_adj = gtk::Adjustment::new(
        settings_rc.borrow().cell_height_scale,
        1.0,
        3.0,
        0.1,
        0.5,
        0.0,
    );
    line_spacing_spin.set_adjustment(Some(&line_spacing_adj));
    line_spacing_spin.set_value(settings_rc.borrow().cell_height_scale);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    line_spacing_spin.connect_value_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if (s.cell_height_scale - row.value()).abs() > 1e-6 {
            s.cell_height_scale = row.value();
            s.save();
            cb(s.clone());
        }
    });

    let col_spacing_adj = gtk::Adjustment::new(
        settings_rc.borrow().cell_width_scale,
        1.0,
        3.0,
        0.1,
        0.5,
        0.0,
    );
    col_spacing_spin.set_adjustment(Some(&col_spacing_adj));
    col_spacing_spin.set_value(settings_rc.borrow().cell_width_scale);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    col_spacing_spin.connect_value_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if (s.cell_width_scale - row.value()).abs() > 1e-6 {
            s.cell_width_scale = row.value();
            s.save();
            cb(s.clone());
        }
    });

    let chat_width_adj = gtk::Adjustment::new(
        settings_rc.borrow().ai_chat_width as f64,
        200.0,
        800.0,
        10.0,
        50.0,
        0.0,
    );
    chat_width_spin.set_adjustment(Some(&chat_width_adj));
    chat_width_spin.set_value(settings_rc.borrow().ai_chat_width as f64);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
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
    let cb = on_change.clone();
    preserve_cwd_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.preserve_working_dir != row.is_active() {
            s.preserve_working_dir = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    enable_progress_bar_switch.set_active(settings_rc.borrow().enable_progress_bar);
    let s_rc_pb = settings_rc.clone();
    let cb_pb = on_change.clone();
    enable_progress_bar_switch.connect_active_notify(move |row| {
        let mut s = s_rc_pb.borrow_mut();
        if s.enable_progress_bar != row.is_active() {
            s.enable_progress_bar = row.is_active();
            s.save();
            cb_pb(s.clone());
        }
    });

    hide_scrollbars_switch.set_active(settings_rc.borrow().hide_scrollbars);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
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
    let cb = on_change.clone();
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
    let cb = on_change.clone();
    dim_inactive_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.dim_inactive_panes != row.is_active() {
            s.dim_inactive_panes = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    fixed_width_tabs_switch.set_active(settings_rc.borrow().fixed_width_tabs);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    fixed_width_tabs_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.fixed_width_tabs != row.is_active() {
            s.fixed_width_tabs = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    colored_tabs_switch.set_active(settings_rc.borrow().colored_tabs);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    colored_tabs_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.colored_tabs != row.is_active() {
            s.colored_tabs = row.is_active();
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
    let cb = on_change.clone();
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

    let cursor_color_dialog = gtk::ColorDialog::builder().with_alpha(false).build();
    let cursor_color_button = gtk::ColorDialogButton::new(Some(cursor_color_dialog));
    cursor_color_button.set_valign(gtk::Align::Center);
    if let Ok(rgba) = gdk::RGBA::parse(&settings_rc.borrow().cursor_color) {
        cursor_color_button.set_rgba(&rgba);
    }

    // Add color picker to the row (as a prefix to keep it to the left of the switch)
    cursor_color_row.add_prefix(&cursor_color_button);

    cursor_color_switch.set_active(settings_rc.borrow().cursor_color_override);
    cursor_color_button.set_sensitive(settings_rc.borrow().cursor_color_override);
    let ccb_clone = cursor_color_button.clone();
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    cursor_color_switch.connect_active_notify(move |switch| {
        ccb_clone.set_sensitive(switch.is_active());
        let mut s = s_rc.borrow_mut();
        if s.cursor_color_override != switch.is_active() {
            s.cursor_color_override = switch.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
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
    let cb = on_change.clone();
    cursor_blinking_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.cursor_blinking != row.is_active() {
            s.cursor_blinking = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let font_row_clone = font_row.clone();
    let theme_row_clone = theme_row.clone();
    let padding_spin_clone = padding_spin.clone();
    let line_spacing_spin_clone = line_spacing_spin.clone();
    let col_spacing_spin_clone = col_spacing_spin.clone();
    let preserve_cwd_switch_clone = preserve_cwd_switch.clone();
    let cursor_shape_combo_clone = cursor_shape_combo.clone();
    let cursor_color_row_clone = cursor_color_row.clone();
    let cursor_blinking_switch_clone = cursor_blinking_switch.clone();
    let enable_progress_bar_switch_clone = enable_progress_bar_switch.clone();
    let hide_scrollbars_switch_clone = hide_scrollbars_switch.clone();
    let invert_scroll_switch_clone = invert_scroll_switch.clone();
    let dim_inactive_switch_clone = dim_inactive_switch.clone();
    let fixed_width_tabs_switch_clone = fixed_width_tabs_switch.clone();
    let colored_tabs_switch_clone = colored_tabs_switch.clone();
    let chat_width_spin_clone = chat_width_spin.clone();
    let color_scheme_combo_clone = color_scheme_combo.clone();
    let opacity_row_clone = opacity_row.clone();
    let background_image_row_clone = background_image_row.clone();

    let widgets = AppearanceWidgets {
        theme_row,
        chat_width_spin,
    };

    let filter = Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let s1 = match_row(
            color_scheme_combo_clone.upcast_ref(),
            "gtk theme appearance dark light follow system color scheme",
        );
        let f1 = match_row(font_row_clone.upcast_ref(), "font family size");
        let t_bg = match_row(
            background_image_row_clone.upcast_ref(),
            "background image wallpaper picture",
        );
        let t1 = match_row(theme_row_clone.upcast_ref(), "terminal theme");
        let t_op = match_row(
            opacity_row_clone.upcast_ref(),
            "opacity transparent background",
        );
        let c_tabs = match_row(
            colored_tabs_switch_clone.upcast_ref(),
            "new tab in random color colored tabs auto color",
        );
        let t2 = match_row(padding_spin_clone.upcast_ref(), "padding px");
        let t3 = match_row(line_spacing_spin_clone.upcast_ref(), "line spacing");
        let t4 = match_row(col_spacing_spin_clone.upcast_ref(), "column spacing");
        let t5 = match_row(
            preserve_cwd_switch_clone.upcast_ref(),
            "preserve working directory new tabs open in active tab directory",
        );
        let c1 = match_row(cursor_shape_combo_clone.upcast_ref(), "cursor shape");
        let c3 = match_row(cursor_color_row_clone.upcast_ref(), "cursor custom color");
        let c4 = match_row(
            cursor_blinking_switch_clone.upcast_ref(),
            "blinking cursor fades in out",
        );
        let pb = match_row(
            enable_progress_bar_switch_clone.upcast_ref(),
            "progress bars show visual osc 9 4 flatpak cargo",
        );
        let l1 = match_row(
            hide_scrollbars_switch_clone.upcast_ref(),
            "hide scrollbars do not show vertical",
        );
        let l2 = match_row(
            invert_scroll_switch_clone.upcast_ref(),
            "invert scroll direction reverse mouse wheel",
        );
        let l3 = match_row(
            dim_inactive_switch_clone.upcast_ref(),
            "dim inactive panes terminal splits slightly",
        );
        let l5 = match_row(
            fixed_width_tabs_switch_clone.upcast_ref(),
            "fixed width tabs do not expand",
        );
        let l6 = match_row(
            chat_width_spin_clone.upcast_ref(),
            "sidebar width px hacky mouse resize overlay split view",
        );

        group_appearance.set_visible(s1 || f1 || t_bg || t1 || t_op || c_tabs);
        group_terminal.set_visible(t2 || t3 || t4 || t5);
        group_cursor.set_visible(c1 || c3 || c4);
        group_layout.set_visible(pb || l1 || l2 || l3 || l5 || l6);

        group_appearance.is_visible()
            || group_terminal.is_visible()
            || group_cursor.is_visible()
            || group_layout.is_visible()
    });

    (widgets, filter)
}
