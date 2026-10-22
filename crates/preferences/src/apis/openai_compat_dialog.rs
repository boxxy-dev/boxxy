use adw::prelude::*;
use boxxy_model_selection::{
    CustomModelConfig, OpenAiApiRoute, OpenAiCompatibleEndpoint, ThinkingLevel,
    fetch_openai_compatible_models, resolve_any_catalog_spec,
};
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

type SaveCallback = Box<dyn Fn(OpenAiCompatibleEndpoint, String)>;

#[derive(Clone)]
pub struct OpenAiCompatEditorPage {
    apis_stack: gtk::Stack,
    back_btn: gtk::Button,
    content_title: adw::WindowTitle,
    name_entry: adw::EntryRow,
    url_entry: adw::EntryRow,
    requires_key_switch: adw::SwitchRow,
    api_key_entry: adw::PasswordEntryRow,
    env_var_entry: adw::EntryRow,
    zero_cost_switch: adw::SwitchRow,
    models_state: Rc<RefCell<Vec<CustomModelConfig>>>,
    sync_picker: Rc<dyn Fn(Option<u32>)>,
    on_save_slot: Rc<RefCell<Option<SaveCallback>>>,
}

impl OpenAiCompatEditorPage {
    pub fn new(
        page: &adw::PreferencesPage,
        apis_stack: &gtk::Stack,
        back_btn: &gtk::Button,
        content_title: &adw::WindowTitle,
    ) -> Self {
        let on_save_slot: Rc<RefCell<Option<SaveCallback>>> = Rc::new(RefCell::new(None));

        // --- 1. Connection & Authentication ---
        let conn_group = adw::PreferencesGroup::builder()
            .title("Endpoint and Authentication")
            .description("Configure the server Base URL and optional authentication credentials.")
            .build();

        let name_entry = adw::EntryRow::builder().title("Provider Name").build();
        let url_entry = adw::EntryRow::builder()
            .title("Base URL (with /v1)")
            .build();
        let requires_key_switch = adw::SwitchRow::builder()
            .title("Requires API Key")
            .subtitle("Disable for unauthenticated local servers.")
            .active(true)
            .build();
        let api_key_entry = adw::PasswordEntryRow::builder().title("API Key").build();
        let env_var_entry = adw::EntryRow::builder()
            .title("API Key Env Var (Optional)")
            .build();
        let zero_cost_switch = adw::SwitchRow::builder()
            .title("Free or Subscription ($0.00 Cost)")
            .subtitle("When enabled, per-token USD charges are not added to session spend.")
            .active(false)
            .build();

        conn_group.add(&name_entry);
        conn_group.add(&url_entry);
        conn_group.add(&requires_key_switch);
        conn_group.add(&api_key_entry);
        conn_group.add(&env_var_entry);
        conn_group.add(&zero_cost_switch);
        page.add(&conn_group);

        requires_key_switch.connect_active_notify({
            let api_key_entry = api_key_entry.clone();
            let env_var_entry = env_var_entry.clone();
            move |sw| {
                let active = sw.is_active();
                api_key_entry.set_sensitive(active);
                env_var_entry.set_sensitive(active);
            }
        });

        // --- 2. Models Selector & Editor ---
        let models_group = adw::PreferencesGroup::builder()
            .title("Models")
            .description(
                "Select a configured model to inspect or edit, fetch live from /models, or add a new model below.",
            )
            .build();

        let header_actions_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let fetch_btn = gtk::Button::builder()
            .label("Fetch /models")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Query GET <base_url>/models and add discovered models")
            .build();
        let remove_model_btn = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "destructive-action"])
            .tooltip_text("Remove selected model from list")
            .build();
        header_actions_box.append(&fetch_btn);
        header_actions_box.append(&remove_model_btn);
        models_group.set_header_suffix(Some(&header_actions_box));

        let model_picker_combo = adw::ComboRow::builder().title("Configured Models").build();
        let model_picker_list = gtk::StringList::new(&["+ Add New Model…"]);
        model_picker_combo.set_model(Some(&model_picker_list));

        let model_slug_entry = adw::EntryRow::builder()
            .title("Model Slug / ID")
            .build();

        let route_combo = adw::ComboRow::builder().title("API Route").build();
        let route_list = gtk::StringList::new(&[
            OpenAiApiRoute::ChatCompletions.label(),
            OpenAiApiRoute::Responses.label(),
        ]);
        route_combo.set_model(Some(&route_list));
        route_combo.set_selected(0);

        let ctx_window_entry = adw::EntryRow::builder()
            .title("Context Window (Tokens, optional)")
            .build();
        let thinking_entry = adw::EntryRow::builder()
            .title("Thinking Modes (comma-separated, e.g. none, low, medium, high)")
            .build();
        let input_cost_entry = adw::EntryRow::builder()
            .title("Input Cost ($ per 1M tokens, optional)")
            .build();
        let output_cost_entry = adw::EntryRow::builder()
            .title("Output Cost ($ per 1M tokens, optional)")
            .build();

        let save_model_btn = gtk::Button::builder()
            .label("Add / Update Model")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        model_slug_entry.add_suffix(&save_model_btn);

        let status_row = adw::ActionRow::builder()
            .title("Catalog Auto-Detection")
            .subtitle("Leave Context Window or Thinking Modes blank to auto-detect from models.dev.")
            .build();

        models_group.add(&model_picker_combo);
        models_group.add(&model_slug_entry);
        models_group.add(&route_combo);
        models_group.add(&ctx_window_entry);
        models_group.add(&thinking_entry);
        models_group.add(&input_cost_entry);
        models_group.add(&output_cost_entry);
        models_group.add(&status_row);
        page.add(&models_group);

        // --- 3. Bottom Action Bar Group (Cancel / Save Endpoint) ---
        let actions_group = adw::PreferencesGroup::new();
        let actions_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::End)
            .margin_top(6)
            .margin_bottom(12)
            .build();

        let cancel_btn = gtk::Button::builder()
            .label("Cancel")
            .css_classes(["pill"])
            .build();
        let save_endpoint_btn = gtk::Button::builder()
            .label("Save Endpoint")
            .css_classes(["suggested-action", "pill"])
            .build();
        actions_box.append(&cancel_btn);
        actions_box.append(&save_endpoint_btn);
        actions_group.add(&actions_box);
        page.add(&actions_group);

        let models_state: Rc<RefCell<Vec<CustomModelConfig>>> = Rc::new(RefCell::new(Vec::new()));
        let syncing_picker = Rc::new(Cell::new(false));

        let load_selected_into_editor = {
            let state = models_state.clone();
            let slug_e = model_slug_entry.clone();
            let route_c = route_combo.clone();
            let ctx_e = ctx_window_entry.clone();
            let think_e = thinking_entry.clone();
            let in_cost_e = input_cost_entry.clone();
            let out_cost_e = output_cost_entry.clone();
            let remove_btn = remove_model_btn.clone();
            let status_row = status_row.clone();
            Rc::new(move |selected_idx: u32| {
                let list = state.borrow();
                let idx = selected_idx as usize;
                if let Some(m) = list.get(idx) {
                    remove_btn.set_sensitive(true);
                    slug_e.set_text(&m.id);
                    route_c.set_selected(match m.route {
                        OpenAiApiRoute::ChatCompletions => 0,
                        OpenAiApiRoute::Responses => 1,
                    });
                    ctx_e.set_text(
                        &m.context_window
                            .map(|w| w.to_string())
                            .unwrap_or_default(),
                    );
                    think_e.set_text(&ThinkingLevel::format_comma_separated(&m.thinking_levels));
                    in_cost_e.set_text(
                        &m.input_cost_per_1m
                            .map(|c| c.to_string())
                            .unwrap_or_default(),
                    );
                    out_cost_e.set_text(
                        &m.output_cost_per_1m
                            .map(|c| c.to_string())
                            .unwrap_or_default(),
                    );
                    if let Some(spec) = resolve_any_catalog_spec(&m.id) {
                        let ctx_info = spec
                            .context_window
                            .map(|c| format!("{c} tokens"))
                            .unwrap_or_else(|| "unknown".to_string());
                        let levels = boxxy_model_selection::thinking_levels_from_spec(spec);
                        let think_info = if levels.is_empty() {
                            "none".to_string()
                        } else {
                            ThinkingLevel::format_comma_separated(&levels)
                        };
                        status_row.set_subtitle(&glib::markup_escape_text(&format!(
                            "Matched in models.dev: {} • Context: {} • Thinking: [{}]",
                            spec.display_name, ctx_info, think_info
                        )));
                    } else {
                        status_row.set_subtitle(
                            "Custom model (not in built-in catalog — uses values configured above).",
                        );
                    }
                } else {
                    remove_btn.set_sensitive(false);
                    slug_e.set_text("");
                    route_c.set_selected(0);
                    ctx_e.set_text("");
                    think_e.set_text("");
                    in_cost_e.set_text("");
                    out_cost_e.set_text("");
                    status_row.set_subtitle(
                        "Type a Model Slug / ID above and click 'Add / Update Model' (or Save Endpoint).",
                    );
                }
            })
        };

        let sync_picker: Rc<dyn Fn(Option<u32>)> = {
            let picker_list = model_picker_list.clone();
            let picker_combo = model_picker_combo.clone();
            let state = models_state.clone();
            let syncing = syncing_picker.clone();
            let load_editor = load_selected_into_editor.clone();
            Rc::new(move |select_idx: Option<u32>| {
                syncing.set(true);
                let labels: Vec<String> = {
                    let list = state.borrow();
                    let mut out: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
                    out.push("+ Add New Model…".to_string());
                    out
                };
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                picker_list.splice(0, picker_list.n_items(), &refs);

                let target = select_idx
                    .unwrap_or(0)
                    .min(picker_list.n_items().saturating_sub(1));
                picker_combo.set_selected(target);
                syncing.set(false);
                load_editor(target);
            })
        };

        model_picker_combo.connect_selected_notify({
            let syncing = syncing_picker.clone();
            let load_editor = load_selected_into_editor.clone();
            move |combo| {
                if !syncing.get() {
                    load_editor(combo.selected());
                }
            }
        });

        let commit_editor_model = {
            let slug_e = model_slug_entry.clone();
            let route_c = route_combo.clone();
            let ctx_e = ctx_window_entry.clone();
            let think_e = thinking_entry.clone();
            let in_cost_e = input_cost_entry.clone();
            let out_cost_e = output_cost_entry.clone();
            let state = models_state.clone();
            Rc::new(move || -> Option<u32> {
                let id = slug_e.text().trim().to_string();
                if id.is_empty() {
                    return None;
                }
                let route = if route_c.selected() == 1 {
                    OpenAiApiRoute::Responses
                } else {
                    OpenAiApiRoute::ChatCompletions
                };
                let context_window = ctx_e
                    .text()
                    .trim()
                    .replace([',', '_'], "")
                    .parse::<u64>()
                    .ok()
                    .filter(|&w| w > 0);
                let thinking_levels = ThinkingLevel::parse_comma_separated(&think_e.text());
                let input_cost_per_1m = in_cost_e
                    .text()
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|c| c.is_finite() && *c >= 0.0);
                let output_cost_per_1m = out_cost_e
                    .text()
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|c| c.is_finite() && *c >= 0.0);

                let mut list = state.borrow_mut();
                if let Some(pos) = list.iter().position(|m| m.id.trim() == id) {
                    list[pos].route = route;
                    list[pos].context_window = context_window;
                    list[pos].thinking_levels = thinking_levels;
                    list[pos].input_cost_per_1m = input_cost_per_1m;
                    list[pos].output_cost_per_1m = output_cost_per_1m;
                    Some(pos as u32)
                } else {
                    list.push(CustomModelConfig {
                        id,
                        display_name: String::new(),
                        route,
                        context_window,
                        thinking_levels,
                        input_cost_per_1m,
                        output_cost_per_1m,
                    });
                    Some((list.len() - 1) as u32)
                }
            })
        };

        save_model_btn.connect_clicked({
            let commit = commit_editor_model.clone();
            let sync_picker = sync_picker.clone();
            let status_row = status_row.clone();
            move |_| {
                if let Some(idx) = commit() {
                    sync_picker(Some(idx));
                    status_row.set_subtitle("Model saved to endpoint model list.");
                }
            }
        });

        remove_model_btn.connect_clicked({
            let picker_combo = model_picker_combo.clone();
            let state = models_state.clone();
            let sync_picker = sync_picker.clone();
            move |_| {
                let idx = picker_combo.selected() as usize;
                let removed = {
                    let mut list = state.borrow_mut();
                    if idx < list.len() {
                        list.remove(idx);
                        true
                    } else {
                        false
                    }
                };
                if removed {
                    sync_picker(Some(0));
                }
            }
        });

        fetch_btn.connect_clicked({
            let url_entry = url_entry.clone();
            let api_key_entry = api_key_entry.clone();
            let env_var_entry = env_var_entry.clone();
            let status_row = status_row.clone();
            let state = models_state.clone();
            let sync_picker = sync_picker.clone();
            move |btn| {
                let base_url = url_entry.text().trim().to_string();
                let direct_key = api_key_entry.text().trim().to_string();
                let env_var = env_var_entry.text().trim().to_string();
                let key = if !direct_key.is_empty() {
                    direct_key
                } else if !env_var.is_empty() {
                    crate::config::Settings::get_env_api_key(&env_var)
                        .or_else(|| std::env::var(&env_var).ok())
                        .unwrap_or_default()
                } else {
                    String::new()
                };

                btn.set_sensitive(false);
                status_row.set_subtitle("Querying endpoint /models…");

                let btn_clone = btn.clone();
                let status_clone = status_row.clone();
                let state_clone = state.clone();
                let sync_clone = sync_picker.clone();
                let base_url_for_route = base_url.clone();

                glib::spawn_future_local(async move {
                    let res = boxxy_sys_utils::runtime()
                        .spawn(async move { fetch_openai_compatible_models(&base_url, &key).await })
                        .await;

                    btn_clone.set_sensitive(true);
                    match res {
                        Ok(Ok(ids)) => {
                            let mut added = 0usize;
                            {
                                let mut list = state_clone.borrow_mut();
                                for id in &ids {
                                    if !list.iter().any(|m| m.id == *id) {
                                        let route = OpenAiApiRoute::infer_for_model(
                                            &base_url_for_route,
                                            id,
                                        );
                                        list.push(
                                            CustomModelConfig::new(id.clone()).with_route(route),
                                        );
                                        added += 1;
                                    }
                                }
                            }
                            sync_clone(Some(0));
                            status_clone.set_subtitle(&glib::markup_escape_text(&format!(
                                "Fetched {} models from endpoint ({} new added).",
                                ids.len(),
                                added
                            )));
                        }
                        Ok(Err(err)) => {
                            status_clone.set_subtitle(&glib::markup_escape_text(&err));
                        }
                        Err(e) => {
                            status_clone
                                .set_subtitle(&glib::markup_escape_text(&format!("Task error: {e}")));
                        }
                    }
                });
            }
        });

        let go_back = {
            let apis_stack = apis_stack.clone();
            let back_btn = back_btn.clone();
            let content_title = content_title.clone();
            Rc::new(move || {
                apis_stack.set_visible_child_name("list");
                back_btn.set_visible(false);
                content_title.set_title("APIs");
            })
        };

        back_btn.connect_clicked({
            let go_back = go_back.clone();
            move |_| {
                go_back();
            }
        });

        cancel_btn.connect_clicked({
            let go_back = go_back.clone();
            move |_| {
                go_back();
            }
        });

        save_endpoint_btn.connect_clicked({
            let name_entry = name_entry.clone();
            let url_entry = url_entry.clone();
            let requires_key_switch = requires_key_switch.clone();
            let api_key_entry = api_key_entry.clone();
            let env_var_entry = env_var_entry.clone();
            let zero_cost_switch = zero_cost_switch.clone();
            let models_state = models_state.clone();
            let commit = commit_editor_model.clone();
            let on_save_slot = on_save_slot.clone();
            let go_back = go_back;
            move |_| {
                let name = name_entry.text().trim().to_string();
                let base_url = url_entry.text().trim().to_string();
                if name.is_empty() || base_url.is_empty() {
                    return;
                }
                let _ = commit();

                let env_var = env_var_entry.text().trim().to_string();
                let models = models_state.borrow().clone();
                let endpoint = OpenAiCompatibleEndpoint {
                    name,
                    base_url,
                    api_key_env: (!env_var.is_empty()).then_some(env_var),
                    requires_api_key: requires_key_switch.is_active(),
                    zero_cost: zero_cost_switch.is_active(),
                    models,
                    enabled: true,
                };
                let key = api_key_entry.text().to_string();
                go_back();
                let cb_opt = on_save_slot.borrow_mut().take();
                if let Some(cb) = cb_opt {
                    cb(endpoint, key);
                }
            }
        });

        Self {
            apis_stack: apis_stack.clone(),
            back_btn: back_btn.clone(),
            content_title: content_title.clone(),
            name_entry,
            url_entry,
            requires_key_switch,
            api_key_entry,
            env_var_entry,
            zero_cost_switch,
            models_state,
            sync_picker,
            on_save_slot,
        }
    }

    pub fn open_editor(
        &self,
        existing: Option<&OpenAiCompatibleEndpoint>,
        existing_api_key: &str,
        on_save: impl Fn(OpenAiCompatibleEndpoint, String) + 'static,
    ) {
        let is_edit = existing.is_some();
        let initial_ep = existing.cloned().unwrap_or_else(|| OpenAiCompatibleEndpoint {
            name: String::new(),
            base_url: String::new(),
            api_key_env: None,
            requires_api_key: true,
            zero_cost: false,
            models: Vec::new(),
            enabled: true,
        });

        self.name_entry.set_text(&initial_ep.name);
        self.url_entry.set_text(&initial_ep.base_url);
        self.requires_key_switch
            .set_active(initial_ep.requires_api_key);
        self.api_key_entry.set_text(existing_api_key);
        self.api_key_entry
            .set_sensitive(initial_ep.requires_api_key);
        self.env_var_entry
            .set_text(initial_ep.api_key_env.as_deref().unwrap_or(""));
        self.env_var_entry
            .set_sensitive(initial_ep.requires_api_key);
        self.zero_cost_switch.set_active(initial_ep.zero_cost);

        *self.models_state.borrow_mut() = initial_ep.models;
        (self.sync_picker)(Some(0));

        *self.on_save_slot.borrow_mut() = Some(Box::new(on_save));

        self.content_title.set_title(if is_edit {
            "Edit OpenAI-Compatible Endpoint"
        } else {
            "Add OpenAI-Compatible Endpoint"
        });
        self.back_btn.set_visible(true);
        self.apis_stack.set_visible_child_name("editor");
    }
}
