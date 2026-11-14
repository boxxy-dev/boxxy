use crate::config::{ModelConfigState, Settings};
use crate::ui_utils::{Debouncer, PageHooks, SearchRow, VoidCallbackSlot};
use adw::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

mod catalog_provider_dialog;
mod openai_compat_dialog;

fn tag_status_title_label_once(widget: &gtk::Widget, title: &str) -> bool {
    if let Some(label) = widget.downcast_ref::<gtk::Label>()
        && label.text() == title
    {
        label.add_css_class("status-title");
        return true;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if tag_status_title_label_once(&c, title) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

fn make_unique_endpoint_name(
    endpoints: &[boxxy_model_selection::OpenAiCompatibleEndpoint],
    desired: &str,
    skip_index: Option<usize>,
) -> String {
    let base = desired.trim();
    let base = if base.is_empty() {
        "Custom Endpoint"
    } else {
        base
    };
    let mut candidate = base.to_string();
    let mut counter = 1usize;
    while endpoints
        .iter()
        .enumerate()
        .any(|(i, ep)| Some(i) != skip_index && ep.name == candidate)
    {
        candidate = format!("{base} ({counter})");
        counter += 1;
    }
    candidate
}

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_apis_page(builder: &gtk::Builder) -> PageHooks {
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

    // Tag the internal title label once on construction so `.model-status-*/.status-title` CSS applies
    model_status_row.set_use_markup(false);
    let _ = tag_status_title_label_once(model_status_row.upcast_ref(), "Models Status");

    let is_syncing = Rc::new(Cell::new(false));
    let initial = Settings::load();

    let update_model_status = {
        let row = model_status_row.clone();
        Rc::new(move |s: &Settings| {
            let is_complete = s.is_models_selection_complete();
            let title = if is_complete {
                "All Models are set"
            } else {
                "Models selection is incomplete"
            };

            row.set_title(title);

            if is_complete {
                row.set_subtitle("Boxxy Claw is ready to use.");
                row.remove_css_class("model-status-warning");
                row.add_css_class("model-status-success");
            } else {
                let subtitle = match s.resolve_model_state(&s.claw_model) {
                    ModelConfigState::MissingApiKey { provider_name: p } => {
                        format!("{p} API key is missing.")
                    }
                    ModelConfigState::OllamaOffline => {
                        "Ollama is offline. Check that it is running.".to_string()
                    }
                    _ => "Open Models Selection to set your models.".to_string(),
                };
                row.set_subtitle(&subtitle);
                row.remove_css_class("model-status-success");
                row.add_css_class("model-status-warning");
            }
        })
    };
    update_model_status(&initial);

    // --- Configured Catalog Providers Management ---
    let catalog_added_rows: Rc<RefCell<Vec<(String, String, adw::PasswordEntryRow)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let last_configured_catalog: Rc<RefCell<Vec<(String, String, bool)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let render_catalog_list: VoidCallbackSlot = Rc::new(RefCell::new(None));

    let compute_catalog_signature = |s: &Settings| -> Vec<(String, String, bool)> {
        boxxy_model_selection::get_providers()
            .into_iter()
            .filter(|p| p.requires_api_key())
            .filter_map(|p| {
                let name = p.name().to_string();
                let effective = s.get_effective_api_key(&name);
                if effective.is_empty() {
                    None
                } else {
                    let direct = s.api_keys.get(&name).cloned().unwrap_or_default();
                    let from_env = direct.trim().is_empty();
                    Some((name, direct, from_env))
                }
            })
            .collect()
    };

    let render_catalog_impl: Rc<dyn Fn()> = {
        let group = dynamic_apis_group.clone();
        let add_row = add_catalog_provider_row.clone();
        let added_rows_inner = catalog_added_rows.clone();
        let last_sig_inner = last_configured_catalog.clone();
        let render_ref = render_catalog_list.clone();
        let update_status = update_model_status.clone();

        Rc::new(move || {
            let mut rows_vec = added_rows_inner.borrow_mut();
            for (_, _, row) in rows_vec.drain(..) {
                group.remove(&row);
            }

            let current = Settings::load();
            *last_sig_inner.borrow_mut() = compute_catalog_signature(&current);

            let providers = boxxy_model_selection::get_providers();
            let mut unconfigured_count = 0usize;

            for prov in &providers {
                if !prov.requires_api_key() {
                    continue;
                }

                let prov_name = prov.name().to_string();
                let direct_key = current.api_keys.get(&prov_name).cloned().unwrap_or_default();
                let effective_key = current.get_effective_api_key(&prov_name);

                if effective_key.is_empty() {
                    unconfigured_count += 1;
                    continue;
                }

                let has_env_fallback = {
                    let mut probe = (*current).clone();
                    probe.api_keys.remove(&prov_name);
                    !probe.get_effective_api_key(&prov_name).is_empty()
                };

                let title = if direct_key.trim().is_empty() && has_env_fallback {
                    format!("{prov_name} API Key (Set from environment)")
                } else {
                    format!("{prov_name} API Key")
                };

                let row = adw::PasswordEntryRow::builder()
                    .use_markup(false)
                    .title(&title)
                    .build();
                row.set_text(&direct_key);

                let debouncer = Debouncer::new();
                debouncer.bind_flush_lifecycle(&row);
                {
                    let prov_name_inner = prov_name.clone();
                    let prov_name_title = prov_name.clone();
                    let update_status_inner = update_status.clone();
                    let row_clone = row.clone();
                    let last_sig_edit = last_sig_inner.clone();
                    let compute_sig = compute_catalog_signature;
                    let debouncer_edit = debouncer.clone();
                    row.connect_changed(move |entry| {
                        let new_val = entry.text().to_string();
                        if new_val.trim().is_empty() && has_env_fallback {
                            row_clone.set_title(&format!(
                                "{prov_name_title} API Key (Set from environment)"
                            ));
                        } else {
                            row_clone.set_title(&format!("{prov_name_title} API Key"));
                        }

                        let prov_save = prov_name_inner.clone();
                        let update_status_save = update_status_inner.clone();
                        let last_sig_save = last_sig_edit.clone();
                        debouncer_edit.schedule(Duration::from_millis(250), move || {
                            // Do NOT call check_and_unset_invalid_models() on text edits so
                            // clearing/pasting a rotated key never wipes claw_model/memory_model.
                            let updated = Settings::update(|s| {
                                s.api_keys.insert(prov_save.clone(), new_val.clone());
                            });
                            *last_sig_save.borrow_mut() = compute_sig(&updated);
                            update_status_save(&updated);
                        });
                    });
                }
                {
                    let debouncer_act = debouncer;
                    row.connect_entry_activated(move |_| debouncer_act.flush());
                }

                let del_btn = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat", "destructive-action"])
                    .tooltip_text("Remove provider")
                    .build();
                {
                    let render_ref_inner = render_ref.clone();
                    let update_status_inner = update_status.clone();
                    let prov_name_del = prov_name.clone();
                    del_btn.connect_clicked(move |_| {
                        Settings::remove_env_api_key(&prov_name_del);
                        let updated = Settings::update(|s| {
                            s.api_keys.remove(&prov_name_del);
                            s.check_and_unset_invalid_models();
                        });
                        update_status_inner(&updated);
                        let render_ref_idle = render_ref_inner.clone();
                        glib::idle_add_local_once(move || {
                            if let Some(render) = render_ref_idle.borrow().as_ref() {
                                render();
                            }
                        });
                    });
                }
                row.add_suffix(&del_btn);

                let extra = match prov_name.as_str() {
                    "Moonshot" => " kimi",
                    "xAI" => " grok",
                    "Z.ai" => " zai glm zhipu",
                    _ => "",
                };
                let haystack = format!("{}{} api key", prov_name.to_lowercase(), extra);

                group.add(&row);
                rows_vec.push((prov_name, haystack, row));
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
        let render_ref = render_catalog_list;
        let update_status = update_model_status.clone();
        add_catalog_provider_row.connect_activated(move |_| {
            let unconfigured: Vec<String> = {
                let s = Settings::load();
                boxxy_model_selection::get_providers()
                    .into_iter()
                    .filter(|p| {
                        p.requires_api_key() && s.get_effective_api_key(p.name()).is_empty()
                    })
                    .map(|p| p.name().to_string())
                    .collect()
            };
            if unconfigured.is_empty() {
                return;
            }

            let render_ref_inner = render_ref.clone();
            let update_status_inner = update_status.clone();

            catalog_editor_page.open_editor(unconfigured, move |provider_name, key| {
                let updated = Settings::update(|s| {
                    s.api_keys.insert(provider_name.clone(), key.clone());
                });
                update_status_inner(&updated);
                let render_ref_idle = render_ref_inner.clone();
                glib::idle_add_local_once(move || {
                    if let Some(render) = render_ref_idle.borrow().as_ref() {
                        render();
                    }
                });
            });
        });
    }

    // --- OpenAI-Compatible Endpoints Management ---
    let compat_added_rows: Rc<RefCell<Vec<(String, adw::ActionRow)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let last_endpoints: Rc<RefCell<Vec<boxxy_model_selection::OpenAiCompatibleEndpoint>>> =
        Rc::new(RefCell::new(Vec::new()));
    let render_compat_list: VoidCallbackSlot = Rc::new(RefCell::new(None));

    let render_compat_impl: Rc<dyn Fn()> = {
        let group = group_openai_compatible.clone();
        let added_rows_inner = compat_added_rows.clone();
        let last_ep_inner = last_endpoints.clone();
        let render_ref = render_compat_list.clone();
        let update_status = update_model_status.clone();
        let editor_page = editor_page.clone();

        Rc::new(move || {
            let mut rows_vec = added_rows_inner.borrow_mut();
            for (_, row) in rows_vec.drain(..) {
                group.remove(&row);
            }

            let endpoints = Settings::load().openai_compatible_endpoints.clone();
            last_ep_inner.borrow_mut().clone_from(&endpoints);

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
                let model_ids: Vec<&str> = ep.models.iter().map(|m| m.id.as_str()).collect();
                let haystack = format!("{} {} {}", ep.name, ep.base_url, model_ids.join(" "))
                    .to_lowercase();

                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .title(&ep.name)
                    .subtitle(&subtitle)
                    .build();

                let switch = gtk::Switch::builder()
                    .active(ep.enabled)
                    .valign(gtk::Align::Center)
                    .build();

                {
                    let update_status_inner = update_status.clone();
                    let ep_name = ep.name.clone();
                    let last_ep_sw = last_ep_inner.clone();
                    switch.connect_active_notify(move |sw| {
                        let active = sw.is_active();
                        let updated = Settings::update(|s| {
                            if let Some(target) = s
                                .openai_compatible_endpoints
                                .iter_mut()
                                .find(|e| e.name == ep_name)
                                && target.enabled != active
                            {
                                target.enabled = active;
                                s.check_and_unset_invalid_models();
                            }
                        });
                        last_ep_sw
                            .borrow_mut()
                            .clone_from(&updated.openai_compatible_endpoints);
                        update_status_inner(&updated);
                    });
                }
                row.add_suffix(&switch);

                let edit_btn = gtk::Button::builder()
                    .icon_name("document-edit-symbolic")
                    .valign(gtk::Align::Center)
                    .tooltip_text("Edit endpoint")
                    .css_classes(["flat"])
                    .build();
                {
                    let ep_clone = ep.clone();
                    let old_name = ep.name.clone();
                    let render_ref_inner = render_ref.clone();
                    let update_status_inner = update_status.clone();
                    let editor_page_edit = editor_page.clone();

                    edit_btn.connect_clicked(move |_| {
                        let initial_key = Settings::load()
                            .api_keys
                            .get(&old_name)
                            .cloned()
                            .unwrap_or_default();
                        let old_name_clone = old_name.clone();
                        let render_ref_inner2 = render_ref_inner.clone();
                        let update_status_inner2 = update_status_inner.clone();

                        editor_page_edit.open_editor(
                            Some(&ep_clone),
                            &initial_key,
                            move |mut new_ep, new_key| {
                                let updated = Settings::update(|s| {
                                    new_ep.name = make_unique_endpoint_name(
                                        &s.openai_compatible_endpoints,
                                        &new_ep.name,
                                        Some(idx),
                                    );
                                    if old_name_clone != new_ep.name {
                                        s.api_keys.remove(&old_name_clone);
                                        for slot in [&mut s.claw_model, &mut s.memory_model] {
                                            if let Some(
                                                boxxy_model_selection::ModelProvider::OpenAiCompatible {
                                                    provider_name,
                                                    ..
                                                },
                                            ) = slot
                                                && *provider_name == old_name_clone
                                            {
                                                provider_name.clone_from(&new_ep.name);
                                            }
                                        }
                                    }
                                    if new_key.trim().is_empty() {
                                        s.api_keys.remove(&new_ep.name);
                                    } else {
                                        s.api_keys.insert(new_ep.name.clone(), new_key.clone());
                                    }
                                    if idx < s.openai_compatible_endpoints.len() {
                                        s.openai_compatible_endpoints[idx] = new_ep.clone();
                                    }
                                    s.sanitize_unconfigured_models();
                                });
                                update_status_inner2(&updated);
                                let render_ref_idle = render_ref_inner2.clone();
                                glib::idle_add_local_once(move || {
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
                    .tooltip_text("Remove endpoint")
                    .css_classes(["flat", "destructive-action"])
                    .build();
                {
                    let render_ref_inner = render_ref.clone();
                    let update_status_inner = update_status.clone();
                    let ep_name = ep.name.clone();
                    del_btn.connect_clicked(move |_| {
                        let updated = Settings::update(|s| {
                            if idx < s.openai_compatible_endpoints.len() {
                                s.openai_compatible_endpoints.remove(idx);
                            }
                            s.api_keys.remove(&ep_name);
                            s.check_and_unset_invalid_models();
                        });
                        update_status_inner(&updated);
                        let render_ref_idle = render_ref_inner.clone();
                        glib::idle_add_local_once(move || {
                            if let Some(render) = render_ref_idle.borrow().as_ref() {
                                render();
                            }
                        });
                    });
                }
                row.add_suffix(&del_btn);

                group.add(&row);
                rows_vec.push((haystack, row));
            }
        })
    };
    *render_compat_list.borrow_mut() = Some(render_compat_impl.clone());
    render_compat_impl();

    {
        let render_ref = render_compat_list;
        let update_status = update_model_status.clone();
        let editor_page_add = editor_page;
        add_openai_compat_row.connect_activated(move |_| {
            let render_ref_inner = render_ref.clone();
            let update_status_inner = update_status.clone();

            editor_page_add.open_editor(None, "", move |mut new_ep, key| {
                let updated = Settings::update(|s| {
                    new_ep.name =
                        make_unique_endpoint_name(&s.openai_compatible_endpoints, &new_ep.name, None);
                    if !key.trim().is_empty() {
                        s.api_keys.insert(new_ep.name.clone(), key.clone());
                    }
                    s.openai_compatible_endpoints.push(new_ep.clone());
                });
                update_status_inner(&updated);
                let render_ref_idle = render_ref_inner.clone();
                glib::idle_add_local_once(move || {
                    if let Some(render) = render_ref_idle.borrow().as_ref() {
                        render();
                    }
                });
            });
        });
    }

    tavily_api_key_entry.set_text(&initial.api_keys.get("Tavily").cloned().unwrap_or_default());
    let tavily_debouncer = Debouncer::new();
    tavily_debouncer.bind_flush_lifecycle(&tavily_api_key_entry);
    {
        let is_syncing = is_syncing.clone();
        let debouncer = tavily_debouncer.clone();
        tavily_api_key_entry.connect_changed(move |entry| {
            if is_syncing.get() {
                return;
            }
            let new_val = entry.text().to_string();
            debouncer.schedule(Duration::from_millis(250), move || {
                Settings::update(|s| {
                    s.api_keys.insert("Tavily".to_string(), new_val.clone());
                });
            });
        });
    }
    tavily_api_key_entry.connect_entry_activated(move |_| tavily_debouncer.flush());

    native_search_switch.set_active(initial.use_native_search);
    {
        let is_syncing = is_syncing.clone();
        native_search_switch.connect_active_notify(move |row| {
            if is_syncing.get() {
                return;
            }
            let active = row.is_active();
            Settings::update(|s| {
                s.use_native_search = active;
            });
        });
    }

    ollama_base_url_entry.set_text(&initial.ollama_base_url);
    let ollama_debouncer = Debouncer::new();
    ollama_debouncer.bind_flush_lifecycle(&ollama_base_url_entry);
    {
        let is_syncing = is_syncing.clone();
        let debouncer = ollama_debouncer.clone();
        ollama_base_url_entry.connect_changed(move |entry| {
            if is_syncing.get() {
                return;
            }
            let new_url = entry.text().to_string();
            debouncer.schedule(Duration::from_millis(250), move || {
                Settings::update(|s| {
                    s.ollama_base_url.clone_from(&new_url);
                });
            });
        });
    }
    ollama_base_url_entry.connect_entry_activated(move |_| ollama_debouncer.flush());

    let s_add_cat = SearchRow::new(
        &add_catalog_provider_row,
        "add provider catalog api key gemini anthropic claude openai openrouter deepseek moonshot kimi xai grok zai glm zhipu mistral codestral minimax groq opencode zen go",
    );
    let s_models = SearchRow::new(
        &model_status_row,
        "open models selection ai claw memories status openai openrouter opencode",
    );
    let s_add_compat = SearchRow::new(
        &add_openai_compat_row,
        "openai compatible endpoints opencode zen go litellm vllm llamacpp custom base url model",
    );
    let s_ollama = SearchRow::new(&ollama_base_url_entry, "base url ollama local");
    let s_tavily = SearchRow::new(&tavily_api_key_entry, "tavily api key search engines");
    let s_native = SearchRow::new(
        &native_search_switch,
        "use native model search engines built in gemini gpt claude anthropic openai",
    );

    let catalog_rows_sync = catalog_added_rows.clone();
    let tavily_sync = tavily_api_key_entry;
    let native_sync = native_search_switch;
    let ollama_sync = ollama_base_url_entry;

    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);

        update_model_status(s);

        let any_cat_focused = catalog_rows_sync
            .borrow()
            .iter()
            .any(|(_, _, r)| r.has_focus() || r.focus_child().is_some());
        if !any_cat_focused && *last_configured_catalog.borrow() != compute_catalog_signature(s) {
            render_catalog_impl();
        }

        if *last_endpoints.borrow() != s.openai_compatible_endpoints {
            render_compat_impl();
        }

        let tavily_val = s.api_keys.get("Tavily").map_or("", String::as_str);
        if !tavily_sync.has_focus()
            && tavily_sync.focus_child().is_none()
            && tavily_sync.text().as_str() != tavily_val
        {
            tavily_sync.set_text(tavily_val);
        }

        if native_sync.is_active() != s.use_native_search {
            native_sync.set_active(s.use_native_search);
        }

        if !ollama_sync.has_focus()
            && ollama_sync.focus_child().is_none()
            && ollama_sync.text().as_str() != s.ollama_base_url
        {
            ollama_sync.set_text(&s.ollama_base_url);
        }

        is_syncing.set(false);
    });

    let add_compat_filter = add_openai_compat_row;
    let filter = Box::new(move |query: &str| {
        let mut any_cat_visible = s_add_cat.apply(query);
        for (_, haystack, row) in catalog_added_rows.borrow().iter() {
            let m = query.is_empty() || haystack.contains(query);
            row.set_visible(m);
            if m {
                any_cat_visible = true;
            }
        }
        dynamic_apis_group.set_visible(any_cat_visible);

        let mut compat_visible = s_add_compat.apply(query);
        for (haystack, row) in compat_added_rows.borrow().iter() {
            let m = query.is_empty() || compat_visible || haystack.contains(query);
            row.set_visible(m);
            if m {
                compat_visible = true;
            }
        }
        if compat_visible {
            add_compat_filter.set_visible(true);
        }
        group_openai_compatible.set_visible(compat_visible);

        let models_visible = s_models.apply(query);
        group_model_status.set_visible(models_visible);

        let ollama_visible = s_ollama.apply(query);
        group_ollama_api.set_visible(ollama_visible);

        let search_visible = s_tavily.apply(query) | s_native.apply(query);
        group_search_engines.set_visible(search_visible);

        any_cat_visible || compat_visible || ollama_visible || models_visible || search_visible
    });

    (sync, filter)
}
