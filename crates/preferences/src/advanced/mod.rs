use crate::config::Settings;
use crate::ui_utils::{Debouncer, PageHooks, SearchRow, open_directory, show_toast};
use adw::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::Cell;
use std::fs;
use std::rc::Rc;
use std::time::Duration;

const DEFAULT_CUSTOM_REGEX: &str =
    r#"(?:https?://[^\s"'<>]+|/[\w.@:/-]+|~[\w.@:/-]+|\.{1,2}/[\w.@:/-]+)"#;

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_advanced_page(
    builder: &gtk::Builder,
    on_reload_engine: Rc<dyn Fn() + 'static>,
) -> PageHooks {
    let login_shell_switch: adw::SwitchRow = builder.object("login_shell_switch").unwrap();
    let use_custom_command_switch: gtk::Switch =
        builder.object("use_custom_command_switch").unwrap();
    let custom_command_entry: adw::EntryRow = builder.object("custom_command_entry").unwrap();
    let show_vte_grid_switch: adw::SwitchRow = builder.object("show_vte_grid_switch").unwrap();
    let pty_persistence_switch: adw::SwitchRow = builder.object("pty_persistence_switch").unwrap();
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

    let is_syncing = Rc::new(Cell::new(false));
    let initial = Settings::load();

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

    bind_bool_switch!(login_shell_switch, login_shell);
    bind_bool_switch!(use_custom_command_switch, use_custom_command);
    bind_bool_switch!(show_vte_grid_switch, show_vte_grid);
    bind_bool_switch!(pty_persistence_switch, pty_persistence);

    custom_command_entry.set_text(&initial.custom_command);
    let cmd_debouncer = Debouncer::new();
    cmd_debouncer.bind_flush_lifecycle(&custom_command_entry);
    {
        let is_syncing = is_syncing.clone();
        let debouncer = cmd_debouncer.clone();
        custom_command_entry.connect_changed(move |entry| {
            if is_syncing.get() {
                return;
            }
            let text = entry.text().to_string();
            debouncer.schedule(Duration::from_millis(300), move || {
                Settings::update(|s| {
                    s.custom_command.clone_from(&text);
                });
            });
        });
    }
    {
        let debouncer = cmd_debouncer;
        custom_command_entry.connect_apply(move |entry| {
            debouncer.cancel();
            let text = entry.text().to_string();
            Settings::update(|s| {
                s.custom_command.clone_from(&text);
            });
        });
    }

    custom_regex_entry.set_text(&initial.custom_regex);
    let regex_debouncer = Debouncer::new();
    regex_debouncer.bind_flush_lifecycle(&custom_regex_entry);
    {
        let is_syncing = is_syncing.clone();
        let debouncer = regex_debouncer.clone();
        custom_regex_entry.connect_changed(move |entry| {
            if is_syncing.get() {
                return;
            }
            let pattern = entry.text().to_string();
            if pattern.trim().is_empty() || regex::Regex::new(&pattern).is_err() {
                debouncer.cancel();
                entry.add_css_class("error");
                entry.set_tooltip_text(Some("Invalid regular expression syntax"));
                return;
            }
            entry.remove_css_class("error");
            entry.set_tooltip_text(None);
            debouncer.schedule(Duration::from_millis(300), move || {
                Settings::update(|s| {
                    s.custom_regex.clone_from(&pattern);
                });
            });
        });
    }

    {
        let regex_entry = custom_regex_entry.clone();
        let is_syncing = is_syncing.clone();
        let debouncer = regex_debouncer;
        reset_regex_btn.connect_clicked(move |_| {
            debouncer.cancel();
            is_syncing.set(true);
            regex_entry.set_text(DEFAULT_CUSTOM_REGEX);
            regex_entry.remove_css_class("error");
            regex_entry.set_tooltip_text(None);
            is_syncing.set(false);

            Settings::update(|s| {
                DEFAULT_CUSTOM_REGEX.clone_into(&mut s.custom_regex);
            });
        });
    }

    open_config_btn.connect_clicked(|btn| {
        open_directory(btn, boxxy_sys_utils::get_config_dir(), None);
    });

    let dialog: adw::Dialog = builder.object("dialog").unwrap();
    reset_config_btn.connect_clicked(move |btn| {
        let confirm = adw::AlertDialog::builder()
            .heading("Reset Everything?")
            .body("This will delete all settings and configuration. This action cannot be undone.")
            .build();
        confirm.add_response("cancel", "Cancel");
        confirm.add_response("reset", "Reset");
        confirm.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
        confirm.set_default_response(Some("cancel"));
        confirm.set_close_response("cancel");

        let reload_cb = on_reload_engine.clone();
        let btn_weak = btn.downgrade();
        confirm.connect_response(None, move |_, response| {
            if response != "reset" {
                return;
            }
            let reload_cb = reload_cb.clone();
            let btn_weak = btn_weak.clone();
            glib::spawn_future_local(async move {
                let reseeded_chars = boxxy_sys_utils::runtime()
                    .spawn_blocking(|| {
                        let config_dir = boxxy_sys_utils::get_config_dir();
                        let _ = fs::remove_dir_all(&config_dir);
                        let fresh = Settings {
                            install_id: Some(uuid::Uuid::new_v4().to_string()),
                            ..Settings::default()
                        };
                        fresh.save();
                        Settings::ensure_claw_skills();
                        let _ = boxxy_themes::app_theme::ensure_app_themes_dir();
                        let _ = boxxy_claw_protocol::character_loader::ensure_default_character();
                        boxxy_claw_protocol::character_loader::load_characters().ok()
                    })
                    .await
                    .ok()
                    .flatten();

                if let Some(chars) = reseeded_chars {
                    boxxy_claw_protocol::characters::CHARACTER_CACHE
                        .store(std::sync::Arc::new(chars));
                }
                reload_cb();
                if let Some(b) = btn_weak.upgrade() {
                    show_toast(&b, "All settings and configuration reset to defaults.");
                }
            });
        });
        confirm.present(Some(&dialog));
    });

    let s_login = SearchRow::new(&login_shell_switch, "login shell spawn terminal");
    let s_cmd = SearchRow::new(&custom_command_entry, "custom command shell");
    let s_grid = SearchRow::new(&show_vte_grid_switch, "vte grid lines cells");
    let s_pty = SearchRow::new(
        &pty_persistence_switch,
        "persistent shells detach background buffer running processes",
    );
    let s_regex = SearchRow::new(&custom_regex_entry, "file path regex ctrl+click");
    let s_reset_rx = SearchRow::new(&row_reset_regex, "reset regex default");
    let s_open_cfg = SearchRow::new(&row_open_config, "open config folder settings");
    let s_reset_cfg = SearchRow::new(
        &row_reset_config,
        "reset everything delete configuration destructive",
    );

    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);

        login_shell_switch.set_active(s.login_shell);
        use_custom_command_switch.set_active(s.use_custom_command);
        show_vte_grid_switch.set_active(s.show_vte_grid);
        pty_persistence_switch.set_active(s.pty_persistence);

        if !custom_command_entry.has_focus()
            && custom_command_entry.text().as_str() != s.custom_command
        {
            custom_command_entry.set_text(&s.custom_command);
        }
        if !custom_regex_entry.has_focus() && custom_regex_entry.text().as_str() != s.custom_regex {
            custom_regex_entry.set_text(&s.custom_regex);
            custom_regex_entry.remove_css_class("error");
            custom_regex_entry.set_tooltip_text(None);
        }

        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let sh = s_login.apply(query)
            | s_cmd.apply(query)
            | s_grid.apply(query)
            | s_pty.apply(query);
        let inter = s_regex.apply(query) | s_reset_rx.apply(query);
        let cfg = s_open_cfg.apply(query) | s_reset_cfg.apply(query);

        group_shell.set_visible(sh);
        group_terminal_interaction.set_visible(inter);
        group_config.set_visible(cfg);

        sh || inter || cfg
    });

    (sync, filter)
}
