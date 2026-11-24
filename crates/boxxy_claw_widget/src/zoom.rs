//! Display-wide zoom scaling for the Claw overlay drawer (`Ctrl++` / `Ctrl+-` / `Ctrl+0`).

use boxxy_preferences::config::Settings;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};

const OVERLAY_ZOOM_DEFAULT_PCT: u32 = 100;
const OVERLAY_ZOOM_MIN_PCT: u32 = 60;
const OVERLAY_ZOOM_MAX_PCT: u32 = 220;
const OVERLAY_ZOOM_STEP_PCT: u32 = 10;

thread_local! {
    static OVERLAY_ZOOM_STATE: (Cell<u32>, RefCell<Option<gtk::CssProvider>>) =
        const { (Cell::new(OVERLAY_ZOOM_DEFAULT_PCT), RefCell::new(None)) };
}

fn apply_overlay_zoom_pct(pct: u32) {
    let clamped = pct.clamp(OVERLAY_ZOOM_MIN_PCT, OVERLAY_ZOOM_MAX_PCT);
    OVERLAY_ZOOM_STATE.with(|(zoom_cell, provider_cell)| {
        if zoom_cell.get() == clamped && provider_cell.borrow().is_some() {
            return;
        }
        zoom_cell.set(clamped);
        let mut slot = provider_cell.borrow_mut();
        let provider = slot.get_or_insert_with(|| {
            let p = gtk::CssProvider::new();
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &p,
                    gtk::STYLE_PROVIDER_PRIORITY_USER,
                );
            }
            p
        });
        provider.load_from_string(&format!(
            ".claw-drawer .virtual-history, .claw-drawer #diagnosis_container, .claw-drawer #command_frame, .claw-drawer .boxxy-msgbar {{ font-size: {clamped}%; }}"
        ));
    });
}

pub fn set_overlay_zoom_pct(pct: u32) {
    apply_overlay_zoom_pct(pct);
}

fn update_and_persist_overlay_zoom_pct(pct: u32) {
    let clamped = pct.clamp(OVERLAY_ZOOM_MIN_PCT, OVERLAY_ZOOM_MAX_PCT);
    apply_overlay_zoom_pct(clamped);
    if Settings::load().overlay_font_scale != clamped {
        Settings::update(|s| {
            s.overlay_font_scale = clamped;
        });
    }
}

pub fn zoom_in_overlay() {
    let next = OVERLAY_ZOOM_STATE
        .with(|(zoom_cell, _)| zoom_cell.get().saturating_add(OVERLAY_ZOOM_STEP_PCT));
    update_and_persist_overlay_zoom_pct(next);
}

pub fn zoom_out_overlay() {
    let next = OVERLAY_ZOOM_STATE
        .with(|(zoom_cell, _)| zoom_cell.get().saturating_sub(OVERLAY_ZOOM_STEP_PCT));
    update_and_persist_overlay_zoom_pct(next);
}

pub fn reset_overlay_zoom() {
    update_and_persist_overlay_zoom_pct(OVERLAY_ZOOM_DEFAULT_PCT);
}
