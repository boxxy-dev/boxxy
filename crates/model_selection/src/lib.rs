use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4 as gtk;
use std::rc::Rc;
use std::cell::RefCell;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeminiModel {
    #[serde(rename = "gemini-3.1-pro-preview")]
    Pro,
    #[serde(rename = "gemini-3.1-flash-lite-preview")]
    Flash,
}

impl fmt::Display for GeminiModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeminiModel::Pro => write!(f, "Gemini 3.1 Pro"),
            GeminiModel::Flash => write!(f, "Gemini 3.1 Flash Lite"),
        }
    }
}

impl GeminiModel {
    pub fn all() -> Vec<GeminiModel> {
        vec![GeminiModel::Pro, GeminiModel::Flash]
    }

    pub fn api_name(&self) -> &'static str {
        match self {
            GeminiModel::Pro => "gemini-3.1-pro-preview",
            GeminiModel::Flash => "gemini-3.1-flash-lite-preview",
        }
    }

    pub fn supports_thinking(&self) -> bool {
        true
    }

    pub fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        match self {
            GeminiModel::Pro => vec![ThinkingLevel::Low, ThinkingLevel::Medium, ThinkingLevel::High],
            GeminiModel::Flash => vec![
                ThinkingLevel::Minimal,
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingLevel {
    #[serde(rename = "minimal")]
    Minimal,
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "high")]
    High,
}

impl fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThinkingLevel::Minimal => write!(f, "Minimal"),
            ThinkingLevel::Low => write!(f, "Low"),
            ThinkingLevel::Medium => write!(f, "Medium"),
            ThinkingLevel::High => write!(f, "High"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelProvider {
    Gemini(GeminiModel, Option<ThinkingLevel>),
    Ollama(String),
}

impl Default for ModelProvider {
    fn default() -> Self {
        ModelProvider::Gemini(GeminiModel::Flash, Some(ThinkingLevel::Low))
    }
}

#[derive(Clone)]
pub struct SingleModelSelector {
    widget: gtk::Box,
    inner: Rc<RefCell<SingleModelSelectorInner>>,
}

struct SingleModelSelectorInner {
    provider_dropdown: gtk::DropDown,
    model_dropdown: gtk::DropDown,
    thinking_dropdown: gtk::DropDown,
    model_list: gtk::StringList,
    thinking_list: gtk::StringList,
    options_vbox: gtk::Box,
    updating: bool,
    ollama_url: String,
    last_selected_ollama_model: String,
}

impl SingleModelSelector {
    pub fn new<F: Fn(ModelProvider) + 'static>(initial: ModelProvider, ollama_url: String, on_change: F) -> Self {
        let main_vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
        main_vbox.set_margin_start(10);
        main_vbox.set_margin_end(10);
        main_vbox.set_margin_top(10);
        main_vbox.set_margin_bottom(10);

        // Provider Section
        let provider_label = gtk::Label::new(Some("Provider"));
        provider_label.set_halign(gtk::Align::Start);
        provider_label.add_css_class("dim-label");
        let provider_list = gtk::StringList::new(&["Gemini", "Ollama"]);
        let provider_dropdown = gtk::DropDown::new(Some(provider_list), None::<&gtk::Expression>);

        // Model Section
        let model_label = gtk::Label::new(Some("Model"));
        model_label.set_halign(gtk::Align::Start);
        model_label.add_css_class("dim-label");
        let model_list = gtk::StringList::new(&[]);
        let model_dropdown = gtk::DropDown::new(Some(model_list.clone()), None::<&gtk::Expression>);

        // Options Section (Dynamic)
        let options_vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);

        let thinking_label = gtk::Label::new(Some("Thinking Level"));
        thinking_label.set_halign(gtk::Align::Start);
        thinking_label.add_css_class("dim-label");
        let thinking_list = gtk::StringList::new(&[]);
        let thinking_dropdown = gtk::DropDown::new(Some(thinking_list.clone()), None::<&gtk::Expression>);

        options_vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        options_vbox.append(&thinking_label);
        options_vbox.append(&thinking_dropdown);

        main_vbox.append(&provider_label);
        main_vbox.append(&provider_dropdown);
        main_vbox.append(&model_label);
        main_vbox.append(&model_dropdown);
        main_vbox.append(&options_vbox);

                let inner = Rc::new(RefCell::new(SingleModelSelectorInner {
            provider_dropdown: provider_dropdown.clone(),
            model_dropdown: model_dropdown.clone(),
            thinking_dropdown: thinking_dropdown.clone(),
            model_list: model_list.clone(),
            thinking_list: thinking_list.clone(),
            options_vbox: options_vbox.clone(),
            updating: false,
            ollama_url: ollama_url.clone(),
            last_selected_ollama_model: if let ModelProvider::Ollama(ref m) = initial { m.clone() } else { String::new() },
        }));

        let self_ = Self { widget: main_vbox, inner };
        self_.set_model_provider_internal(initial);

        let on_change = Rc::new(on_change);
        let s_clone = self_.clone();
        let update_state = {
            let on_change = on_change.clone();
            Rc::new(move || {
                let inner = s_clone.inner.borrow();
                if inner.updating { return; }

                let p_idx = inner.provider_dropdown.selected();
                let m_idx = inner.model_dropdown.selected();
                let t_idx = inner.thinking_dropdown.selected();

                let am = GeminiModel::all();
                let new_prov = if p_idx == 0 {
                    if m_idx < am.len() as u32 {
                        let model = am[m_idx as usize];
                        let levels = model.available_thinking_levels();
                        let thinking = if t_idx < levels.len() as u32 && model.supports_thinking() {
                            Some(levels[t_idx as usize])
                        } else {
                            None
                        };
                        ModelProvider::Gemini(model, thinking)
                    } else {
                        ModelProvider::Gemini(GeminiModel::Flash, None)
                    }
                } else {
                    let mut m_name = String::new();
                    if m_idx < inner.model_list.n_items()
                        && let Some(item) = inner.model_list.item(m_idx).and_then(|o| o.downcast::<gtk::StringObject>().ok()) {
                            m_name = item.string().to_string();
                        }
                    if m_name == "Loading..." {
                        m_name = String::new();
                    }
                    ModelProvider::Ollama(m_name)
                };

                if let ModelProvider::Ollama(ref name) = new_prov {
                    if name.is_empty() {
                        return;
                    }
                    if let Ok(mut inner) = s_clone.inner.try_borrow_mut() {
                        inner.last_selected_ollama_model = name.clone();
                    }
                }
                on_change(new_prov);
            })
        };

        // Connect Provider selection change
        provider_dropdown.connect_selected_notify({
            let update_state = Rc::clone(&update_state);
            let s_clone = self_.clone();
            move |dropdown| {
                let mut should_update = false;
                {
                    if let Ok(mut inner) = s_clone.inner.try_borrow_mut() {
                        if inner.updating { return; }
                        inner.updating = true;

                        let p_idx = dropdown.selected();
                        inner.model_list.splice(0, inner.model_list.n_items(), &[]);
                        inner.thinking_list.splice(0, inner.thinking_list.n_items(), &[]);

                        if p_idx == 0 {
                            let am = GeminiModel::all();
                            for model in &am {
                                inner.model_list.append(&model.to_string());
                            }
                            if let Some(model) = am.first() {
                                let levels = model.available_thinking_levels();
                                for level in &levels {
                                    inner.thinking_list.append(&level.to_string());
                                }
                                inner.options_vbox.set_visible(model.supports_thinking());
                            }
                            inner.model_dropdown.set_selected(0);
                            if inner.thinking_list.n_items() > 0 {
                                inner.thinking_dropdown.set_selected(0);
                            }
                            inner.updating = false;
                            should_update = true;
                        } else {
                            inner.model_list.append("Loading...");
                            inner.options_vbox.set_visible(false);
                            inner.model_dropdown.set_selected(0);
                            inner.updating = false;

                            let url = inner.ollama_url.clone();
                            drop(inner); // Drop inner before spawning the future
                            let s_clone2 = s_clone.clone();
                            let us = Rc::clone(&update_state);
                            gtk::glib::spawn_future_local(async move {
                                let client = reqwest::Client::new();
                                let endpoint = format!("{}/api/tags", url.trim_end_matches('/'));
                                let mut fetched = vec![];
                                if let Ok(resp) = client.get(&endpoint).send().await
                                    && let Ok(json) = resp.json::<serde_json::Value>().await
                                        && let Some(arr) = json.get("models").and_then(|m| m.as_array()) {
                                            for m in arr {
                                                if let Some(n) = m.get("name").and_then(|s| s.as_str()) {
                                                    fetched.push(n.to_string());
                                                }
                                            }
                                        }

                                if let Ok(mut inner) = s_clone2.inner.try_borrow_mut()
                                    && inner.provider_dropdown.selected() == 1 {
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
                                        drop(inner);
                                        us();
                                    }
                            });
                        }
                    } else {
                        return; // Already borrowed
                    }
                }
                if should_update {
                    update_state();
                }
            }
        });

        model_dropdown.connect_selected_notify({
            let update_state = Rc::clone(&update_state);
            let s_clone = self_.clone();
            move |dropdown| {
                let mut should_update = false;
                {
                    if let Ok(mut inner) = s_clone.inner.try_borrow_mut() {
                        if inner.updating { return; }
                        if inner.provider_dropdown.selected() == 0 {
                            inner.updating = true;
                            let m_idx = dropdown.selected();
                            let am = GeminiModel::all();
                            if m_idx < am.len() as u32 {
                                let model = am[m_idx as usize];
                                inner.thinking_list.splice(0, inner.thinking_list.n_items(), &[]);
                                let levels = model.available_thinking_levels();
                                for level in &levels {
                                    inner.thinking_list.append(&level.to_string());
                                }
                                if inner.thinking_list.n_items() > 0 {
                                    inner.thinking_dropdown.set_selected(0);
                                }
                                inner.options_vbox.set_visible(model.supports_thinking());
                            }
                            inner.updating = false;
                            should_update = true;
                        }
                    } else {
                        // Already borrowed, avoid panic.
                        return;
                    }
                }
                if should_update {
                    update_state();
                }
            }
        });

        thinking_dropdown.connect_selected_notify({
            let update_state = Rc::clone(&update_state);
            let s_clone = self_.clone();
            move |_| {
                if let Ok(inner) = s_clone.inner.try_borrow()
                    && !inner.updating {
                        drop(inner);
                        update_state();
                    }
            }
        });

        let url = ollama_url.clone();
        let s_clone2 = self_.clone();
        gtk::glib::spawn_future_local(async move {
            let client = reqwest::Client::new();
            let endpoint = format!("{}/api/tags", url.trim_end_matches('/'));
            let mut fetched = vec![];
            if let Ok(resp) = client.get(&endpoint).send().await
                && let Ok(json) = resp.json::<serde_json::Value>().await
                    && let Some(arr) = json.get("models").and_then(|m| m.as_array()) {
                        for m in arr {
                            if let Some(n) = m.get("name").and_then(|s| s.as_str()) {
                                fetched.push(n.to_string());
                            }
                        }
                    }
                            let mut inner = s_clone2.inner.borrow_mut();
                            if inner.provider_dropdown.selected() == 1 && !fetched.is_empty() {
                                let mut current_model = String::new();
                                let m_idx = inner.model_dropdown.selected();
                                if m_idx < inner.model_list.n_items()
                                    && let Some(item) = inner.model_list.item(m_idx).and_then(|o| o.downcast::<gtk::StringObject>().ok()) {
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
                                    if !current_model.is_empty() && current_model != "Ollama Offline" && current_model != "Loading..." {
                                        inner.model_list.append(&current_model);
                                        found_pos = Some((fetched.len()) as u32);
                                    } else {
                                        found_pos = Some(0);
                                    }
                                }

                inner.model_dropdown.set_selected(found_pos.unwrap());
                inner.updating = false;
            }
        });

        self_
    }

    pub fn set_model_provider(&self, provider: ModelProvider) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.updating = true;
            drop(inner);
            self.set_model_provider_internal(provider);
            if let Ok(mut inner) = self.inner.try_borrow_mut() {
                inner.updating = false;
            }
        }
    }

    fn set_model_provider_internal(&self, provider: ModelProvider) {
        if let Ok(inner) = self.inner.try_borrow() {
            let (p_idx, m_str, thinking) = match provider {
                ModelProvider::Gemini(m, t) => (0, m.to_string(), t),
                ModelProvider::Ollama(m) => (1, m.clone(), None),
            };

            inner.provider_dropdown.set_selected(p_idx);
            inner.model_list.splice(0, inner.model_list.n_items(), &[]);
            inner.thinking_list.splice(0, inner.thinking_list.n_items(), &[]);

            if p_idx == 0 {
                let all_models = GeminiModel::all();
                for model in &all_models {
                    inner.model_list.append(&model.to_string());
                }
                if let Some(pos) = all_models.iter().position(|m| m.to_string() == m_str) {
                    inner.model_dropdown.set_selected(pos as u32);
                    let model = all_models[pos];
                    let levels = model.available_thinking_levels();
                    for level in &levels {
                        inner.thinking_list.append(&level.to_string());
                    }
                    if let Some(t) = thinking
                        && let Some(l_pos) = levels.iter().position(|l| l == &t) {
                            inner.thinking_dropdown.set_selected(l_pos as u32);
                        }
                    inner.options_vbox.set_visible(model.supports_thinking());
                }
            } else {
                inner.model_list.append("Loading...");
                inner.options_vbox.set_visible(false);

                let url = inner.ollama_url.clone();
                drop(inner); // Drop borrow early

                let s_clone2 = self.clone();
                gtk::glib::spawn_future_local(async move {
                    let client = reqwest::Client::new();
                    let endpoint = format!("{}/api/tags", url.trim_end_matches('/'));
                    let mut fetched = vec![];
                    if let Ok(resp) = client.get(&endpoint).send().await
                        && let Ok(json) = resp.json::<serde_json::Value>().await
                            && let Some(arr) = json.get("models").and_then(|m| m.as_array()) {
                                for m in arr {
                                    if let Some(n) = m.get("name").and_then(|s| s.as_str()) {
                                        fetched.push(n.to_string());
                                    }
                                }
                            }

                    if let Ok(mut inner) = s_clone2.inner.try_borrow_mut()
                        && inner.provider_dropdown.selected() == 1 {
                            let mut current_model = String::new();
                            let m_idx = inner.model_dropdown.selected();
                            let n_items = inner.model_list.n_items();
                            let item_obj = inner.model_list.item(m_idx).and_then(|o| o.downcast::<gtk::StringObject>().ok());
                            if m_idx < n_items
                                && let Some(item) = item_obj {
                                    current_model = item.string().to_string();
                                }
                            if current_model == "Loading..." {
                                current_model = String::new();
                            }

                            inner.updating = true;
                            inner.model_list.splice(0, inner.model_list.n_items(), &[]);

                            let mut found_pos = None;
                            if fetched.is_empty() {
                                inner.model_list.append("Ollama Offline");
                                found_pos = Some(0);
                            } else {
                                for (i, f) in fetched.iter().enumerate() {
                                    inner.model_list.append(f);
                                    if f == &current_model {
                                        found_pos = Some(i as u32);
                                    }
                                }

                                if found_pos.is_none() {
                                    if !current_model.is_empty() && current_model != "Ollama Offline" && current_model != "Loading..." {
                                        inner.model_list.append(&current_model);
                                        found_pos = Some((fetched.len()) as u32);
                                    } else {
                                        found_pos = Some(0);
                                    }
                                }
                            }

                            inner.model_dropdown.set_selected(found_pos.unwrap());
                            inner.updating = false;
                        }
                });
            }
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }
}

#[derive(Clone)]
pub struct GlobalModelSelectorDialog {
    dialog: libadwaita::Dialog,
    pub ai_chat_selector: SingleModelSelector,
    pub claw_selector: SingleModelSelector,
    pub memory_selector: SingleModelSelector,
}

impl GlobalModelSelectorDialog {
    pub fn new<F1, F2, F3>(
        init_ai: ModelProvider,
        init_apps: ModelProvider,
        init_memory: Option<ModelProvider>,
        ollama_url: String,
        on_ai_change: F1,
        on_apps_change: F2,
        on_memory_change: F3,
    ) -> Self
    where
        F1: Fn(ModelProvider) + 'static,
        F2: Fn(ModelProvider) + 'static,
        F3: Fn(Option<ModelProvider>) + 'static,
    {
        let dialog = libadwaita::Dialog::builder()
            .title("Models Selection")
            .content_width(450)
            .content_height(350)
            .build();

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
        stack.set_hhomogeneous(true);
        stack.set_vhomogeneous(true);

        let ai_chat_selector = SingleModelSelector::new(init_ai, ollama_url.clone(), on_ai_change);
        let claw_selector = SingleModelSelector::new(init_apps.clone(), ollama_url.clone(), on_apps_change);
        
        let mem_initial = init_memory.unwrap_or(init_apps);
        let memory_selector = SingleModelSelector::new(mem_initial, ollama_url, move |new_prov| {
            on_memory_change(Some(new_prov));
        });

        stack.add_titled(ai_chat_selector.widget(), Some("ai"), "AI Assistant");
        stack.add_titled(claw_selector.widget(), Some("claw"), "Boxxy Claw");
        stack.add_titled(memory_selector.widget(), Some("memory"), "Memories");

        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));
        switcher.set_margin_top(6);
        switcher.set_margin_start(10);
        switcher.set_margin_end(10);
        switcher.set_margin_bottom(6);
        switcher.set_halign(gtk::Align::Center);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.append(&switcher);
        container.append(&stack);

        let close_btn = gtk::Button::builder()
            .label("Done")
            .margin_start(10)
            .margin_end(10)
            .margin_bottom(10)
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build();
        container.append(&close_btn);

        let d_clone = dialog.clone();
        close_btn.connect_clicked(move |_| {
            d_clone.close();
        });

        dialog.set_child(Some(&container));

        Self {
            dialog,
            ai_chat_selector,
            claw_selector,
            memory_selector,
        }
    }

    pub fn dialog(&self) -> &libadwaita::Dialog {
        &self.dialog
    }

    pub fn present(&self, parent: Option<&impl IsA<gtk::Widget>>) {
        self.dialog.present(parent);
    }
}
