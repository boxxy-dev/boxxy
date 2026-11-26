use crate::config::Settings;
use crate::ui_utils::{PageHooks, SearchRow, open_uri};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

#[allow(clippy::too_many_lines)]
pub(crate) fn populate_about_page(
    page: &adw::PreferencesPage,
    toast_overlay: &adw::ToastOverlay,
) -> PageHooks {
    let is_syncing = Rc::new(Cell::new(false));

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
        .css_classes(["title-1"])
        .halign(gtk::Align::Center)
        .build();
    logo_box.append(&logo);
    logo_box.append(&app_name);
    header_group.add(&logo_box);
    page.add(&header_group);

    // Metadata
    let group = adw::PreferencesGroup::builder().build();
    let full_version = concat!(env!("CARGO_PKG_VERSION"), "+", env!("GIT_HASH"));
    let channel = boxxy_sys_utils::detect_package_type();
    let version_display = format_about_version(full_version, channel);

    let copy_icon = gtk::Image::from_icon_name("boxxy-edit-copy-symbolic");
    let version_row = adw::ActionRow::builder()
        .use_markup(false)
        .title("Version")
        .subtitle(&version_display)
        .activatable(true)
        .build();
    version_row.add_suffix(&copy_icon);

    {
        let toast_overlay = toast_overlay.clone();
        version_row.connect_activated(move |row| {
            row.display().clipboard().set_text(full_version);
            toast_overlay.add_toast(adw::Toast::new("Version copied to clipboard"));
        });
    }

    let dev_row = adw::ActionRow::builder()
        .title("Developer")
        .subtitle("Mii")
        .build();

    let license_row = adw::ActionRow::builder()
        .title("License")
        .subtitle(env!("CARGO_PKG_LICENSE"))
        .build();

    let build_link_row = |title: &str, url: &'static str| -> adw::ActionRow {
        let icon = gtk::Image::from_icon_name("boxxy-external-link-symbolic");
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(url)
            .activatable(true)
            .build();
        row.add_suffix(&icon);
        row.connect_activated(move |r| open_uri(r, url));
        row
    };

    let site_row = build_link_row("Website", "https://boxxy.dev");
    let releases_row = build_link_row("Changelogs", "https://boxxy.dev/releases/");
    let issues_row = build_link_row("Report an Issue", "https://github.com/boxxy-dev/boxxy/issues");

    group.add(&version_row);
    group.add(&dev_row);
    group.add(&license_row);
    group.add(&site_row);
    group.add(&releases_row);
    group.add(&issues_row);
    page.add(&group);

    // Boxxy News Group
    let news_group = adw::PreferencesGroup::builder()
        .title("Boxxy News")
        .description("Occasional release highlights, feature announcements, and community updates from the Boxxy project.")
        .build();

    let news_switch = adw::SwitchRow::builder()
        .title("News Notifications")
        .subtitle("Show an in-app notification when new announcements are published.")
        .active(Settings::load().enable_broadcasts)
        .build();

    {
        let is_syncing = is_syncing.clone();
        news_switch.connect_active_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let active = row.is_active();
            Settings::update(|s| {
                s.enable_broadcasts = active;
            });
        });
    }

    let read_news_row = build_link_row("Read News on Web", "https://boxxy.dev/news/");

    news_group.add(&news_switch);
    news_group.add(&read_news_row);
    page.add(&news_group);

    // Telemetry Group
    let telemetry_group = adw::PreferencesGroup::builder()
        .title("Telemetry")
        .description("During the Preview Phase, telemetry helps us identify performance issues and improve AI accuracy. Help us improve Boxxy by sharing anonymous usage statistics.")
        .build();

    let telemetry_switch = adw::SwitchRow::builder()
        .title("Anonymous Usage Data")
        .subtitle("No PII, paths, or prompt text are ever collected.")
        .active(Settings::load().enable_telemetry)
        .build();

    {
        let is_syncing = is_syncing.clone();
        telemetry_switch.connect_active_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let active = row.is_active();
            Settings::update(|s| {
                s.enable_telemetry = active;
            });
        });
    }

    let view_stats_row = build_link_row("View Live Statistics", "https://boxxy.dev/live");

    telemetry_group.add(&telemetry_switch);
    telemetry_group.add(&view_stats_row);
    page.add(&telemetry_group);

    let meta_rows = vec![
        SearchRow::new(&version_row, "commit git hash"),
        SearchRow::new(&dev_row, "author"),
        SearchRow::new(&license_row, ""),
        SearchRow::new(&site_row, ""),
        SearchRow::new(&releases_row, "changelog whats new"),
        SearchRow::new(&issues_row, "bug github"),
    ];
    let news_rows = vec![
        SearchRow::new(
            &news_switch,
            "news broadcasts announcements updates notifications",
        ),
        SearchRow::new(&read_news_row, "news broadcasts announcements updates"),
    ];
    let telem_rows = vec![
        SearchRow::new(&telemetry_switch, "telemetry privacy metrics"),
        SearchRow::new(&view_stats_row, "telemetry metrics"),
    ];

    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);
        if news_switch.is_active() != s.enable_broadcasts {
            news_switch.set_active(s.enable_broadcasts);
        }
        if telemetry_switch.is_active() != s.enable_telemetry {
            telemetry_switch.set_active(s.enable_telemetry);
        }
        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let mut meta_vis = false;
        for r in &meta_rows {
            meta_vis |= r.apply(query);
        }
        let mut news_vis = false;
        for r in &news_rows {
            news_vis |= r.apply(query);
        }
        let mut telem_vis = false;
        for r in &telem_rows {
            telem_vis |= r.apply(query);
        }
        header_group.set_visible(query.is_empty());
        group.set_visible(meta_vis);
        news_group.set_visible(news_vis);
        telemetry_group.set_visible(telem_vis);
        meta_vis || news_vis || telem_vis
    });

    (sync, filter)
}

fn format_about_version(full_version: &str, channel: &str) -> String {
    format!("{full_version} ({channel})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_version_includes_channel_while_keeping_raw_version_separate() {
        let raw = "1.0.0+abc1234";
        assert_eq!(format_about_version(raw, "native"), "1.0.0+abc1234 (native)");
        assert_eq!(
            format_about_version(raw, "flatpak"),
            "1.0.0+abc1234 (flatpak)"
        );
        assert_eq!(format_about_version(raw, "distro"), "1.0.0+abc1234 (distro)");
    }
}
