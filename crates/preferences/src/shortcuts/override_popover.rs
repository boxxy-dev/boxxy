use super::build_chord_pill;
use adw::prelude::*;
use boxxy_keybindings::{Chord, OutgoingOverride, ShortcutId, spec_for};
use gtk4 as gtk;
use libadwaita as adw;
use std::rc::Rc;

/// Callback invoked when the user clicks "Go to Action" inside an Override Popover.
pub(super) type JumpToActionFn = Rc<dyn Fn(ShortcutId)>;

/// Builds and shows a compact override summary `GtkPopover` anchored to `anchor`.
pub(super) fn show_outgoing_override_popover(
    anchor: &gtk::Button,
    winner_id: ShortcutId,
    overrides: &[OutgoingOverride],
    on_jump: &JumpToActionFn,
) {
    let popover = gtk::Popover::builder()
        .has_arrow(true)
        .autohide(true)
        .position(gtk::PositionType::Bottom)
        .css_classes(["shortcut-override-popover"])
        .build();

    let root_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root_box.set_margin_top(12);
    root_box.set_margin_bottom(10);
    root_box.set_margin_start(14);
    root_box.set_margin_end(14);
    root_box.set_width_request(280);

    let winner_spec = spec_for(winner_id);

    for (idx, ov) in overrides.iter().enumerate() {
        if idx > 0 {
            root_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }

        let section = gtk::Box::new(gtk::Orientation::Vertical, 6);

        match ov {
            OutgoingOverride::Action {
                chord,
                shadowed_action,
                shadowed_has_active_alt,
            } => {
                let target_spec = spec_for(*shadowed_action);

                section.append(&build_popover_header(
                    &format!("Overrides {}", target_spec.title),
                    *chord,
                ));

                let body_text = if *shadowed_has_active_alt {
                    format!(
                        "{} takes priority over {} ({}). Its alternate shortcut is still active.",
                        winner_spec.title,
                        target_spec.title,
                        target_spec.category.title(),
                    )
                } else {
                    format!(
                        "{} takes priority over {} ({}).",
                        winner_spec.title,
                        target_spec.title,
                        target_spec.category.title(),
                    )
                };
                section.append(&build_popover_body(&body_text));

                let jump_btn = gtk::Button::builder()
                    .label(format!("Go to {} →", target_spec.title))
                    .halign(gtk::Align::End)
                    .css_classes(["flat", "shortcut-popover-link"])
                    .build();
                let pop_weak = popover.downgrade();
                let target_id = *shadowed_action;
                let on_jump_cb = on_jump.clone();
                jump_btn.connect_clicked(move |_| {
                    if let Some(p) = pop_weak.upgrade() {
                        p.popdown();
                    }
                    on_jump_cb(target_id);
                });
                section.append(&jump_btn);
            }
            OutgoingOverride::TerminalBuiltin {
                chord,
                builtin_label,
                can_make_performable,
            } => {
                section.append(&build_popover_header(
                    &format!("Overrides {builtin_label}"),
                    *chord,
                ));

                let body_text = if *can_make_performable {
                    format!(
                        "Intercepts {builtin_label} in the terminal. Enable Smart Pass-Through to forward it when idle."
                    )
                } else {
                    format!("Intercepts {builtin_label} while the terminal is focused.")
                };
                section.append(&build_popover_body(&body_text));

                if *can_make_performable {
                    let smart_btn = gtk::Button::builder()
                        .label("Enable Smart Pass-Through")
                        .halign(gtk::Align::End)
                        .css_classes(["suggested-action", "caption"])
                        .build();
                    let pop_weak = popover.downgrade();
                    let target_chord = *chord;
                    smart_btn.connect_clicked(move |_| {
                        if let Some(p) = pop_weak.upgrade() {
                            p.popdown();
                        }
                        enable_performable_for_chord(winner_id, target_chord);
                    });
                    section.append(&smart_btn);
                }
            }
        }

        root_box.append(&section);
    }

    popover.set_child(Some(&root_box));
    attach_transient_popover(anchor, &popover);
}

/// Shows a clean summary popover when clicking a shadowed shortcut pill.
pub(super) fn show_shadowed_chord_popover(
    anchor: &gtk::Button,
    owner_id: ShortcutId,
    blocker_id: ShortcutId,
    chord: Chord,
    on_jump: &JumpToActionFn,
) {
    let has_alt = boxxy_keybindings::keymap()
        .diagnostics_for(owner_id)
        .chord_statuses
        .iter()
        .any(|(_, st)| *st == boxxy_keybindings::ChordStatus::Active);

    let blocker_spec = spec_for(blocker_id);
    let owner_spec = spec_for(owner_id);

    let popover = gtk::Popover::builder()
        .has_arrow(true)
        .autohide(true)
        .position(gtk::PositionType::Bottom)
        .css_classes(["shortcut-override-popover"])
        .build();

    let root_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root_box.set_margin_top(12);
    root_box.set_margin_bottom(10);
    root_box.set_margin_start(14);
    root_box.set_margin_end(14);
    root_box.set_width_request(280);

    root_box.append(&build_popover_header(
        &format!("Overridden by {}", blocker_spec.title),
        chord,
    ));

    let body_text = if has_alt {
        format!(
            "{} ({}) takes priority for this shortcut. Your primary shortcut for {} is still active.",
            blocker_spec.title,
            blocker_spec.category.title(),
            owner_spec.title,
        )
    } else {
        format!(
            "{} ({}) takes priority for this shortcut, so pressing it will not trigger {}.",
            blocker_spec.title,
            blocker_spec.category.title(),
            owner_spec.title,
        )
    };
    root_box.append(&build_popover_body(&body_text));

    let jump_btn = gtk::Button::builder()
        .label(format!("Go to {} →", blocker_spec.title))
        .halign(gtk::Align::End)
        .css_classes(["flat", "shortcut-popover-link"])
        .build();
    let pop_weak = popover.downgrade();
    let on_jump_cb = on_jump.clone();
    jump_btn.connect_clicked(move |_| {
        if let Some(p) = pop_weak.upgrade() {
            p.popdown();
        }
        on_jump_cb(blocker_id);
    });
    root_box.append(&jump_btn);

    popover.set_child(Some(&root_box));
    attach_transient_popover(anchor, &popover);
}

fn build_popover_header(title: &str, chord: Chord) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title_lbl = gtk::Label::builder()
        .label(title)
        .hexpand(true)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["heading"])
        .build();
    header.append(&title_lbl);
    header.append(&build_chord_pill(chord, &[]));
    header
}

fn build_popover_body(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(38)
        .css_classes(["dim-label"])
        .build()
}

fn enable_performable_for_chord(id: ShortcutId, target: Chord) {
    let km = boxxy_keybindings::keymap();
    let updated: Vec<Chord> = km
        .chords_for(id)
        .iter()
        .map(|&c| {
            if c.signature() == target.signature() {
                Chord {
                    performable: true,
                    ..c
                }
            } else {
                c
            }
        })
        .collect();
    boxxy_keybindings::set_action_chords(id, updated, false);
}

fn attach_transient_popover(anchor: &gtk::Button, popover: &gtk::Popover) {
    // Suppress the anchor's tooltip while the popover is open so they never overlap.
    let had_tooltip = anchor.has_tooltip();
    if had_tooltip {
        anchor.set_has_tooltip(false);
    }

    popover.set_parent(anchor);
    let pop_closed = popover.downgrade();
    let anchor_closed = anchor.downgrade();
    popover.connect_closed(move |_| {
        if had_tooltip && let Some(a) = anchor_closed.upgrade() {
            a.set_has_tooltip(true);
        }
        let pop_inner = pop_closed.clone();
        gtk::glib::idle_add_local_once(move || {
            if let Some(p) = pop_inner.upgrade()
                && p.parent().is_some()
            {
                p.unparent();
            }
        });
    });
    let pop_destroy = popover.downgrade();
    anchor.connect_destroy(move |_| {
        if let Some(p) = pop_destroy.upgrade()
            && p.parent().is_some()
        {
            p.unparent();
        }
    });
    popover.popup();
}
