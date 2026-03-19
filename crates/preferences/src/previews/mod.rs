use crate::config::Settings;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

fn add_class_to_description_label(widget: &gtk::Widget) {
    // We look for labels that are likely to be descriptions (they are dim and small by default in Adwaita)
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        // Only target if it contains the hyperlink text
        if label.text().contains("--hyperlink") {
            label.add_css_class("caption");
            return;
        }
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        add_class_to_description_label(&c);
        child = c.next_sibling();
    }
}

pub fn setup_previews_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let image_preview_trigger_combo: adw::ComboRow =
        builder.object("image_preview_trigger_combo").unwrap();
    let preview_max_width_spin: adw::SpinRow = builder.object("preview_max_width_spin").unwrap();
    let preview_max_height_spin: adw::SpinRow = builder.object("preview_max_height_spin").unwrap();
    let group_image_previews: adw::PreferencesGroup =
        builder.object("group_image_previews").unwrap();

    // Style the group description as a caption
    add_class_to_description_label(group_image_previews.upcast_ref());

    let preview_triggers_list =
        gtk::StringList::new(&["Disabled", "On Click (Shift+Click)", "On Hover"]);
    image_preview_trigger_combo.set_model(Some(&preview_triggers_list));
    let trigger_idx = match settings_rc.borrow().image_preview_trigger {
        crate::config::ImagePreviewTrigger::None => 0,
        crate::config::ImagePreviewTrigger::Click => 1,
        crate::config::ImagePreviewTrigger::Hover => 2,
    };
    image_preview_trigger_combo.set_selected(trigger_idx);
    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    image_preview_trigger_combo.connect_selected_notify(move |row| {
        let trigger = match row.selected() {
            0 => crate::config::ImagePreviewTrigger::None,
            1 => crate::config::ImagePreviewTrigger::Click,
            2 => crate::config::ImagePreviewTrigger::Hover,
            _ => return,
        };
        let mut s = s_rc.borrow_mut();
        if s.image_preview_trigger != trigger {
            s.image_preview_trigger = trigger;
            s.save();
            cb(s.clone());
        }
    });

    let preview_max_width_adj = gtk::Adjustment::new(
        settings_rc.borrow().preview_max_width as f64,
        90.0,
        600.0,
        10.0,
        50.0,
        0.0,
    );
    preview_max_width_spin.set_adjustment(Some(&preview_max_width_adj));
    preview_max_width_spin.set_value(settings_rc.borrow().preview_max_width as f64);
    let s_rc2 = settings_rc.clone();
    let cb2 = on_change.clone();
    preview_max_width_spin.connect_value_notify(move |row| {
        let mut s = s_rc2.borrow_mut();
        let v = row.value() as i32;
        if s.preview_max_width != v {
            s.preview_max_width = v;
            s.save();
            cb2(s.clone());
        }
    });

    let preview_max_height_adj = gtk::Adjustment::new(
        settings_rc.borrow().preview_max_height as f64,
        90.0,
        600.0,
        10.0,
        50.0,
        0.0,
    );
    preview_max_height_spin.set_adjustment(Some(&preview_max_height_adj));
    preview_max_height_spin.set_value(settings_rc.borrow().preview_max_height as f64);
    let s_rc3 = settings_rc.clone();
    let cb3 = on_change.clone();
    preview_max_height_spin.connect_value_notify(move |row| {
        let mut s = s_rc3.borrow_mut();
        let v = row.value() as i32;
        if s.preview_max_height != v {
            s.preview_max_height = v;
            s.save();
            cb3(s.clone());
        }
    });

    let image_preview_trigger_combo_clone = image_preview_trigger_combo.clone();
    let preview_max_width_spin_clone = preview_max_width_spin.clone();
    let preview_max_height_spin_clone = preview_max_height_spin.clone();
    let group_image_previews_clone = group_image_previews.clone();

    Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let p1 = query.is_empty()
            || "use --hyperlink for file previews ensure cli tools eza ls path copying"
                .contains(query);
        let p2 = match_row(
            image_preview_trigger_combo_clone.upcast_ref(),
            "image previews display small popup",
        );
        let p3 = match_row(
            preview_max_width_spin_clone.upcast_ref(),
            "maximum width px",
        );
        let p4 = match_row(
            preview_max_height_spin_clone.upcast_ref(),
            "maximum height px",
        );

        group_image_previews_clone.set_visible(p1 || p2 || p3 || p4);
        group_image_previews_clone.is_visible()
    })
}
