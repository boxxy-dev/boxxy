use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::state::{AppInput, AppWindowInner, ToastRequest};
use crate::widgets::notification::{Notification, NotificationLevel};

pub fn push_global_notification(inner: &mut AppWindowInner, ready: Notification) {
    let is_update = ready.level == NotificationLevel::Update;
    if is_update {
        if inner.active_update_notification.as_ref().is_some_and(|n| {
            n.id == ready.id && n.message == ready.message && n.details == ready.details
        }) {
            return;
        }
        inner.active_update_notification = Some(ready.clone());
    }

    // Agent reminders and long-task desktop notifications are emitted directly
    // by `boxxy-agent` over `org.freedesktop.Notifications` so they work reliably
    // on non-GNOME desktops (COSMIC, KDE, Sway) as well as GNOME. Only send
    // `gio::Notification` here for app-level update alerts when unfocused.
    let should_notify = is_update && !inner.window.is_active();
    if should_notify && let Some(app) = inner.window.application() {
        let notif = gtk4::gio::Notification::new(&ready.title);
        notif.set_body(Some(&ready.message));

        if ready.icon_name == "boxxyclaw-symbolic" {
            if let Ok(bytes) = gtk4::gio::resources_lookup_data(
                "/dev/boxxy/BoxxyTerminal/icons/boxxyclaw.svg",
                gtk4::gio::ResourceLookupFlags::NONE,
            ) {
                notif.set_icon(&gtk4::gio::BytesIcon::new(&bytes));
            }
        } else if ready.icon_name == "boxxy-up-arrow-in-a-star-symbolic" {
            let icon = gtk4::gio::ThemedIcon::new("software-update-available-symbolic");
            notif.set_icon(&icon);
        } else if ready.icon_name.starts_with('/') {
            let file = gtk4::gio::File::for_path(&ready.icon_name);
            let icon = gtk4::gio::FileIcon::new(&file);
            notif.set_icon(&icon);
        } else if !ready.icon_name.is_empty() {
            let icon = gtk4::gio::ThemedIcon::new(&ready.icon_name);
            notif.set_icon(&icon);
        }

        app.send_notification(None, &notif);
    }

    let toast = adw::Toast::new(&ready.message);
    toast.set_use_markup(false);

    if is_update {
        toast.set_timeout(0); // Permanent until dismissed
        toast.set_button_label(Some("Details"));

        let tx = inner.tx.clone();
        let notification = ready.clone();
        let toast_overlay = inner.toast_overlay.clone();
        toast.connect_button_clicked(move |_| {
            present_update_details_dialog(&toast_overlay, &notification, &tx);
        });
    } else if let Some(rich_widget) = build_character_reminder_toast_widget(&ready) {
        toast.set_custom_title(Some(&rich_widget));
        toast.set_timeout(6);
    } else {
        toast.set_timeout(5);
    }

    // Only show the in-app toast if it's not suppressed.
    // This avoids UI spam for events that the user can already see in the active Claw drawer.
    if !ready.suppress_toast {
        inner.toast_overlay.add_toast(toast);
    }
}

fn present_update_details_dialog(
    toast_overlay: &adw::ToastOverlay,
    notification: &Notification,
    tx: &async_channel::Sender<AppInput>,
) {
    let current_commit = notification
        .details
        .iter()
        .find(|(k, _)| k == "Current Commit")
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    let new_commit = notification
        .details
        .iter()
        .find(|(k, _)| k == "New Commit")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "nightly".to_string());

    let body_markup = if !current_commit.is_empty()
        && current_commit != "unknown"
        && !new_commit.is_empty()
        && new_commit != "nightly"
    {
        let compare_url = format!(
            "https://github.com/{}/{}/compare/{}...{}",
            crate::updater::REPO_OWNER,
            crate::updater::REPO_NAME,
            gtk4::glib::markup_escape_text(current_commit),
            gtk4::glib::markup_escape_text(&new_commit),
        );
        format!(
            "<a href=\"{compare_url}\"><span underline=\"none\">See commit changes on GitHub</span></a>"
        )
    } else {
        gtk4::glib::markup_escape_text(&notification.message).to_string()
    };

    let dialog = adw::AlertDialog::builder()
        .heading(&notification.title)
        .body(&body_markup)
        .body_use_markup(true)
        .build();

    let details_grid = gtk4::Grid::new();
    details_grid.set_column_spacing(12);
    details_grid.set_row_spacing(6);

    let mut row = 0;
    for (key, value) in &notification.details {
        if key == "Url" || key == "ChecksumUrl" {
            continue;
        }
        let key_label = gtk4::Label::builder()
            .label(key)
            .halign(gtk4::Align::Start)
            .css_classes(["dim-label"])
            .build();
        let value_label = gtk4::Label::builder()
            .label(value)
            .halign(gtk4::Align::Start)
            .build();

        details_grid.attach(&key_label, 0, row, 1, 1);
        details_grid.attach(&value_label, 1, row, 1, 1);
        row += 1;
    }
    dialog.set_extra_child(Some(&details_grid));

    for action in &notification.actions {
        dialog.add_response(&action.action_name, &action.label);
        if action.is_primary {
            dialog.set_response_appearance(&action.action_name, adw::ResponseAppearance::Suggested);
        }
    }

    let url = notification
        .details
        .iter()
        .find(|(k, _)| k == "Url")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let date = notification
        .details
        .iter()
        .find(|(k, _)| k == "Date")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let checksum_url = notification
        .details
        .iter()
        .find(|(k, _)| k == "ChecksumUrl")
        .map(|(_, v)| v.clone());
    let id = notification.id.clone();
    let tx_dialog = tx.clone();

    if let Some(root) = toast_overlay.root().and_downcast::<gtk4::Window>() {
        dialog.choose(Some(&root), gtk4::gio::Cancellable::NONE, move |response| {
            if response == "win.start-download" {
                let _ = tx_dialog.send_blocking(AppInput::StartUpdateDownload(
                    url.clone(),
                    date.clone(),
                    new_commit.clone(),
                    checksum_url.clone(),
                ));
                let _ = tx_dialog.send_blocking(AppInput::DismissNotification(id.clone()));
            } else if response == "win.apply-update" {
                let _ = tx_dialog.send_blocking(AppInput::ApplyUpdateAndRestart);
            } else if response == "win.dismiss-notification" {
                let _ = tx_dialog.send_blocking(AppInput::DismissNotification(id.clone()));
            }
            let _ = tx_dialog.send_blocking(AppInput::FocusActiveTerminal);
        });
    }
}

pub fn dismiss_notification(inner: &mut AppWindowInner, id: &str) {
    if inner
        .active_update_notification
        .as_ref()
        .is_some_and(|n| n.id == id)
    {
        inner.active_update_notification = None;
    }
}

pub fn start_update_download(
    inner: &AppWindowInner,
    url: String,
    date: String,
    new_commit: String,
    checksum_url: Option<String>,
) {
    let tx_download = inner.tx.clone();
    tokio::spawn(async move {
        match crate::updater::Updater::download_update(url, date, new_commit, checksum_url).await {
            Ok(path) => {
                let _ = tx_download
                    .send(AppInput::UpdateDownloaded(
                        path.to_string_lossy().to_string(),
                    ))
                    .await;
            }
            Err(e) => {
                log::error!("Failed to download update: {e:#}");
                let _ = tx_download
                    .send(AppInput::ShowToast(ToastRequest::ephemeral(
                        "Update download failed — check CLI logs",
                    )))
                    .await;
            }
        }
    });
}

pub fn update_downloaded(inner: &mut AppWindowInner) {
    let ready = Notification::new_update_ready("nightly");
    let toast = adw::Toast::new(&ready.message);
    toast.set_timeout(0); // Permanent until dismissed
    toast.set_button_label(Some("Restart"));

    let tx = inner.tx.clone();
    toast.connect_button_clicked(move |_| {
        let _ = tx.send_blocking(AppInput::ApplyUpdateAndRestart);
    });

    inner.toast_overlay.add_toast(toast);
}

pub fn apply_update_and_restart(inner: &AppWindowInner) {
    if let Err(e) = crate::updater::Updater::apply_update_and_restart() {
        log::error!("Failed to apply update and restart: {e:#}");
        let toast = adw::Toast::new("Update install failed — check CLI logs");
        toast.set_timeout(3);
        inner.toast_overlay.add_toast(toast);
    }
}

fn build_character_reminder_toast_widget(notif: &Notification) -> Option<gtk4::Widget> {
    let char_name = notif
        .character_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let root = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    root.set_valign(gtk4::Align::Center);
    root.set_margin_top(2);
    root.set_margin_bottom(2);
    root.set_margin_start(2);

    let avatar = adw::Avatar::new(28, Some(char_name), true);
    avatar.set_valign(gtk4::Align::Center);
    avatar.set_hexpand(false);
    avatar.set_vexpand(false);

    // Load custom character avatar off the GTK main thread so presenting a reminder
    // toast never blocks UI rendering on synchronous disk I/O.
    if let Some(path) = notif.avatar_path.clone() {
        let avatar_weak = avatar.downgrade();
        gtk4::glib::spawn_future_local(async move {
            let bytes_opt = boxxy_sys_utils::runtime()
                .spawn(async move { tokio::fs::read(path).await.ok() })
                .await
                .ok()
                .flatten();
            if let Some(bytes) = bytes_opt
                && let Some(av) = avatar_weak.upgrade()
            {
                let gbytes = gtk4::glib::Bytes::from_owned(bytes);
                if let Ok(texture) = gtk4::gdk::Texture::from_bytes(&gbytes) {
                    av.set_custom_image(Some(&texture));
                }
            }
        });
    }
    root.append(&avatar);

    let text_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
    text_vbox.set_valign(gtk4::Align::Center);
    text_vbox.set_hexpand(true);
    text_vbox.set_vexpand(false);

    let header_hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    header_hbox.set_halign(gtk4::Align::Start);
    header_hbox.set_valign(gtk4::Align::Center);

    let name_lbl = gtk4::Label::builder()
        .css_classes(["caption", "heading"])
        .halign(gtk4::Align::Start)
        .valign(gtk4::Align::Center)
        .xalign(0.0)
        .build();
    let escaped_name = gtk4::glib::markup_escape_text(char_name);
    let is_valid_hex = notif.character_color.as_deref().is_some_and(|c| {
        c.strip_prefix('#').is_some_and(|h| {
            matches!(h.len(), 3 | 4 | 6 | 8) && h.chars().all(|ch| ch.is_ascii_hexdigit())
        })
    });
    if let Some(ref color) = notif.character_color
        && is_valid_hex
    {
        name_lbl.set_markup(&format!(
            "<span foreground=\"{color}\"><b>{escaped_name}</b></span>"
        ));
    } else {
        name_lbl.set_markup(&format!("<b>{escaped_name}</b>"));
    }
    header_hbox.append(&name_lbl);

    let verb_lbl = gtk4::Label::builder()
        .label("reminds")
        .css_classes(["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .valign(gtk4::Align::Center)
        .xalign(0.0)
        .build();
    header_hbox.append(&verb_lbl);
    text_vbox.append(&header_hbox);

    let msg_lbl = gtk4::Label::builder()
        .label(&notif.message)
        .css_classes(["claw-toast-message"])
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .max_width_chars(46)
        .lines(2)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .build();
    text_vbox.append(&msg_lbl);

    root.append(&text_vbox);
    Some(root.upcast())
}
