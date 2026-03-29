use gstreamer as gst;
use gstreamer::prelude::*;

pub fn play_task_completion_sound() {
    // We use a simple playbin pipeline for audio playback
    let uri = "resource:///dev/boxxy/BoxxyTerminal/sounds/task.wav";

    // GStreamer is initialized in main.rs
    let pipeline = gst::ElementFactory::make("playbin")
        .build()
        .expect("Failed to create playbin element");

    pipeline.set_property("uri", uri);

    // Start playback
    let _ = pipeline.set_state(gst::State::Playing);

    // We want to stop the pipeline once the sound finishes to free resources.
    // However, for a short UI sound, we can just let it be or use a timeout.
    // Better: listen for the EOS (End of Stream) message on the bus.

    // We use a separate thread or a GLib timeout to poll the bus if we really care,
    // but for a one-off "ping", the overhead of playbin staying in Playing state
    // until the app closes is minimal.
    // Let's do it properly with a GLib source to avoid leaking elements.

    let pipeline_weak = pipeline.downgrade();
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        if let Some(p) = pipeline_weak.upgrade() {
            let bus = p.bus().unwrap();
            if let Some(msg) = bus.pop() {
                match msg.view() {
                    gst::MessageView::Eos(_) | gst::MessageView::Error(_) => {
                        let _ = p.set_state(gst::State::Null);
                        return gtk4::glib::ControlFlow::Break;
                    }
                    _ => (),
                }
            }
            gtk4::glib::ControlFlow::Continue
        } else {
            gtk4::glib::ControlFlow::Break
        }
    });
}
