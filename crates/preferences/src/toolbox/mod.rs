use crate::config::{Settings, validate_vibe_trusted_dir};
use crate::ui_utils::{PageHooks, SearchRow, show_toast};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn rebuild_vibe_location_rows(
    group: &adw::PreferencesGroup,
    dynamic_rows: &Rc<RefCell<Vec<(String, adw::ActionRow)>>>,
    dirs: &[String],
) {
    for (_, old_row) in dynamic_rows.borrow_mut().drain(..) {
        group.remove(&old_row);
    }

    let mut built = Vec::with_capacity(dirs.len());
    for dir in dirs {
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .title(dir)
            .build();
        let folder_icon = gtk::Image::from_icon_name("boxxy-folder-open-symbolic");
        folder_icon.add_css_class("dim-label");
        row.add_prefix(&folder_icon);

        let remove_btn = gtk::Button::builder()
            .icon_name("boxxy-user-trash-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Remove Trusted Folder")
            .css_classes(["flat", "circular"])
            .build();

        let group_clone = group.clone();
        let rows_clone = dynamic_rows.clone();
        let dir_to_remove = dir.clone();
        remove_btn.connect_clicked(move |_| {
            let updated = Settings::update(|s| {
                s.vibe_trusted_dirs.retain(|d| d != &dir_to_remove);
            });
            rebuild_vibe_location_rows(&group_clone, &rows_clone, &updated.vibe_trusted_dirs);
        });

        row.add_suffix(&remove_btn);
        group.add(&row);
        built.push((dir.clone(), row));
    }
    *dynamic_rows.borrow_mut() = built;
}

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_toolbox_page(builder: &gtk::Builder) -> PageHooks {
    let web_search_by_default_switch: adw::SwitchRow =
        builder.object("web_search_by_default_switch").unwrap();
    let max_roundtable_turns_spin: adw::SpinRow =
        builder.object("max_roundtable_turns_spin").unwrap();
    let vibe_location_entry: adw::EntryRow = builder.object("vibe_location_entry").unwrap();
    let vibe_location_browse_btn: gtk::Button =
        builder.object("vibe_location_browse_btn").unwrap();
    let enable_file_tools_switch: adw::SwitchRow =
        builder.object("enable_file_tools_switch").unwrap();
    let enable_system_tools_switch: adw::SwitchRow =
        builder.object("enable_system_tools_switch").unwrap();
    let enable_dangerous_tools_switch: adw::SwitchRow =
        builder.object("enable_dangerous_tools_switch").unwrap();
    let enable_web_tools_switch: adw::SwitchRow =
        builder.object("enable_web_tools_switch").unwrap();
    let enable_web_search_switch: adw::SwitchRow =
        builder.object("enable_web_search_switch").unwrap();
    let enable_os_context_switch: adw::SwitchRow =
        builder.object("enable_os_context_switch").unwrap();
    let enable_clipboard_tools_switch: adw::SwitchRow =
        builder.object("enable_clipboard_tools_switch").unwrap();
    let enable_auto_dreaming_switch: adw::SwitchRow =
        builder.object("enable_auto_dreaming_switch").unwrap();

    let group_toolbox_permissions: adw::PreferencesGroup =
        builder.object("group_toolbox_permissions").unwrap();
    let group_vibe_locations: adw::PreferencesGroup =
        builder.object("group_vibe_locations").unwrap();
    let group_toolbox_dreaming: adw::PreferencesGroup =
        builder.object("group_toolbox_dreaming").unwrap();
    let group_toolbox_tools: adw::PreferencesGroup = builder.object("group_toolbox_tools").unwrap();

    let is_syncing = Rc::new(Cell::new(false));
    let initial = Settings::load();

    let vibe_dynamic_rows: Rc<RefCell<Vec<(String, adw::ActionRow)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let last_vibe_dirs: Rc<RefCell<Vec<String>>> =
        Rc::new(RefCell::new(initial.vibe_trusted_dirs.clone()));
    rebuild_vibe_location_rows(
        &group_vibe_locations,
        &vibe_dynamic_rows,
        &initial.vibe_trusted_dirs,
    );

    let try_add_vibe_location = {
        let group_clone = group_vibe_locations.clone();
        let rows_clone = vibe_dynamic_rows.clone();
        let last_dirs = last_vibe_dirs.clone();
        let entry_clone = vibe_location_entry.clone();
        Rc::new(move |raw: String| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return;
            }
            match validate_vibe_trusted_dir(trimmed) {
                Ok(valid_path) => {
                    let updated = Settings::update(|s| {
                        if !s.vibe_trusted_dirs.contains(&valid_path) {
                            s.vibe_trusted_dirs.push(valid_path.clone());
                        }
                    });
                    entry_clone.set_text("");
                    last_dirs
                        .borrow_mut()
                        .clone_from(&updated.vibe_trusted_dirs);
                    rebuild_vibe_location_rows(
                        &group_clone,
                        &rows_clone,
                        &updated.vibe_trusted_dirs,
                    );
                }
                Err(msg) => {
                    show_toast(&entry_clone, msg);
                }
            }
        })
    };

    {
        let add_apply = try_add_vibe_location.clone();
        vibe_location_entry.connect_apply(move |entry| {
            add_apply(entry.text().to_string());
        });
    }

    vibe_location_browse_btn.connect_clicked(move |btn| {
        let dialog = gtk::FileDialog::builder()
            .title("Select Trusted Folder for Vibe Mode")
            .modal(true)
            .build();
        let root_win = btn.root().and_downcast::<gtk::Window>();
        let add_cb = try_add_vibe_location.clone();
        dialog.select_folder(
            root_win.as_ref(),
            gtk::gio::Cancellable::NONE,
            move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    add_cb(path.to_string_lossy().into_owned());
                }
            },
        );
    });

    // Initialize values
    {
        let turns_val = f64::from(initial.max_roundtable_turns.clamp(1, 100));
        let turns_adj = gtk::Adjustment::new(turns_val, 1.0, 100.0, 1.0, 5.0, 0.0);
        max_roundtable_turns_spin.set_adjustment(Some(&turns_adj));
        max_roundtable_turns_spin.set_value(turns_val);
    }

    {
        let is_syncing = is_syncing.clone();
        max_roundtable_turns_spin.connect_value_notify(move |row: &adw::SpinRow| {
            if is_syncing.get() {
                return;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let val = (row.value().round() as u32).clamp(1, 100);
            Settings::update(|s| {
                s.max_roundtable_turns = val;
            });
        });
    }

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

    bind_bool_switch!(web_search_by_default_switch, web_search_on_by_default);
    bind_bool_switch!(enable_auto_dreaming_switch, enable_auto_dreaming);
    bind_bool_switch!(enable_file_tools_switch, enable_file_tools);
    bind_bool_switch!(enable_system_tools_switch, enable_system_tools);
    bind_bool_switch!(enable_dangerous_tools_switch, enable_dangerous_tools);
    bind_bool_switch!(enable_web_tools_switch, enable_web_tools);
    bind_bool_switch!(enable_web_search_switch, enable_web_search);
    bind_bool_switch!(enable_os_context_switch, enable_os_context);
    bind_bool_switch!(enable_clipboard_tools_switch, enable_clipboard_tools);

    let s_web_def = SearchRow::new(
        &web_search_by_default_switch,
        "web search on by default permissions toolbox",
    );
    let s_turns = SearchRow::new(
        &max_roundtable_turns_spin,
        "character chat turn limit max roundtable turns rounds debate drama autonomous",
    );
    let s_vibe_entry = SearchRow::new(
        &vibe_location_entry,
        "vibe mode locations trusted folders directories permissions auto run safe commands",
    );
    let s_dream = SearchRow::new(
        &enable_auto_dreaming_switch,
        "auto-dreaming memory consolidation durable facts",
    );
    let s_file = SearchRow::new(&enable_file_tools_switch, "file tools read write list delete");
    let s_sys = SearchRow::new(&enable_system_tools_switch, "system tools monitoring processes");
    let s_danger = SearchRow::new(&enable_dangerous_tools_switch, "dangerous terminate kill");
    let s_web = SearchRow::new(&enable_web_tools_switch, "web tools fetch http documentation");
    let s_search = SearchRow::new(&enable_web_search_switch, "web search tavily providers");
    let s_os = SearchRow::new(&enable_os_context_switch, "location time context environment");
    let s_clip = SearchRow::new(&enable_clipboard_tools_switch, "clipboard copy paste");

    let group_vibe_sync = group_vibe_locations.clone();
    let rows_vibe_sync = vibe_dynamic_rows.clone();
    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);

        web_search_by_default_switch.set_active(s.web_search_on_by_default);
        let turns_val = f64::from(s.max_roundtable_turns.clamp(1, 100));
        if (max_roundtable_turns_spin.value() - turns_val).abs() > 1e-6 {
            max_roundtable_turns_spin.set_value(turns_val);
        }
        enable_auto_dreaming_switch.set_active(s.enable_auto_dreaming);
        enable_file_tools_switch.set_active(s.enable_file_tools);
        enable_system_tools_switch.set_active(s.enable_system_tools);
        enable_dangerous_tools_switch.set_active(s.enable_dangerous_tools);
        enable_web_tools_switch.set_active(s.enable_web_tools);
        enable_web_search_switch.set_active(s.enable_web_search);
        enable_os_context_switch.set_active(s.enable_os_context);
        enable_clipboard_tools_switch.set_active(s.enable_clipboard_tools);

        if *last_vibe_dirs.borrow() != s.vibe_trusted_dirs {
            last_vibe_dirs.borrow_mut().clone_from(&s.vibe_trusted_dirs);
            rebuild_vibe_location_rows(&group_vibe_sync, &rows_vibe_sync, &s.vibe_trusted_dirs);
        }

        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let perm_vis = s_web_def.apply(query) | s_turns.apply(query);
        let mut vibe_vis = s_vibe_entry.apply(query);
        for (dir, row) in vibe_dynamic_rows.borrow().iter() {
            let dir_matched =
                query.is_empty() || vibe_vis || dir.to_lowercase().contains(query);
            row.set_visible(dir_matched);
            if dir_matched {
                vibe_vis = true;
            }
        }
        if vibe_vis {
            vibe_location_entry.set_visible(true);
        }

        let dream_vis = s_dream.apply(query);
        let tools_vis = s_file.apply(query)
            | s_sys.apply(query)
            | s_danger.apply(query)
            | s_web.apply(query)
            | s_search.apply(query)
            | s_os.apply(query)
            | s_clip.apply(query);

        group_toolbox_permissions.set_visible(perm_vis);
        group_vibe_locations.set_visible(vibe_vis);
        group_toolbox_dreaming.set_visible(dream_vis);
        group_toolbox_tools.set_visible(tools_vis);

        perm_vis || vibe_vis || dream_vis || tools_vis
    });

    (sync, filter)
}
