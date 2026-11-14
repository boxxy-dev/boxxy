use crate::config::{Currency, Settings};
use crate::ui_utils::{PageHooks, SearchRow, show_toast};
use adw::prelude::*;
use gtk4 as gtk;
use gtk4::gdk;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

fn is_valid_accelerator(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Some((key, mods)) = gtk::accelerator_parse(trimmed) else {
        return false;
    };
    if !gtk::accelerator_valid(key, mods) {
        return false;
    }
    let has_non_shift_mod = !mods.difference(gdk::ModifierType::SHIFT_MASK).is_empty();
    let is_function_key = matches!(
        key,
        gdk::Key::F1
            | gdk::Key::F2
            | gdk::Key::F3
            | gdk::Key::F4
            | gdk::Key::F5
            | gdk::Key::F6
            | gdk::Key::F7
            | gdk::Key::F8
            | gdk::Key::F9
            | gdk::Key::F10
            | gdk::Key::F11
            | gdk::Key::F12
    );
    has_non_shift_mod || is_function_key
}

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_claw_ui_page(builder: &gtk::Builder) -> PageHooks {
    let overlay_bottom_gap_switch: adw::SwitchRow =
        builder.object("overlay_bottom_gap_switch").unwrap();
    let always_show_code_blocks_switch: adw::SwitchRow =
        builder.object("always_show_code_blocks_switch").unwrap();
    let enable_tips_switch: adw::SwitchRow = builder.object("enable_tips_switch").unwrap();
    let overlay_font_scale_spin: adw::SpinRow =
        builder.object("overlay_font_scale_spin").unwrap();
    let currency_combo: adw::ComboRow = builder.object("currency_combo").unwrap();
    let enable_timer_sounds_switch: adw::SwitchRow =
        builder.object("enable_timer_sounds_switch").unwrap();
    let enable_task_sounds_switch: adw::SwitchRow =
        builder.object("enable_task_sounds_switch").unwrap();
    let hide_character_badge_switch: adw::SwitchRow =
        builder.object("hide_character_badge_switch").unwrap();
    let row_claw_shortcut: adw::ActionRow = builder.object("row_claw_shortcut").unwrap();
    let row_reset_claw_shortcut: adw::ActionRow =
        builder.object("row_reset_claw_shortcut").unwrap();
    let claw_msgbar_shortcut_entry: gtk::Entry =
        builder.object("claw_msgbar_shortcut_entry").unwrap();
    let reset_claw_shortcut_btn: gtk::Button = builder.object("reset_claw_shortcut_btn").unwrap();
    let group_claw_sounds: adw::PreferencesGroup = builder.object("group_claw_sounds").unwrap();
    let group_claw_identity: adw::PreferencesGroup = builder.object("group_claw_identity").unwrap();
    let group_claw_ui_behavior: adw::PreferencesGroup =
        builder.object("group_claw_ui_behavior").unwrap();
    let group_claw_ui_shortcuts: adw::PreferencesGroup =
        builder.object("group_claw_ui_shortcuts").unwrap();

    let is_syncing = Rc::new(Cell::new(false));
    let initial = Settings::load();

    // Configure searchable currency dropdown
    let currency_labels: Vec<&str> = Currency::all().iter().map(|c| c.label()).collect();
    let currency_list = gtk::StringList::new(&currency_labels);
    let expr = gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<&gtk::Expression>,
        "string",
    );
    currency_combo.set_expression(Some(&expr));
    currency_combo.set_enable_search(true);
    currency_combo.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
    currency_combo.set_model(Some(&currency_list));

    let currency_to_idx = |c: Currency| -> u32 {
        u32::try_from(Currency::all().iter().position(|&item| item == c).unwrap_or(0)).unwrap_or(0)
    };

    // Initial sync
    {
        let scale_val = f64::from(initial.overlay_font_scale.clamp(60, 220));
        let scale_adj = gtk::Adjustment::new(scale_val, 60.0, 220.0, 10.0, 20.0, 0.0);
        overlay_font_scale_spin.set_adjustment(Some(&scale_adj));
        overlay_font_scale_spin.set_value(scale_val);
        currency_combo.set_selected(currency_to_idx(initial.currency));
        claw_msgbar_shortcut_entry.set_text(&initial.claw_msgbar_shortcut);
    }

    {
        let is_syncing = is_syncing.clone();
        overlay_font_scale_spin.connect_value_notify(move |row: &adw::SpinRow| {
            if is_syncing.get() {
                return;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let val = (row.value().round() as u32).clamp(60, 220);
            Settings::update(|s| {
                s.overlay_font_scale = val;
            });
        });
    }

    {
        let is_syncing = is_syncing.clone();
        currency_combo.connect_selected_notify(move |row: &adw::ComboRow| {
            if is_syncing.get() {
                return;
            }
            let idx = row.selected() as usize;
            let Some(&currency) = Currency::all().get(idx) else {
                return;
            };
            Settings::update(|s| {
                s.currency = currency;
            });
        });
    }

    macro_rules! bind_bool_switch {
        ($widget:ident, $field:ident) => {{
            $widget.set_active(initial.$field);
            let is_syncing = is_syncing.clone();
            $widget.connect_active_notify(move |row: &adw::SwitchRow| {
                if is_syncing.get() {
                    return;
                }
                let active = row.is_active();
                Settings::update(|s| {
                    s.$field = active;
                });
            });
        }};
    }

    bind_bool_switch!(overlay_bottom_gap_switch, overlay_bottom_gap);
    bind_bool_switch!(always_show_code_blocks_switch, always_show_code_blocks);
    bind_bool_switch!(enable_tips_switch, enable_tips);
    bind_bool_switch!(enable_timer_sounds_switch, enable_timer_sounds);
    bind_bool_switch!(enable_task_sounds_switch, enable_task_sounds);
    bind_bool_switch!(hide_character_badge_switch, hide_character_badge);

    // Live syntax validation on accelerator entry
    claw_msgbar_shortcut_entry.connect_changed(|entry| {
        if is_valid_accelerator(&entry.text()) {
            entry.remove_css_class("error");
            entry.set_tooltip_text(None);
        } else {
            entry.add_css_class("error");
            entry.set_tooltip_text(Some(
                "Invalid GTK accelerator (examples: <Ctrl>slash, <Alt>space, F12)",
            ));
        }
    });

    let save_shortcut = {
        let is_syncing = is_syncing.clone();
        Rc::new(move |entry: &gtk::Entry, explicit_activate: bool| {
            if is_syncing.get() {
                return;
            }
            let val = entry.text().trim().to_string();
            if !is_valid_accelerator(&val) {
                if explicit_activate {
                    entry.add_css_class("error");
                    show_toast(
                        entry,
                        "Invalid shortcut format. Try e.g. <Ctrl>slash or <Alt>space.",
                    );
                } else {
                    // Revert unsaved invalid input on focus-out / unmap so UI matches persisted state
                    is_syncing.set(true);
                    entry.set_text(&Settings::load().claw_msgbar_shortcut);
                    entry.remove_css_class("error");
                    entry.set_tooltip_text(None);
                    is_syncing.set(false);
                }
                return;
            }
            entry.remove_css_class("error");
            Settings::update(|s| {
                s.claw_msgbar_shortcut.clone_from(&val);
            });
        })
    };

    {
        let save_sc = save_shortcut.clone();
        claw_msgbar_shortcut_entry.connect_activate(move |entry| save_sc(entry, true));
    }
    {
        let focus_out = gtk::EventControllerFocus::new();
        let save_sc = save_shortcut.clone();
        let entry_fo = claw_msgbar_shortcut_entry.clone();
        focus_out.connect_leave(move |_| save_sc(&entry_fo, false));
        claw_msgbar_shortcut_entry.add_controller(focus_out);
    }
    {
        let save_sc = save_shortcut;
        claw_msgbar_shortcut_entry.connect_unmap(move |entry| save_sc(entry, false));
    }

    {
        let s_entry = claw_msgbar_shortcut_entry.clone();
        let is_syncing = is_syncing.clone();
        reset_claw_shortcut_btn.connect_clicked(move |_| {
            let default_shortcut = "<Ctrl>slash";
            is_syncing.set(true);
            s_entry.set_text(default_shortcut);
            s_entry.remove_css_class("error");
            s_entry.set_tooltip_text(None);
            is_syncing.set(false);

            Settings::update(|s| {
                default_shortcut.clone_into(&mut s.claw_msgbar_shortcut);
            });
        });
    }

    let s_timer = SearchRow::new(&enable_timer_sounds_switch, "timer sounds reminder");
    let s_task = SearchRow::new(&enable_task_sounds_switch, "task sounds finishing");
    let s_badge = SearchRow::new(&hide_character_badge_switch, "hide character badge identity");
    let s_gap = SearchRow::new(&overlay_bottom_gap_switch, "bottom space gap drawer margin");
    let s_code = SearchRow::new(
        &always_show_code_blocks_switch,
        "code diff blocks expand collapse trace ctrl shift d",
    );
    let s_tips = SearchRow::new(&enable_tips_switch, "tips usage hints rotating help");
    let s_scale = SearchRow::new(&overlay_font_scale_spin, "overlay font scale zoom percentage");
    let s_curr = SearchRow::new(
        &currency_combo,
        "cost currency spend usd eur cny inr brl dollar euro yuan rupee real",
    );
    let s_accel = SearchRow::new(&row_claw_shortcut, "shortcut accelerator message bar keybinding");
    let s_reset = SearchRow::new(&row_reset_claw_shortcut, "reset default claw shortcut");

    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);

        overlay_bottom_gap_switch.set_active(s.overlay_bottom_gap);
        always_show_code_blocks_switch.set_active(s.always_show_code_blocks);
        enable_tips_switch.set_active(s.enable_tips);

        let scale_val = f64::from(s.overlay_font_scale.clamp(60, 220));
        if (overlay_font_scale_spin.value() - scale_val).abs() > 1e-6 {
            overlay_font_scale_spin.set_value(scale_val);
        }

        let target_curr = currency_to_idx(s.currency);
        if currency_combo.selected() != target_curr {
            currency_combo.set_selected(target_curr);
        }

        enable_timer_sounds_switch.set_active(s.enable_timer_sounds);
        enable_task_sounds_switch.set_active(s.enable_task_sounds);
        hide_character_badge_switch.set_active(s.hide_character_badge);

        if !claw_msgbar_shortcut_entry.has_focus()
            && claw_msgbar_shortcut_entry.text() != s.claw_msgbar_shortcut
        {
            claw_msgbar_shortcut_entry.set_text(&s.claw_msgbar_shortcut);
            claw_msgbar_shortcut_entry.remove_css_class("error");
            claw_msgbar_shortcut_entry.set_tooltip_text(None);
        }

        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let behavior = s_gap.apply(query)
            | s_code.apply(query)
            | s_tips.apply(query)
            | s_scale.apply(query)
            | s_curr.apply(query);
        let sounds = s_timer.apply(query) | s_task.apply(query);
        let identity = s_badge.apply(query);
        let shortcuts = s_accel.apply(query) | s_reset.apply(query);

        group_claw_ui_behavior.set_visible(behavior);
        group_claw_sounds.set_visible(sounds);
        group_claw_identity.set_visible(identity);
        group_claw_ui_shortcuts.set_visible(shortcuts);

        behavior || sounds || identity || shortcuts
    });

    (sync, filter)
}
