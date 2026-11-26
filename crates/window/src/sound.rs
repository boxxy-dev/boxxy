use gstreamer as gst;
use gstreamer::prelude::*;
use std::cell::Cell;
use std::sync::OnceLock;

static GST_READY: OnceLock<bool> = OnceLock::new();

#[must_use]
pub fn is_gstreamer_ready() -> bool {
    GST_READY.get().copied().unwrap_or(false)
}

/// Ensures GStreamer is initialized at most once. Can be called from a background
/// pre-warm task after window presentation or lazily on first audio playback.
pub fn ensure_gstreamer_init() -> bool {
    *GST_READY.get_or_init(|| match gst::init() {
        Ok(()) => true,
        Err(e) => {
            log::warn!("Failed to initialize GStreamer: {e}");
            false
        }
    })
}

pub fn play_timer_completion_sound() {
    play_sound("resource:///dev/boxxy/BoxxyTerminal/sounds/timer.wav");
}

pub fn play_task_completion_sound() {
    play_sound("resource:///dev/boxxy/BoxxyTerminal/sounds/task.wav");
}

fn play_sound(uri: &str) {
    if !ensure_gstreamer_init() {
        return;
    }

    let Ok(pipeline) = gst::ElementFactory::make("playbin").build() else {
        log::warn!("GStreamer 'playbin' element unavailable; skipping sound '{uri}'");
        return;
    };

    pipeline.set_property("uri", uri);

    if pipeline.set_state(gst::State::Playing).is_err() {
        let _ = pipeline.set_state(gst::State::Null);
        return;
    }

    // Hold a strong reference to `pipeline` inside the polling closure so GStreamer
    // does not dispose `playbin` prematurely while it is in `Playing` state.
    let ticks = Cell::new(0u32);
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        ticks.set(ticks.get() + 1);
        let Some(bus) = pipeline.bus() else {
            let _ = pipeline.set_state(gst::State::Null);
            return gtk4::glib::ControlFlow::Break;
        };

        if let Some(msg) = bus.pop_filtered(&[gst::MessageType::Eos, gst::MessageType::Error]) {
            if let gst::MessageView::Error(err) = msg.view() {
                log::debug!("GStreamer sound playback error: {}", err.error());
            }
            let _ = pipeline.set_state(gst::State::Null);
            return gtk4::glib::ControlFlow::Break;
        }

        // Safety bound (15s) so a stalled audio sink never retains the pipeline indefinitely.
        if ticks.get() >= 150 {
            let _ = pipeline.set_state(gst::State::Null);
            return gtk4::glib::ControlFlow::Break;
        }

        gtk4::glib::ControlFlow::Continue
    });
}
