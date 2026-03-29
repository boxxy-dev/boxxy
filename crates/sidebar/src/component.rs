use crate::commands::CommandRegistry;
use crate::types::{ChatMessage, Role};
use crate::widgets::build_message_widget;
use boxxy_model_selection::{GlobalModelSelectorDialog, ModelProvider};
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use rig::message::Message;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct AiSidebarComponent {
    widget: gtk::Box,
    inner: Rc<RefCell<AiSidebarInner>>,
}

pub(crate) struct AiSidebarInner {
    pub message_list: gtk::Box,
    pub scroll_adj: gtk::Adjustment,
    pub input_entry: gtk::Entry,
    pub input_buffer: gtk::EntryBuffer,
    pub history: Vec<ChatMessage>,
    pub model_provider: Option<ModelProvider>,
    pub is_loading: bool,
    pub generation_task: Option<tokio::task::JoinHandle<()>>,
    pub action_btn: gtk::Button,
    pub command_registry: Rc<CommandRegistry>,
    pub autocomplete_ctrl: Rc<boxxy_core_widgets::autocomplete::AutocompleteController>,
    pub model_selector: GlobalModelSelectorDialog,
    pub usage_label: gtk::Label,
    pub total_tokens_used: Rc<std::cell::Cell<u64>>,
}

impl std::fmt::Debug for AiSidebarComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiSidebarComponent").finish()
    }
}

impl AiSidebarComponent {
    pub fn new() -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        widget.set_margin_top(6);
        widget.set_margin_bottom(6);
        widget.set_margin_start(6);
        widget.set_margin_end(6);

        let scroll_window = gtk::ScrolledWindow::new();
        scroll_window.set_vexpand(true);
        scroll_window.set_hscrollbar_policy(gtk::PolicyType::Never);

        let message_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        message_list.set_margin_top(8);
        message_list.set_margin_bottom(8);
        scroll_window.set_child(Some(&message_list));

        widget.append(&scroll_window);

        let input_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let input_buffer = gtk::EntryBuffer::new(None::<&str>);
        let input_entry = gtk::Entry::new();
        input_entry.set_hexpand(true);
        input_entry.set_placeholder_text(Some("Type your message or / for commands"));
        input_entry.set_buffer(&input_buffer);
        input_box.append(&input_entry);

        let action_btn = gtk::Button::from_icon_name("boxxy-paper-plane-symbolic");
        action_btn.set_tooltip_text(Some("Send"));
        input_box.append(&action_btn);

        let command_registry = Rc::new(CommandRegistry::new());
        let providers: Vec<Box<dyn boxxy_core_widgets::autocomplete::CompletionProvider>> =
            vec![Box::new(crate::commands::SidebarCommandProvider {
                registry: command_registry.clone(),
            })];
        let c_input_entry = input_entry.clone();
        let autocomplete_ctrl = boxxy_core_widgets::autocomplete::AutocompleteController::new(
            &input_entry,
            providers,
            Some(Box::new(move |_| {
                c_input_entry.emit_activate();
            })),
        );

        let usage_label = gtk::Label::builder()
            .label("Context: 0 tokens")
            .css_classes(["caption", "dim-label"])
            .margin_bottom(4)
            .visible(false)
            .build();
        widget.append(&usage_label);

        let settings = boxxy_preferences::Settings::load();
        let initial_model = settings.ai_chat_model.clone();
        let initial_claw_model = settings.claw_model.clone();
        let ollama_url = settings.ollama_base_url.clone();
        let initial_memory_model = settings.memory_model.clone();
        let api_keys = settings.api_keys.clone();

        let model_selector = GlobalModelSelectorDialog::new(
            initial_model.clone(),
            initial_claw_model,
            initial_memory_model,
            ollama_url,
            api_keys,
            move |provider| {
                let mut settings = boxxy_preferences::Settings::load();
                settings.ai_chat_model = provider;
                settings.save();
            },
            move |provider| {
                let mut settings = boxxy_preferences::Settings::load();
                settings.claw_model = provider;
                settings.save();
            },
            move |provider| {
                let mut settings = boxxy_preferences::Settings::load();
                settings.memory_model = provider;
                settings.save();
            },
        );

        let inner = Rc::new(RefCell::new(AiSidebarInner {
            message_list,
            scroll_adj: scroll_window.vadjustment(),
            input_entry: input_entry.clone(),
            input_buffer: input_buffer.clone(),
            history: Vec::new(),
            model_provider: initial_model.clone(),
            is_loading: false,
            generation_task: None,
            action_btn,
            command_registry,
            autocomplete_ctrl,
            model_selector: model_selector.clone(),
            usage_label,
            total_tokens_used: Rc::new(std::cell::Cell::new(0)),
        }));

        let comp = Self { widget, inner };

        let comp_clone = comp.clone();
        let mut settings_rx = boxxy_preferences::SETTINGS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            while let Ok(settings) = settings_rx.recv().await {
                let ai_model = settings.ai_chat_model.clone();
                let claw_model = settings.claw_model.clone();
                let mut inner = comp_clone.inner.borrow_mut();
                if inner.model_provider != ai_model {
                    inner.model_provider = ai_model.clone();
                }
                inner
                    .model_selector
                    .ai_chat_selector
                    .set_model_provider(ai_model);
                inner
                    .model_selector
                    .claw_selector
                    .set_model_provider(claw_model);
            }
        });

        comp.widget.append(&input_box);

        let comp_clone = comp.clone();
        input_entry.connect_activate(move |_| {
            let is_loading = comp_clone.inner.borrow().is_loading;
            if !is_loading {
                comp_clone.send_message();
            }
        });

        let comp_clone = comp.clone();
        comp.inner.borrow().action_btn.connect_clicked(move |_| {
            let is_loading = comp_clone.inner.borrow().is_loading;
            if is_loading {
                comp_clone.cancel_generation();
            } else {
                comp_clone.send_message();
            }
        });

        comp
    }

    pub fn model_selector(&self) -> GlobalModelSelectorDialog {
        self.inner.borrow().model_selector.clone()
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn grab_focus(&self) {
        self.inner.borrow().input_entry.grab_focus();
    }

    pub fn show_model_selector(&self) {
        let inner = self.inner.borrow();
        inner.model_selector.present(Some(&inner.input_entry));
    }

    pub fn clear_history(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.history.clear();
        while let Some(child) = inner.message_list.last_child() {
            inner.message_list.remove(&child);
        }
        inner.input_entry.grab_focus();
    }

    pub fn cancel_generation(&self) {
        let mut inner = self.inner.borrow_mut();
        if let Some(task) = inner.generation_task.take() {
            task.abort();
        }
        inner.is_loading = false;
        inner.action_btn.set_icon_name("boxxy-paper-plane-symbolic");
        inner.action_btn.set_tooltip_text(Some("Send"));
        inner.input_entry.grab_focus();
    }

    pub fn send_message(&self) {
        let (content, is_loading, registry) = {
            let inner = self.inner.borrow();
            (
                inner.input_buffer.text().to_string(),
                inner.is_loading,
                inner.command_registry.clone(),
            )
        };

        if content.trim().is_empty() || is_loading {
            return;
        }

        if content.starts_with('/') {
            self.inner.borrow().autocomplete_ctrl.hide();
            let _handled = registry.handle(&content, self);
            self.inner.borrow_mut().input_buffer.set_text("");
            return;
        }

        let mut inner = self.inner.borrow_mut();
        let (prompt, history_to_send) = if inner.history.is_empty() {
            (content.clone(), vec![])
        } else {
            let hist: Vec<Message> = inner.history.iter().map(|m| m.to_rig_message()).collect();
            (content.clone(), hist)
        };

        let user_msg = ChatMessage {
            role: Role::User,
            content: content.clone(),
        };
        inner.history.push(user_msg.clone());
        inner.message_list.append(&build_message_widget(&user_msg));
        inner.input_buffer.set_text("");

        inner.is_loading = true;
        inner
            .action_btn
            .set_icon_name("boxxy-media-playback-stop-symbolic");
        inner.action_btn.set_tooltip_text(Some("Stop Generating"));

        inner.input_entry.grab_focus();

        Self::smart_scroll(&inner.scroll_adj);

        let provider = inner.model_provider.clone();
        drop(inner);

        let settings = boxxy_preferences::Settings::load();
        let creds = boxxy_ai_core::AiCredentials::new(
            settings.api_keys.clone(),
            settings.ollama_base_url.clone(),
        );

        let data = gtk::gio::resources_lookup_data(
            "/dev/boxxy/BoxxyTerminal/prompts/ai_chat.md",
            gtk::gio::ResourceLookupFlags::NONE,
        )
        .expect("Failed to load ai_chat prompt resource");
        let system_prompt =
            String::from_utf8(data.to_vec()).expect("Prompt resource is not valid UTF-8");

        let comp_clone = self.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();

        let handle = tokio::spawn(async move {
            let agent = boxxy_ai_core::create_agent(&provider, &creds, &system_prompt);
            let res = agent.chat(&prompt, history_to_send).await;
            let _ = tx.send(res);
        });

        self.inner.borrow_mut().generation_task = Some(handle);

        glib::spawn_future_local(async move {
            if let Ok(res) = rx.await {
                match res {
                    Ok((r, usage)) => comp_clone.receive_response(r, usage),
                    Err(e) => comp_clone.receive_response(format!("Error: {e}"), None),
                }
            }
        });
    }

    fn receive_response(&self, content: String, usage: Option<rig::completion::Usage>) {
        let mut inner = self.inner.borrow_mut();
        if !inner.is_loading {
            return;
        }

        if let Some(usage) = usage {
            let total = inner.total_tokens_used.get() + usage.total_tokens;
            inner.total_tokens_used.set(total);
            inner
                .usage_label
                .set_label(&format!("Context: {total} tokens"));
            inner.usage_label.set_visible(true);
        }

        let ai_msg = ChatMessage {
            role: Role::Assistant,
            content,
        };
        inner.history.push(ai_msg.clone());
        inner.message_list.append(&build_message_widget(&ai_msg));

        inner.is_loading = false;
        inner.action_btn.set_icon_name("boxxy-paper-plane-symbolic");
        inner.action_btn.set_tooltip_text(Some("Send"));

        Self::smart_scroll(&inner.scroll_adj);
    }

    fn smart_scroll(adj: &gtk::Adjustment) {
        let adj = adj.clone();
        glib::idle_add_local_once(move || {
            let value = adj.value();
            let upper = adj.upper();
            let page_size = adj.page_size();

            // If we are close to the bottom (within 100 pixels), keep scrolling
            if value > upper - page_size - 100.0 || value < 1.0 {
                adj.set_value(upper - page_size);
            }
        });
    }
}

impl Default for AiSidebarComponent {
    fn default() -> Self {
        Self::new()
    }
}
