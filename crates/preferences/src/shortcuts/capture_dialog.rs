#![allow(clippy::redundant_pub_crate)]

use crate::ui_utils::VoidCallbackSlot;
use adw::prelude::*;
use boxxy_keybindings::{
    ACCEL_MOD_MASK, Chord, OutgoingOverride, ShortcutId, is_standalone_modifier, spec_for,
};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// Opens the interactive `ShortcutCaptureDialog` modal sheet for `target_id`.
#[allow(clippy::too_many_lines)]
pub(crate) fn present_capture_dialog(parent: &impl IsA<gtk::Widget>, target_id: ShortcutId) {
    let builder =
        gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/shortcut_capture_dialog.ui");

    let dialog: adw::Dialog = builder.object("capture_dialog").unwrap();
    let action_title_label: gtk::Label = builder.object("action_title_label").unwrap();
    let default_hint_label: gtk::Label = builder.object("default_hint_label").unwrap();
    let preview_slot: gtk::Box = builder.object("preview_slot").unwrap();
    let prompt_label: gtk::Label = builder.object("prompt_label").unwrap();
    let existing_section: gtk::Box = builder.object("existing_section").unwrap();
    let existing_chips_wrap: adw::WrapBox = builder.object("existing_chips_wrap").unwrap();
    let performable_row: gtk::Box = builder.object("performable_row").unwrap();
    let performable_switch: gtk::Switch = builder.object("performable_switch").unwrap();
    let conflict_card: gtk::Box = builder.object("conflict_card").unwrap();
    let conflict_label: gtk::Label = builder.object("conflict_label").unwrap();
    let disable_btn: gtk::Button = builder.object("disable_btn").unwrap();
    let reset_default_btn: gtk::Button = builder.object("reset_default_btn").unwrap();
    let add_secondary_btn: gtk::Button = builder.object("add_secondary_btn").unwrap();
    let save_btn: gtk::Button = builder.object("save_btn").unwrap();

    let spec = spec_for(target_id);
    action_title_label.set_label(spec.title);

    let default_text = if spec.default_triggers.is_empty() {
        "Default: Unbound".to_string()
    } else {
        format!("Default: {}", spec.default_triggers.join(" / "))
    };
    default_hint_label.set_label(&default_text);
    performable_row.set_visible(target_id.supports_performable());

    let pending_chord: Rc<RefCell<Option<Chord>>> = Rc::new(RefCell::new(None));
    let working_chords: Rc<RefCell<Vec<Chord>>> =
        Rc::new(RefCell::new(boxxy_keybindings::keymap().chords_for(target_id).to_vec()));

    let refresh_ui: Rc<dyn Fn()> = {
        let performable_switch = performable_switch.clone();
        let reset_default_btn = reset_default_btn.clone();
        let disable_btn = disable_btn.clone();
        let add_secondary_btn = add_secondary_btn.clone();
        let save_btn = save_btn.clone();
        let pending_chord = pending_chord.clone();
        let working_chords = working_chords.clone();

        let slot: VoidCallbackSlot = Rc::new(RefCell::new(None));
        let slot_weak = Rc::downgrade(&slot);

        let closure: Rc<dyn Fn()> = Rc::new(move || {
            while let Some(child) = preview_slot.first_child() {
                preview_slot.remove(&child);
            }
            while let Some(child) = existing_chips_wrap.first_child() {
                existing_chips_wrap.remove(&child);
            }

            let km = boxxy_keybindings::keymap();
            let current_list = working_chords.borrow().clone();
            let candidate = *pending_chord.borrow();

            reset_default_btn.set_sensitive(
                km.is_customized(target_id)
                    || current_list != boxxy_keybindings::store::default_chords_for(target_id)
                    || candidate.is_some(),
            );
            disable_btn.set_sensitive(!current_list.is_empty() || candidate.is_some());

            // Render assigned shortcut chips when multiple or existing shortcuts exist
            if current_list.is_empty() {
                existing_section.set_visible(false);
            } else {
                existing_section.set_visible(true);
                for (idx, ch) in current_list.iter().copied().enumerate() {
                    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    chip.add_css_class("shortcut-existing-chip");
                    chip.append(&adw::ShortcutLabel::new(&ch.to_gtk_accel()));
                    if ch.performable {
                        chip.append(
                            &gtk::Label::builder()
                                .label("smart")
                                .css_classes(["shortcut-smart-pill"])
                                .valign(gtk::Align::Center)
                                .build(),
                        );
                    }
                    let rm_btn = gtk::Button::builder()
                        .icon_name("boxxy-cross-small-symbolic")
                        .tooltip_text("Remove this shortcut")
                        .css_classes(["flat", "circular"])
                        .valign(gtk::Align::Center)
                        .build();
                    let wc_rm = working_chords.clone();
                    let slot_rm = slot_weak.clone();
                    rm_btn.connect_clicked(move |_| {
                        let next: Vec<Chord> = wc_rm
                            .borrow()
                            .iter()
                            .copied()
                            .enumerate()
                            .filter_map(|(i, c)| (i != idx).then_some(c))
                            .collect();
                        wc_rm.replace(next.clone());
                        boxxy_keybindings::set_action_chords(target_id, next, false);
                        if let Some(rc) = slot_rm.upgrade()
                            && let Some(ref f) = *rc.borrow()
                        {
                            f();
                        }
                    });
                    chip.append(&rm_btn);
                    existing_chips_wrap.append(&chip);
                }
            }

            if let Some(mut cand) = candidate {
                cand.performable = target_id.supports_performable() && performable_switch.is_active();
                preview_slot.append(&adw::ShortcutLabel::new(&cand.to_gtk_accel()));
                prompt_label.set_label("Press another combination to change, or click Set Shortcut");

                let already_in_list = current_list
                    .iter()
                    .any(|c| c.signature() == cand.signature());
                add_secondary_btn.set_visible(!current_list.is_empty() && !already_in_list);
                save_btn.set_sensitive(true);

                // Preview conflicts & terminal shadows before saving
                if let Some(owner) = km.find_conflicting_owner(cand, Some(target_id)) {
                    save_btn.set_label("Replace");
                    conflict_label.set_label(&format!(
                        "Currently assigned to \"{}\". Replacing will unbind it there.",
                        spec_for(owner).title
                    ));
                    conflict_card.set_visible(true);
                } else {
                    save_btn.set_label("Set Shortcut");
                    if let Some(shadow_msg) = preview_terminal_shadow(target_id, cand) {
                        conflict_label.set_label(&shadow_msg);
                        conflict_card.set_visible(true);
                    } else {
                        conflict_card.set_visible(false);
                    }
                }
            } else {
                if let Some(first) = current_list.first() {
                    preview_slot.append(&adw::ShortcutLabel::new(&first.to_gtk_accel()));
                } else {
                    preview_slot.append(
                        &gtk::Label::builder()
                            .label("Listening for keys…")
                            .css_classes(["dim-label"])
                            .build(),
                    );
                }
                prompt_label.set_label(
                    "Press a key combination, Backspace to disable, or Esc to cancel",
                );
                add_secondary_btn.set_visible(false);
                save_btn.set_label("Set Shortcut");
                save_btn.set_sensitive(false);
                conflict_card.set_visible(false);
            }
        });

        *slot.borrow_mut() = Some(closure.clone());
        closure
    };

    refresh_ui();

    {
        let refresh = refresh_ui.clone();
        performable_switch.connect_active_notify(move |_| {
            refresh();
        });
    }

    // Capture-phase key recorder
    let key_ctrl = gtk::EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let dialog_weak = dialog.downgrade();
        let pending_chord = pending_chord.clone();
        let performable_switch = performable_switch.clone();
        let refresh = refresh_ui.clone();
        key_ctrl.connect_key_pressed(move |ctrl, keyval, keycode, raw_mods| {
            let Some(dlg) = dialog_weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if is_standalone_modifier(keyval) {
                return glib::Propagation::Stop;
            }

            let clean_mods = raw_mods & ACCEL_MOD_MASK;

            // Bare Escape cancels and closes the dialog.
            if keyval == gdk::Key::Escape && clean_mods.is_empty() {
                dlg.close();
                return glib::Propagation::Stop;
            }

            // Bare Backspace disables the shortcut immediately when no chord is staged,
            // or clears the staged preview chord if one was captured.
            if keyval == gdk::Key::BackSpace && clean_mods.is_empty() {
                if pending_chord.borrow().is_some() {
                    pending_chord.replace(None);
                    refresh();
                } else {
                    boxxy_keybindings::set_action_chords(target_id, Vec::new(), false);
                    dlg.close();
                }
                return glib::Propagation::Stop;
            }

            // Resolve a layout-independent Latin chord via GDK's keymap (`key_match` + `map_keycode`)
            // so recording `Ctrl+Shift+V` while on a Greek/Russian layout records `<Control><Shift>v`.
            let (resolved_key, resolved_mods) = resolve_recorded_key(ctrl, &dlg, keyval, keycode, clean_mods);

            // Reject unmodified printable characters (except F1..F35 or dedicated media/clipboard keys)
            // so accidental single letter presses don't hijack typing.
            if resolved_mods.difference(gdk::ModifierType::SHIFT_MASK).is_empty()
                && !is_allowed_unmodified_key(resolved_key)
            {
                return glib::Propagation::Stop;
            }

            let captured = Chord::new(
                resolved_key,
                resolved_mods,
                target_id.supports_performable() && performable_switch.is_active(),
            );
            pending_chord.replace(Some(captured));
            refresh();
            glib::Propagation::Stop
        });
    }
    dialog.add_controller(key_ctrl);

    {
        let dlg_weak = dialog.downgrade();
        disable_btn.connect_clicked(move |_| {
            boxxy_keybindings::set_action_chords(target_id, Vec::new(), false);
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });
    }

    {
        let dlg_weak = dialog.downgrade();
        reset_default_btn.connect_clicked(move |_| {
            boxxy_keybindings::reset_action(target_id);
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });
    }

    {
        let dlg_weak = dialog.downgrade();
        let pending = pending_chord.clone();
        let perf_sw = performable_switch.clone();
        save_btn.connect_clicked(move |_| {
            let Some(mut ch) = *pending.borrow() else {
                return;
            };
            ch.performable = target_id.supports_performable() && perf_sw.is_active();
            boxxy_keybindings::set_action_chords(target_id, vec![ch], true);
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });
    }

    {
        let dlg_weak = dialog.downgrade();
        let pending = pending_chord;
        let wc = working_chords;
        let perf_sw = performable_switch;
        add_secondary_btn.connect_clicked(move |_| {
            let Some(mut ch) = *pending.borrow() else {
                return;
            };
            ch.performable = target_id.supports_performable() && perf_sw.is_active();
            let mut next = wc.borrow().clone();
            if !next.iter().any(|c| c.signature() == ch.signature()) {
                next.push(ch);
            }
            boxxy_keybindings::set_action_chords(target_id, next, true);
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });
    }

    dialog.present(Some(parent));
}

fn resolve_recorded_key(
    ctrl: &gtk::EventControllerKey,
    dlg: &adw::Dialog,
    raw_key: gdk::Key,
    keycode: u32,
    clean_mods: gdk::ModifierType,
) -> (gdk::Key, gdk::ModifierType) {
    // If the active layout produced a non-ASCII script keyval (e.g. Greek `Ω` or Cyrillic `М`),
    // map the hardware keycode to the user's Latin XKB group so the saved chord is layout-agnostic.
    if raw_key.to_unicode().is_some_and(|c| !c.is_ascii())
        && let Some(entries) = dlg.display().map_keycode(keycode)
        && let Some((_, ascii_kv)) = entries
            .iter()
            .find(|(km, kv)| km.level() == 0 && kv.to_unicode().is_some_and(|c| c.is_ascii_graphic()))
    {
        return (*ascii_kv, clean_mods);
    }

    let mut mods = clean_mods;
    // Strip consumed Shift when Shift produced a distinct punctuation symbol (e.g. `+` on US `Shift+=`),
    // while keeping Shift on letters (`A`..`Z`) and `ISO_Left_Tab` (`Shift+Tab`).
    if mods.contains(gdk::ModifierType::SHIFT_MASK)
        && raw_key != gdk::Key::ISO_Left_Tab
        && !raw_key
            .to_lower()
            .to_unicode()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && let Some(ev) = ctrl.current_event()
        && let Ok(key_ev) = ev.downcast::<gdk::KeyEvent>()
        && key_ev
            .consumed_modifiers()
            .contains(gdk::ModifierType::SHIFT_MASK)
    {
        mods.remove(gdk::ModifierType::SHIFT_MASK);
    }

    (raw_key, mods)
}

fn is_allowed_unmodified_key(key: gdk::Key) -> bool {
    if let Some(name) = key.name()
        && let Some(num_str) = name.strip_prefix('F')
        && num_str.parse::<u8>().is_ok_and(|n| (1..=35).contains(&n))
    {
        return true;
    }
    matches!(
        key,
        gdk::Key::Copy | gdk::Key::Paste | gdk::Key::Cut | gdk::Key::Menu
    )
}

fn preview_terminal_shadow(target_id: ShortcutId, chord: Chord) -> Option<String> {
    let mut map = BTreeMap::new();
    map.insert(
        target_id,
        boxxy_keybindings::OverrideValue::Chords(vec![chord]),
    );
    let probe = boxxy_keybindings::CompiledKeymap::from_overrides(map, Vec::new(), None);
    for ov in &probe.diagnostics_for(target_id).outgoing_overrides {
        if let OutgoingOverride::TerminalBuiltin {
            builtin_label,
            can_make_performable,
            ..
        } = ov
        {
            if *can_make_performable {
                return Some(format!(
                    "Overrides {builtin_label}. Enable Smart Shortcut above to pass through when idle."
                ));
            }
            return Some(format!("Overrides {builtin_label} while terminal is focused."));
        }
    }
    None
}
