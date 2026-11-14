use crate::config::Settings;
use crate::ui_utils::{PageHooks, SearchRow, open_uri};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

pub(crate) fn setup_integrations_page(builder: &gtk::Builder) -> PageHooks {
    let nautilus_integration_switch: adw::SwitchRow =
        builder.object("nautilus_integration_switch").unwrap();
    let gnome_companion_row: adw::ActionRow = builder.object("gnome_companion_row").unwrap();
    let cosmic_companion_row: adw::ActionRow = builder.object("cosmic_companion_row").unwrap();

    let group_integrations_gnome: adw::PreferencesGroup =
        builder.object("group_integrations_gnome").unwrap();
    let group_integrations_cosmic: adw::PreferencesGroup =
        builder.object("group_integrations_cosmic").unwrap();

    let is_syncing = Rc::new(Cell::new(false));
    nautilus_integration_switch.set_active(Settings::load().nautilus_integration);

    {
        let is_syncing = is_syncing.clone();
        nautilus_integration_switch.connect_active_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let active = row.is_active();
            Settings::update(|s| {
                s.nautilus_integration = active;
            });
            boxxy_sys_utils::runtime().spawn_blocking(move || {
                Settings::sync_nautilus_extension(active);
            });
        });
    }

    gnome_companion_row.connect_activated(|row| {
        if let Some(uri) = row.subtitle() {
            open_uri(row, uri.as_str());
        }
    });

    cosmic_companion_row.connect_activated(|row| {
        if let Some(uri) = row.subtitle() {
            open_uri(row, uri.as_str());
        }
    });

    let s_nautilus = SearchRow::new(
        &nautilus_integration_switch,
        "gnome nautilus integration files context menu open with boxxy",
    );
    let s_gnome = SearchRow::new(&gnome_companion_row, "gnome shell extension companion");
    let s_cosmic = SearchRow::new(&cosmic_companion_row, "cosmic boxxy companion applet");

    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);
        if nautilus_integration_switch.is_active() != s.nautilus_integration {
            nautilus_integration_switch.set_active(s.nautilus_integration);
        }
        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let g = s_nautilus.apply(query) | s_gnome.apply(query);
        let c = s_cosmic.apply(query);
        group_integrations_gnome.set_visible(g);
        group_integrations_cosmic.set_visible(c);
        g || c
    });

    (sync, filter)
}
