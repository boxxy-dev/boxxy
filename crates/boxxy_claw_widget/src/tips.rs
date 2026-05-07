use boxxy_preferences::config::SETTINGS_EVENT_BUS;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const TIPS: &[&str] = &[
    "There is a Boxxy Extension for GNOME Shell",
    "Try `@character: direct` to talk to another character",
    "Use `/resume: session` to pick up where you left off",
    "You can change `Ctrl+/` from preferences",
    "You can ask a character to go to sleep",
    "Hold `Shift` while clicking an image or video to preview",
    "Characters can see your terminal output",
    "You can DnD images to conversation if your model supports it",
    "You can disable these tips from preferences",
    "You can ask a character to remind you something in 2 mins",
    "You can disable sounds from preferences",
    "Are you using Arch, btw?",
    "You can press Esc in the Okay state",
    "Try editing the personality of your characters",
];

fn next_index(current: usize) -> usize {
    let n = TIPS.len();
    let now = gtk::glib::monotonic_time() as u64;
    // Mix time and current index for better entropy
    let mut x = now ^ (current as u64).wrapping_mul(0x9E3779B97F4A7C15);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;

    let candidate = (x as usize) % (n - 1);
    if candidate >= current {
        candidate + 1
    } else {
        candidate
    }
}

#[derive(Clone)]
pub struct TipsCycle {
    label: gtk::Label,
    revealer: gtk::Revealer,
    timer_id: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    show_timer_id: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    hide_timer_id: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    current_index: Rc<Cell<usize>>,
    enabled: Rc<Cell<bool>>,
    last_shown_at: Rc<Cell<u64>>,
    is_active: Rc<Cell<bool>>,
}

impl TipsCycle {
    pub fn new(label: gtk::Label, revealer: gtk::Revealer) -> Self {
        let now = gtk::glib::monotonic_time() as usize;
        let initial_index = (now ^ (now >> 12)) % TIPS.len();

        let cycle = Self {
            label,
            revealer,
            timer_id: Rc::new(RefCell::new(None)),
            show_timer_id: Rc::new(RefCell::new(None)),
            hide_timer_id: Rc::new(RefCell::new(None)),
            current_index: Rc::new(Cell::new(initial_index)),
            enabled: Rc::new(Cell::new(false)),
            last_shown_at: Rc::new(Cell::new(0)),
            is_active: Rc::new(Cell::new(false)),
        };

        let mut rx = SETTINGS_EVENT_BUS.subscribe();
        let cycle_clone = cycle.clone();
        gtk::glib::spawn_future_local(async move {
            while let Ok(settings) = rx.recv().await {
                cycle_clone.set_enabled(settings.enable_tips);
            }
        });

        cycle
    }

    pub fn start(&self) {
        if self.is_active.get() || !self.enabled.get() {
            return;
        }

        self.is_active.set(true);
        self.cancel_hide_timer();

        // Delay showing the tip by 1 second to avoid flickering on fast turns
        let revealer_clone = self.revealer.clone();
        let label_clone = self.label.clone();
        let current_index_clone = self.current_index.clone();
        let last_shown = self.last_shown_at.clone();
        let show_timer_rc = self.show_timer_id.clone();

        let show_id = gtk::glib::timeout_add_local_once(Duration::from_secs(1), move || {
            // Clear the ID so cancel_show_timer doesn't try to remove a finished source
            if let Ok(mut timer) = show_timer_rc.try_borrow_mut() {
                timer.take();
            }

            let next = next_index(current_index_clone.get());
            current_index_clone.set(next);
            label_clone.set_text(TIPS[next]);
            revealer_clone.set_reveal_child(true);
            last_shown.set(gtk::glib::monotonic_time() as u64);
        });

        *self.show_timer_id.borrow_mut() = Some(show_id);

        // Start the rotation timer (15s)
        let cycle_revealer = self.revealer.clone();
        let cycle_label = self.label.clone();
        let cycle_index = self.current_index.clone();

        let source_id = gtk::glib::timeout_add_local(Duration::from_secs(15), move || {
            if !cycle_revealer.reveals_child() {
                return gtk::glib::ControlFlow::Continue;
            }
            let next = next_index(cycle_index.get());
            cycle_index.set(next);

            // Fade out, change text, fade in
            cycle_revealer.set_reveal_child(false);
            let lbl = cycle_label.clone();
            let rev = cycle_revealer.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(300), move || {
                lbl.set_text(TIPS[next]);
                rev.set_reveal_child(true);
            });

            gtk::glib::ControlFlow::Continue
        });

        *self.timer_id.borrow_mut() = Some(source_id);
    }

    pub fn stop(&self) {
        if !self.is_active.get() {
            return;
        }

        self.is_active.set(false);
        self.cancel_show_timer();

        if let Some(source_id) = self.timer_id.borrow_mut().take() {
            let _ = source_id.remove();
        }

        // If it's already hidden, we're done
        if !self.revealer.reveals_child() {
            return;
        }

        // Enforce minimum 6 seconds display time
        let now = gtk::glib::monotonic_time() as u64;
        let elapsed_us = now.saturating_sub(self.last_shown_at.get());
        let min_duration_us = 6_000_000;

        if elapsed_us < min_duration_us {
            let remaining_ms = ((min_duration_us - elapsed_us) / 1000) as u64;
            let rev = self.revealer.clone();
            let hide_timer_rc = self.hide_timer_id.clone();
            let hide_id = gtk::glib::timeout_add_local_once(Duration::from_millis(remaining_ms), move || {
                if let Ok(mut timer) = hide_timer_rc.try_borrow_mut() {
                    timer.take();
                }
                rev.set_reveal_child(false);
            });
            *self.hide_timer_id.borrow_mut() = Some(hide_id);
        } else {
            self.revealer.set_reveal_child(false);
        }
    }

    fn cancel_show_timer(&self) {
        if let Some(source_id) = self.show_timer_id.borrow_mut().take() {
            let _ = source_id.remove();
        }
    }

    fn cancel_hide_timer(&self) {
        if let Some(source_id) = self.hide_timer_id.borrow_mut().take() {
            let _ = source_id.remove();
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.set(enabled);
        if !enabled {
            self.is_active.set(false);
            self.cancel_show_timer();
            self.cancel_hide_timer();
            if let Some(source_id) = self.timer_id.borrow_mut().take() {
                let _ = source_id.remove();
            }
            self.revealer.set_reveal_child(false);
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.get()
    }
}
