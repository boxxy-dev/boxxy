use boxxy_preferences::config::SETTINGS_EVENT_BUS;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

pub const TIPS: &[&str] = &[
    "There is a Boxxy Extension for GNOME Shell",
    "Type `@file` to attach files or `:character` to mention another character",
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

pub const PENDING_TIPS: &[&str] = &[
    "Launching in the background…",
    "Working in the background — you can keep typing",
    "Background task in progress",
];

fn next_index(current: usize, n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let now = gtk::glib::monotonic_time().cast_unsigned();
    // Mix time and current index for better entropy
    let mut x = now ^ (current as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;

    let candidate = usize::try_from(x).unwrap_or(0) % (n - 1);
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
    fade_timer_id: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    hide_timer_id: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    current_index: Rc<Cell<usize>>,
    enabled: Rc<Cell<bool>>,
    last_shown_at: Rc<Cell<u64>>,
    is_active: Rc<Cell<bool>>,
    active_tips_ptr: Rc<Cell<usize>>,
}

impl TipsCycle {
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(label: gtk::Label, revealer: gtk::Revealer) -> Self {
        let now = usize::try_from(gtk::glib::monotonic_time().cast_unsigned()).unwrap_or(0);
        let initial_index = (now ^ (now >> 12)) % TIPS.len();

        let cycle = Self {
            label: label.clone(),
            revealer: revealer.clone(),
            timer_id: Rc::new(RefCell::new(None)),
            show_timer_id: Rc::new(RefCell::new(None)),
            fade_timer_id: Rc::new(RefCell::new(None)),
            hide_timer_id: Rc::new(RefCell::new(None)),
            current_index: Rc::new(Cell::new(initial_index)),
            enabled: Rc::new(Cell::new(false)),
            last_shown_at: Rc::new(Cell::new(0)),
            is_active: Rc::new(Cell::new(false)),
            active_tips_ptr: Rc::new(Cell::new(0)),
        };

        // Hold only weak references in the global broadcast loop so closing a pane
        // frees `TipsCycle` and its GTK widgets on the next settings event (or immediately).
        let label_weak = label.downgrade();
        let revealer_weak = revealer.downgrade();
        let enabled_weak = Rc::downgrade(&cycle.enabled);
        let active_weak = Rc::downgrade(&cycle.is_active);
        let tips_ptr_weak = Rc::downgrade(&cycle.active_tips_ptr);
        let timer_weak = Rc::downgrade(&cycle.timer_id);
        let show_weak = Rc::downgrade(&cycle.show_timer_id);
        let fade_weak = Rc::downgrade(&cycle.fade_timer_id);
        let hide_weak = Rc::downgrade(&cycle.hide_timer_id);

        let mut rx = SETTINGS_EVENT_BUS.subscribe();
        gtk::glib::spawn_future_local(async move {
            loop {
                let settings = match rx.recv().await {
                    Ok(s) => s,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        (*boxxy_preferences::Settings::load()).clone()
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let (
                    Some(_lbl),
                    Some(rev),
                    Some(enabled_rc),
                    Some(active_rc),
                    Some(tips_ptr_rc),
                    Some(timer_rc),
                    Some(show_rc),
                    Some(fade_rc),
                    Some(hide_rc),
                ) = (
                    label_weak.upgrade(),
                    revealer_weak.upgrade(),
                    enabled_weak.upgrade(),
                    active_weak.upgrade(),
                    tips_ptr_weak.upgrade(),
                    timer_weak.upgrade(),
                    show_weak.upgrade(),
                    fade_weak.upgrade(),
                    hide_weak.upgrade(),
                )
                else {
                    break;
                };

                enabled_rc.set(settings.enable_tips);
                if !settings.enable_tips {
                    active_rc.set(false);
                    tips_ptr_rc.set(0);
                    for slot in [&show_rc, &fade_rc, &hide_rc, &timer_rc] {
                        if let Some(sid) = slot.borrow_mut().take() {
                            sid.remove();
                        }
                    }
                    rev.set_reveal_child(false);
                }
            }
        });

        cycle
    }

    pub fn start(&self) {
        self.start_with_messages(TIPS);
    }

    pub fn start_with_messages(&self, tips: &'static [&'static str]) {
        if !self.enabled.get() || tips.is_empty() {
            return;
        }
        let next_ptr = tips.as_ptr() as usize;
        if self.is_active.get() && self.active_tips_ptr.get() == next_ptr {
            return;
        }

        // If switching message sets (e.g. Pending -> Thinking), tear down the previous timers cleanly.
        self.cancel_show_timer();
        self.cancel_fade_timer();
        self.cancel_hide_timer();
        if let Some(source_id) = self.timer_id.borrow_mut().take() {
            source_id.remove();
        }

        self.is_active.set(true);
        self.active_tips_ptr.set(next_ptr);

        // Delay showing the tip by 1 second to avoid flickering on fast turns.
        let revealer_clone = self.revealer.clone();
        let label_clone = self.label.clone();
        let current_index_clone = self.current_index.clone();
        let last_shown = self.last_shown_at.clone();
        let show_timer_rc = self.show_timer_id.clone();

        let show_id = gtk::glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Ok(mut timer) = show_timer_rc.try_borrow_mut() {
                timer.take();
            }

            let next = next_index(current_index_clone.get(), tips.len());
            current_index_clone.set(next);
            label_clone.set_text(tips[next]);
            revealer_clone.set_reveal_child(true);
            last_shown.set(gtk::glib::monotonic_time().cast_unsigned());
        });

        *self.show_timer_id.borrow_mut() = Some(show_id);

        // Start the rotation timer (15s).
        let cycle_revealer = self.revealer.clone();
        let cycle_label = self.label.clone();
        let cycle_index = self.current_index.clone();
        let cycle_fade_slot = self.fade_timer_id.clone();
        let cycle_last_shown = self.last_shown_at.clone();

        let source_id = gtk::glib::timeout_add_local(Duration::from_secs(15), move || {
            if !cycle_revealer.reveals_child() {
                return gtk::glib::ControlFlow::Continue;
            }
            let next = next_index(cycle_index.get(), tips.len());
            cycle_index.set(next);

            // Fade out, change text, fade in (tracked in `fade_timer_id` so `stop()` /
            // `hide_immediately()` cancels it cleanly).
            cycle_revealer.set_reveal_child(false);
            if let Some(prev_fade) = cycle_fade_slot.borrow_mut().take() {
                prev_fade.remove();
            }
            let lbl = cycle_label.clone();
            let rev = cycle_revealer.clone();
            let fade_slot_inner = cycle_fade_slot.clone();
            let shown_inner = cycle_last_shown.clone();
            let fade_id =
                gtk::glib::timeout_add_local_once(Duration::from_millis(300), move || {
                    if let Ok(mut slot) = fade_slot_inner.try_borrow_mut() {
                        slot.take();
                    }
                    lbl.set_text(tips[next]);
                    rev.set_reveal_child(true);
                    shown_inner.set(gtk::glib::monotonic_time().cast_unsigned());
                });
            *cycle_fade_slot.borrow_mut() = Some(fade_id);

            gtk::glib::ControlFlow::Continue
        });

        *self.timer_id.borrow_mut() = Some(source_id);
    }

    pub fn hide_immediately(&self) {
        self.is_active.set(false);
        self.active_tips_ptr.set(0);
        self.cancel_show_timer();
        self.cancel_fade_timer();
        self.cancel_hide_timer();
        if let Some(source_id) = self.timer_id.borrow_mut().take() {
            source_id.remove();
        }
        self.revealer.set_reveal_child(false);
    }

    pub fn stop(&self) {
        if !self.is_active.get() {
            return;
        }

        self.is_active.set(false);
        self.active_tips_ptr.set(0);
        self.cancel_show_timer();
        self.cancel_fade_timer();

        if let Some(source_id) = self.timer_id.borrow_mut().take() {
            source_id.remove();
        }

        if !self.revealer.reveals_child() {
            return;
        }

        // Enforce minimum 6 seconds display time.
        let now = gtk::glib::monotonic_time().cast_unsigned();
        let elapsed_us = now.saturating_sub(self.last_shown_at.get());
        let min_duration_us = 6_000_000;

        if elapsed_us < min_duration_us {
            let remaining_ms = (min_duration_us - elapsed_us) / 1000;
            let rev = self.revealer.clone();
            let hide_timer_rc = self.hide_timer_id.clone();
            let hide_id =
                gtk::glib::timeout_add_local_once(Duration::from_millis(remaining_ms), move || {
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
            source_id.remove();
        }
    }

    fn cancel_fade_timer(&self) {
        if let Some(source_id) = self.fade_timer_id.borrow_mut().take() {
            source_id.remove();
        }
    }

    fn cancel_hide_timer(&self) {
        if let Some(source_id) = self.hide_timer_id.borrow_mut().take() {
            source_id.remove();
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.set(enabled);
        if !enabled {
            self.hide_immediately();
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.get()
    }
}
