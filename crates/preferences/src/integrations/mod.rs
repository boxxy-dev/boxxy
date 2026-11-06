use crate::config::Settings;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

pub fn setup_integrations_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let nautilus_integration_switch: adw::SwitchRow =
        builder.object("nautilus_integration_switch").unwrap();
    let gnome_companion_row: adw::ActionRow = builder.object("gnome_companion_row").unwrap();
    let cosmic_companion_row: adw::ActionRow = builder.object("cosmic_companion_row").unwrap();

    let group_integrations_gnome: adw::PreferencesGroup =
        builder.object("group_integrations_gnome").unwrap();
    let group_integrations_cosmic: adw::PreferencesGroup =
        builder.object("group_integrations_cosmic").unwrap();

    nautilus_integration_switch.set_active(settings_rc.borrow().nautilus_integration);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    nautilus_integration_switch.connect_active_notify(move |row| {
        let active = row.is_active();
        let mut s = s_rc.borrow_mut();
        if s.nautilus_integration != active {
            s.nautilus_integration = active;
            s.save();
            boxxy_sys_utils::runtime().spawn_blocking(move || {
                Settings::sync_nautilus_extension(active);
            });
            cb(s.clone());
        }
    });

    let gnome_companion_row_clone = gnome_companion_row.clone();
    gnome_companion_row.connect_activated(move |_| {
        if let Some(uri) = gnome_companion_row_clone.subtitle() {
            let _ = gtk::gio::AppInfo::launch_default_for_uri(
                uri.as_str(),
                None::<&gtk::gio::AppLaunchContext>,
            );
        }
    });

    let cosmic_companion_row_clone = cosmic_companion_row.clone();
    cosmic_companion_row.connect_activated(move |_| {
        if let Some(uri) = cosmic_companion_row_clone.subtitle() {
            let _ = gtk::gio::AppInfo::launch_default_for_uri(
                uri.as_str(),
                None::<&gtk::gio::AppLaunchContext>,
            );
        }
    });

    let nautilus_integration_switch_clone = nautilus_integration_switch.clone();
    let gnome_companion_row_filter = gnome_companion_row.clone();
    let cosmic_companion_row_filter = cosmic_companion_row.clone();

    Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let g1 = match_row(
            nautilus_integration_switch_clone.upcast_ref(),
            "gnome nautilus integration files context menu open with boxxy",
        );
        let g2 = match_row(
            gnome_companion_row_filter.upcast_ref(),
            "gnome shell extension companion https://github.com/boxxy-dev/gnome-shell-boxxy-companion",
        );
        let c1 = match_row(
            cosmic_companion_row_filter.upcast_ref(),
            "cosmic boxxy companion applet https://github.com/boxxy-dev/cosmic-boxxy-companion",
        );

        group_integrations_gnome.set_visible(g1 || g2);
        group_integrations_cosmic.set_visible(c1);

        group_integrations_gnome.is_visible() || group_integrations_cosmic.is_visible()
    })
}
