use crate::config::Settings;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub fn setup_claw_ui_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let maintain_overlay_history_switch: adw::SwitchRow =
        builder.object("maintain_overlay_history_switch").unwrap();
    let enable_timer_sounds_switch: adw::SwitchRow =
        builder.object("enable_timer_sounds_switch").unwrap();
    let enable_task_sounds_switch: adw::SwitchRow =
        builder.object("enable_task_sounds_switch").unwrap();
    let claw_msgbar_shortcut_entry: gtk::Entry =
        builder.object("claw_msgbar_shortcut_entry").unwrap();
    let reset_claw_shortcut_btn: gtk::Button = builder.object("reset_claw_shortcut_btn").unwrap();
    let group_claw_sounds: adw::PreferencesGroup = builder.object("group_claw_sounds").unwrap();
    let group_claw_ui_behavior: adw::PreferencesGroup =
        builder.object("group_claw_ui_behavior").unwrap();
    let group_claw_ui_shortcuts: adw::PreferencesGroup =
        builder.object("group_claw_ui_shortcuts").unwrap();

    // Guard to prevent re-entrancy panics during synchronous UI updates
    let is_updating = Rc::new(Cell::new(false));

    // Initial sync
    {
        let s = settings_rc.borrow();
        maintain_overlay_history_switch.set_active(s.maintain_overlay_history);
        enable_timer_sounds_switch.set_active(s.enable_timer_sounds);
        enable_task_sounds_switch.set_active(s.enable_task_sounds);
        claw_msgbar_shortcut_entry.set_text(&s.claw_msgbar_shortcut);
    }

    // Maintain-overlay-history toggle
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let is_up = is_updating.clone();
    maintain_overlay_history_switch.connect_active_notify(move |row: &adw::SwitchRow| {
        if is_up.get() {
            return;
        }
        let val = row.is_active();
        let mut s = s_rc.borrow_mut();
        if s.maintain_overlay_history != val {
            s.maintain_overlay_history = val;
            s.save();
            cb(s.clone());
        }
    });

    // Enable-timer-sounds toggle
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let is_up = is_updating.clone();
    enable_timer_sounds_switch.connect_active_notify(move |row: &adw::SwitchRow| {
        if is_up.get() {
            return;
        }
        let val = row.is_active();
        let mut s = s_rc.borrow_mut();
        if s.enable_timer_sounds != val {
            s.enable_timer_sounds = val;
            s.save();
            cb(s.clone());
        }
    });

    // Enable-task-sounds toggle
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let is_up = is_updating.clone();
    enable_task_sounds_switch.connect_active_notify(move |row: &adw::SwitchRow| {
        if is_up.get() {
            return;
        }
        let val = row.is_active();
        let mut s = s_rc.borrow_mut();
        if s.enable_task_sounds != val {
            s.enable_task_sounds = val;
            s.save();
            cb(s.clone());
        }
    });

    // Save helper shared by activate and focus-out
    let save_shortcut = {
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let is_up = is_updating.clone();
        Rc::new(move |entry: &gtk::Entry| {
            if is_up.get() {
                return;
            }
            let val = entry.text().to_string();
            let mut s = s_rc.borrow_mut();
            if s.claw_msgbar_shortcut != val {
                s.claw_msgbar_shortcut = val;
                s.save();
                cb(s.clone());
            }
        })
    };

    let save_sc = save_shortcut.clone();
    claw_msgbar_shortcut_entry.connect_activate(move |row| save_sc(row));

    let focus_out = gtk::EventControllerFocus::new();
    let save_sc2 = save_shortcut.clone();
    let entry_fo = claw_msgbar_shortcut_entry.clone();
    focus_out.connect_leave(move |_| save_sc2(&entry_fo));
    claw_msgbar_shortcut_entry.add_controller(focus_out);

    let s_entry = claw_msgbar_shortcut_entry.clone();
    let s_rc_reset = settings_rc.clone();
    let cb_reset = on_change.clone();
    let is_up_shortcut = is_updating.clone();
    reset_claw_shortcut_btn.connect_clicked(move |_| {
        let default_shortcut = "<Ctrl>slash";
        is_up_shortcut.set(true);
        s_entry.set_text(default_shortcut);
        is_up_shortcut.set(false);

        let mut s = s_rc_reset.borrow_mut();
        if s.claw_msgbar_shortcut != default_shortcut {
            s.claw_msgbar_shortcut = default_shortcut.to_string();
            s.save();
            cb_reset(s.clone());
        }
    });

    let group_claw_sounds_clone = group_claw_sounds.clone();
    let group_claw_ui_behavior_clone = group_claw_ui_behavior.clone();
    let group_claw_ui_shortcuts_clone = group_claw_ui_shortcuts.clone();
    let maintain_overlay_history_switch_clone = maintain_overlay_history_switch.clone();
    let enable_timer_sounds_switch_clone = enable_timer_sounds_switch.clone();
    let enable_task_sounds_switch_clone = enable_task_sounds_switch.clone();
    let claw_msgbar_shortcut_entry_clone = claw_msgbar_shortcut_entry.clone();
    let reset_claw_shortcut_btn_clone = reset_claw_shortcut_btn.clone();

    Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let timer_sounds = match_row(
            enable_timer_sounds_switch_clone.upcast_ref(),
            "enable timer sounds scheduled reminder notification finishing",
        );
        let task_sounds = match_row(
            enable_task_sounds_switch_clone.upcast_ref(),
            "enable task sounds agent long-running task finishing",
        );

        let maintain = match_row(
            maintain_overlay_history_switch_clone.upcast_ref(),
            "maintain session history overlay log scrollable conversation",
        );

        let s_accel = match_row(
            claw_msgbar_shortcut_entry_clone.upcast_ref(),
            "shortcut accelerator message bar claw keybinding",
        );
        let reset_shortcut = match_row(
            reset_claw_shortcut_btn_clone.parent().unwrap().upcast_ref(),
            "reset default claw ui shortcut keybinding",
        );

        let behavior_visible = maintain;
        let sounds_visible = timer_sounds || task_sounds;
        let shortcut_visible = s_accel || reset_shortcut;

        group_claw_sounds_clone.set_visible(sounds_visible);
        group_claw_ui_behavior_clone.set_visible(behavior_visible);
        group_claw_ui_shortcuts_clone.set_visible(shortcut_visible);

        behavior_visible || sounds_visible || shortcut_visible
    })
}
