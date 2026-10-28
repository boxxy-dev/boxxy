use crate::config::{ModelConfigState, Settings};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

mod catalog_provider_dialog;
mod openai_compat_dialog;

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
    let apis_stack: gtk::Stack = builder.object("apis_stack").unwrap();
    let apis_back_btn: gtk::Button = builder.object("apis_back_btn").unwrap();
    let content_title: adw::WindowTitle = builder.object("content_title").unwrap();
    let page_openai_compat_editor: adw::PreferencesPage =
        builder.object("page_openai_compat_editor").unwrap();
    let page_catalog_provider_editor: adw::PreferencesPage =
        builder.object("page_catalog_provider_editor").unwrap();
    let dynamic_apis_group: adw::PreferencesGroup = builder.object("dynamic_apis_group").unwrap();
    let add_catalog_provider_row: adw::ActionRow =
        builder.object("add_catalog_provider_row").unwrap();
    let group_openai_compatible: adw::PreferencesGroup =
        builder.object("group_openai_compatible").unwrap();
    let add_openai_compat_row: adw::ActionRow = builder.object("add_openai_compat_row").unwrap();
    let group_search_engines: adw::PreferencesGroup =
        builder.object("group_search_engines").unwrap();

    let editor_page = openai_compat_dialog::OpenAiCompatEditorPage::new(
        &page_openai_compat_editor,
        &apis_stack,
        &apis_back_btn,
        &content_title,
    );
    let catalog_editor_page = catalog_provider_dialog::CatalogProviderEditorPage::new(
        &page_catalog_provider_editor,
        &apis_stack,
        &apis_back_btn,
        &content_title,
    );
    let tavily_api_key_entry: adw::PasswordEntryRow =
        builder.object("tavily_api_key_entry").unwrap();
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
            let is_complete = s.is_models_selection_complete();
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
                let subtitle = match (
                    s.resolve_model_state(&s.ai_chat_model),
                    s.resolve_model_state(&s.claw_model),
                ) {
                    (ModelConfigState::MissingApiKey { provider_name: p }, _)
                    | (_, ModelConfigState::MissingApiKey { provider_name: p }) => {
                        format!("{} API key is missing.", p)
                    }
                    (ModelConfigState::OllamaOffline, _) | (_, ModelConfigState::OllamaOffline) => {
                        "Ollama is offline. Check that it is running.".to_string()
                    }
                    _ => "Open Models Selection to set your models.".to_string(),
                };
                row.set_subtitle(&subtitle);
                row.remove_css_class("model-status-success");
                row.add_css_class("model-status-warning");
            }

            // Surgical class addition to the internal title label
            add_class_to_title_label(row.upcast_ref(), title);
        }
    };
    update_model_status();

    // --- Configured Catalog Providers Management ---
    let catalog_added_rows =
        Rc::new(RefCell::new(Vec::<(String, adw::PasswordEntryRow)>::new()));
    let render_catalog_list: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

    let render_catalog_impl = {
        let group = dynamic_apis_group.clone();
        let add_row = add_catalog_provider_row.clone();
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let added_rows_inner = catalog_added_rows.clone();
        let render_ref = render_catalog_list.clone();
        let update_status = update_model_status.clone();

        Rc::new(move || {
            let mut rows_vec = added_rows_inner.borrow_mut();
            for (_, row) in rows_vec.drain(..) {
                group.remove(&row);
            }

            let providers = boxxy_model_selection::get_providers();
            let mut unconfigured_count = 0usize;

            for prov in &providers {
                if !prov.requires_api_key() {
                    continue;
                }

                let prov_name = prov.name().to_string();
                let (direct_key, effective_key) = {
                    let s = s_rc.borrow();
                    let direct = s.api_keys.get(&prov_name).cloned().unwrap_or_default();
                    let effective = s.get_effective_api_key(&prov_name);
                    (direct, effective)
                };

                if effective_key.is_empty() {
                    unconfigured_count += 1;
                    continue;
                }

                let title = if direct_key.trim().is_empty() {
                    format!("{} API Key (Set from environment)", prov_name)
                } else {
                    format!("{} API Key", prov_name)
                };

                let row = adw::PasswordEntryRow::builder().title(&title).build();
                row.set_text(&direct_key);

                {
                    let s_rc_inner = s_rc.clone();
                    let cb_inner = cb.clone();
                    let prov_name_inner = prov_name.clone();
                    let prov_name_title = prov_name.clone();
                    let update_status_inner = update_status.clone();
                    let row_clone = row.clone();
                    row.connect_changed(move |entry| {
                        let mut settings_to_save = None;
                        {
                            let mut s = s_rc_inner.borrow_mut();
                            let new_val = entry.text().to_string();
                            if s.api_keys.get(&prov_name_inner) != Some(&new_val) {
                                let is_empty = new_val.trim().is_empty();
                                s.api_keys.insert(prov_name_inner.clone(), new_val);
                                s.check_and_unset_invalid_models();
                                s.save();
                                settings_to_save = Some(s.clone());

                                if is_empty {
                                    if Settings::get_env_api_key(&prov_name_inner).is_some() {
                                        row_clone.set_title(&format!(
                                            "{} API Key (Set from environment)",
                                            prov_name_title
                                        ));
                                    }
                                } else {
                                    row_clone
                                        .set_title(&format!("{} API Key", prov_name_title));
                                }
                            }
                        }

                        if let Some(s) = settings_to_save {
                            update_status_inner();
                            cb_inner(s);
                        }
                    });
                }

                let del_btn = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat", "destructive-action"])
                    .tooltip_text("Remove provider")
                    .build();
                {
                    let s_rc_inner = s_rc.clone();
                    let cb_inner = cb.clone();
                    let render_ref_inner = render_ref.clone();
                    let update_status_inner = update_status.clone();
                    let prov_name_del = prov_name.clone();
                    del_btn.connect_clicked(move |_| {
                        Settings::remove_env_api_key(&prov_name_del);
                        let s_clone = {
                            let mut s = s_rc_inner.borrow_mut();
                            s.api_keys.remove(&prov_name_del);
                            s.check_and_unset_invalid_models();
                            s.save();
                            s.clone()
                        };
                        update_status_inner();
                        cb_inner(s_clone);
                        let render_ref_idle = render_ref_inner.clone();
                        gtk::glib::idle_add_local_once(move || {
                            if let Some(render) = render_ref_idle.borrow().as_ref() {
                                render();
                            }
                        });
                    });
                }
                row.add_suffix(&del_btn);

                group.add(&row);
                rows_vec.push((prov_name, row));
            }

            if unconfigured_count == 0 {
                add_row.set_sensitive(false);
                add_row.set_subtitle("All built-in catalog providers are configured.");
            } else {
                add_row.set_sensitive(true);
                add_row.set_subtitle("Configure a built-in AI provider from the catalog.");
            }
        })
    };
    *render_catalog_list.borrow_mut() = Some(render_catalog_impl.clone());
    render_catalog_impl();

    {
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let render_ref = render_catalog_list.clone();
        let update_status = update_model_status.clone();
        let catalog_editor_page = catalog_editor_page;
        add_catalog_provider_row.connect_activated(move |_| {
            let unconfigured: Vec<String> = {
                let s = s_rc.borrow();
                boxxy_model_selection::get_providers()
                    .into_iter()
                    .filter(|p| p.requires_api_key() && s.get_effective_api_key(p.name()).is_empty())
                    .map(|p| p.name().to_string())
                    .collect()
            };
            if unconfigured.is_empty() {
                return;
            }

            let s_rc_inner = s_rc.clone();
            let cb_inner = cb.clone();
            let render_ref_inner = render_ref.clone();
            let update_status_inner = update_status.clone();

            catalog_editor_page.open_editor(unconfigured, move |provider_name, key| {
                let s_clone = {
                    let mut s = s_rc_inner.borrow_mut();
                    s.api_keys.insert(provider_name, key);
                    s.check_and_unset_invalid_models();
                    s.save();
                    s.clone()
                };
                update_status_inner();
                cb_inner(s_clone);
                let render_ref_idle = render_ref_inner.clone();
                gtk::glib::idle_add_local_once(move || {
                    if let Some(render) = render_ref_idle.borrow().as_ref() {
                        render();
                    }
                });
            });
        });
    }

    // Listen to SETTINGS_EVENT_BUS for real-time model setting changes (e.g., from the model selector dialog or env discovery)
    let settings_rc_bus = settings_rc.clone();
    let update_status_bus = update_model_status.clone();
    let render_catalog_bus = render_catalog_impl.clone();
    let catalog_rows_bus = catalog_added_rows.clone();
    let mut settings_rx = crate::config::SETTINGS_EVENT_BUS.subscribe();
    gtk::glib::spawn_future_local(async move {
        loop {
            let settings = match settings_rx.recv().await {
                Ok(s) => s,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    (*crate::config::Settings::load()).clone()
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            *settings_rc_bus.borrow_mut() = settings;
            update_status_bus();
            let any_focused = catalog_rows_bus
                .borrow()
                .iter()
                .any(|(_, r)| r.has_focus() || r.focus_child().is_some());
            if !any_focused {
                render_catalog_bus();
            }
        }
    });

    // --- OpenAI-Compatible Endpoints Management ---
    let compat_added_rows = Rc::new(RefCell::new(Vec::<adw::ActionRow>::new()));
    let render_compat_list: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

    let render_compat_impl = {
        let group = group_openai_compatible.clone();
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let added_rows_inner = compat_added_rows.clone();
        let render_ref = render_compat_list.clone();
        let update_status = update_model_status.clone();
        let editor_page = editor_page.clone();

        Rc::new(move || {
            let mut rows_vec = added_rows_inner.borrow_mut();
            for row in rows_vec.drain(..) {
                group.remove(&row);
            }

            let endpoints = {
                let s = s_rc.borrow();
                s.openai_compatible_endpoints.clone()
            };

            for (idx, ep) in endpoints.into_iter().enumerate() {
                let model_count = ep.models.len();
                let cost_badge = if ep.zero_cost {
                    " • Free/Subscription"
                } else {
                    ""
                };
                let subtitle = format!(
                    "{} • {} {}{}",
                    ep.base_url,
                    model_count,
                    if model_count == 1 { "model" } else { "models" },
                    cost_badge
                );

                let row = adw::ActionRow::builder()
                    .title(gtk::glib::markup_escape_text(&ep.name))
                    .subtitle(gtk::glib::markup_escape_text(&subtitle))
                    .build();

                let switch = gtk::Switch::builder()
                    .active(ep.enabled)
                    .valign(gtk::Align::Center)
                    .build();

                {
                    let s_rc_inner = s_rc.clone();
                    let cb_inner = cb.clone();
                    let update_status_inner = update_status.clone();
                    let ep_name = ep.name.clone();
                    switch.connect_active_notify(move |sw| {
                        let mut saved = None;
                        {
                            let mut s = s_rc_inner.borrow_mut();
                            if let Some(target) = s
                                .openai_compatible_endpoints
                                .iter_mut()
                                .find(|e| e.name == ep_name)
                                && target.enabled != sw.is_active()
                            {
                                target.enabled = sw.is_active();
                                s.check_and_unset_invalid_models();
                                s.save();
                                saved = Some(s.clone());
                            }
                        }
                        if let Some(s_val) = saved {
                            update_status_inner();
                            cb_inner(s_val);
                        }
                    });
                }
                row.add_suffix(&switch);

                let edit_btn = gtk::Button::builder()
                    .icon_name("document-edit-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                {
                    let s_rc_inner = s_rc.clone();
                    let cb_inner = cb.clone();
                    let ep_clone = ep.clone();
                    let old_name = ep.name.clone();
                    let render_ref_inner = render_ref.clone();
                    let update_status_inner = update_status.clone();
                    let editor_page_edit = editor_page.clone();

                    edit_btn.connect_clicked(move |_| {
                        let initial_key = s_rc_inner
                            .borrow()
                            .api_keys
                            .get(&old_name)
                            .cloned()
                            .unwrap_or_default();
                        let s_rc_inner2 = s_rc_inner.clone();
                        let cb_inner2 = cb_inner.clone();
                        let old_name_clone = old_name.clone();
                        let render_ref_inner2 = render_ref_inner.clone();
                        let update_status_inner2 = update_status_inner.clone();

                        editor_page_edit.open_editor(
                            Some(&ep_clone),
                            &initial_key,
                            move |new_ep, new_key| {
                                let s_clone = {
                                    let mut s = s_rc_inner2.borrow_mut();
                                    if old_name_clone != new_ep.name {
                                        s.api_keys.remove(&old_name_clone);
                                        let s_mut = &mut *s;
                                        for slot in [
                                            &mut s_mut.ai_chat_model,
                                            &mut s_mut.claw_model,
                                            &mut s_mut.memory_model,
                                        ] {
                                            if let Some(
                                                boxxy_model_selection::ModelProvider::OpenAiCompatible {
                                                    provider_name,
                                                    ..
                                                },
                                            ) = slot
                                                && *provider_name == old_name_clone
                                            {
                                                *provider_name = new_ep.name.clone();
                                            }
                                        }
                                    }
                                    s.api_keys.insert(new_ep.name.clone(), new_key);
                                    if let Some(target) = s
                                        .openai_compatible_endpoints
                                        .iter_mut()
                                        .find(|e| e.name == old_name_clone)
                                    {
                                        *target = new_ep;
                                    }
                                    s.check_and_unset_invalid_models();
                                    s.save();
                                    s.clone()
                                };
                                update_status_inner2();
                                cb_inner2(s_clone);
                                let render_ref_idle = render_ref_inner2.clone();
                                gtk::glib::idle_add_local_once(move || {
                                    if let Some(render) = render_ref_idle.borrow().as_ref() {
                                        render();
                                    }
                                });
                            },
                        );
                    });
                }
                row.add_suffix(&edit_btn);

                let del_btn = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat", "destructive-action"])
                    .build();
                {
                    let s_rc_inner = s_rc.clone();
                    let cb_inner = cb.clone();
                    let render_ref_inner = render_ref.clone();
                    let update_status_inner = update_status.clone();
                    let ep_name = ep.name.clone();
                    del_btn.connect_clicked(move |_| {
                        let s_clone = {
                            let mut s = s_rc_inner.borrow_mut();
                            if idx < s.openai_compatible_endpoints.len() {
                                s.openai_compatible_endpoints.remove(idx);
                            }
                            s.api_keys.remove(&ep_name);
                            s.check_and_unset_invalid_models();
                            s.save();
                            s.clone()
                        };
                        update_status_inner();
                        cb_inner(s_clone);
                        let render_ref_idle = render_ref_inner.clone();
                        gtk::glib::idle_add_local_once(move || {
                            if let Some(render) = render_ref_idle.borrow().as_ref() {
                                render();
                            }
                        });
                    });
                }
                row.add_suffix(&del_btn);

                group.add(&row);
                rows_vec.push(row);
            }
        })
    };
    *render_compat_list.borrow_mut() = Some(render_compat_impl.clone());
    render_compat_impl();

    {
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let render_ref = render_compat_list.clone();
        let update_status = update_model_status.clone();
        let editor_page_add = editor_page;
        add_openai_compat_row.connect_activated(move |_| {
            let s_rc_inner = s_rc.clone();
            let cb_inner = cb.clone();
            let render_ref_inner = render_ref.clone();
            let update_status_inner = update_status.clone();

            editor_page_add.open_editor(None, "", move |mut new_ep, key| {
                let s_clone = {
                    let mut s = s_rc_inner.borrow_mut();
                    let mut unique_name = new_ep.name.clone();
                    let mut counter = 1;
                    while s
                        .openai_compatible_endpoints
                        .iter()
                        .any(|e| e.name == unique_name)
                    {
                        unique_name = format!("{} ({})", new_ep.name, counter);
                        counter += 1;
                    }
                    new_ep.name = unique_name.clone();
                    if !key.trim().is_empty() {
                        s.api_keys.insert(unique_name, key);
                    }
                    s.openai_compatible_endpoints.push(new_ep);
                    s.check_and_unset_invalid_models();
                    s.save();
                    s.clone()
                };
                update_status_inner();
                cb_inner(s_clone);
                let render_ref_idle = render_ref_inner.clone();
                gtk::glib::idle_add_local_once(move || {
                    if let Some(render) = render_ref_idle.borrow().as_ref() {
                        render();
                    }
                });
            });
        });
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

    native_search_switch.set_active(settings_rc.borrow().use_native_search);
    let s_rc_native_search = settings_rc.clone();
    let cb_native_search = on_change.clone();
    native_search_switch.connect_active_notify(move |row| {
        let mut settings_to_save = None;
        {
            let mut s = s_rc_native_search.borrow_mut();
            if s.use_native_search != row.is_active() {
                s.use_native_search = row.is_active();
                s.save();
                settings_to_save = Some(s.clone());
            }
        }

        if let Some(s) = settings_to_save {
            cb_native_search(s);
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
    let add_catalog_provider_row_clone = add_catalog_provider_row.clone();
    let catalog_added_rows_search = catalog_added_rows.clone();
    let add_openai_compat_row_clone = add_openai_compat_row.clone();
    let group_openai_compatible_clone = group_openai_compatible.clone();

    Box::new(move |query: &str| {
        update_model_status();
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let mut any_visible = match_row(
            add_catalog_provider_row_clone.upcast_ref(),
            "add provider catalog api key gemini anthropic claude openai openrouter deepseek moonshot kimi xai grok zai glm zhipu mistral codestral minimax groq",
        );

        let models_visible = match_row(
            model_status_row_clone.upcast_ref(),
            "open models selection ai claw memories status openai openrouter opencode",
        );
        group_model_status.set_visible(models_visible);

        for (name, row) in catalog_added_rows_search.borrow().iter() {
            let extra = match name.as_str() {
                "Moonshot" => " kimi",
                "xAI" => " grok",
                "Z.ai" => " zai glm zhipu",
                _ => "",
            };
            if match_row(row.upcast_ref(), &format!("{}{} api key", name, extra)) {
                any_visible = true;
            }
        }

        let compat_visible = match_row(
            add_openai_compat_row_clone.upcast_ref(),
            "openai compatible endpoints opencode zen go litellm vllm llamacpp custom base url model",
        );
        group_openai_compatible_clone.set_visible(compat_visible);

        let ollama_visible = match_row(ollama_base_url_entry_clone.upcast_ref(), "base url ollama");
        let search_tavily = match_row(
            tavily_api_key_entry.upcast_ref(),
            "tavily api key search engines",
        );
        let search_native = match_row(
            native_search_switch.upcast_ref(),
            "use native model search engines built in gemini gpt claude anthropic openai",
        );

        let search_visible = search_tavily || search_native;

        group_ollama_api.set_visible(ollama_visible);
        group_search_engines.set_visible(search_visible);
        dynamic_apis_group.set_visible(any_visible);

        any_visible || compat_visible || ollama_visible || models_visible || search_visible
    })
}
