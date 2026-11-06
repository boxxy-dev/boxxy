use crate::config::Settings;
use adw::prelude::*;
use boxxy_claw_protocol::skill_loader::{
    SkillSummary, discover_skill_summaries, get_skills_dir, is_skill_disabled,
};
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn open_skills_folder(widget: &impl IsA<gtk::Widget>) {
    let dir = get_skills_dir();
    let _ = std::fs::create_dir_all(&dir);
    let uri = format!("file://{}", dir.display());
    let _ = gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);

    if let Some(overlay) = widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    {
        let toast = adw::Toast::new("Changes in the skills folder apply automatically.");
        toast.set_timeout(3);
        overlay.add_toast(toast);
    }
}

pub fn setup_skills_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
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
    open_row.connect_activated(|row| {
        open_skills_folder(row);
    });
    manage_group.add(&open_row);
    page.add(&manage_group);

    let added_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
    let last_summaries: Rc<RefCell<Option<Vec<SkillSummary>>>> = Rc::new(RefCell::new(None));
    let has_skills = Rc::new(Cell::new(false));

    let refresh_skills: Rc<dyn Fn(bool)> = {
        let skills_group = skills_group.clone();
        let manage_group = manage_group.clone();
        let added_rows = added_rows.clone();
        let last_summaries = last_summaries.clone();
        let has_skills = has_skills.clone();
        let settings_rc = settings_rc.clone();
        let on_change = on_change.clone();

        Rc::new(move |force: bool| {
            let summaries = discover_skill_summaries();
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

            for old_row in added_rows.borrow_mut().drain(..) {
                skills_group.remove(&old_row);
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

                add_row.connect_activated(|row| {
                    open_skills_folder(row);
                });

                skills_group.add(&add_row);
                added_rows.borrow_mut().push(add_row);
            } else {
                manage_group.set_visible(true);

                let disabled_snapshot = settings_rc.borrow().disabled_skills.clone();
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

                    let s_rc = settings_rc.clone();
                    let cb = on_change.clone();
                    let slug = skill.slug.clone();
                    let name = skill.name.clone();
                    switch.connect_active_notify(move |sw| {
                        let is_enabled = sw.is_active();
                        let mut changed = false;
                        let snapshot = {
                            let mut s = s_rc.borrow_mut();
                            if is_enabled {
                                let before = s.disabled_skills.len();
                                s.disabled_skills.retain(|d| d != &slug && d != &name);
                                changed = s.disabled_skills.len() != before;
                            } else if !is_skill_disabled(&s.disabled_skills, &slug, &name) {
                                s.disabled_skills.push(slug.clone());
                                changed = true;
                            }
                            if changed {
                                s.save();
                            }
                            s.clone()
                        };
                        if changed {
                            cb(snapshot);
                        }
                    });

                    row.add_suffix(&switch);
                    row.set_activatable_widget(Some(&switch));
                    skills_group.add(&row);
                    added_rows.borrow_mut().push(row);
                }
            }
        })
    };

    // Lazily populate only when the Skills page becomes mapped/visible, and poll
    // for filesystem changes strictly while the page remains mapped.
    let refresh_on_map = refresh_skills.clone();
    page.connect_map(move |page_widget| {
        refresh_on_map(true);

        let page_weak = page_widget.downgrade();
        let refresh_poll = refresh_on_map.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(1000), move || {
            let Some(p) = page_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !p.is_mapped() {
                return glib::ControlFlow::Break;
            }
            refresh_poll(false);
            glib::ControlFlow::Continue
        });
    });

    let skills_group_filter = skills_group;
    let manage_group_filter = manage_group;
    let has_skills_filter = has_skills;
    let summaries_filter = last_summaries;
    Box::new(move |query: &str| {
        let matches = query.is_empty()
            || "skills skill md workflow agent instructions toolbox enable disable open folder add"
                .contains(query)
            || summaries_filter.borrow().as_ref().is_some_and(|list| {
                list.iter().any(|s| {
                    s.name.to_lowercase().contains(query)
                        || s.description.to_lowercase().contains(query)
                        || s.slug.to_lowercase().contains(query)
                })
            });
        skills_group_filter.set_visible(matches);
        manage_group_filter.set_visible(matches && has_skills_filter.get());
        matches
    })
}
