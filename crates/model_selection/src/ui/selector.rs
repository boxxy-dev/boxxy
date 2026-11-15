use crate::models::{
    ModelProvider, OpenAiCompatibleEndpoint, catalog_thinking_levels_for_any_slug,
};
use crate::registry::{AiProvider, get_providers_with_custom};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Visual arrangement of the Provider, Model, and Thinking Level controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectorLayout {
    /// Stacked vertically (used in Character Override detail cards and Dreams tab).
    #[default]
    Vertical,
    /// Side-by-side 3-column bar (used for "Default for all characters").
    Horizontal,
}

/// Configuration options for [`SingleModelSelector`].
#[derive(Clone)]
pub struct SelectorOptions {
    pub layout: SelectorLayout,
    /// When `true`, prepends `"Select a provider…"` at index 0 so `None` can be represented.
    /// When `false`, only real providers appear in the dropdown (used when an explicit
    /// `"Use default model"` switch already owns the `None` state).
    pub allow_none: bool,
    pub ollama_cache: Option<OllamaTagsCache>,
}

impl Default for SelectorOptions {
    fn default() -> Self {
        Self {
            layout: SelectorLayout::Vertical,
            allow_none: true,
            ollama_cache: None,
        }
    }
}

/// Shared single-flight cache for `GET {ollama_url}/api/tags` across all selectors
/// in a single dialog open so we never fire duplicate localhost HTTP requests.
#[derive(Clone)]
pub struct OllamaTagsCache {
    state: Rc<RefCell<OllamaCacheState>>,
}

struct OllamaCacheState {
    url: String,
    fetched: Option<Vec<String>>,
    in_flight: bool,
    waiters: Vec<Rc<dyn Fn(Vec<String>)>>,
}

impl OllamaTagsCache {
    #[must_use]
    pub fn new(ollama_url: String) -> Self {
        Self {
            state: Rc::new(RefCell::new(OllamaCacheState {
                url: ollama_url,
                fetched: None,
                in_flight: false,
                waiters: Vec::new(),
            })),
        }
    }

    pub fn with_models<F: Fn(Vec<String>) + 'static>(&self, cb: F) {
        let cb_rc: Rc<dyn Fn(Vec<String>)> = Rc::new(cb);
        let cached = {
            let mut st = self.state.borrow_mut();
            if let Some(ref models) = st.fetched {
                Some(models.clone())
            } else {
                st.waiters.push(cb_rc.clone());
                if !st.in_flight {
                    st.in_flight = true;
                    let url = st.url.clone();
                    let state_rc = self.state.clone();
                    gtk::glib::spawn_future_local(async move {
                        let fetched = fetch_ollama_models(&url).await;
                        let waiters = {
                            let mut s = state_rc.borrow_mut();
                            s.in_flight = false;
                            s.fetched = Some(fetched.clone());
                            std::mem::take(&mut s.waiters)
                        };
                        for w in waiters {
                            w(fetched.clone());
                        }
                    });
                }
                None
            }
        };
        if let Some(models) = cached {
            cb_rc(models);
        }
    }
}

async fn fetch_ollama_models(url: &str) -> Vec<String> {
    let client = reqwest::Client::new();
    let endpoint = format!("{}/api/tags", url.trim_end_matches('/'));
    let mut fetched = Vec::new();
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
    fetched
}

#[derive(Clone)]
pub struct SingleModelSelector {
    widget: gtk::Box,
    updating: Rc<Cell<bool>>,
    inner: Rc<RefCell<SingleModelSelectorInner>>,
}

struct SingleModelSelectorInner {
    layout: SelectorLayout,
    allow_none: bool,
    provider_dropdown: gtk::DropDown,
    model_col: gtk::Box,
    model_dropdown: gtk::DropDown,
    model_entry: gtk::Entry,
    options_vbox: gtk::Box,
    thinking_dropdown: gtk::DropDown,
    model_list: gtk::StringList,
    thinking_list: gtk::StringList,
    ollama_cache: OllamaTagsCache,
    providers: Vec<Box<dyn AiProvider>>,
    last_selected_ollama_model: String,
    last_selected_openrouter_model: String,
    on_change_callback: Option<Rc<dyn Fn()>>,
}

impl SingleModelSelectorInner {
    fn selected_provider_index(&self) -> Option<usize> {
        let raw = self.provider_dropdown.selected();
        if raw == gtk::INVALID_LIST_POSITION {
            return None;
        }
        if self.allow_none {
            if raw == 0 {
                None
            } else {
                let idx = (raw - 1) as usize;
                (idx < self.providers.len()).then_some(idx)
            }
        } else {
            let idx = raw as usize;
            (idx < self.providers.len()).then_some(idx)
        }
    }

    const fn provider_dropdown_pos(&self, p_idx: usize) -> u32 {
        if self.allow_none {
            (p_idx + 1) as u32
        } else {
            p_idx as u32
        }
    }

    fn apply_empty_selection_ui(&self) {
        self.model_list.splice(0, self.model_list.n_items(), &[]);
        self.thinking_list
            .splice(0, self.thinking_list.n_items(), &[]);
        self.model_entry.set_visible(false);

        if self.layout == SelectorLayout::Horizontal {
            self.model_col.set_visible(true);
            self.model_dropdown.set_visible(true);
            self.model_list.append("—");
            self.model_dropdown.set_selected(0);
            self.model_dropdown.set_sensitive(false);

            self.options_vbox.set_visible(true);
            self.thinking_list.append("—");
            self.thinking_dropdown.set_selected(0);
            self.thinking_dropdown.set_sensitive(false);
        } else {
            self.model_col.set_visible(false);
            self.options_vbox.set_visible(false);
        }
    }

    fn apply_thinking_levels(&self, levels: &[String], selected_idx: u32) {
        self.thinking_list
            .splice(0, self.thinking_list.n_items(), &[]);
        if levels.is_empty() {
            self.thinking_list.append("Default");
            self.thinking_dropdown.set_selected(0);
            self.thinking_dropdown.set_sensitive(false);
            self.options_vbox.set_visible(true);
        } else {
            for level in levels {
                self.thinking_list.append(level);
            }
            let safe_idx = selected_idx.min(levels.len().saturating_sub(1) as u32);
            self.thinking_dropdown.set_selected(safe_idx);
            self.thinking_dropdown.set_sensitive(true);
            self.options_vbox.set_visible(true);
        }
    }
}

impl SingleModelSelector {
    pub fn new<F: Fn(Option<ModelProvider>) + 'static>(
        initial: Option<ModelProvider>,
        ollama_url: String,
        api_keys: std::collections::HashMap<String, String>,
        on_change: F,
    ) -> Self {
        Self::new_with_custom(initial, ollama_url, api_keys, Vec::new(), on_change)
    }

    pub fn new_with_custom<F: Fn(Option<ModelProvider>) + 'static>(
        initial: Option<ModelProvider>,
        ollama_url: String,
        api_keys: std::collections::HashMap<String, String>,
        custom_endpoints: Vec<OpenAiCompatibleEndpoint>,
        on_change: F,
    ) -> Self {
        Self::new_with_options(
            initial,
            ollama_url,
            api_keys,
            custom_endpoints,
            SelectorOptions::default(),
            on_change,
        )
    }

    pub fn new_with_options<F: Fn(Option<ModelProvider>) + 'static>(
        initial: Option<ModelProvider>,
        ollama_url: String,
        api_keys: std::collections::HashMap<String, String>,
        custom_endpoints: Vec<OpenAiCompatibleEndpoint>,
        options: SelectorOptions,
        on_change: F,
    ) -> Self {
        let providers: Vec<Box<dyn AiProvider>> = get_providers_with_custom(&custom_endpoints)
            .into_iter()
            .filter(|p| {
                !p.requires_api_key()
                    || api_keys.get(p.name()).map(|k| !k.is_empty()).unwrap_or(false)
            })
            .collect();

        let ollama_cache = options
            .ollama_cache
            .unwrap_or_else(|| OllamaTagsCache::new(ollama_url));
        let updating = Rc::new(Cell::new(false));

        let root_orientation = match options.layout {
            SelectorLayout::Vertical => gtk::Orientation::Vertical,
            SelectorLayout::Horizontal => gtk::Orientation::Horizontal,
        };
        let main_box = gtk::Box::new(root_orientation, 10);
        main_box.set_hexpand(true);

        if providers.is_empty() {
            let empty_label = gtk::Label::new(Some(
                "No configured AI providers found. Add an API key in Manage APIs below.",
            ));
            empty_label.set_justify(gtk::Justification::Center);
            empty_label.set_wrap(true);
            empty_label.add_css_class("dim-label");
            empty_label.set_margin_top(12);
            empty_label.set_margin_bottom(12);
            empty_label.set_hexpand(true);
            main_box.append(&empty_label);

            return Self {
                widget: main_box,
                updating,
                inner: Rc::new(RefCell::new(SingleModelSelectorInner {
                    layout: options.layout,
                    allow_none: options.allow_none,
                    provider_dropdown: gtk::DropDown::new(
                        None::<gtk::StringList>,
                        None::<&gtk::Expression>,
                    ),
                    model_col: gtk::Box::new(gtk::Orientation::Vertical, 4),
                    model_dropdown: gtk::DropDown::new(
                        None::<gtk::StringList>,
                        None::<&gtk::Expression>,
                    ),
                    model_entry: gtk::Entry::new(),
                    options_vbox: gtk::Box::new(gtk::Orientation::Vertical, 4),
                    thinking_dropdown: gtk::DropDown::new(
                        None::<gtk::StringList>,
                        None::<&gtk::Expression>,
                    ),
                    model_list: gtk::StringList::new(&[]),
                    thinking_list: gtk::StringList::new(&[]),
                    ollama_cache,
                    providers: vec![],
                    last_selected_ollama_model: String::new(),
                    last_selected_openrouter_model: String::new(),
                    on_change_callback: None,
                })),
            };
        }

        // 1. Provider Column
        let provider_col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        provider_col.set_hexpand(true);
        let provider_label = gtk::Label::new(Some("Provider"));
        provider_label.set_halign(gtk::Align::Start);
        provider_label.add_css_class("caption");
        provider_label.add_css_class("dim-label");

        let mut provider_names = Vec::with_capacity(providers.len() + 1);
        if options.allow_none {
            provider_names.push("Select a provider…".to_string());
        }
        provider_names.extend(providers.iter().map(|p| p.name().to_string()));
        let provider_names_refs: Vec<&str> = provider_names.iter().map(String::as_str).collect();
        let provider_list = gtk::StringList::new(&provider_names_refs);
        let provider_dropdown = gtk::DropDown::new(Some(provider_list), None::<&gtk::Expression>);
        provider_dropdown.set_hexpand(true);
        provider_col.append(&provider_label);
        provider_col.append(&provider_dropdown);

        // 2. Model Column
        let model_col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        model_col.set_hexpand(true);
        let model_label = gtk::Label::new(Some("Model"));
        model_label.set_halign(gtk::Align::Start);
        model_label.add_css_class("caption");
        model_label.add_css_class("dim-label");
        let model_list = gtk::StringList::new(&[]);
        let model_dropdown = gtk::DropDown::new(Some(model_list.clone()), None::<&gtk::Expression>);
        model_dropdown.set_hexpand(true);

        let model_entry = gtk::Entry::builder()
            .placeholder_text("e.g. anthropic/claude-3.7-sonnet")
            .hexpand(true)
            .visible(false)
            .build();

        model_col.append(&model_label);
        model_col.append(&model_dropdown);
        model_col.append(&model_entry);

        // 3. Thinking Level Column
        let options_vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
        options_vbox.set_hexpand(true);
        let thinking_label = gtk::Label::new(Some("Thinking Level"));
        thinking_label.set_halign(gtk::Align::Start);
        thinking_label.add_css_class("caption");
        thinking_label.add_css_class("dim-label");
        let thinking_list = gtk::StringList::new(&[]);
        let thinking_dropdown =
            gtk::DropDown::new(Some(thinking_list.clone()), None::<&gtk::Expression>);
        thinking_dropdown.set_hexpand(true);

        options_vbox.append(&thinking_label);
        options_vbox.append(&thinking_dropdown);

        main_box.append(&provider_col);
        main_box.append(&model_col);
        main_box.append(&options_vbox);

        let inner = Rc::new(RefCell::new(SingleModelSelectorInner {
            layout: options.layout,
            allow_none: options.allow_none,
            provider_dropdown: provider_dropdown.clone(),
            model_col,
            model_dropdown: model_dropdown.clone(),
            model_entry: model_entry.clone(),
            options_vbox,
            thinking_dropdown: thinking_dropdown.clone(),
            model_list,
            thinking_list,
            ollama_cache: ollama_cache.clone(),
            providers,
            last_selected_ollama_model: if let Some(ModelProvider::Ollama(ref m)) = initial {
                m.clone()
            } else {
                String::new()
            },
            last_selected_openrouter_model: match &initial {
                Some(ModelProvider::OpenRouter(m)) => m.clone(),
                Some(ModelProvider::OpenAiCompatible { model_id, .. }) => model_id.clone(),
                _ => String::new(),
            },
            on_change_callback: None,
        }));

        let self_ = Self {
            widget: main_box,
            updating: updating.clone(),
            inner,
        };

        self_.updating.set(true);
        self_.set_model_provider_internal(initial);
        self_.updating.set(false);

        let on_change = Rc::new(on_change);
        let update_state: Rc<dyn Fn()> = {
            let s_clone = self_.clone();
            let on_change = on_change.clone();
            Rc::new(move || {
                if s_clone.updating.get() {
                    return;
                }
                let current = s_clone.get_current_provider();
                if let Some(ref prov) = current {
                    match prov {
                        ModelProvider::Ollama(name) if name.is_empty() => return,
                        ModelProvider::Ollama(name) => {
                            if let Ok(mut inner_mut) = s_clone.inner.try_borrow_mut() {
                                inner_mut.last_selected_ollama_model.clone_from(name);
                            }
                        }
                        ModelProvider::OpenRouter(name)
                        | ModelProvider::OpenAiCompatible { model_id: name, .. } => {
                            if name.is_empty() {
                                return;
                            }
                            if let Ok(mut inner_mut) = s_clone.inner.try_borrow_mut() {
                                inner_mut.last_selected_openrouter_model.clone_from(name);
                            }
                        }
                        _ => {}
                    }
                }
                on_change(current);
            })
        };

        if let Ok(mut inner) = self_.inner.try_borrow_mut() {
            inner.on_change_callback = Some(update_state.clone());
        }

        provider_dropdown.connect_selected_notify({
            let s_clone = self_.clone();
            let update_state = Rc::clone(&update_state);
            move |_| {
                if !s_clone.updating.get() && s_clone.on_provider_changed() {
                    update_state();
                }
            }
        });

        model_dropdown.connect_selected_notify({
            let s_clone = self_.clone();
            let update_state = Rc::clone(&update_state);
            move |_| {
                if !s_clone.updating.get() && s_clone.on_model_changed() {
                    update_state();
                }
            }
        });

        model_entry.connect_changed({
            let update_state = Rc::clone(&update_state);
            let s_clone = self_.clone();
            move |entry| {
                if s_clone.updating.get() {
                    return;
                }
                if let Ok(mut inner) = s_clone.inner.try_borrow_mut() {
                    let slug = entry.text().to_string();
                    inner.last_selected_openrouter_model.clone_from(&slug);
                    let is_custom_openai = inner
                        .selected_provider_index()
                        .and_then(|idx| inner.providers.get(idx))
                        .is_some_and(|p| p.uses_custom_entry() && p.name() != "OpenRouter");
                    if is_custom_openai {
                        s_clone.updating.set(true);
                        let levels: Vec<String> = catalog_thinking_levels_for_any_slug(&slug)
                            .into_iter()
                            .map(|l| l.to_string())
                            .collect();
                        inner.apply_thinking_levels(&levels, 0);
                        s_clone.updating.set(false);
                    }
                    drop(inner);
                    update_state();
                }
            }
        });

        thinking_dropdown.connect_selected_notify({
            let update_state = Rc::clone(&update_state);
            let s_clone = self_.clone();
            move |_| {
                if !s_clone.updating.get() {
                    update_state();
                }
            }
        });

        // Pre-warm or apply shared Ollama tags if Ollama is available
        let has_ollama = self_
            .inner
            .borrow()
            .providers
            .iter()
            .any(|p| p.name() == "Ollama");
        if has_ollama {
            let s_ollama = self_.clone();
            ollama_cache.with_models(move |fetched| {
                s_ollama.apply_fetched_ollama_models(&fetched, None);
            });
        }

        self_
    }

    fn apply_fetched_ollama_models(
        &self,
        fetched: &[String],
        explicit_target: Option<&ModelProvider>,
    ) {
        if let Ok(inner) = self.inner.try_borrow() {
            let is_ollama = inner
                .selected_provider_index()
                .and_then(|idx| inner.providers.get(idx))
                .is_some_and(|p| p.name() == "Ollama");
            if !is_ollama {
                return;
            }

            self.updating.set(true);
            let current_model = if let Some(ModelProvider::Ollama(target)) = explicit_target {
                target.clone()
            } else if !inner.last_selected_ollama_model.is_empty() {
                inner.last_selected_ollama_model.clone()
            } else {
                let m_idx = inner.model_dropdown.selected();
                inner
                    .model_list
                    .item(m_idx)
                    .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                    .map(|item| item.string().to_string())
                    .unwrap_or_default()
            };

            inner.model_list.splice(0, inner.model_list.n_items(), &[]);
            if fetched.is_empty() {
                inner.model_list.append("Ollama Offline");
                inner.model_dropdown.set_selected(0);
                inner.model_dropdown.set_sensitive(false);
            } else {
                inner.model_dropdown.set_sensitive(true);
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
                        && current_model != "—"
                    {
                        inner.model_list.append(&current_model);
                        found_pos = Some(fetched.len() as u32);
                    } else {
                        found_pos = Some(0);
                    }
                }
                inner.model_dropdown.set_selected(found_pos.unwrap_or(0));
            }
            inner.apply_thinking_levels(&[], 0);
            self.updating.set(false);

            let cb = inner.on_change_callback.clone();
            drop(inner);
            if explicit_target.is_none()
                && let Some(cb) = cb
            {
                cb();
            }
        }
    }

    #[must_use]
    pub fn has_providers(&self) -> bool {
        !self.inner.borrow().providers.is_empty()
    }

    #[must_use]
    pub fn has_provider_named(&self, provider_name: &str) -> bool {
        self.inner
            .borrow()
            .providers
            .iter()
            .any(|p| p.name() == provider_name)
    }

    #[must_use]
    pub fn get_current_provider(&self) -> Option<ModelProvider> {
        let inner = self.inner.borrow();
        let p_idx = inner.selected_provider_index()?;
        let m_idx = inner.model_dropdown.selected();
        let t_idx = inner.thinking_dropdown.selected();

        inner.providers.get(p_idx).and_then(|prov_def| {
            let mut m_name = None;
            if prov_def.uses_custom_entry() {
                let text = inner.model_entry.text().trim().to_string();
                if text.is_empty() {
                    return None;
                }
                m_name = Some(text);
            } else if let Some(item) = inner
                .model_list
                .item(m_idx)
                .and_then(|o| o.downcast::<gtk::StringObject>().ok())
            {
                let s = item.string().to_string();
                if s == "Loading..." || s == "Ollama Offline" || s == "—" {
                    if prov_def.name() == "Ollama" {
                        return None;
                    }
                } else {
                    m_name = Some(s);
                }
            }
            Some(prov_def.create_model_provider(m_idx, m_name, Some(t_idx)))
        })
    }

    pub fn on_provider_changed(&self) -> bool {
        let mut should_update = false;
        if let Ok(inner) = self.inner.try_borrow() {
            if self.updating.get() {
                return false;
            }
            self.updating.set(true);

            let Some(p_idx) = inner.selected_provider_index() else {
                inner.apply_empty_selection_ui();
                self.updating.set(false);
                return true;
            };

            inner.model_col.set_visible(true);
            inner.model_list.splice(0, inner.model_list.n_items(), &[]);

            if let Some(prov_def) = inner.providers.get(p_idx) {
                let is_ollama = prov_def.name() == "Ollama";
                let uses_entry = prov_def.uses_custom_entry();

                if uses_entry {
                    inner.model_dropdown.set_visible(false);
                    inner.model_entry.set_visible(true);
                    let seed = if !inner.last_selected_openrouter_model.is_empty() {
                        inner.last_selected_openrouter_model.clone()
                    } else {
                        prov_def.get_models().into_iter().next().unwrap_or_default()
                    };
                    inner.model_entry.set_text(&seed);
                    let levels: Vec<String> = if prov_def.name() != "OpenRouter" && !seed.is_empty()
                    {
                        catalog_thinking_levels_for_any_slug(&seed)
                            .into_iter()
                            .map(|l| l.to_string())
                            .collect()
                    } else {
                        Vec::new()
                    };
                    inner.apply_thinking_levels(&levels, 0);
                    should_update = true;
                } else if !is_ollama {
                    inner.model_dropdown.set_visible(true);
                    inner.model_dropdown.set_sensitive(true);
                    inner.model_entry.set_visible(false);
                    for model in prov_def.get_models() {
                        inner.model_list.append(&model);
                    }
                    inner.model_dropdown.set_selected(0);
                    let levels = prov_def.get_thinking_levels(0);
                    let def_idx = prov_def.default_thinking_idx(0);
                    inner.apply_thinking_levels(&levels, def_idx);
                    should_update = true;
                } else {
                    inner.model_dropdown.set_visible(true);
                    inner.model_dropdown.set_sensitive(false);
                    inner.model_entry.set_visible(false);
                    inner.model_list.append("Loading...");
                    inner.model_dropdown.set_selected(0);
                    inner.apply_thinking_levels(&[], 0);

                    let cache = inner.ollama_cache.clone();
                    let s_clone = self.clone();
                    self.updating.set(false);
                    drop(inner);
                    cache.with_models(move |fetched| {
                        s_clone.apply_fetched_ollama_models(&fetched, None);
                    });
                    return false;
                }
            }
            self.updating.set(false);
        }
        should_update
    }

    pub fn on_model_changed(&self) -> bool {
        let mut should_update = false;
        if let Ok(inner) = self.inner.try_borrow() {
            if self.updating.get() {
                return false;
            }
            let Some(prov_idx) = inner.selected_provider_index() else {
                return false;
            };
            if let Some(prov) = inner.providers.get(prov_idx) {
                if prov.name() != "Ollama" {
                    self.updating.set(true);
                    let m_idx = inner.model_dropdown.selected();
                    let levels = prov.get_thinking_levels(m_idx);
                    let def_idx = prov.default_thinking_idx(m_idx);
                    inner.apply_thinking_levels(&levels, def_idx);
                    self.updating.set(false);
                }
                should_update = true;
            }
        }
        should_update
    }

    pub fn set_model_provider(&self, provider: Option<ModelProvider>) {
        if provider.is_some() && self.get_current_provider() == provider {
            return;
        }
        self.updating.set(true);
        self.set_model_provider_internal(provider);
        self.updating.set(false);
    }

    fn set_model_provider_internal(&self, provider: Option<ModelProvider>) {
        let Ok(inner) = self.inner.try_borrow() else {
            return;
        };
        if inner.providers.is_empty() {
            return;
        }

        let target = match provider {
            Some(p) if inner.providers.iter().any(|cand| cand.name() == p.provider_name()) => {
                Some(p)
            }
            _ if !inner.allow_none => Some(
                inner.providers[0].create_model_provider(
                    0,
                    inner.providers[0].get_models().into_iter().next(),
                    Some(inner.providers[0].default_thinking_idx(0)),
                ),
            ),
            _ => None,
        };

        let Some(provider) = target else {
            inner.provider_dropdown.set_selected(0);
            inner.apply_empty_selection_ui();
            return;
        };

        let prov_name = provider.provider_name();
        let Some(p_idx) = inner.providers.iter().position(|p| p.name() == prov_name) else {
            inner.provider_dropdown.set_selected(0);
            inner.apply_empty_selection_ui();
            return;
        };

        inner
            .provider_dropdown
            .set_selected(inner.provider_dropdown_pos(p_idx));
        inner.model_col.set_visible(true);
        let prov_def = &inner.providers[p_idx];
        let is_ollama = prov_name == "Ollama";
        let uses_entry = prov_def.uses_custom_entry();

        if uses_entry {
            inner.model_dropdown.set_visible(false);
            inner.model_entry.set_visible(true);
            match provider {
                ModelProvider::OpenRouter(ref target_m) => {
                    inner.model_entry.set_text(target_m);
                    inner.apply_thinking_levels(&[], 0);
                }
                ModelProvider::OpenAiCompatible {
                    ref model_id,
                    thinking,
                    ..
                } => {
                    inner.model_entry.set_text(model_id);
                    let raw_levels = catalog_thinking_levels_for_any_slug(model_id);
                    let level_strs: Vec<String> =
                        raw_levels.iter().map(ToString::to_string).collect();
                    let sel = thinking
                        .and_then(|t| raw_levels.iter().position(|l| *l == t))
                        .unwrap_or(0) as u32;
                    inner.apply_thinking_levels(&level_strs, sel);
                }
                _ => {
                    inner.apply_thinking_levels(&[], 0);
                }
            }
        } else if !is_ollama {
            inner.model_dropdown.set_visible(true);
            inner.model_dropdown.set_sensitive(true);
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
            if !prov_def.supports_thinking(m_idx) {
                inner.apply_thinking_levels(&[], 0);
            } else {
                inner.thinking_dropdown.set_sensitive(true);
                inner.options_vbox.set_visible(true);
            }
        } else {
            inner.model_dropdown.set_visible(true);
            inner.model_entry.set_visible(false);
            inner.model_list.splice(0, inner.model_list.n_items(), &[]);
            inner.model_list.append("Loading...");
            inner.model_dropdown.set_selected(0);
            inner.model_dropdown.set_sensitive(false);
            inner.apply_thinking_levels(&[], 0);

            let cache = inner.ollama_cache.clone();
            let s_clone = self.clone();
            let prov_clone = provider;
            drop(inner);
            cache.with_models(move |fetched| {
                s_clone.apply_fetched_ollama_models(&fetched, Some(&prov_clone));
            });
        }
    }

    pub fn set_sensitive(&self, sensitive: bool) {
        self.widget.set_sensitive(sensitive);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }
}
