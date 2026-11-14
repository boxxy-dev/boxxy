use crate::config::{CursorShape, Settings};
use crate::ui_utils::{Debouncer, PageHooks, SearchRow, make_editable_color_dot};
use adw::prelude::*;
use gtk::pango;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_appearance_page(
    builder: &gtk::Builder,
    on_open_themes: Rc<dyn Fn() + 'static>,
) -> PageHooks {
    let select_bg_image_btn: gtk::Button = builder.object("select_bg_image_btn").unwrap();
    let clear_bg_image_btn: gtk::Button = builder.object("clear_bg_image_btn").unwrap();
    let background_image_row: adw::ActionRow = builder.object("background_image_row").unwrap();
    let font_row: adw::ActionRow = builder.object("font_row").unwrap();
    let theme_row: adw::ActionRow = builder.object("theme_row").unwrap();
    let opacity_row: adw::ActionRow = builder.object("opacity_row").unwrap();
    let opacity_scale: gtk::Scale = builder.object("opacity_scale").unwrap();
    let frosted_glass_row: adw::SwitchRow = builder.object("frosted_glass_row").unwrap();
    let padding_spin: adw::SpinRow = builder.object("padding_spin").unwrap();
    let line_spacing_spin: adw::SpinRow = builder.object("line_spacing_spin").unwrap();
    let col_spacing_spin: adw::SpinRow = builder.object("col_spacing_spin").unwrap();
    let preserve_cwd_switch: adw::SwitchRow = builder.object("preserve_cwd_switch").unwrap();
    let cursor_shape_combo: adw::ComboRow = builder.object("cursor_shape_combo").unwrap();
    let cursor_color_switch: gtk::Switch = builder.object("cursor_color_switch").unwrap();
    let cursor_color_row: adw::ActionRow = builder.object("cursor_color_row").unwrap();
    let cursor_blinking_switch: adw::SwitchRow = builder.object("cursor_blinking_switch").unwrap();
    let mouse_autohide_switch: adw::SwitchRow = builder.object("mouse_autohide_switch").unwrap();
    let enable_progress_bar_switch: adw::SwitchRow =
        builder.object("enable_progress_bar_switch").unwrap();
    let hide_scrollbars_switch: adw::SwitchRow = builder.object("hide_scrollbars_switch").unwrap();
    let invert_scroll_switch: adw::SwitchRow = builder.object("invert_scroll_switch").unwrap();
    let dim_inactive_switch: adw::SwitchRow = builder.object("dim_inactive_switch").unwrap();
    let fixed_width_tabs_switch: adw::SwitchRow =
        builder.object("fixed_width_tabs_switch").unwrap();
    let smart_tab_naming_switch: adw::SwitchRow =
        builder.object("smart_tab_naming_switch").unwrap();
    let colored_tabs_switch: adw::SwitchRow = builder.object("colored_tabs_switch").unwrap();
    let chat_width_spin: adw::SpinRow = builder.object("chat_width_spin").unwrap();

    let group_appearance: adw::PreferencesGroup = builder.object("group_appearance").unwrap();
    let group_terminal: adw::PreferencesGroup = builder.object("group_terminal").unwrap();
    let group_cursor: adw::PreferencesGroup = builder.object("group_cursor").unwrap();
    let group_layout: adw::PreferencesGroup = builder.object("group_layout").unwrap();

    let is_syncing = Rc::new(Cell::new(false));
    let initial = Settings::load();

    // 0.1 Background Image & Opacity
    let opacity_adj = gtk::Adjustment::new(initial.opacity, 0.1, 1.0, 0.05, 0.1, 0.0);
    opacity_scale.set_adjustment(&opacity_adj);

    let update_bg_ui = {
        let row = background_image_row.clone();
        let clear_btn = clear_bg_image_btn.clone();
        Rc::new(move |bg_path: Option<&str>| {
            if let Some(path) = bg_path {
                let filename = std::path::Path::new(path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown");
                row.set_subtitle(&glib::markup_escape_text(filename));
                clear_btn.set_sensitive(true);
            } else {
                row.set_subtitle("No background image selected");
                clear_btn.set_sensitive(false);
            }
        })
    };
    update_bg_ui(initial.background_image_path.as_deref());

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

    {
        let update_bg_ui = update_bg_ui.clone();
        let opacity_adj = opacity_adj.clone();
        let is_syncing = is_syncing.clone();
        select_bg_image_btn.connect_clicked(move |btn| {
            let root_win = btn.root().and_downcast::<gtk::Window>();
            let update_bg_ui = update_bg_ui.clone();
            let opacity_adj = opacity_adj.clone();
            let is_syncing = is_syncing.clone();
            file_dialog.open(
                root_win.as_ref(),
                gtk::gio::Cancellable::NONE,
                move |res| {
                    let Ok(file) = res else { return };
                    let Some(src_path) = file.path() else { return };
                    let old_bg = Settings::load().background_image_path.clone();
                    let update_bg_ui = update_bg_ui.clone();
                    let opacity_adj = opacity_adj.clone();
                    let is_syncing = is_syncing.clone();
                    glib::spawn_future_local(async move {
                        let copied = boxxy_sys_utils::runtime()
                            .spawn_blocking(move || {
                                let new_path = boxxy_themes::copy_background_image(&src_path)?;
                                if let Some(old) = old_bg {
                                    boxxy_themes::delete_background_image(&old);
                                }
                                Some(new_path)
                            })
                            .await
                            .ok()
                            .flatten();

                        if let Some(new_path) = copied {
                            let mut nudged_opacity = None;
                            Settings::update(|s| {
                                s.background_image_path = Some(new_path.clone());
                                if s.opacity >= 0.999 {
                                    s.opacity = 0.80;
                                    nudged_opacity = Some(0.80);
                                }
                            });
                            if let Some(new_op) = nudged_opacity {
                                is_syncing.set(true);
                                opacity_adj.set_value(new_op);
                                is_syncing.set(false);
                            }
                            update_bg_ui(Some(&new_path));
                        }
                    });
                },
            );
        });
    }

    {
        let update_bg_ui = update_bg_ui.clone();
        let opacity_adj = opacity_adj.clone();
        let is_syncing = is_syncing.clone();
        clear_bg_image_btn.connect_clicked(move |_| {
            let old_path = Settings::load().background_image_path.clone();
            let Some(old) = old_path else { return };

            let reset_opacity = !Settings::load().frosted_glass;
            Settings::update(|s| {
                s.background_image_path = None;
                if reset_opacity {
                    s.opacity = 1.0;
                }
            });
            if reset_opacity {
                is_syncing.set(true);
                opacity_adj.set_value(1.0);
                is_syncing.set(false);
            }
            update_bg_ui(None);

            boxxy_sys_utils::runtime().spawn_blocking(move || {
                boxxy_themes::delete_background_image(&old);
            });
        });
    }

    // 1. Font
    let font_dialog = gtk::FontDialog::new();
    let font_button = gtk::FontDialogButton::new(Some(font_dialog));
    font_button.set_valign(gtk::Align::Center);
    let mut initial_desc = pango::FontDescription::new();
    initial_desc.set_family(&initial.font_name);
    initial_desc.set_size(i32::from(initial.font_size) * pango::SCALE);
    font_button.set_font_desc(&initial_desc);
    font_row.add_suffix(&font_button);

    {
        let is_syncing = is_syncing.clone();
        font_button.connect_font_desc_notify(move |btn| {
            if is_syncing.get() {
                return;
            }
            if let Some(desc) = btn.font_desc()
                && let Some(family) = desc.family()
            {
                let size_pt = u16::try_from((desc.size() / pango::SCALE).clamp(4, 256)).unwrap_or(12);
                let family_str = family.to_string();
                Settings::update(|s| {
                    s.font_name.clone_from(&family_str);
                    s.font_size = size_pt;
                });
            }
        });
    }

    // 2. Adjustments
    let opacity_debouncer = Debouncer::new();
    opacity_debouncer.bind_flush_lifecycle(&opacity_scale);
    {
        let is_syncing = is_syncing.clone();
        let debouncer = opacity_debouncer;
        opacity_scale.connect_value_changed(move |scale| {
            if is_syncing.get() {
                return;
            }
            let val = scale.value();
            debouncer.schedule(Duration::from_millis(40), move || {
                Settings::update(|s| {
                    if (s.opacity - val).abs() > 1e-4 {
                        s.opacity = val;
                    }
                });
            });
        });
    }

    frosted_glass_row.set_active(initial.frosted_glass);
    {
        let is_syncing = is_syncing.clone();
        let opacity_adj = opacity_adj.clone();
        frosted_glass_row.connect_active_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let active = row.is_active();
            let mut nudged_opacity = None;
            Settings::update(|s| {
                if s.frosted_glass != active {
                    s.frosted_glass = active;
                    if active {
                        if s.opacity >= 0.999 {
                            s.opacity = 0.80;
                            nudged_opacity = Some(0.80);
                        }
                    } else if s.background_image_path.is_none() {
                        s.opacity = 1.0;
                        nudged_opacity = Some(1.0);
                    }
                }
            });
            if let Some(new_op) = nudged_opacity {
                is_syncing.set(true);
                opacity_adj.set_value(new_op);
                is_syncing.set(false);
            }
        });
    }

    let padding_adj = gtk::Adjustment::new(f64::from(initial.padding), 0.0, 60.0, 2.0, 8.0, 0.0);
    padding_spin.set_adjustment(Some(&padding_adj));
    padding_spin.set_value(f64::from(initial.padding));
    {
        let is_syncing = is_syncing.clone();
        padding_spin.connect_value_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            let v = row.value().round() as i32;
            Settings::update(|s| {
                s.padding = v;
            });
        });
    }

    let line_spacing_adj =
        gtk::Adjustment::new(initial.cell_height_scale, 1.0, 3.0, 0.1, 0.5, 0.0);
    line_spacing_spin.set_adjustment(Some(&line_spacing_adj));
    line_spacing_spin.set_value(initial.cell_height_scale);
    {
        let is_syncing = is_syncing.clone();
        line_spacing_spin.connect_value_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let val = row.value();
            Settings::update(|s| {
                if (s.cell_height_scale - val).abs() > 1e-6 {
                    s.cell_height_scale = val;
                }
            });
        });
    }

    let col_spacing_adj = gtk::Adjustment::new(initial.cell_width_scale, 1.0, 3.0, 0.1, 0.5, 0.0);
    col_spacing_spin.set_adjustment(Some(&col_spacing_adj));
    col_spacing_spin.set_value(initial.cell_width_scale);
    {
        let is_syncing = is_syncing.clone();
        col_spacing_spin.connect_value_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let val = row.value();
            Settings::update(|s| {
                if (s.cell_width_scale - val).abs() > 1e-6 {
                    s.cell_width_scale = val;
                }
            });
        });
    }

    let chat_width_adj =
        gtk::Adjustment::new(f64::from(initial.ai_chat_width), 200.0, 800.0, 10.0, 50.0, 0.0);
    chat_width_spin.set_adjustment(Some(&chat_width_adj));
    chat_width_spin.set_value(f64::from(initial.ai_chat_width));
    {
        let is_syncing = is_syncing.clone();
        chat_width_spin.connect_value_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            let v = row.value().round() as i32;
            Settings::update(|s| {
                s.ai_chat_width = v;
            });
        });
    }

    // 3. Theme ActionRow
    theme_row.set_subtitle(&glib::markup_escape_text(&initial.theme));
    theme_row.connect_activated(move |_| {
        dialog.close();
        on_open_themes();
    });

    // 4. Switches
    macro_rules! bind_bool_switch {
        ($widget:ident, $field:ident) => {{
            $widget.set_active(initial.$field);
            let is_syncing = is_syncing.clone();
            $widget.connect_active_notify(move |row| {
                if is_syncing.get() {
                    return;
                }
                let active = row.is_active();
                Settings::update(|s| {
                    s.$field = active;
                });
            });
        }};
    }

    bind_bool_switch!(preserve_cwd_switch, preserve_working_dir);
    bind_bool_switch!(enable_progress_bar_switch, enable_progress_bar);
    bind_bool_switch!(hide_scrollbars_switch, hide_scrollbars);
    bind_bool_switch!(invert_scroll_switch, invert_scroll);
    bind_bool_switch!(dim_inactive_switch, dim_inactive_panes);
    bind_bool_switch!(fixed_width_tabs_switch, fixed_width_tabs);
    bind_bool_switch!(smart_tab_naming_switch, smart_tab_naming);
    bind_bool_switch!(colored_tabs_switch, colored_tabs);
    bind_bool_switch!(cursor_blinking_switch, cursor_blinking);
    bind_bool_switch!(mouse_autohide_switch, mouse_autohide);

    // 5. Cursor settings
    let cursor_shapes_list = gtk::StringList::new(&["Block", "I-Beam", "Underline"]);
    cursor_shape_combo.set_model(Some(&cursor_shapes_list));
    let shape_to_idx = |shape: CursorShape| -> u32 {
        match shape {
            CursorShape::Block => 0,
            CursorShape::IBeam => 1,
            CursorShape::Underline => 2,
        }
    };
    cursor_shape_combo.set_selected(shape_to_idx(initial.cursor_shape));
    {
        let is_syncing = is_syncing.clone();
        cursor_shape_combo.connect_selected_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let shape = match row.selected() {
                0 => CursorShape::Block,
                1 => CursorShape::IBeam,
                2 => CursorShape::Underline,
                _ => return,
            };
            Settings::update(|s| {
                s.cursor_shape = shape;
            });
        });
    }

    let (cursor_color_button, set_cursor_dot_color) =
        make_editable_color_dot(&initial.cursor_color, "Cursor Color", move |hex| {
            Settings::update(|s| {
                s.cursor_color.clone_from(&hex);
            });
        });
    cursor_color_row.add_prefix(&cursor_color_button);

    cursor_color_switch.set_active(initial.cursor_color_override);
    cursor_color_button.set_sensitive(initial.cursor_color_override);
    {
        let is_syncing = is_syncing.clone();
        let ccb_clone = cursor_color_button.clone();
        cursor_color_switch.connect_active_notify(move |switch| {
            let active = switch.is_active();
            ccb_clone.set_sensitive(active);
            if is_syncing.get() {
                return;
            }
            Settings::update(|s| {
                s.cursor_color_override = active;
            });
        });
    }

    // Build pre-indexed search rows before moving widgets into sync closure
    let s_font = SearchRow::new(&font_row, "font family size");
    let s_bg = SearchRow::new(&background_image_row, "background image wallpaper picture");
    let s_theme = SearchRow::new(&theme_row, "terminal colors terminal theme palette");
    let s_opacity = SearchRow::new(&opacity_row, "opacity transparent background tint");
    let s_glass = SearchRow::new(
        &frosted_glass_row,
        "frosted glass blur mutter wayland translucency",
    );
    let s_colored_tabs =
        SearchRow::new(&colored_tabs_switch, "random color colored tabs auto color");
    let s_pad = SearchRow::new(&padding_spin, "padding px");
    let s_line = SearchRow::new(&line_spacing_spin, "line spacing cell height");
    let s_col = SearchRow::new(&col_spacing_spin, "column spacing cell width");
    let s_cwd = SearchRow::new(&preserve_cwd_switch, "preserve working directory cwd");
    let s_cshape = SearchRow::new(&cursor_shape_combo, "cursor shape block beam underline");
    let s_ccolor = SearchRow::new(&cursor_color_row, "cursor custom color");
    let s_cblink = SearchRow::new(&cursor_blinking_switch, "blinking cursor");
    let s_cmouse = SearchRow::new(&mouse_autohide_switch, "hide mouse pointer cursor autohide");
    let s_pb = SearchRow::new(&enable_progress_bar_switch, "progress bars osc 9 4");
    let s_scroll = SearchRow::new(&hide_scrollbars_switch, "hide scrollbars vertical");
    let s_inv = SearchRow::new(&invert_scroll_switch, "invert scroll direction wheel");
    let s_dim = SearchRow::new(&dim_inactive_switch, "dim inactive panes splits");
    let s_fixed_tabs = SearchRow::new(&fixed_width_tabs_switch, "fixed width tabs");
    let s_smart_tabs = SearchRow::new(&smart_tab_naming_switch, "smart tab naming basename");
    let s_cwidth = SearchRow::new(&chat_width_spin, "sidebar width px split view");

    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);

        update_bg_ui(s.background_image_path.as_deref());
        theme_row.set_subtitle(&glib::markup_escape_text(&s.theme));

        let mut desc = pango::FontDescription::new();
        desc.set_family(&s.font_name);
        desc.set_size(i32::from(s.font_size) * pango::SCALE);
        if font_button.font_desc().as_ref() != Some(&desc) {
            font_button.set_font_desc(&desc);
        }

        if (opacity_adj.value() - s.opacity).abs() > 1e-4 {
            opacity_adj.set_value(s.opacity);
        }
        if frosted_glass_row.is_active() != s.frosted_glass {
            frosted_glass_row.set_active(s.frosted_glass);
        }
        if (padding_spin.value() - f64::from(s.padding)).abs() > 1e-6 {
            padding_spin.set_value(f64::from(s.padding));
        }
        if (line_spacing_spin.value() - s.cell_height_scale).abs() > 1e-6 {
            line_spacing_spin.set_value(s.cell_height_scale);
        }
        if (col_spacing_spin.value() - s.cell_width_scale).abs() > 1e-6 {
            col_spacing_spin.set_value(s.cell_width_scale);
        }
        if (chat_width_spin.value() - f64::from(s.ai_chat_width)).abs() > 1e-6 {
            chat_width_spin.set_value(f64::from(s.ai_chat_width));
        }

        preserve_cwd_switch.set_active(s.preserve_working_dir);
        enable_progress_bar_switch.set_active(s.enable_progress_bar);
        hide_scrollbars_switch.set_active(s.hide_scrollbars);
        invert_scroll_switch.set_active(s.invert_scroll);
        dim_inactive_switch.set_active(s.dim_inactive_panes);
        fixed_width_tabs_switch.set_active(s.fixed_width_tabs);
        smart_tab_naming_switch.set_active(s.smart_tab_naming);
        colored_tabs_switch.set_active(s.colored_tabs);
        cursor_blinking_switch.set_active(s.cursor_blinking);
        mouse_autohide_switch.set_active(s.mouse_autohide);

        let target_shape = shape_to_idx(s.cursor_shape);
        if cursor_shape_combo.selected() != target_shape {
            cursor_shape_combo.set_selected(target_shape);
        }
        cursor_color_switch.set_active(s.cursor_color_override);
        cursor_color_button.set_sensitive(s.cursor_color_override);
        set_cursor_dot_color(&s.cursor_color);

        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let a = s_font.apply(query)
            | s_bg.apply(query)
            | s_theme.apply(query)
            | s_opacity.apply(query)
            | s_glass.apply(query)
            | s_colored_tabs.apply(query);
        let t = s_pad.apply(query)
            | s_line.apply(query)
            | s_col.apply(query)
            | s_cwd.apply(query);
        let c = s_cshape.apply(query)
            | s_ccolor.apply(query)
            | s_cblink.apply(query)
            | s_cmouse.apply(query);
        let l = s_pb.apply(query)
            | s_scroll.apply(query)
            | s_inv.apply(query)
            | s_dim.apply(query)
            | s_fixed_tabs.apply(query)
            | s_smart_tabs.apply(query)
            | s_cwidth.apply(query);

        group_appearance.set_visible(a);
        group_terminal.set_visible(t);
        group_cursor.set_visible(c);
        group_layout.set_visible(l);

        a || t || c || l
    });

    (sync, filter)
}
