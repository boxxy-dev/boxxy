use crate::config::{ImagePreviewTrigger, Settings};
use crate::ui_utils::{PageHooks, SearchRow};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_previews_page(builder: &gtk::Builder) -> PageHooks {
    let image_preview_trigger_combo: adw::ComboRow =
        builder.object("image_preview_trigger_combo").unwrap();
    let preview_max_width_spin: adw::SpinRow = builder.object("preview_max_width_spin").unwrap();
    let preview_max_height_spin: adw::SpinRow = builder.object("preview_max_height_spin").unwrap();
    let group_image_previews: adw::PreferencesGroup =
        builder.object("group_image_previews").unwrap();

    let is_syncing = Rc::new(Cell::new(false));
    let initial = Settings::load();

    let preview_triggers_list =
        gtk::StringList::new(&["Disabled", "On Click (Shift+Click)", "On Hover"]);
    image_preview_trigger_combo.set_model(Some(&preview_triggers_list));

    let trigger_to_idx = |t: ImagePreviewTrigger| -> u32 {
        match t {
            ImagePreviewTrigger::None => 0,
            ImagePreviewTrigger::Click => 1,
            ImagePreviewTrigger::Hover => 2,
        }
    };
    image_preview_trigger_combo.set_selected(trigger_to_idx(initial.image_preview_trigger));

    {
        let is_syncing = is_syncing.clone();
        image_preview_trigger_combo.connect_selected_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let trigger = match row.selected() {
                0 => ImagePreviewTrigger::None,
                1 => ImagePreviewTrigger::Click,
                2 => ImagePreviewTrigger::Hover,
                _ => return,
            };
            Settings::update(|s| {
                s.image_preview_trigger = trigger;
            });
        });
    }

    let preview_max_width_adj = gtk::Adjustment::new(
        f64::from(initial.preview_max_width),
        90.0,
        600.0,
        10.0,
        50.0,
        0.0,
    );
    preview_max_width_spin.set_adjustment(Some(&preview_max_width_adj));
    preview_max_width_spin.set_value(f64::from(initial.preview_max_width));
    {
        let is_syncing = is_syncing.clone();
        preview_max_width_spin.connect_value_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            let v = row.value().round() as i32;
            Settings::update(|s| {
                s.preview_max_width = v;
            });
        });
    }

    let preview_max_height_adj = gtk::Adjustment::new(
        f64::from(initial.preview_max_height),
        90.0,
        600.0,
        10.0,
        50.0,
        0.0,
    );
    preview_max_height_spin.set_adjustment(Some(&preview_max_height_adj));
    preview_max_height_spin.set_value(f64::from(initial.preview_max_height));
    {
        let is_syncing = is_syncing.clone();
        preview_max_height_spin.connect_value_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            let v = row.value().round() as i32;
            Settings::update(|s| {
                s.preview_max_height = v;
            });
        });
    }

    let s_trigger = SearchRow::new(&image_preview_trigger_combo, "image previews popup hyperlink");
    let s_width = SearchRow::new(&preview_max_width_spin, "maximum width px");
    let s_height = SearchRow::new(&preview_max_height_spin, "maximum height px");

    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);
        let idx = trigger_to_idx(s.image_preview_trigger);
        if image_preview_trigger_combo.selected() != idx {
            image_preview_trigger_combo.set_selected(idx);
        }
        if (preview_max_width_spin.value() - f64::from(s.preview_max_width)).abs() > 1e-6 {
            preview_max_width_spin.set_value(f64::from(s.preview_max_width));
        }
        if (preview_max_height_spin.value() - f64::from(s.preview_max_height)).abs() > 1e-6 {
            preview_max_height_spin.set_value(f64::from(s.preview_max_height));
        }
        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let p_desc = query.is_empty()
            || "use --hyperlink for file previews ensure cli tools eza ls path copying"
                .contains(query);
        let p_rows = s_trigger.apply(query) | s_width.apply(query) | s_height.apply(query);
        let visible = p_desc || p_rows;
        group_image_previews.set_visible(visible);
        visible
    });

    (sync, filter)
}
