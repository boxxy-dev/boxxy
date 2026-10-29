use crate::config::{Settings, validate_vibe_trusted_dir};
use adw::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

fn show_toolbox_toast(widget: &impl IsA<gtk::Widget>, message: &str) {
    let mut current: Option<gtk::Widget> = Some(widget.clone().upcast());
    while let Some(w) = current {
        if let Ok(overlay) = w.clone().downcast::<adw::ToastOverlay>() {
            overlay.add_toast(adw::Toast::new(message));
            return;
        }
        current = w.parent();
    }
}

fn rebuild_vibe_location_rows(
    group: &adw::PreferencesGroup,
    dynamic_rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    settings_rc: &Rc<RefCell<Settings>>,
    on_change: &Rc<dyn Fn(Settings) + 'static>,
) {
    for old_row in dynamic_rows.borrow_mut().drain(..) {
        group.remove(&old_row);
    }

    let dirs = settings_rc.borrow().vibe_trusted_dirs.clone();
    for dir in dirs {
        let row = adw::ActionRow::builder()
            .title(&dir)
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
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let dir_to_remove = dir.clone();
        remove_btn.connect_clicked(move |_| {
            {
                let mut s = s_rc.borrow_mut();
                s.vibe_trusted_dirs.retain(|d| d != &dir_to_remove);
                s.save();
                cb(s.clone());
            }
            rebuild_vibe_location_rows(&group_clone, &rows_clone, &s_rc, &cb);
        });

        row.add_suffix(&remove_btn);
        group.add(&row);
        dynamic_rows.borrow_mut().push(row);
    }
}

pub fn setup_toolbox_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let web_search_by_default_switch: adw::SwitchRow =
        builder.object("web_search_by_default_switch").unwrap();
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

    let vibe_dynamic_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
    rebuild_vibe_location_rows(
        &group_vibe_locations,
        &vibe_dynamic_rows,
        &settings_rc,
        &on_change,
    );

    let try_add_vibe_location = {
        let group_clone = group_vibe_locations.clone();
        let rows_clone = vibe_dynamic_rows.clone();
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let entry_clone = vibe_location_entry.clone();
        Rc::new(move |raw: String| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return;
            }
            match validate_vibe_trusted_dir(trimmed) {
                Ok(valid_path) => {
                    {
                        let mut s = s_rc.borrow_mut();
                        if !s.vibe_trusted_dirs.contains(&valid_path) {
                            s.vibe_trusted_dirs.push(valid_path);
                            s.save();
                            cb(s.clone());
                        }
                    }
                    entry_clone.set_text("");
                    rebuild_vibe_location_rows(&group_clone, &rows_clone, &s_rc, &cb);
                }
                Err(msg) => {
                    show_toolbox_toast(&entry_clone, msg);
                }
            }
        })
    };

    let add_apply = try_add_vibe_location.clone();
    vibe_location_entry.connect_apply(move |entry| {
        add_apply(entry.text().to_string());
    });

    let add_browse = try_add_vibe_location;
    vibe_location_browse_btn.connect_clicked(move |btn| {
        let dialog = gtk::FileDialog::builder()
            .title("Select Trusted Folder for Vibe Mode")
            .modal(true)
            .build();
        let root_win = btn.root().and_downcast::<gtk::Window>();
        let add_cb = add_browse.clone();
        dialog.select_folder(
            root_win.as_ref(),
            None::<&gtk::gio::Cancellable>,
            move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    add_cb(path.to_string_lossy().into_owned());
                }
            },
        );
    });

    // Keep list synced if trusted folders are added from a terminal pane while Preferences is open.
    {
        let group_weak = group_vibe_locations.downgrade();
        let rows_sync = vibe_dynamic_rows.clone();
        let s_rc_sync = settings_rc.clone();
        let cb_sync = on_change.clone();
        let mut rx = crate::config::SETTINGS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            while let Ok(updated) = rx.recv().await {
                let Some(group) = group_weak.upgrade() else {
                    break;
                };
                let changed = s_rc_sync.borrow().vibe_trusted_dirs != updated.vibe_trusted_dirs;
                if changed {
                    s_rc_sync.borrow_mut().vibe_trusted_dirs = updated.vibe_trusted_dirs;
                    rebuild_vibe_location_rows(&group, &rows_sync, &s_rc_sync, &cb_sync);
                }
            }
        });
    }

    // Initialize values
    web_search_by_default_switch.set_active(settings_rc.borrow().web_search_on_by_default);
    enable_auto_dreaming_switch.set_active(settings_rc.borrow().enable_auto_dreaming);
    enable_file_tools_switch.set_active(settings_rc.borrow().enable_file_tools);
    enable_system_tools_switch.set_active(settings_rc.borrow().enable_system_tools);
    enable_dangerous_tools_switch.set_active(settings_rc.borrow().enable_dangerous_tools);
    enable_web_tools_switch.set_active(settings_rc.borrow().enable_web_tools);
    enable_web_search_switch.set_active(settings_rc.borrow().enable_web_search);
    enable_os_context_switch.set_active(settings_rc.borrow().enable_os_context);
    enable_clipboard_tools_switch.set_active(settings_rc.borrow().enable_clipboard_tools);

    // Connect signals
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    web_search_by_default_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.web_search_on_by_default != row.is_active() {
            s.web_search_on_by_default = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_auto_dreaming_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_auto_dreaming != row.is_active() {
            s.enable_auto_dreaming = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_file_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_file_tools != row.is_active() {
            s.enable_file_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_system_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_system_tools != row.is_active() {
            s.enable_system_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_dangerous_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_dangerous_tools != row.is_active() {
            s.enable_dangerous_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_web_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_web_tools != row.is_active() {
            s.enable_web_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_web_search_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_web_search != row.is_active() {
            s.enable_web_search = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_os_context_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_os_context != row.is_active() {
            s.enable_os_context = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    enable_clipboard_tools_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.enable_clipboard_tools != row.is_active() {
            s.enable_clipboard_tools = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    let web_search_by_default_switch_clone = web_search_by_default_switch.clone();
    let vibe_location_entry_clone = vibe_location_entry.clone();
    let enable_file_tools_switch_clone = enable_file_tools_switch.clone();
    let enable_system_tools_switch_clone = enable_system_tools_switch.clone();
    let enable_dangerous_tools_switch_clone = enable_dangerous_tools_switch.clone();
    let enable_web_tools_switch_clone = enable_web_tools_switch.clone();
    let enable_os_context_switch_clone = enable_os_context_switch.clone();
    let enable_clipboard_tools_switch_clone = enable_clipboard_tools_switch.clone();
    let enable_auto_dreaming_switch_clone = enable_auto_dreaming_switch.clone();

    Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let ag_web_default = match_row(
            web_search_by_default_switch_clone.upcast_ref(),
            "web search on by default allowed automatically permissions toolbox",
        );
        let ag_vibe_locs = match_row(
            vibe_location_entry_clone.upcast_ref(),
            "vibe mode locations trusted folders directories permissions auto run safe commands",
        );
        let ag_dream = match_row(
            enable_auto_dreaming_switch_clone.upcast_ref(),
            "enable auto-dreaming memory consolidation durable facts patterns",
        );
        let ag3 = match_row(
            enable_file_tools_switch_clone.upcast_ref(),
            "enable file tools read write list delete search toolbox",
        );
        let ag4 = match_row(
            enable_system_tools_switch_clone.upcast_ref(),
            "enable system tools monitoring list processes",
        );
        let ag5 = match_row(
            enable_dangerous_tools_switch_clone.upcast_ref(),
            "enable dangerous tools terminate kill processes",
        );
        let ag6 = match_row(
            enable_web_tools_switch_clone.upcast_ref(),
            "enable web tools fetch content documentation",
        );
        let ag_search = match_row(
            enable_web_search_switch.upcast_ref(),
            "enable web search tools tavily providers",
        );
        let ag_os = match_row(
            enable_os_context_switch_clone.upcast_ref(),
            "enable location and time context injection environment",
        );
        let ag7 = match_row(
            enable_clipboard_tools_switch_clone.upcast_ref(),
            "enable clipboard tools read write copy paste",
        );

        group_toolbox_permissions.set_visible(ag_web_default);
        group_vibe_locations.set_visible(ag_vibe_locs);
        group_toolbox_dreaming.set_visible(ag_dream);
        group_toolbox_tools.set_visible(ag3 || ag4 || ag5 || ag6 || ag_search || ag_os || ag7);

        group_toolbox_permissions.is_visible()
            || group_vibe_locations.is_visible()
            || group_toolbox_dreaming.is_visible()
            || group_toolbox_tools.is_visible()
    })
}
