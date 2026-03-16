use libadwaita as adw;
use gtk4 as gtk;
use adw::prelude::*;

pub fn populate_about_page(page: &adw::PreferencesPage) -> Box<dyn Fn(&str) -> bool> {
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
        .icon_name("play.mii.Boxxy")
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

    let ext_icon0 = gtk::Image::from_icon_name("external-link-symbolic");
    let version_row = adw::ActionRow::builder()
        .title("Version")
        .subtitle(env!("CARGO_PKG_VERSION"))
        .activatable(true)
        .build();
    version_row.add_suffix(&ext_icon0);
    rows.push(version_row.clone());

    version_row.connect_activated(move |_| {
        let uri = "https://github.com/miifrommera/boxxy/releases";
        let _ = gtk::gio::AppInfo::launch_default_for_uri(uri, None::<&gtk::gio::AppLaunchContext>);
    });

    let dev_row = adw::ActionRow::builder()
        .title("Developer")
        .subtitle("Mii")
        .build();
    rows.push(dev_row.clone());

    let license_row = adw::ActionRow::builder()
        .title("License")
        .subtitle(env!("CARGO_PKG_LICENSE"))
        .build();
    rows.push(license_row.clone());

    let ext_icon1 = gtk::Image::from_icon_name("external-link-symbolic");
    let site_row = adw::ActionRow::builder()
        .title("Website")
        .subtitle("https://github.com/miifrommera/boxxy/")
        .activatable(true)
        .build();
    site_row.add_suffix(&ext_icon1);
    rows.push(site_row.clone());

    let site_row_clone = site_row.clone();
    site_row.connect_activated(move |_| {
        let uri = site_row_clone.subtitle().unwrap().to_string();
        let _ = gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);
    });

    let ext_icon2 = gtk::Image::from_icon_name("external-link-symbolic");
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
        let _ = gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);
    });

    group.add(&version_row);
    group.add(&dev_row);
    group.add(&license_row);
    group.add(&site_row);
    group.add(&issues_row);

    page.add(&group);
    elements.push((group, rows));

    Box::new(move |query: &str| {
        let mut page_visible = false;
        for (group, rows) in &elements {
            let mut group_visible = false;
            for row in rows {
                let title = row.title().to_lowercase();
                let subtitle = row.subtitle().unwrap_or_default().to_lowercase();
                let m = query.is_empty() || title.contains(query) || subtitle.contains(query);
                row.set_visible(m);
                if m { group_visible = true; }
            }
            group.set_visible(group_visible);
            if group_visible { page_visible = true; }
        }
        page_visible
    })
}
