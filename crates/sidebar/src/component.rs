use crate::commands::CommandRegistry;
use crate::types::{ChatMessage, ChatMessageObject, Role};
use boxxy_core_widgets::{ObjectExtSafe, bind_property_async};
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
    pub list_store: gtk::gio::ListStore,
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

        let list_store = gtk::gio::ListStore::new::<ChatMessageObject>();
        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let registry = Rc::new(boxxy_viewer::ViewerRegistry::new_with_defaults());
            let viewer = boxxy_viewer::StructuredViewer::new(registry);

            let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
            container.set_margin_top(8);
            container.set_margin_bottom(8);
            container.set_margin_start(8);
            container.set_margin_end(8);

            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            let bubble_box = viewer.widget();
            bubble_box.add_css_class("message-bubble");
            row.append(bubble_box);
            container.append(&row);

            // Store the viewer in the container so we can access it during bind
            container.set_safe_data("viewer", viewer);

            list_item.set_child(Some(&container));
        });

        factory.connect_bind(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let container = list_item.child().and_downcast::<gtk::Box>().unwrap();
            let row = container.first_child().and_downcast::<gtk::Box>().unwrap();
            let bubble_box = row.first_child().and_downcast::<gtk::Box>().unwrap();

            if let Some(msg_obj) = list_item.item().and_downcast::<ChatMessageObject>() {
                let viewer = container
                    .get_safe_data::<boxxy_viewer::StructuredViewer>("viewer")
                    .unwrap();

                // Sync initial content
                viewer.set_content(&msg_obj.content());

                // Update styling based on role
                if msg_obj.role() == Role::User {
                    row.set_halign(gtk::Align::End);
                    row.set_margin_start(48);
                    row.set_margin_end(8);
                    bubble_box.add_css_class("user-message");
                    bubble_box.remove_css_class("assistant-message");
                } else {
                    row.set_halign(gtk::Align::Start);
                    row.set_margin_start(8);
                    row.set_margin_end(48);
                    bubble_box.add_css_class("assistant-message");
                    bubble_box.remove_css_class("user-message");
                }

                // Use a dedicated utility to bind the property asynchronously.
                // This handles cross-thread updates safely and returns a handler ID for cleanup.
                let viewer_clone = viewer.clone();
                let handler_id =
                    bind_property_async(&msg_obj, "content", &bubble_box, move |_, content| {
                        viewer_clone.set_content(&content);
                    });

                // Store handler ID safely using Quarks (no unsafe pointer manipulation needed)
                container.set_safe_data("handler_id", handler_id);
            }
        });

        factory.connect_unbind(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let container = list_item.child().and_downcast::<gtk::Box>().unwrap();
            let viewer = container
                .get_safe_data::<boxxy_viewer::StructuredViewer>("viewer")
                .unwrap();

            // Clean up streaming signal handler safely
            if let Some(obj) = list_item.item().and_downcast::<ChatMessageObject>() {
                if let Some(handler_id) =
                    container.steal_safe_data::<glib::SignalHandlerId>("handler_id")
                {
                    obj.disconnect(handler_id);
                }
            }

            viewer.clear();
        });

        let selection_model = gtk::NoSelection::new(Some(list_store.clone()));
        let list_view = gtk::ListView::new(Some(selection_model), Some(factory));
        list_view.set_show_separators(false);
        list_view.add_css_class("virtual-history");

        scroll_window.set_child(Some(&list_view));
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
        let api_keys = settings.get_effective_api_keys();

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

        let adj_scroll = scroll_window.vadjustment();
        let list_view_scroll = list_view.clone();
        list_store.connect_items_changed(move |store, _, _, _| {
            let adj = adj_scroll.clone();
            let lv = list_view_scroll.clone();
            let n_items = store.n_items();

            glib::idle_add_local_once(move || {
                // User-scroll guard: only auto-scroll if already at the bottom
                let is_at_bottom = adj.value() + adj.page_size() >= adj.upper() - 100.0;
                if is_at_bottom && n_items > 0 {
                    lv.scroll_to(n_items - 1, gtk::ListScrollFlags::FOCUS, None);
                }
            });
        });

        let inner = Rc::new(RefCell::new(AiSidebarInner {
            list_store,
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
        inner.list_store.remove_all();
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

        let user_msg = ChatMessageObject::new(Role::User, content.clone());
        inner.list_store.append(&user_msg);
        inner.input_buffer.set_text("");

        inner.is_loading = true;
        inner
            .action_btn
            .set_icon_name("boxxy-media-playback-stop-symbolic");
        inner.action_btn.set_tooltip_text(Some("Stop Generating"));

        inner.input_entry.grab_focus();

        let provider = inner.model_provider.clone();
        drop(inner);

        let settings = boxxy_preferences::Settings::load();
        let creds = boxxy_ai_core::AiCredentials::new(
            settings.get_effective_api_keys(),
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

        let ai_msg = ChatMessageObject::new(Role::Assistant, content);
        inner.list_store.append(&ai_msg);

        inner.is_loading = false;
        inner.action_btn.set_icon_name("boxxy-paper-plane-symbolic");
        inner.action_btn.set_tooltip_text(Some("Send"));
    }
}

impl Default for AiSidebarComponent {
    fn default() -> Self {
        Self::new()
    }
}
