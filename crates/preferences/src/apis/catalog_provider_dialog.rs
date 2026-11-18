use crate::ui_utils::SubpageHeader;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

type AddProviderCallback = Rc<dyn Fn(String, String)>;

#[derive(Clone)]
pub(super) struct CatalogProviderEditorPage {
    apis_stack: gtk::Stack,
    header: SubpageHeader,
    group: adw::PreferencesGroup,
    added_rows: Rc<RefCell<Vec<adw::PasswordEntryRow>>>,
}

impl CatalogProviderEditorPage {
    pub(super) fn new(
        page: &adw::PreferencesPage,
        apis_stack: &gtk::Stack,
        header: &SubpageHeader,
    ) -> Self {
        let group = adw::PreferencesGroup::builder()
            .title("Enter an API key and click Add to enable a provider.")
            .build();

        page.add(&group);

        Self {
            apis_stack: apis_stack.clone(),
            header: header.clone(),
            group,
            added_rows: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub(super) fn open_editor(
        &self,
        unconfigured_providers: Vec<String>,
        on_add: impl Fn(String, String) + 'static,
    ) {
        if unconfigured_providers.is_empty() {
            return;
        }

        let on_add_rc: AddProviderCallback = Rc::new(on_add);

        {
            let mut rows = self.added_rows.borrow_mut();
            for row in rows.drain(..) {
                self.group.remove(&row);
            }
        }

        let mut new_rows = Vec::with_capacity(unconfigured_providers.len());

        for prov_name in unconfigured_providers {
            let row = adw::PasswordEntryRow::builder()
                .use_markup(false)
                .title(format!("{prov_name} API Key"))
                .build();

            let add_btn = gtk::Button::builder()
                .label("Add")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "suggested-action"])
                .sensitive(false)
                .build();

            row.connect_changed({
                let add_btn = add_btn.clone();
                move |entry| {
                    add_btn.set_sensitive(!entry.text().trim().is_empty());
                }
            });

            let commit_row = {
                let row_weak = row.downgrade();
                let group = self.group.clone();
                let rows_rc = self.added_rows.clone();
                let apis_stack = self.apis_stack.clone();
                let header = self.header.clone();
                let on_add = on_add_rc.clone();
                Rc::new(move || {
                    let Some(row) = row_weak.upgrade() else {
                        return;
                    };
                    let key = row.text().trim().to_string();
                    if key.is_empty() {
                        return;
                    }

                    on_add(prov_name.clone(), key);

                    let remaining = {
                        let mut rows = rows_rc.borrow_mut();
                        rows.retain(|r| r != &row);
                        rows.len()
                    };
                    group.remove(&row);

                    if remaining == 0 {
                        apis_stack.set_visible_child_name("list");
                        header.leave("APIs");
                    }
                })
            };

            add_btn.connect_clicked({
                let commit_row = commit_row.clone();
                move |_| {
                    commit_row();
                }
            });

            row.connect_entry_activated(move |_| {
                commit_row();
            });

            row.add_suffix(&add_btn);
            self.group.add(&row);
            new_rows.push(row);
        }

        *self.added_rows.borrow_mut() = new_rows;

        self.header.enter("APIs", "Back to APIs", "Add AI Provider");
        self.apis_stack.set_visible_child_name("catalog_editor");
    }
}
