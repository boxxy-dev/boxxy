use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;

pub fn populate_about_page(
    page: &adw::PreferencesPage,
    toast_overlay: &adw::ToastOverlay,
) -> Box<dyn Fn(&str) -> bool> {
    let mut elements = Vec::new();

    // App Logo and Name
    let header_group = adw::PreferencesGroup::builder().build();
    let logo_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(24)
        .build();

    let logo = gtk::Image::builder()
        .icon_name("dev.boxxy.BoxxyTerminal")
        .pixel_size(128)
        .halign(gtk::Align::Center)
        .build();

    let app_name = gtk::Label::builder()
        .label("Boxxy Terminal")
        .css_classes(vec!["title-1".to_string()])
        .halign(gtk::Align::Center)
        .build();
    logo_box.append(&logo);
    logo_box.append(&app_name);
    header_group.add(&logo_box);
    page.add(&header_group);

    // Metadata
    let group = adw::PreferencesGroup::builder().build();
    let mut rows = Vec::new();

    // Version row with Copy button (Using baked-in env vars)
    let version_str = format!("{} {}", env!("CARGO_PKG_VERSION"), env!("GIT_HASH"));

    let copy_icon = gtk::Image::from_icon_name("boxxy-edit-copy-symbolic");
    let version_row = adw::ActionRow::builder()
        .title("Version")
        .subtitle(&version_str)
        .activatable(true)
        .build();
    version_row.add_suffix(&copy_icon);
    rows.push(version_row.clone());

    let toast_overlay_clone = toast_overlay.clone();
    let version_str_clone = version_str.clone();
    version_row.connect_activated(move |row| {
        let display = row.display();
        let clipboard = display.clipboard();
        clipboard.set_text(&version_str_clone);

        let toast = adw::Toast::new("Version copied to clipboard");
        toast_overlay_clone.add_toast(toast);
    });

    // Developer
    let dev_row = adw::ActionRow::builder()
        .title("Developer")
        .subtitle("Mii")
        .build();
    rows.push(dev_row.clone());

    // License
    let license_row = adw::ActionRow::builder()
        .title("License")
        .subtitle(env!("CARGO_PKG_LICENSE"))
        .build();
    rows.push(license_row.clone());

    // Website
    let ext_icon1 = gtk::Image::from_icon_name("boxxy-external-link-symbolic");
    let site_row = adw::ActionRow::builder()
        .title("Website")
        .subtitle("https://boxxy.dev")
        .activatable(true)
        .build();
    site_row.add_suffix(&ext_icon1);
    rows.push(site_row.clone());

    let site_row_clone = site_row.clone();
    site_row.connect_activated(move |_| {
        let uri = site_row_clone.subtitle().unwrap().to_string();
        let _ =
            gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);
    });

    // Releases Row
    let ext_icon_releases = gtk::Image::from_icon_name("boxxy-external-link-symbolic");
    let releases_row = adw::ActionRow::builder()
        .title("Changelogs")
        .subtitle("https://boxxy.dev/releases/")
        .activatable(true)
        .build();
    releases_row.add_suffix(&ext_icon_releases);
    rows.push(releases_row.clone());

    let releases_row_clone = releases_row.clone();
    releases_row.connect_activated(move |_| {
        let uri = releases_row_clone.subtitle().unwrap().to_string();
        let _ =
            gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);
    });

    // Issues
    let ext_icon2 = gtk::Image::from_icon_name("boxxy-external-link-symbolic");
    let issues_row = adw::ActionRow::builder()
        .title("Report an Issue")
        .subtitle("https://github.com/miifrommera/boxxy/issues")
        .activatable(true)
        .build();
    issues_row.add_suffix(&ext_icon2);
    rows.push(issues_row.clone());

    let issues_row_clone = issues_row.clone();
    issues_row.connect_activated(move |_| {
        let uri = issues_row_clone.subtitle().unwrap().to_string();
        let _ =
            gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);
    });

    group.add(&version_row);
    group.add(&dev_row);
    group.add(&license_row);
    group.add(&site_row);
    group.add(&releases_row);
    group.add(&issues_row);

    page.add(&group);
    elements.push((group, rows));

    // Telemetry Group
    let telemetry_group = adw::PreferencesGroup::builder()
        .title("Telemetry")
        .description("During the Preview Phase, telemetry helps us identify performance issues and improve AI accuracy. Help us improve Boxxy by sharing anonymous usage statistics.")
        .build();
    let mut telemetry_rows = Vec::new();

    let telemetry_switch = gtk::Switch::builder()
        .valign(gtk::Align::Center)
        .active(crate::config::Settings::load().enable_telemetry)
        .build();

    let telemetry_row = adw::ActionRow::builder()
        .title("Anonymous Usage Data")
        .subtitle("No PII, paths, or prompt text are ever collected. (Requires restart)")
        .build();
    telemetry_row.add_suffix(&telemetry_switch);
    telemetry_rows.push(telemetry_row.clone());

    telemetry_switch.connect_state_set(move |_, state| {
        let mut settings = crate::config::Settings::load();
        settings.enable_telemetry = state;
        settings.save();
        glib::Propagation::Proceed
    });

    let view_stats_row = adw::ActionRow::builder()
        .title("View Live Statistics")
        .subtitle("https://boxxy.dev/live")
        .activatable(true)
        .build();
    let ext_icon3 = gtk::Image::from_icon_name("boxxy-external-link-symbolic");
    view_stats_row.add_suffix(&ext_icon3);
    telemetry_rows.push(view_stats_row.clone());

    view_stats_row.connect_activated(move |_| {
        let _ = gtk::gio::AppInfo::launch_default_for_uri(
            "https://boxxy.dev/live",
            None::<&gtk::gio::AppLaunchContext>,
        );
    });

    telemetry_group.add(&telemetry_row);
    telemetry_group.add(&view_stats_row);
    page.add(&telemetry_group);
    elements.push((telemetry_group, telemetry_rows));

    Box::new(move |query: &str| {
        let mut page_visible = false;
        for (group, rows) in &elements {
            let mut group_visible = false;
            for row in rows {
                let title = row.title().to_lowercase();
                let subtitle = row.subtitle().unwrap_or_default().to_lowercase();
                let m = query.is_empty() || title.contains(query) || subtitle.contains(query);
                row.set_visible(m);
                if m {
                    group_visible = true;
                }
            }
            group.set_visible(group_visible);
            if group_visible {
                page_visible = true;
            }
        }
        page_visible
    })
}
