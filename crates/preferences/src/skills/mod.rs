use crate::config::Settings;
use crate::ui_utils::{PageHooks, bind_mapped_poller, open_directory};
use adw::prelude::*;
use boxxy_claw_protocol::skill_loader::{
    SkillSummary, discover_skill_summaries, get_skills_dir, is_skill_disabled,
};
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

struct SkillRowHandle {
    slug: String,
    name: String,
    row: adw::ActionRow,
    switch: Option<gtk::Switch>,
}

fn open_skills_folder(widget: &impl IsA<gtk::Widget>) {
    open_directory(
        widget,
        get_skills_dir(),
        Some("Changes in the skills folder apply automatically."),
    );
}

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_skills_page(builder: &gtk::Builder) -> PageHooks {
    let page: adw::PreferencesPage = builder.object("page_skills").unwrap();

    let skills_group = adw::PreferencesGroup::new();
    skills_group.set_title("Skills");
    skills_group.set_description(Some(
        "Reusable markdown instructions that teach agents specialized workflows.",
    ));
    page.add(&skills_group);

    let manage_group = adw::PreferencesGroup::new();
    manage_group.set_title("Manage");
    manage_group.set_visible(false);

    let open_row = adw::ActionRow::new();
    open_row.set_title("Open Skills Folder");
    open_row.set_subtitle("Create new skills or remove existing ones in your file manager");
    open_row.set_activatable(true);
    let folder_icon = gtk::Image::from_icon_name("folder-open-symbolic");
    folder_icon.set_valign(gtk::Align::Center);
    open_row.add_suffix(&folder_icon);
    open_row.connect_activated(open_skills_folder);
    manage_group.add(&open_row);
    page.add(&manage_group);

    let is_syncing = Rc::new(Cell::new(false));
    let added_rows: Rc<RefCell<Vec<SkillRowHandle>>> = Rc::new(RefCell::new(Vec::new()));
    let last_summaries: Rc<RefCell<Option<Vec<SkillSummary>>>> = Rc::new(RefCell::new(None));
    let has_skills = Rc::new(Cell::new(false));
    let scan_in_flight = Rc::new(Cell::new(false));

    let refresh_skills: Rc<dyn Fn(bool)> = {
        let skills_group = skills_group.clone();
        let manage_group = manage_group.clone();
        let added_rows = added_rows.clone();
        let last_summaries = last_summaries.clone();
        let has_skills = has_skills.clone();
        let is_syncing = is_syncing.clone();

        Rc::new(move |force: bool| {
            if scan_in_flight.get() {
                return;
            }
            scan_in_flight.set(true);

            let skills_group = skills_group.clone();
            let manage_group = manage_group.clone();
            let added_rows = added_rows.clone();
            let last_summaries = last_summaries.clone();
            let has_skills = has_skills.clone();
            let is_syncing = is_syncing.clone();
            let scan_in_flight = scan_in_flight.clone();

            glib::spawn_future_local(async move {
                let summaries = boxxy_sys_utils::runtime()
                    .spawn_blocking(discover_skill_summaries)
                    .await
                    .unwrap_or_default();
                scan_in_flight.set(false);

                if !force
                    && last_summaries
                        .borrow()
                        .as_ref()
                        .is_some_and(|prev| prev == &summaries)
                {
                    return;
                }
                *last_summaries.borrow_mut() = Some(summaries.clone());
                has_skills.set(!summaries.is_empty());

                for old in added_rows.borrow_mut().drain(..) {
                    skills_group.remove(&old.row);
                }

                if summaries.is_empty() {
                    manage_group.set_visible(false);

                    let add_row = adw::ActionRow::new();
                    add_row.set_title("Add a Skill");
                    add_row.set_subtitle(
                        "Drop a skill folder (with SKILL.md) or .md file into the skills folder",
                    );
                    add_row.set_activatable(true);

                    let add_icon = gtk::Image::from_icon_name("list-add-symbolic");
                    add_icon.set_valign(gtk::Align::Center);
                    add_row.add_suffix(&add_icon);
                    add_row.connect_activated(open_skills_folder);

                    skills_group.add(&add_row);
                    added_rows.borrow_mut().push(SkillRowHandle {
                        slug: String::new(),
                        name: String::new(),
                        row: add_row,
                        switch: None,
                    });
                } else {
                    manage_group.set_visible(true);

                    let disabled_snapshot = Settings::load().disabled_skills.clone();
                    for skill in summaries {
                        let row = adw::ActionRow::new();
                        row.set_title(&glib::markup_escape_text(&skill.name));
                        row.set_subtitle(&glib::markup_escape_text(&skill.description));

                        let enabled =
                            !is_skill_disabled(&disabled_snapshot, &skill.slug, &skill.name);
                        let switch = gtk::Switch::builder()
                            .active(enabled)
                            .valign(gtk::Align::Center)
                            .build();

                        let slug = skill.slug.clone();
                        let name = skill.name.clone();
                        let is_syncing_sw = is_syncing.clone();
                        switch.connect_active_notify(move |sw| {
                            if is_syncing_sw.get() {
                                return;
                            }
                            let is_enabled = sw.is_active();
                            let slug = slug.clone();
                            let name = name.clone();
                            Settings::update(|s| {
                                if is_enabled {
                                    s.disabled_skills.retain(|d| d != &slug && d != &name);
                                } else if !is_skill_disabled(&s.disabled_skills, &slug, &name) {
                                    s.disabled_skills.push(slug.clone());
                                }
                            });
                        });

                        row.add_suffix(&switch);
                        row.set_activatable_widget(Some(&switch));
                        skills_group.add(&row);
                        added_rows.borrow_mut().push(SkillRowHandle {
                            slug: skill.slug,
                            name: skill.name,
                            row,
                            switch: Some(switch),
                        });
                    }
                }
            });
        })
    };

    {
        let on_map = refresh_skills.clone();
        let on_tick = refresh_skills;
        bind_mapped_poller(
            &page,
            Duration::from_millis(1000),
            move || on_map(false),
            move || on_tick(false),
        );
    }

    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);
        for item in added_rows.borrow().iter() {
            if let Some(ref sw) = item.switch {
                let enabled = !is_skill_disabled(&s.disabled_skills, &item.slug, &item.name);
                if sw.is_active() != enabled {
                    sw.set_active(enabled);
                }
            }
        }
        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let matches = query.is_empty()
            || "skills skill md workflow agent instructions toolbox enable disable open folder add"
                .contains(query)
            || last_summaries.borrow().as_ref().is_some_and(|list| {
                list.iter().any(|s| {
                    s.name.to_lowercase().contains(query)
                        || s.description.to_lowercase().contains(query)
                        || s.slug.to_lowercase().contains(query)
                })
            });
        skills_group.set_visible(matches);
        manage_group.set_visible(matches && has_skills.get());
        matches
    });

    (sync, filter)
}
