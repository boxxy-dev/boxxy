use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;

use crate::state::{AppInput, AppWindowInner};

const WELCOME_DIALOG_RESOURCE: &str = "/dev/boxxy/BoxxyTerminal/ui/welcome_dialog.ui";
const WELCOME_VIDEO_RESOURCE: &str = "/dev/boxxy/BoxxyTerminal/videos/welcome.mp4";

pub fn open_welcome(inner: &AppWindowInner) {
    if crate::sound::is_gstreamer_ready() {
        show_welcome_dialog(inner);
        return;
    }

    // Warm GStreamer plugin scanning off the GTK main thread if triggered before post-startup prewarm.
    let tx_welcome = inner.tx.clone();
    gtk4::glib::spawn_future_local(async move {
        let _ = boxxy_sys_utils::runtime()
            .spawn_blocking(crate::sound::ensure_gstreamer_init)
            .await;
        let _ = tx_welcome.send(AppInput::ShowWelcomeDialog).await;
    });
}

pub fn show_welcome_dialog(inner: &AppWindowInner) {
    if inner
        .window
        .visible_dialog()
        .is_some_and(|d| d.widget_name() == "welcome_dialog")
    {
        return;
    }

    let _ = crate::sound::ensure_gstreamer_init();

    let builder = gtk4::Builder::from_resource(WELCOME_DIALOG_RESOURCE);
    let Some(dialog) = builder.object::<adw::Dialog>("welcome_dialog") else {
        return;
    };

    let media_file = gtk4::MediaFile::for_resource(WELCOME_VIDEO_RESOURCE);
    media_file.set_loop(false);

    let picture_opt = builder.object::<gtk4::Picture>("video_picture");
    if let Some(ref picture) = picture_opt {
        picture.set_paintable(Some(&media_file));
    }

    media_file.connect_error_notify(|stream| {
        if let Some(err) = stream.error() {
            log::warn!("Welcome video playback error: {err}");
        }
    });

    // Ensure initial playback starts reliably once the GStreamer stream finishes async preparation.
    let initial_started = Cell::new(false);
    media_file.connect_prepared_notify(move |stream| {
        if stream.is_prepared() && !initial_started.replace(true) && !stream.is_playing() {
            stream.play();
        }
    });
    media_file.play();

    if let Some(play_again_btn) = builder.object::<gtk4::Button>("play_again_btn") {
        let media_replay = media_file.clone();
        play_again_btn.connect_clicked(move |_| {
            if media_replay.is_ended() {
                media_replay.play();
            } else {
                media_replay.seek(0);
                if !media_replay.is_playing() {
                    media_replay.play();
                }
            }
        });
    }

    if let Some(lets_go_btn) = builder.object::<gtk4::Button>("lets_go_btn") {
        let dialog_weak = dialog.downgrade();
        lets_go_btn.connect_clicked(move |_| {
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
        });
    }

    // Explicit Capture-phase Escape handler so Escape closes immediately regardless of focus state.
    let key_ctrl = gtk4::EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    {
        let dialog_weak = dialog.downgrade();
        key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk4::gdk::Key::Escape {
                if let Some(d) = dialog_weak.upgrade() {
                    d.close();
                }
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        });
    }
    dialog.add_controller(key_ctrl);

    {
        let tx_close = inner.tx.clone();
        let picture_weak = picture_opt
            .as_ref()
            .map(gtk4::prelude::ObjectExt::downgrade);
        let media_cleanup = media_file;
        dialog.connect_closed(move |_| {
            media_cleanup.pause();
            media_cleanup.clear();
            if let Some(ref pw) = picture_weak
                && let Some(pic) = pw.upgrade()
            {
                pic.set_paintable(gtk4::gdk::Paintable::NONE);
            }
            let _ = tx_close.send_blocking(AppInput::FocusActiveTerminal);
        });
    }

    dialog.present(Some(&inner.window));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk4::test]
    fn welcome_dialog_ui_parses_with_4_by_3_aspect_and_floating_controls() {
        let _ = adw::init();

        let builder = gtk4::Builder::from_string(include_str!(
            "../../../../resources/ui/welcome_dialog.ui"
        ));
        let dialog: adw::Dialog = builder
            .object("welcome_dialog")
            .expect("welcome_dialog must exist in welcome_dialog.ui");

        let width = dialog.content_width();
        let height = dialog.content_height();
        assert!(width > 0 && height > 0);
        assert_eq!(
            width * 3,
            height * 4,
            "welcome_dialog must maintain a 4:3 aspect ratio ({width}x{height})"
        );
        assert!(dialog.can_close());
        assert_eq!(
            dialog.presentation_mode(),
            adw::DialogPresentationMode::Floating
        );

        let picture: gtk4::Picture = builder
            .object("video_picture")
            .expect("video_picture must exist");
        assert_eq!(picture.content_fit(), gtk4::ContentFit::Cover);

        let play_again: gtk4::Button = builder
            .object("play_again_btn")
            .expect("play_again_btn must exist");
        assert!(!play_again.gets_focus_on_click());

        let lets_go: gtk4::Button = builder
            .object("lets_go_btn")
            .expect("lets_go_btn must exist");
        assert_eq!(lets_go.label().as_deref(), Some("Let's Go"));
    }
}
