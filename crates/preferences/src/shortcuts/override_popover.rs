use adw::prelude::*;
use boxxy_keybindings::{Chord, OutgoingOverride, ShortcutId, spec_for};
use gtk4 as gtk;
use libadwaita as adw;
use std::rc::Rc;

/// Callback invoked when the user clicks "Go to Action" inside an Override Popover.
pub(super) type JumpToActionFn = Rc<dyn Fn(ShortcutId)>;

/// Builds and shows a compact graphical Override Map `GtkPopover` anchored to `anchor`.
#[allow(clippy::too_many_lines)]
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
        .build();

    let root_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root_box.set_margin_top(10);
    root_box.set_margin_bottom(10);
    root_box.set_margin_start(12);
    root_box.set_margin_end(12);
    root_box.set_width_request(290);

    // Header row
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let bolt = gtk::Image::from_icon_name("boxxy-vibe-symbolic");
    let title = gtk::Label::builder()
        .label("Shortcut Override")
        .css_classes(["heading"])
        .halign(gtk::Align::Start)
        .build();
    header.append(&bolt);
    header.append(&title);
    root_box.append(&header);

    let winner_spec = spec_for(winner_id);

    for ov in overrides {
        let flow = gtk::Box::new(gtk::Orientation::Vertical, 4);

        let chord = match ov {
            OutgoingOverride::Action { chord, .. }
            | OutgoingOverride::TerminalBuiltin { chord, .. } => *chord,
        };

        // Active winner node
        let active_node = build_flow_node(
            "●",
            winner_spec.title,
            Some(winner_spec.category.title()),
            Some(chord),
            "shortcut-override-node-active",
        );
        flow.append(&active_node);

        // Connector arrow
        let conn = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        conn.set_halign(gtk::Align::Center);
        let arrow = gtk::Image::from_icon_name("boxxy-down-symbolic");
        arrow.set_pixel_size(12);
        arrow.add_css_class("dim-label");
        let conn_lbl = gtk::Label::builder()
            .label("overrides")
            .css_classes(["caption", "dim-label"])
            .build();
        conn.append(&arrow);
        conn.append(&conn_lbl);
        flow.append(&conn);

        // Shadowed target node + quick action
        match ov {
            OutgoingOverride::Action {
                shadowed_action,
                shadowed_has_active_alt,
                ..
            } => {
                let target_spec = spec_for(*shadowed_action);
                let sub = if *shadowed_has_active_alt {
                    format!("{} · alternate shortcut active", target_spec.category.title())
                } else {
                    target_spec.category.title().to_string()
                };
                let target_node = build_flow_node(
                    "○",
                    target_spec.title,
                    Some(&sub),
                    None,
                    "shortcut-override-node-shadowed",
                );
                flow.append(&target_node);

                let jump_btn = gtk::Button::builder()
                    .label(format!("Go to {}", target_spec.title))
                    .halign(gtk::Align::End)
                    .css_classes(["flat", "caption"])
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
                flow.append(&jump_btn);
            }
            OutgoingOverride::TerminalBuiltin {
                builtin_label,
                can_make_performable,
                ..
            } => {
                let target_node = build_flow_node(
                    "○",
                    builtin_label,
                    Some("Terminal Input"),
                    None,
                    "shortcut-override-node-shadowed",
                );
                flow.append(&target_node);

                if *can_make_performable {
                    let smart_btn = gtk::Button::builder()
                        .label("Enable Smart Pass-Through")
                        .halign(gtk::Align::End)
                        .css_classes(["suggested-action", "caption"])
                        .build();
                    let pop_weak = popover.downgrade();
                    smart_btn.connect_clicked(move |_| {
                        if let Some(p) = pop_weak.upgrade() {
                            p.popdown();
                        }
                        enable_performable_for_chord(winner_id, chord);
                    });
                    flow.append(&smart_btn);
                }
            }
        }

        root_box.append(&flow);
    }

    popover.set_child(Some(&root_box));
    attach_transient_popover(anchor, &popover);
}

/// Shows the graphical Override Map when clicking a dimmed, shadowed chord chip
/// (e.g. clicking the shadowed `Ctrl+Shift+Up` chip on `Jump to Previous Command Block`).
pub(super) fn show_shadowed_chord_popover(
    anchor: &gtk::Button,
    owner_id: ShortcutId,
    blocker_id: ShortcutId,
    chord: Chord,
    on_jump: &JumpToActionFn,
) {
    let overrides = [OutgoingOverride::Action {
        chord,
        shadowed_action: owner_id,
        shadowed_has_active_alt: boxxy_keybindings::keymap()
            .diagnostics_for(owner_id)
            .chord_statuses
            .iter()
            .any(|(_, st)| *st == boxxy_keybindings::ChordStatus::Active),
    }];

    // Reuse the visual diagram, offering a jump button straight to the winning `blocker_id`.
    let popover = gtk::Popover::builder()
        .has_arrow(true)
        .autohide(true)
        .position(gtk::PositionType::Bottom)
        .build();

    let root_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root_box.set_margin_top(10);
    root_box.set_margin_bottom(10);
    root_box.set_margin_start(12);
    root_box.set_margin_end(12);
    root_box.set_width_request(290);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.append(&gtk::Image::from_icon_name("boxxy-vibe-symbolic"));
    header.append(
        &gtk::Label::builder()
            .label("Shadowed Shortcut")
            .css_classes(["heading"])
            .build(),
    );
    root_box.append(&header);

    let blocker_spec = spec_for(blocker_id);
    let owner_spec = spec_for(owner_id);

    root_box.append(&build_flow_node(
        "●",
        blocker_spec.title,
        Some(blocker_spec.category.title()),
        Some(chord),
        "shortcut-override-node-active",
    ));

    let conn = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    conn.set_halign(gtk::Align::Center);
    conn.append(&gtk::Image::from_icon_name("boxxy-down-symbolic"));
    conn.append(
        &gtk::Label::builder()
            .label("overrides")
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    root_box.append(&conn);

    let has_alt = matches!(
        overrides[0],
        OutgoingOverride::Action {
            shadowed_has_active_alt: true,
            ..
        }
    );
    let owner_sub = if has_alt {
        format!("{} · primary shortcut still active", owner_spec.category.title())
    } else {
        owner_spec.category.title().to_string()
    };
    root_box.append(&build_flow_node(
        "○",
        owner_spec.title,
        Some(&owner_sub),
        None,
        "shortcut-override-node-shadowed",
    ));

    let jump_btn = gtk::Button::builder()
        .label(format!("Go to {}", blocker_spec.title))
        .halign(gtk::Align::End)
        .css_classes(["flat", "caption"])
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

fn build_flow_node(
    marker: &str,
    title: &str,
    subtitle: Option<&str>,
    chord: Option<Chord>,
    css_class: &str,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class(css_class);

    let dot = gtk::Label::builder()
        .label(marker)
        .valign(gtk::Align::Center)
        .css_classes(["caption"])
        .build();
    row.append(&dot);

    let text_col = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text_col.set_hexpand(true);
    text_col.set_valign(gtk::Align::Center);
    text_col.append(
        &gtk::Label::builder()
            .label(title)
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption-heading"])
            .build(),
    );
    if let Some(sub) = subtitle {
        text_col.append(
            &gtk::Label::builder()
                .label(sub)
                .halign(gtk::Align::Start)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
    }
    row.append(&text_col);

    if let Some(c) = chord {
        let sc = adw::ShortcutLabel::new(&c.to_gtk_accel());
        sc.set_valign(gtk::Align::Center);
        row.append(&sc);
    }

    row
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
    popover.set_parent(anchor);
    let pop_closed = popover.downgrade();
    popover.connect_closed(move |_| {
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
