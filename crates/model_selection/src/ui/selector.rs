use crate::models::ModelProvider;
use crate::registry::{AiProvider, get_providers};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct SingleModelSelector {
    widget: gtk::Box,
    inner: Rc<RefCell<SingleModelSelectorInner>>,
}
struct SingleModelSelectorInner {
    provider_dropdown: gtk::DropDown,
    model_dropdown: gtk::DropDown,
    model_entry: gtk::Entry,
    thinking_dropdown: gtk::DropDown,
    model_list: gtk::StringList,
    thinking_list: gtk::StringList,
    options_vbox: gtk::Box,
    updating: bool,
    ollama_url: String,
    providers: Vec<Box<dyn AiProvider>>,
    last_selected_ollama_model: String,
    last_selected_openrouter_model: String,
    on_change_callback: Option<Rc<dyn Fn()>>,
}

impl SingleModelSelector {
    pub fn new<F: Fn(Option<ModelProvider>) + 'static>(
        initial: Option<ModelProvider>,
        ollama_url: String,
        _api_keys: std::collections::HashMap<String, String>,
        on_change: F,
    ) -> Self {
        let providers = get_providers();

        let main_vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
        main_vbox.set_margin_start(10);
        main_vbox.set_margin_end(10);
        main_vbox.set_margin_top(10);
        main_vbox.set_margin_bottom(10);

        if providers.is_empty() {
            let empty_label = gtk::Label::new(Some(
                "No configured AI providers found.\nPlease set your API keys in Preferences first.",
            ));
            empty_label.set_justify(gtk::Justification::Center);
            empty_label.add_css_class("dim-label");
            empty_label.set_margin_top(20);
            empty_label.set_margin_bottom(20);
            main_vbox.append(&empty_label);

            return Self {
                widget: main_vbox,
                inner: Rc::new(RefCell::new(SingleModelSelectorInner {
                    provider_dropdown: gtk::DropDown::new(
                        None::<gtk::StringList>,
                        None::<&gtk::Expression>,
                    ),
                    model_dropdown: gtk::DropDown::new(
                        None::<gtk::StringList>,
                        None::<&gtk::Expression>,
                    ),
                    model_entry: gtk::Entry::new(),
                    thinking_dropdown: gtk::DropDown::new(
                        None::<gtk::StringList>,
                        None::<&gtk::Expression>,
                    ),
                    model_list: gtk::StringList::new(&[]),
                    thinking_list: gtk::StringList::new(&[]),
                    options_vbox: gtk::Box::new(gtk::Orientation::Vertical, 0),
                    updating: false,
                    ollama_url,
                    providers: vec![],
                    last_selected_ollama_model: String::new(),
                    last_selected_openrouter_model: String::new(),
                    on_change_callback: None,
                })),
            };
        }

        // Provider Section
        let provider_label = gtk::Label::new(Some("Provider"));
        provider_label.set_halign(gtk::Align::Start);
        provider_label.add_css_class("dim-label");

        let provider_names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
        let provider_list = gtk::StringList::new(&provider_names);
        let provider_dropdown = gtk::DropDown::new(Some(provider_list), None::<&gtk::Expression>);

        // Model Section
        let model_label = gtk::Label::new(Some("Model"));
        model_label.set_halign(gtk::Align::Start);
        model_label.add_css_class("dim-label");
        let model_list = gtk::StringList::new(&[]);
        let model_dropdown = gtk::DropDown::new(Some(model_list.clone()), None::<&gtk::Expression>);
        
        let model_entry = gtk::Entry::builder()
            .placeholder_text("e.g. anthropic/claude-3.5-sonnet")
            .visible(false)
            .build();

        // Options Section (Dynamic)
        let options_vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);

        let thinking_label = gtk::Label::new(Some("Thinking Level"));
        thinking_label.set_halign(gtk::Align::Start);
        thinking_label.add_css_class("dim-label");
        let thinking_list = gtk::StringList::new(&[]);
        let thinking_dropdown =
            gtk::DropDown::new(Some(thinking_list.clone()), None::<&gtk::Expression>);

        options_vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        options_vbox.append(&thinking_label);
        options_vbox.append(&thinking_dropdown);

        main_vbox.append(&provider_label);
        main_vbox.append(&provider_dropdown);
        main_vbox.append(&model_label);
        main_vbox.append(&model_dropdown);
        main_vbox.append(&model_entry);
        main_vbox.append(&options_vbox);

        let inner = Rc::new(RefCell::new(SingleModelSelectorInner {
            provider_dropdown: provider_dropdown.clone(),
            model_dropdown: model_dropdown.clone(),
            model_entry: model_entry.clone(),
            thinking_dropdown: thinking_dropdown.clone(),
            model_list: model_list.clone(),
            thinking_list: thinking_list.clone(),
            options_vbox: options_vbox.clone(),
            updating: false,
            ollama_url: ollama_url.clone(),
            providers,
            last_selected_ollama_model: if let Some(ModelProvider::Ollama(ref m)) = initial {
                m.clone()
            } else {
                String::new()
            },
            last_selected_openrouter_model: if let Some(ModelProvider::OpenRouter(ref m)) = initial {
                m.clone()
            } else {
                String::new()
            },
            on_change_callback: None,
        }));

        let self_ = Self {
            widget: main_vbox,
            inner,
        };

        if let Some(initial_prov) = initial {
            self_.set_model_provider_internal(Some(initial_prov));
        } else {
            self_.on_provider_changed();
        }

        let on_change = Rc::new(on_change);
        let s_clone = self_.clone();
        let update_state: Rc<dyn Fn()> = {
            let on_change = on_change.clone();
            Rc::new(move || {
                let inner = s_clone.inner.borrow();
                if inner.updating {
                    return;
                }

                let p_idx = inner.provider_dropdown.selected();
                let m_idx = inner.model_dropdown.selected();
                let t_idx = inner.thinking_dropdown.selected();

                if let Some(prov_def) = inner.providers.get(p_idx as usize) {
                    let mut m_name = None;
                    if let Some(item) = inner
                        .model_list
                        .item(m_idx)
                        .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                    {
                        let s = item.string().to_string();
                        if s != "Loading..." && s != "Ollama Offline" {
                            m_name = Some(s);
                        }
                    }

                    let new_prov = if prov_def.name() == "OpenRouter" {
                        prov_def.create_model_provider(m_idx, Some(inner.model_entry.text().to_string()), Some(t_idx))
                    } else {
                        prov_def.create_model_provider(m_idx, m_name, Some(t_idx))
                    };

                    if let ModelProvider::Ollama(ref name) = new_prov {
                        if name.is_empty() {
                            return;
                        }
                        if let Ok(mut inner_mut) = s_clone.inner.try_borrow_mut() {
                            inner_mut.last_selected_ollama_model = name.clone();
                        }
                    } else if let ModelProvider::OpenRouter(ref name) = new_prov {
                        if name.is_empty() {
                            return;
                        }
                        if let Ok(mut inner_mut) = s_clone.inner.try_borrow_mut() {
                            inner_mut.last_selected_openrouter_model = name.clone();
                        }
                    }
                    on_change(Some(new_prov));
                }
            })
        };

        if let Ok(mut inner) = self_.inner.try_borrow_mut() {
            inner.on_change_callback = Some(update_state.clone());
        }

        // Connect Provider selection change
        provider_dropdown.connect_selected_notify({
            let s_clone = self_.clone();
            let update_state = Rc::clone(&update_state);
            move |_| {
                if s_clone.on_provider_changed() {
                    update_state();
                }
            }
        });

        model_dropdown.connect_selected_notify({
            let s_clone = self_.clone();
            let update_state = Rc::clone(&update_state);
            move |_| {
                if s_clone.on_model_changed() {
                    update_state();
                }
            }
        });
        
        model_entry.connect_changed({
            let update_state = Rc::clone(&update_state);
            let s_clone = self_.clone();
            move |entry| {
                if let Ok(mut inner) = s_clone.inner.try_borrow_mut() {
                    inner.last_selected_openrouter_model = entry.text().to_string();
                    if !inner.updating {
                        drop(inner);
                        update_state();
                    }
                }
            }
        });

        thinking_dropdown.connect_selected_notify({
            let update_state = Rc::clone(&update_state);
            let s_clone = self_.clone();
            move |_| {
                if let Ok(inner) = s_clone.inner.try_borrow()
                    && !inner.updating
                {
                    drop(inner);
                    update_state();
                }
            }
        });

        // Initial Ollama fetch
        let url = ollama_url.clone();
        let s_clone2 = self_.clone();
        gtk::glib::spawn_future_local(async move {
            let client = reqwest::Client::new();
            let endpoint = format!("{}/api/tags", url.trim_end_matches('/'));
            let mut fetched = vec![];
            if let Ok(resp) = client.get(&endpoint).send().await
                && let Ok(json) = resp.json::<serde_json::Value>().await
                && let Some(arr) = json.get("models").and_then(|m| m.as_array())
            {
                for m in arr {
                    if let Some(n) = m.get("name").and_then(|s| s.as_str()) {
                        fetched.push(n.to_string());
                    }
                }
            }

            if let Ok(mut inner) = s_clone2.inner.try_borrow_mut() {
                let is_ollama = if let Some(p) = inner
                    .providers
                    .get(inner.provider_dropdown.selected() as usize)
                {
                    p.name() == "Ollama"
                } else {
                    false
                };

                if is_ollama && !fetched.is_empty() {
                    let mut current_model = String::new();
                    let m_idx = inner.model_dropdown.selected();
                    if let Some(item) = inner
                        .model_list
                        .item(m_idx)
                        .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                    {
                        current_model = item.string().to_string();
                    }
                    inner.updating = true;
                    inner.model_list.splice(0, inner.model_list.n_items(), &[]);

                    let mut found_pos = None;
                    for (i, f) in fetched.iter().enumerate() {
                        inner.model_list.append(f);
                        if f == &current_model {
                            found_pos = Some(i as u32);
                        }
                    }

                    if found_pos.is_none() {
                        if !current_model.is_empty()
                            && current_model != "Ollama Offline"
                            && current_model != "Loading..."
                        {
                            inner.model_list.append(&current_model);
                            found_pos = Some((fetched.len()) as u32);
                        } else {
                            found_pos = Some(0);
                        }
                    }

                    inner.model_dropdown.set_selected(found_pos.unwrap());
                    inner.updating = false;

                    let cb = inner.on_change_callback.clone();
                    drop(inner);
                    if let Some(cb) = cb {
                        cb();
                    }
                }
            }
        });

        self_
    }

    pub fn get_current_provider(&self) -> Option<ModelProvider> {
        let inner = self.inner.borrow();
        let p_idx = inner.provider_dropdown.selected();
        let m_idx = inner.model_dropdown.selected();
        let t_idx = inner.thinking_dropdown.selected();

        inner.providers.get(p_idx as usize).map(|prov_def| {
            let mut m_name = None;
            if prov_def.name() == "OpenRouter" {
                m_name = Some(inner.model_entry.text().to_string());
            } else if let Some(item) = inner
                .model_list
                .item(m_idx)
                .and_then(|o| o.downcast::<gtk::StringObject>().ok())
            {
                let s = item.string().to_string();
                if s != "Loading..." && s != "Ollama Offline" {
                    m_name = Some(s);
                }
            }
            prov_def.create_model_provider(m_idx, m_name, Some(t_idx))
        })
    }

    pub fn on_provider_changed(&self) -> bool {
        let mut should_update = false;
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            if inner.updating {
                return false;
            }
            inner.updating = true;

            let p_idx = inner.provider_dropdown.selected();
            inner.model_list.splice(0, inner.model_list.n_items(), &[]);
            inner
                .thinking_list
                .splice(0, inner.thinking_list.n_items(), &[]);

            if let Some(prov_def) = inner.providers.get(p_idx as usize) {
                let is_ollama = prov_def.name() == "Ollama";
                let is_openrouter = prov_def.name() == "OpenRouter";

                if is_openrouter {
                    inner.model_dropdown.set_visible(false);
                    inner.model_entry.set_visible(true);
                    inner.options_vbox.set_visible(false); // No thinking dropdown for OpenRouter yet
                    if !inner.last_selected_openrouter_model.is_empty() {
                        inner.model_entry.set_text(&inner.last_selected_openrouter_model);
                    }
                    inner.updating = false;
                    should_update = true;
                } else if !is_ollama {
                    inner.model_dropdown.set_visible(true);
                    inner.model_entry.set_visible(false);
                    for model in prov_def.get_models() {
                        inner.model_list.append(&model);
                    }
                    let levels = prov_def.get_thinking_levels(0);
                    for level in &levels {
                        inner.thinking_list.append(level);
                    }
                    inner
                        .options_vbox
                        .set_visible(prov_def.supports_thinking(0));
                    inner.model_dropdown.set_selected(0);
                    if inner.thinking_list.n_items() > 0 {
                        inner.thinking_dropdown.set_selected(0);
                    }
                    inner.updating = false;
                    should_update = true;
                } else {
                    inner.model_dropdown.set_visible(true);
                    inner.model_entry.set_visible(false);
                    inner.model_list.append("Loading...");
                    inner.options_vbox.set_visible(false);
                    inner.model_dropdown.set_selected(0);
                    inner.updating = false;

                    let url = inner.ollama_url.clone();
                    let s_clone = self.clone();
                    gtk::glib::spawn_future_local(async move {
                        let client = reqwest::Client::new();
                        let endpoint = format!("{}/api/tags", url.trim_end_matches('/'));
                        let mut fetched = vec![];
                        if let Ok(resp) = client.get(&endpoint).send().await
                            && let Ok(json) = resp.json::<serde_json::Value>().await
                            && let Some(arr) = json.get("models").and_then(|m| m.as_array())
                        {
                            for m in arr {
                                if let Some(n) = m.get("name").and_then(|s| s.as_str()) {
                                    fetched.push(n.to_string());
                                }
                            }
                        }

                        if let Ok(mut inner) = s_clone.inner.try_borrow_mut()
                            && inner.provider_dropdown.selected() as usize
                                == inner
                                    .providers
                                    .iter()
                                    .position(|p| p.name() == "Ollama")
                                    .unwrap_or(999)
                        {
                            // Still Ollama
                            inner.updating = true;
                            inner.model_list.splice(0, inner.model_list.n_items(), &[]);
                            if fetched.is_empty() {
                                inner.model_list.append("Ollama Offline");
                            } else {
                                for f in fetched {
                                    inner.model_list.append(&f);
                                }
                            }
                            inner.model_dropdown.set_selected(0);
                            inner.updating = false;

                            let cb = inner.on_change_callback.clone();
                            drop(inner);
                            if let Some(cb) = cb {
                                cb();
                            }
                        }
                    });
                }
            }
        }
        should_update
    }

    pub fn on_model_changed(&self) -> bool {
        let mut should_update = false;
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            if inner.updating {
                return false;
            }
            let p_idx = inner.provider_dropdown.selected();

            let prov_name = inner.providers.get(p_idx as usize).map(|prov| prov.name());

            if let Some(name) = prov_name {
                if name != "Ollama" {
                    inner.updating = true;
                    let m_idx = inner.model_dropdown.selected();
                    inner
                        .thinking_list
                        .splice(0, inner.thinking_list.n_items(), &[]);

                    let levels = inner.providers[p_idx as usize].get_thinking_levels(m_idx);
                    for level in &levels {
                        inner.thinking_list.append(level);
                    }
                    if inner.thinking_list.n_items() > 0 {
                        inner.thinking_dropdown.set_selected(0);
                    }
                    let supports = inner.providers[p_idx as usize].supports_thinking(m_idx);
                    inner.options_vbox.set_visible(supports);
                    inner.updating = false;
                    should_update = true;
                } else {
                    // It is Ollama, we don't have thinking levels, but we still need to update
                    should_update = true;
                }
            }
        }
        should_update
    }

    pub fn set_model_provider(&self, provider: Option<ModelProvider>) {
        if let Some(ref p) = provider
            && self.get_current_provider().as_ref() == Some(p)
        {
            return;
        }

        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.updating = true;
            drop(inner);
            self.set_model_provider_internal(provider);
            if let Ok(mut inner) = self.inner.try_borrow_mut() {
                inner.updating = false;
            }
        }
    }

    fn set_model_provider_internal(&self, provider: Option<ModelProvider>) {
        let provider = match provider {
            Some(p) => p,
            None => return,
        };

        if let Ok(inner) = self.inner.try_borrow() {
            let prov_name = provider.provider_name();
            if let Some(p_idx) = inner.providers.iter().position(|p| p.name() == prov_name) {
                inner.provider_dropdown.set_selected(p_idx as u32);
                let prov_def = &inner.providers[p_idx];

                let is_ollama = prov_name == "Ollama";
                let is_openrouter = prov_name == "OpenRouter";
                
                if is_openrouter {
                    inner.model_dropdown.set_visible(false);
                    inner.model_entry.set_visible(true);
                    inner.options_vbox.set_visible(false);
                    if let ModelProvider::OpenRouter(ref target_m) = provider {
                        inner.model_entry.set_text(target_m);
                    }
                } else if !is_ollama {
                    inner.model_dropdown.set_visible(true);
                    inner.model_entry.set_visible(false);
                    inner.model_list.splice(0, inner.model_list.n_items(), &[]);
                    for m in prov_def.get_models() {
                        inner.model_list.append(&m);
                    }
                    prov_def.sync_ui(
                        &provider,
                        &inner.model_dropdown,
                        &inner.thinking_dropdown,
                        &inner.model_list,
                        &inner.thinking_list,
                    );
                    let m_idx = inner.model_dropdown.selected();
                    inner
                        .options_vbox
                        .set_visible(prov_def.supports_thinking(m_idx));
                } else {
                    inner.model_dropdown.set_visible(true);
                    inner.model_entry.set_visible(false);
                    // Check if we already have the target Ollama model in the list
                    let mut already_set = false;
                    if let ModelProvider::Ollama(ref target_m) = provider {
                        for i in 0..inner.model_list.n_items() {
                            if let Some(item) = inner
                                .model_list
                                .item(i)
                                .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                                && item.string().as_str() == target_m
                            {
                                inner.model_dropdown.set_selected(i);
                                already_set = true;
                                break;
                            }
                        }
                    }

                    if already_set {
                        return;
                    }

                    inner.model_list.append("Loading...");
                    inner.options_vbox.set_visible(false);

                    let url = inner.ollama_url.clone();
                    drop(inner);

                    let s_clone2 = self.clone();
                    let prov_clone = provider.clone();
                    gtk::glib::spawn_future_local(async move {
                        let client = reqwest::Client::new();
                        let endpoint = format!("{}/api/tags", url.trim_end_matches('/'));
                        let mut fetched = vec![];
                        if let Ok(resp) = client.get(&endpoint).send().await
                            && let Ok(json) = resp.json::<serde_json::Value>().await
                            && let Some(arr) = json.get("models").and_then(|m| m.as_array())
                        {
                            for m in arr {
                                if let Some(n) = m.get("name").and_then(|s| s.as_str()) {
                                    fetched.push(n.to_string());
                                }
                            }
                        }

                        if let Ok(mut inner) = s_clone2.inner.try_borrow_mut() {
                            let is_ollama = if let Some(p) = inner
                                .providers
                                .get(inner.provider_dropdown.selected() as usize)
                            {
                                p.name() == "Ollama"
                            } else {
                                false
                            };

                            if is_ollama {
                                inner.updating = true;
                                inner.model_list.splice(0, inner.model_list.n_items(), &[]);
                                let prov_def = if let Some(p) =
                                    inner.providers.iter().find(|p| p.name() == "Ollama")
                                {
                                    p
                                } else {
                                    inner.updating = false;
                                    return;
                                };

                                if fetched.is_empty() {
                                    inner.model_list.append("Ollama Offline");
                                } else {
                                    for f in fetched {
                                        inner.model_list.append(&f);
                                    }
                                }
                                prov_def.sync_ui(
                                    &prov_clone,
                                    &inner.model_dropdown,
                                    &inner.thinking_dropdown,
                                    &inner.model_list,
                                    &inner.thinking_list,
                                );
                                inner.updating = false;
                            }
                        }
                    });
                }
            }
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }
}
