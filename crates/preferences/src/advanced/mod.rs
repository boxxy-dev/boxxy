use crate::config::Settings;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

pub fn setup_advanced_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
    on_reload_engine: Rc<dyn Fn() + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let login_shell_switch: adw::SwitchRow = builder.object("login_shell_switch").unwrap();
    let show_vte_grid_switch: adw::SwitchRow = builder.object("show_vte_grid_switch").unwrap();
    let hide_agent_badge_switch: adw::SwitchRow =
        builder.object("hide_agent_badge_switch").unwrap();
    let custom_regex_entry: adw::EntryRow = builder.object("custom_regex_entry").unwrap();
    let reset_regex_btn: gtk::Button = builder.object("reset_regex_btn").unwrap();
    let open_config_btn: gtk::Button = builder.object("open_config_btn").unwrap();
    let reset_config_btn: gtk::Button = builder.object("reset_config_btn").unwrap();

    let row_reset_regex: adw::ActionRow = builder.object("row_reset_regex").unwrap();
    let row_open_config: adw::ActionRow = builder.object("row_open_config").unwrap();
    let row_reset_config: adw::ActionRow = builder.object("row_reset_config").unwrap();
    let group_shell: adw::PreferencesGroup = builder.object("group_shell").unwrap();
    let group_terminal_interaction: adw::PreferencesGroup =
        builder.object("group_terminal_interaction").unwrap();
    let group_config: adw::PreferencesGroup = builder.object("group_config").unwrap();

    login_shell_switch.set_active(settings_rc.borrow().login_shell);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
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
    let cb = on_change.clone();
    show_vte_grid_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.show_vte_grid != row.is_active() {
            s.show_vte_grid = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    hide_agent_badge_switch.set_active(settings_rc.borrow().hide_agent_badge);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    hide_agent_badge_switch.connect_active_notify(move |row| {
        let mut s = s_rc.borrow_mut();
        if s.hide_agent_badge != row.is_active() {
            s.hide_agent_badge = row.is_active();
            s.save();
            cb(s.clone());
        }
    });

    custom_regex_entry.set_text(&settings_rc.borrow().custom_regex);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    custom_regex_entry.connect_changed(move |entry| {
        let mut s = s_rc.borrow_mut();
        if s.custom_regex != entry.text().as_str() {
            s.custom_regex = entry.text().to_string();
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
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

    open_config_btn.connect_clicked(|_| {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir();
            if !config_dir.exists() {
                let _ = fs::create_dir_all(config_dir);
            }
            let uri = format!("file://{}", config_dir.display());
            let _ = gtk::gio::AppInfo::launch_default_for_uri(
                &uri,
                None::<&gtk::gio::AppLaunchContext>,
            );
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let dialog: adw::Dialog = builder.object("dialog").unwrap();
    let reload_cb = on_reload_engine.clone();
    reset_config_btn.connect_clicked(move |_| {
        let confirm = adw::AlertDialog::builder()
            .heading("Reset Everything?")
            .body("This will delete all settings and permanently remove all installed Boxxy apps. This action cannot be undone.")
            .build();
        confirm.add_response("cancel", "Cancel");
        confirm.add_response("reset", "Reset");
        confirm.set_response_appearance("reset", adw::ResponseAppearance::Destructive);

        let s_rc2 = s_rc.clone();
        let cb2 = cb.clone();
        let reload_cb2 = reload_cb.clone();
        confirm.connect_response(None, move |_, response| {
            if response == "reset" {
                if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
                    let _ = fs::remove_dir_all(dirs.config_dir());
                    crate::config::Settings::ensure_claw_skills();
                }
                let updated_settings = crate::config::Settings::default();
                {
                    let mut s = s_rc2.borrow_mut();
                    *s = updated_settings.clone();
                    s.save();
                }
                cb2(updated_settings);
                reload_cb2();
            }
        });
        confirm.present(Some(&dialog));
    });

    let login_shell_switch_clone = login_shell_switch.clone();
    let show_vte_grid_switch_clone = show_vte_grid_switch.clone();
    let hide_agent_badge_switch_clone = hide_agent_badge_switch.clone();
    let custom_regex_entry_clone = custom_regex_entry.clone();
    let row_reset_regex_clone = row_reset_regex.clone();
    let row_open_config_clone = row_open_config.clone();
    let row_reset_config_clone = row_reset_config.clone();

    Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let ad1 = match_row(
            login_shell_switch_clone.upcast_ref(),
            "login shell spawn terminal",
        );
        let ad2 = match_row(
            show_vte_grid_switch_clone.upcast_ref(),
            "show vte grid lines representing cells",
        );
        let ad2b = match_row(
            hide_agent_badge_switch_clone.upcast_ref(),
            "hide agent identity badge top right corner",
        );
        let ad3 = match_row(
            custom_regex_entry_clone.upcast_ref(),
            "file path regex ctrl+click freezes",
        );
        let ad4 = match_row(row_reset_regex_clone.upcast_ref(), "reset to default");
        let ad5 = match_row(
            row_open_config_clone.upcast_ref(),
            "open config folder manage settings",
        );
        let ad6 = match_row(
            row_reset_config_clone.upcast_ref(),
            "reset everything delete all apps destructive",
        );

        group_shell.set_visible(ad1 || ad2 || ad2b);
        group_terminal_interaction.set_visible(ad3 || ad4);
        group_config.set_visible(ad5 || ad6);

        group_shell.is_visible()
            || group_terminal_interaction.is_visible()
            || group_config.is_visible()
    })
}
