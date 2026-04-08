use crate::config::Settings;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

fn add_class_to_title_label(widget: &gtk::Widget, title: &str) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>()
        && label.text() == title
    {
        label.add_css_class("status-title");
        return;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        add_class_to_title_label(&c, title);
        child = c.next_sibling();
    }
}

pub fn setup_apis_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let dynamic_apis_group: adw::PreferencesGroup = builder.object("dynamic_apis_group").unwrap();
    let group_search_engines: adw::PreferencesGroup = builder.object("group_search_engines").unwrap();
    let tavily_api_key_entry: adw::PasswordEntryRow = builder.object("tavily_api_key_entry").unwrap();
    let native_search_switch: adw::SwitchRow = builder.object("native_search_switch").unwrap();
    let ollama_base_url_entry: adw::EntryRow = builder.object("ollama_base_url_entry").unwrap();
    let group_ollama_api: adw::PreferencesGroup = builder.object("group_ollama_api").unwrap();
    let group_model_status: adw::PreferencesGroup = builder.object("group_model_status").unwrap();
    let model_status_row: adw::ActionRow = builder.object("model_status_row").unwrap();

    let update_model_status = {
        let row = model_status_row.clone();
        let settings = settings_rc.clone();
        move || {
            let s = settings.borrow();
            let is_complete = s.ai_chat_model.is_some() && s.claw_model.is_some();
            let title = if is_complete {
                "All Models are set"
            } else {
                "Models selection is incomplete"
            };

            row.set_title(title);

            if is_complete {
                row.set_subtitle("AI Chat and Boxxy Claw are ready to use.");
                row.remove_css_class("model-status-warning");
                row.add_css_class("model-status-success");
            } else {
                row.set_subtitle("Open Models Selection to set your models.");
                row.remove_css_class("model-status-success");
                row.add_css_class("model-status-warning");
            }

            // Surgical class addition to the internal title label
            add_class_to_title_label(row.upcast_ref(), title);
        }
    };
    update_model_status();

    let providers = boxxy_model_selection::get_providers();
    let mut dynamic_rows = Vec::new();

    for prov in &providers {
        if prov.requires_api_key() {
            let row = adw::PasswordEntryRow::builder()
                .title(format!("{} API Key", prov.name()))
                .build();

            let initial_key = settings_rc
                .borrow()
                .api_keys
                .get(prov.name())
                .cloned()
                .unwrap_or_default();
            row.set_text(&initial_key);

            let prov_name_for_env = prov.name().to_string();
            if initial_key.is_empty() {
                if let Some(_env_key) = Settings::get_env_api_key(&prov_name_for_env) {
                    row.set_title(&format!("{} API Key (Set from environment)", prov.name()));
                }
            }

            let s_rc = settings_rc.clone();
            let cb = on_change.clone();
            let prov_name = prov.name().to_string();
            let update_status = update_model_status.clone();
            let row_clone = row.clone();
            let prov_name_clone = prov.name();
            row.connect_changed(move |entry| {
                let mut settings_to_save = None;
                {
                    let mut s = s_rc.borrow_mut();
                    let new_val = entry.text().to_string();
                    if s.api_keys.get(&prov_name) != Some(&new_val) {
                        let is_empty = new_val.is_empty();
                        s.api_keys.insert(prov_name.clone(), new_val);
                        s.save();
                        settings_to_save = Some(s.clone());

                        if is_empty {
                            if let Some(_env_key) = Settings::get_env_api_key(&prov_name) {
                                row_clone.set_title(&format!(
                                    "{} API Key (Set from environment)",
                                    prov_name_clone
                                ));
                            }
                        } else {
                            row_clone.set_title(&format!("{} API Key", prov_name_clone));
                        }
                    }
                }

                if let Some(s) = settings_to_save {
                    update_status();
                    cb(s);
                }
            });

            dynamic_apis_group.add(&row);
            dynamic_rows.push((prov.name().to_string(), row));
        }
    }

    tavily_api_key_entry.set_text(
        &settings_rc
            .borrow()
            .api_keys
            .get("Tavily")
            .cloned()
            .unwrap_or_default(),
    );
    let s_rc_tavily = settings_rc.clone();
    let cb_tavily = on_change.clone();
    let update_status_tavily = update_model_status.clone();
    tavily_api_key_entry.connect_changed(move |entry| {
        let mut settings_to_save = None;
        {
            let mut s = s_rc_tavily.borrow_mut();
            let new_val = entry.text().to_string();
            if s.api_keys.get("Tavily") != Some(&new_val) {
                s.api_keys.insert("Tavily".to_string(), new_val);
                s.save();
                settings_to_save = Some(s.clone());
            }
        }

        if let Some(s) = settings_to_save {
            update_status_tavily();
            cb_tavily(s);
        }
    });

    ollama_base_url_entry.set_text(&settings_rc.borrow().ollama_base_url);
    let s_rc3 = settings_rc.clone();
    let cb3 = on_change.clone();
    let update_status3 = update_model_status.clone();
    ollama_base_url_entry.connect_changed(move |entry| {
        let mut settings_to_save = None;
        {
            let mut s = s_rc3.borrow_mut();
            if s.ollama_base_url != entry.text().as_str() {
                s.ollama_base_url = entry.text().to_string();
                s.save();
                settings_to_save = Some(s.clone());
            }
        }

        if let Some(s) = settings_to_save {
            update_status3();
            cb3(s);
        }
    });

    let ollama_base_url_entry_clone = ollama_base_url_entry.clone();
    let model_status_row_clone = model_status_row.clone();

    Box::new(move |query: &str| {
        update_model_status();
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let mut any_visible = false;

        let models_visible = match_row(
            model_status_row_clone.upcast_ref(),
            "open models selection ai claw memories status openai",
        );
        group_model_status.set_visible(models_visible);

        for (name, row) in &dynamic_rows {
            if match_row(row.upcast_ref(), &format!("{} api key", name)) {
                any_visible = true;
            }
        }

        let ollama_visible = match_row(ollama_base_url_entry_clone.upcast_ref(), "base url ollama");
        let search_tavily = match_row(
            tavily_api_key_entry.upcast_ref(),
            "tavily api key search engines",
        );
        let search_native = match_row(
            native_search_switch.upcast_ref(),
            "use native model search engines built in gemini gpt claude not implemented",
        );
        
        let search_visible = search_tavily || search_native;
        group_ollama_api.set_visible(ollama_visible);
        group_search_engines.set_visible(search_visible);
        dynamic_apis_group.set_visible(any_visible);

        any_visible || ollama_visible || models_visible || search_visible
    })
}
