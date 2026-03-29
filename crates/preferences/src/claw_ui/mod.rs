use crate::config::{CLAW_HEIGHT_BOUNDS, CLAW_WIDTH_BOUNDS, Settings};
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
    let claw_popover_width_spin: adw::SpinRow = builder.object("claw_popover_width_spin").unwrap();
    let claw_popover_max_height_spin: adw::SpinRow =
        builder.object("claw_popover_max_height_spin").unwrap();
    let claw_msgbar_shortcut_entry: gtk::Entry =
        builder.object("claw_msgbar_shortcut_entry").unwrap();
    let reset_claw_dimensions_btn: gtk::Button =
        builder.object("reset_claw_dimensions_btn").unwrap();
    let reset_claw_shortcut_btn: gtk::Button = builder.object("reset_claw_shortcut_btn").unwrap();
    let group_claw_ui_dimensions: adw::PreferencesGroup =
        builder.object("group_claw_ui_dimensions").unwrap();
    let group_claw_ui_shortcuts: adw::PreferencesGroup =
        builder.object("group_claw_ui_shortcuts").unwrap();

    // Guard to prevent re-entrancy panics during synchronous UI updates
    let is_updating = Rc::new(Cell::new(false));

    let width_adj: gtk::Adjustment = builder.object("claw_popover_width_adj").unwrap();
    width_adj.set_lower(CLAW_WIDTH_BOUNDS.min as f64);
    width_adj.set_upper(CLAW_WIDTH_BOUNDS.max as f64);
    width_adj.set_step_increment(20.0);
    width_adj.set_page_increment(100.0);

    let height_adj: gtk::Adjustment = builder.object("claw_popover_max_height_adj").unwrap();
    height_adj.set_lower(CLAW_HEIGHT_BOUNDS.min as f64);
    height_adj.set_upper(CLAW_HEIGHT_BOUNDS.max as f64);
    height_adj.set_step_increment(20.0);
    height_adj.set_page_increment(100.0);

    // Initial sync
    {
        let s = settings_rc.borrow();
        claw_popover_width_spin.set_value(s.claw_popover_width as f64);
        claw_popover_max_height_spin.set_value(s.claw_popover_max_height as f64);
        claw_msgbar_shortcut_entry.set_text(&s.claw_msgbar_shortcut);
    }

    // Connect signals
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let is_up = is_updating.clone();
    claw_popover_width_spin.connect_value_notify(move |row: &adw::SpinRow| {
        if is_up.get() {
            return;
        }
        let val = row.value() as i32;
        let mut s = s_rc.borrow_mut();
        if s.claw_popover_width != val {
            s.claw_popover_width = val;
            s.save();
            cb(s.clone());
        }
    });

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let is_up = is_updating.clone();
    claw_popover_max_height_spin.connect_value_notify(move |row: &adw::SpinRow| {
        if is_up.get() {
            return;
        }
        let val = row.value() as i32;
        let mut s = s_rc.borrow_mut();
        if s.claw_popover_max_height != val {
            s.claw_popover_max_height = val;
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

    // Reset Buttons
    let w_spin = claw_popover_width_spin.clone();
    let h_spin = claw_popover_max_height_spin.clone();
    let is_up_dim = is_updating.clone();
    let s_rc_dim = settings_rc.clone();
    let cb_dim = on_change.clone();
    reset_claw_dimensions_btn.connect_clicked(move |_| {
        is_up_dim.set(true);
        w_spin.set_value(CLAW_WIDTH_BOUNDS.default as f64);
        h_spin.set_value(CLAW_HEIGHT_BOUNDS.default as f64);
        is_up_dim.set(false);

        let mut s = s_rc_dim.borrow_mut();
        if s.claw_popover_width != CLAW_WIDTH_BOUNDS.default
            || s.claw_popover_max_height != CLAW_HEIGHT_BOUNDS.default
        {
            s.claw_popover_width = CLAW_WIDTH_BOUNDS.default;
            s.claw_popover_max_height = CLAW_HEIGHT_BOUNDS.default;
            s.save();
            cb_dim(s.clone());
        }
    });

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

    let group_claw_ui_dimensions_clone = group_claw_ui_dimensions.clone();
    let group_claw_ui_shortcuts_clone = group_claw_ui_shortcuts.clone();
    let claw_popover_width_spin_clone = claw_popover_width_spin.clone();
    let claw_popover_max_height_spin_clone = claw_popover_max_height_spin.clone();
    let claw_msgbar_shortcut_entry_clone = claw_msgbar_shortcut_entry.clone();
    let reset_claw_dimensions_btn_clone = reset_claw_dimensions_btn.clone();
    let reset_claw_shortcut_btn_clone = reset_claw_shortcut_btn.clone();

    Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let w = match_row(
            claw_popover_width_spin_clone.upcast_ref(),
            "width maximum claw popover size",
        );
        let h = match_row(
            claw_popover_max_height_spin_clone.upcast_ref(),
            "height maximum claw popover size",
        );
        let reset_dim = match_row(
            reset_claw_dimensions_btn_clone
                .parent()
                .unwrap()
                .upcast_ref(),
            "reset default claw ui dimensions size",
        );

        let s_accel = match_row(
            claw_msgbar_shortcut_entry_clone.upcast_ref(),
            "shortcut accelerator message bar claw keybinding",
        );
        let reset_shortcut = match_row(
            reset_claw_shortcut_btn_clone.parent().unwrap().upcast_ref(),
            "reset default claw ui shortcut keybinding",
        );

        let dim_visible = w || h || reset_dim;
        let shortcut_visible = s_accel || reset_shortcut;

        group_claw_ui_dimensions_clone.set_visible(dim_visible);
        group_claw_ui_shortcuts_clone.set_visible(shortcut_visible);

        dim_visible || shortcut_visible
    })
}
