use crate::ui_utils::PageFilter;
use adw::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;

pub(crate) fn populate_shortcuts_page(page: &adw::PreferencesPage) -> PageFilter {
    let categories = boxxy_keybindings::get_shortcuts_by_category();
    let mut elements: Vec<(adw::PreferencesGroup, Vec<(adw::ActionRow, String)>)> = Vec::new();

    for category in categories {
        let group = adw::PreferencesGroup::builder()
            .title(glib::markup_escape_text(category.name))
            .build();

        let mut rows = Vec::with_capacity(category.items.len());
        let category_lower = category.name.to_lowercase();

        for (title, keybinding) in category.items {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(title))
                .build();

            let shortcut_label = adw::ShortcutLabel::new(keybinding.trigger);
            shortcut_label.set_valign(gtk::Align::Center);

            row.add_suffix(&shortcut_label);
            group.add(&row);

            let haystack = format!(
                "{} {} {}",
                category_lower,
                title.to_lowercase(),
                keybinding.trigger.to_lowercase()
            );
            rows.push((row, haystack));
        }

        page.add(&group);
        elements.push((group, rows));
    }

    Box::new(move |query: &str| {
        let mut page_visible = false;
        for (group, rows) in &elements {
            let mut group_visible = false;
            for (row, haystack) in rows {
                let matched = query.is_empty() || haystack.contains(query);
                row.set_visible(matched);
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
