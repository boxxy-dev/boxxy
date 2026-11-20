pub(crate) mod capture_dialog;
mod override_popover;

use crate::ui_utils::{PageFilter, bind_mapped_poller, show_toast};
use adw::prelude::*;
use boxxy_keybindings::{
    ChordStatus, CompiledKeymap, ShortcutCategory, ShortcutId, spec_for,
};
use capture_dialog::present_capture_dialog;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use override_popover::{
    JumpToActionFn, show_outgoing_override_popover, show_shadowed_chord_popover,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

struct ShortcutRowEntry {
    id: ShortcutId,
    row: adw::ActionRow,
    override_btn: gtk::Button,
    custom_pill: gtk::Label,
    chords_box: gtk::Box,
    reset_btn: gtk::Button,
    haystack: RefCell<String>,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn populate_shortcuts_page(page: &adw::PreferencesPage) -> PageFilter {
    // ── Top Management Group (Warning Banner + Edit Config + Reset All) ──────
    let mgmt_group = adw::PreferencesGroup::new();

    let warning_banner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    warning_banner.add_css_class("shortcut-conflict-banner");
    warning_banner.set_margin_bottom(8);
    warning_banner.set_visible(false);
    warning_banner.append(&gtk::Image::from_icon_name("boxxy-dialog-warning-symbolic"));
    let warning_label = gtk::Label::builder()
        .hexpand(true)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption"])
        .build();
    warning_banner.append(&warning_label);
    mgmt_group.add(&warning_banner);

    let mgmt_row = adw::ActionRow::builder()
        .title("Custom Keybindings File")
        .subtitle("Stored in keybindings.toml for easy dotfile sharing and manual editing")
        .build();

    let edit_file_btn = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .tooltip_text("Open keybindings.toml in your default text editor")
        .build();
    let edit_btn_content = adw::ButtonContent::builder()
        .icon_name("boxxy-document-edit-symbolic")
        .label("Edit File")
        .build();
    edit_file_btn.set_child(Some(&edit_btn_content));

    let reset_all_btn = gtk::Button::builder()
        .label("Reset All")
        .valign(gtk::Align::Center)
        .tooltip_text("Restore all shortcuts to Boxxy defaults")
        .css_classes(["destructive-action"])
        .build();

    let mgmt_suffix = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    mgmt_suffix.set_valign(gtk::Align::Center);
    mgmt_suffix.append(&edit_file_btn);
    mgmt_suffix.append(&reset_all_btn);
    mgmt_row.add_suffix(&mgmt_suffix);
    mgmt_group.add(&mgmt_row);
    page.add(&mgmt_group);

    {
        let page_weak = page.downgrade();
        edit_file_btn.connect_clicked(move |btn| {
            open_keybindings_in_editor(btn);
            if let Some(p) = page_weak.upgrade() {
                let _ = p;
            }
        });
    }

    {
        let page_weak = page.downgrade();
        reset_all_btn.connect_clicked(move |_| {
            let Some(p) = page_weak.upgrade() else {
                return;
            };
            confirm_reset_all(&p);
        });
    }

    // ── Build Category Groups & Action Rows ──────────────────────────────────
    let mut groups: Vec<(adw::PreferencesGroup, Vec<Rc<ShortcutRowEntry>>)> =
        Vec::with_capacity(ShortcutCategory::ALL.len());
    let mut all_entries: Vec<Rc<ShortcutRowEntry>> = Vec::with_capacity(ShortcutId::ALL.len());

    // Shared jump callback used by Override Popovers (`[Go to Action ->]`).
    let jump_slot: Rc<RefCell<Option<JumpToActionFn>>> = Rc::new(RefCell::new(None));
    let on_jump: JumpToActionFn = {
        let slot = jump_slot.clone();
        Rc::new(move |target_id| {
            if let Some(ref f) = *slot.borrow() {
                f(target_id);
            }
        })
    };

    for cat in ShortcutCategory::ALL {
        let group = adw::PreferencesGroup::builder()
            .title(glib::markup_escape_text(cat.title()))
            .build();
        let mut group_rows = Vec::new();

        for id in ShortcutId::ALL {
            let spec = spec_for(id);
            if spec.category != cat {
                continue;
            }

            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(spec.title))
                .activatable(true)
                .build();

            let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            suffix.set_valign(gtk::Align::Center);

            let override_btn = gtk::Button::builder()
                .icon_name("boxxy-vibe-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text("View shortcut override map")
                .css_classes(["flat", "shortcut-override-badge"])
                .visible(false)
                .build();

            let custom_pill = gtk::Label::builder()
                .label("Custom")
                .valign(gtk::Align::Center)
                .css_classes(["model-badge-custom", "caption"])
                .visible(false)
                .build();

            let chords_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            chords_box.set_valign(gtk::Align::Center);

            let default_desc = if spec.default_triggers.is_empty() {
                "Unbound".to_string()
            } else {
                spec.default_triggers.join(" / ")
            };
            let reset_btn = gtk::Button::builder()
                .icon_name("edit-undo-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text(format!("Reset to default ({default_desc})"))
                .css_classes(["flat", "circular"])
                .visible(false)
                .build();

            suffix.append(&override_btn);
            suffix.append(&custom_pill);
            suffix.append(&chords_box);
            suffix.append(&reset_btn);
            row.add_suffix(&suffix);

            {
                let row_weak = row.downgrade();
                row.connect_activated(move |_| {
                    if let Some(r) = row_weak.upgrade() {
                        present_capture_dialog(&r, id);
                    }
                });
            }

            {
                reset_btn.connect_clicked(move |_| {
                    boxxy_keybindings::reset_action(id);
                });
            }

            {
                let on_jump_ov = on_jump.clone();
                override_btn.connect_clicked(move |btn| {
                    let km = boxxy_keybindings::keymap();
                    let diag = km.diagnostics_for(id);
                    if !diag.outgoing_overrides.is_empty() {
                        show_outgoing_override_popover(
                            btn,
                            id,
                            &diag.outgoing_overrides,
                            &on_jump_ov,
                        );
                    }
                });
            }

            group.add(&row);

            let entry = Rc::new(ShortcutRowEntry {
                id,
                row,
                override_btn,
                custom_pill,
                chords_box,
                reset_btn,
                haystack: RefCell::new(String::new()),
            });
            group_rows.push(entry.clone());
            all_entries.push(entry);
        }

        page.add(&group);
        groups.push((group, group_rows));
    }

    // Wire smooth scroll + highlight pulse for `[Go to Action ->]` inside Override Popovers.
    {
        let page_weak = page.downgrade();
        let entries_for_jump = all_entries.clone();
        *jump_slot.borrow_mut() = Some(Rc::new(move |target_id| {
            let Some(p) = page_weak.upgrade() else {
                return;
            };
            let Some(target_entry) = entries_for_jump.iter().find(|e| e.id == target_id) else {
                return;
            };
            scroll_to_and_pulse_row(&p, &target_entry.row);
        }));
    }

    let sync_all_rows: Rc<dyn Fn()> = {
        let entries = all_entries;
        let reset_all_btn = reset_all_btn;
        let warning_banner = warning_banner;
        let warning_label = warning_label;
        let on_jump_sync = on_jump;

        Rc::new(move || {
            let km = boxxy_keybindings::keymap();
            reset_all_btn.set_sensitive(km.has_any_custom_overrides());

            if km.warnings().is_empty() {
                warning_banner.set_visible(false);
            } else {
                let msg = km
                    .warnings()
                    .iter()
                    .map(|w| format!("{}: {}", w.key, w.message))
                    .collect::<Vec<_>>()
                    .join(" · ");
                warning_label.set_label(&format!("keybindings.toml warning — {msg}"));
                warning_banner.set_visible(true);
            }

            for entry in &entries {
                render_row_state(entry, &km, &on_jump_sync);
            }
        })
    };

    sync_all_rows();

    // Live updates from UI changes (`KEYBINDINGS_EVENT_BUS`)
    {
        let sync = sync_all_rows.clone();
        let page_weak = page.downgrade();
        let mut rx = boxxy_keybindings::KEYBINDINGS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            loop {
                match rx.recv().await {
                    Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
                if page_weak.upgrade().is_none() {
                    break;
                }
                sync();
            }
        });
    }

    // Off-thread mtime check strictly while the Shortcuts page is mapped on screen
    let check_disk = || {
        boxxy_sys_utils::runtime().spawn_blocking(|| {
            let _ = boxxy_keybindings::reload_if_changed_on_disk();
        });
    };
    bind_mapped_poller(page, Duration::from_millis(750), check_disk, check_disk);

    Box::new(move |query: &str| {
        let mgmt_match = query.is_empty()
            || "custom keybindings.toml file reset defaults".contains(query);
        mgmt_group.set_visible(mgmt_match);

        let mut page_visible = mgmt_match;
        for (group, rows) in &groups {
            let mut group_visible = false;
            for entry in rows {
                let matched = query.is_empty() || entry.haystack.borrow().contains(query);
                entry.row.set_visible(matched);
                if matched {
                    group_visible = true;
                }
            }
            group.set_visible(group_visible);
            if group_visible {
                page_visible = true;
            }
        }
        page_visible
    })
}

fn render_row_state(
    entry: &ShortcutRowEntry,
    km: &CompiledKeymap,
    on_jump: &JumpToActionFn,
) {
    let spec = spec_for(entry.id);
    let diag = km.diagnostics_for(entry.id);
    let is_custom = km.is_customized(entry.id);

    entry.override_btn.set_visible(diag.has_outgoing_overrides());
    entry.custom_pill.set_visible(is_custom);
    entry.reset_btn.set_visible(is_custom);

    while let Some(child) = entry.chords_box.first_child() {
        entry.chords_box.remove(&child);
    }

    if diag.chord_statuses.is_empty() {
        let lbl = gtk::Label::builder()
            .label("Unbound")
            .valign(gtk::Align::Center)
            .css_classes(["shortcut-unbound-pill"])
            .build();
        entry.chords_box.append(&lbl);
    } else {
        for &(chord, status) in &diag.chord_statuses {
            match status {
                ChordStatus::Active => {
                    entry
                        .chords_box
                        .append(&adw::ShortcutLabel::new(&chord.to_gtk_accel()));
                    if chord.performable {
                        entry.chords_box.append(
                            &gtk::Label::builder()
                                .label("smart")
                                .valign(gtk::Align::Center)
                                .css_classes(["shortcut-smart-pill"])
                                .build(),
                        );
                    }
                }
                ChordStatus::ShadowedBy(blocker_id) => {
                    let shadowed_btn = gtk::Button::builder()
                        .valign(gtk::Align::Center)
                        .tooltip_text(format!(
                            "Overridden by {} — click for details",
                            spec_for(blocker_id).title
                        ))
                        .css_classes(["flat", "shortcut-chip-shadowed"])
                        .build();
                    shadowed_btn.set_child(Some(&adw::ShortcutLabel::new(&chord.to_gtk_accel())));

                    let owner_id = entry.id;
                    let on_jump_chip = on_jump.clone();
                    shadowed_btn.connect_clicked(move |btn| {
                        show_shadowed_chord_popover(
                            btn,
                            owner_id,
                            blocker_id,
                            chord,
                            &on_jump_chip,
                        );
                    });
                    entry.chords_box.append(&shadowed_btn);
                }
            }
        }
    }

    // Update pre-indexed lowercase search haystack including live custom triggers & states
    let triggers_str: String = diag
        .chord_statuses
        .iter()
        .map(|(c, _)| format!("{} {}", c.to_human_string(), c.to_gtk_accel()))
        .collect::<Vec<_>>()
        .join(" ");
    let flags_str = match (is_custom, diag.has_outgoing_overrides() || diag.has_shadowed_chords()) {
        (true, true) => "custom modified override conflict",
        (true, false) => "custom modified",
        (false, true) => "override conflict",
        (false, false) => "",
    };
    *entry.haystack.borrow_mut() = format!(
        "{} {} {} {} {} {}",
        spec.category.title(),
        spec.title,
        entry.id.as_str(),
        spec.keywords.join(" "),
        triggers_str,
        flags_str,
    )
    .to_lowercase();
}

fn open_keybindings_in_editor(anchor: &impl IsA<gtk::Widget>) {
    let widget_weak = anchor.downgrade();
    glib::spawn_future_local(async move {
        let res = boxxy_sys_utils::runtime()
            .spawn_blocking(boxxy_keybindings::ensure_keybindings_file)
            .await;

        let Some(w) = widget_weak.upgrade() else {
            return;
        };

        let path = match res {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                show_toast(&w, &format!("Failed to prepare keybindings.toml: {e}"));
                return;
            }
            Err(e) => {
                log::warn!("Task join error creating keybindings.toml: {e}");
                return;
            }
        };

        let root_win = w.root().and_downcast::<gtk::Window>();
        let file = gtk::gio::File::for_path(&path);
        let file_fallback = file.clone();
        let launcher = gtk::FileLauncher::new(Some(&file));
        launcher.launch(
            root_win.as_ref(),
            gtk::gio::Cancellable::NONE,
            move |launch_res| {
                if let Err(err) = launch_res
                    && !matches!(
                        err.kind::<gtk::DialogError>(),
                        Some(gtk::DialogError::Cancelled | gtk::DialogError::Dismissed)
                    )
                {
                    let uri = file_fallback.uri();
                    let _ = gtk::gio::AppInfo::launch_default_for_uri(
                        uri.as_str(),
                        None::<&gtk::gio::AppLaunchContext>,
                    );
                }
            },
        );
    });
}

fn confirm_reset_all(parent: &adw::PreferencesPage) {
    let alert = adw::AlertDialog::builder()
        .heading("Reset All Shortcuts?")
        .body("This will restore every customized or disabled shortcut back to Boxxy's defaults.")
        .default_response("cancel")
        .close_response("cancel")
        .build();
    alert.add_response("cancel", "Cancel");
    alert.add_response("reset", "Reset All");
    alert.set_response_appearance("reset", adw::ResponseAppearance::Destructive);

    let parent_weak = parent.downgrade();
    alert.connect_response(None, move |_, response| {
        if response == "reset" {
            boxxy_keybindings::reset_all();
            if let Some(p) = parent_weak.upgrade() {
                show_toast(&p, "All shortcuts reset to defaults");
            }
        }
    });
    alert.present(Some(parent));
}

fn scroll_to_and_pulse_row(page: &adw::PreferencesPage, row: &adw::ActionRow) {
    if let Some(scrolled) = row
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_downcast::<gtk::ScrolledWindow>()
        && let Some(content) = scrolled.child()
        && let Some(bounds) = row.compute_bounds(&content)
    {
        let adj = scrolled.vadjustment();
        let target_y = adj
            .page_size()
            .mul_add(-0.35, f64::from(bounds.y()))
            .clamp(adj.lower(), (adj.upper() - adj.page_size()).max(adj.lower()));
        adj.set_value(target_y);
    } else {
        let _ = page;
    }

    row.add_css_class("shortcut-row-pulse");
    let row_weak = row.downgrade();
    glib::timeout_add_local_once(Duration::from_millis(950), move || {
        if let Some(r) = row_weak.upgrade() {
            r.remove_css_class("shortcut-row-pulse");
        }
    });
}
