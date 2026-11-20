use crate::config::{Currency, Settings};
use crate::shortcuts::capture_dialog::present_capture_dialog;
use crate::ui_utils::{PageHooks, SearchRow};
use adw::prelude::*;
use boxxy_keybindings::ShortcutId;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

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

    {
        let scale_val = f64::from(initial.overlay_font_scale.clamp(60, 220));
        let scale_adj = gtk::Adjustment::new(scale_val, 60.0, 220.0, 10.0, 20.0, 0.0);
        overlay_font_scale_spin.set_adjustment(Some(&scale_adj));
        overlay_font_scale_spin.set_value(scale_val);
        currency_combo.set_selected(currency_to_idx(initial.currency));
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

    // Live Ask Boxxy-Claw shortcut row synced with `boxxy-keybindings`
    let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    suffix.set_valign(gtk::Align::Center);
    let custom_pill = gtk::Label::builder()
        .label("Custom")
        .valign(gtk::Align::Center)
        .css_classes(["model-badge-custom", "caption"])
        .visible(false)
        .build();
    let chords_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    chords_box.set_valign(gtk::Align::Center);
    let reset_btn = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("Reset to default (Ctrl+/)")
        .css_classes(["flat", "circular"])
        .visible(false)
        .build();
    suffix.append(&custom_pill);
    suffix.append(&chords_box);
    suffix.append(&reset_btn);
    row_claw_shortcut.add_suffix(&suffix);

    {
        let row_weak = row_claw_shortcut.downgrade();
        row_claw_shortcut.connect_activated(move |_| {
            if let Some(r) = row_weak.upgrade() {
                present_capture_dialog(&r, ShortcutId::AskClaw);
            }
        });
    }
    reset_btn.connect_clicked(|_| {
        boxxy_keybindings::reset_action(ShortcutId::AskClaw);
    });

    let refresh_claw_shortcut = {
        let custom_pill = custom_pill;
        let chords_box = chords_box;
        let reset_btn = reset_btn;
        Rc::new(move || {
            let km = boxxy_keybindings::keymap();
            let is_custom = km.is_customized(ShortcutId::AskClaw);
            custom_pill.set_visible(is_custom);
            reset_btn.set_visible(is_custom);

            while let Some(child) = chords_box.first_child() {
                chords_box.remove(&child);
            }
            let chords = km.chords_for(ShortcutId::AskClaw);
            if chords.is_empty() {
                chords_box.append(
                    &gtk::Label::builder()
                        .label("Unbound")
                        .valign(gtk::Align::Center)
                        .css_classes(["shortcut-unbound-pill"])
                        .build(),
                );
            } else {
                for ch in chords {
                    chords_box.append(&adw::ShortcutLabel::new(&ch.to_gtk_accel()));
                }
            }
        })
    };
    refresh_claw_shortcut();

    {
        let refresh = refresh_claw_shortcut;
        let row_weak = row_claw_shortcut.downgrade();
        let mut rx = boxxy_keybindings::KEYBINDINGS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            loop {
                match rx.recv().await {
                    Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
                if row_weak.upgrade().is_none() {
                    break;
                }
                refresh();
            }
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
    let s_accel = SearchRow::new(
        &row_claw_shortcut,
        "shortcut accelerator ask claw message bar keybinding ctrl slash",
    );

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
        let shortcuts = s_accel.apply(query);

        group_claw_ui_behavior.set_visible(behavior);
        group_claw_sounds.set_visible(sounds);
        group_claw_identity.set_visible(identity);
        group_claw_ui_shortcuts.set_visible(shortcuts);

        behavior || sounds || identity || shortcuts
    });

    (sync, filter)
}
