use gstreamer as gst;
use gstreamer::prelude::*;

pub fn play_timer_completion_sound() {
    play_sound("resource:///dev/boxxy/BoxxyTerminal/sounds/timer.wav");
}

pub fn play_task_completion_sound() {
    play_sound("resource:///dev/boxxy/BoxxyTerminal/sounds/task.wav");
}

fn play_sound(uri: &str) {
    // GStreamer is initialized in main.rs
    let pipeline = gst::ElementFactory::make("playbin")
        .build()
        .expect("Failed to create playbin element");

    pipeline.set_property("uri", uri);

    // Start playback
    let _ = pipeline.set_state(gst::State::Playing);

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
