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
    pub autocomplete_popover: gtk::Popover,
    pub autocomplete_list: gtk::ListBox,
    pub model_selector: GlobalModelSelectorDialog,
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

        let autocomplete_popover = gtk::Popover::new();
        autocomplete_popover.set_position(gtk::PositionType::Top);
        autocomplete_popover.set_autohide(false);
        autocomplete_popover.set_has_arrow(false);
        autocomplete_popover.set_parent(&input_entry);
        autocomplete_popover.add_css_class("autocomplete-popover");

        let autocomplete_list = gtk::ListBox::new();
        autocomplete_list.set_selection_mode(gtk::SelectionMode::Single);
        autocomplete_list.add_css_class("boxed-list");
        autocomplete_list.set_focusable(false);

        let autocomplete_scroll = gtk::ScrolledWindow::new();
        autocomplete_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        autocomplete_scroll.set_propagate_natural_height(true);
        autocomplete_scroll.set_max_content_height(300);
        autocomplete_scroll.set_focusable(false);
        autocomplete_scroll.set_child(Some(&autocomplete_list));

        autocomplete_popover.set_child(Some(&autocomplete_scroll));

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
            command_registry: Rc::new(CommandRegistry::new()),
            autocomplete_popover,
            autocomplete_list,
            model_selector: model_selector.clone(),
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

        let comp_clone = comp.clone();
        input_entry.connect_changed(move |entry| {
            let text = entry.text().to_string();

            let (completions, popover, list) = {
                let Ok(inner) = comp_clone.inner.try_borrow() else {
                    return;
                };
                (
                    inner.command_registry.get_completions(&text),
                    inner.autocomplete_popover.clone(),
                    inner.autocomplete_list.clone(),
                )
            };

            if text.starts_with('/') && !text.contains(' ') {
                if !completions.is_empty() {
                    while let Some(child) = list.first_child() {
                        list.remove(&child);
                    }
                    for cmd in completions {
                        let label = gtk::Label::new(Some(cmd));
                        label.set_halign(gtk::Align::Start);
                        label.set_margin_top(8);
                        label.set_margin_bottom(8);
                        label.set_margin_start(8);
                        label.set_margin_end(8);
                        let row = gtk::ListBoxRow::new();
                        row.set_child(Some(&label));
                        row.set_widget_name(cmd);
                        list.append(&row);
                    }
                    list.select_row(list.row_at_index(0).as_ref());

                    let width = entry.width();
                    if width > 0 {
                        popover.set_width_request(width);
                    }

                    if !popover.is_visible() {
                        popover.popup();
                        entry.grab_focus_without_selecting();
                    }
                } else {
                    popover.popdown();
                }
            } else {
                popover.popdown();
            }
        });

        let comp_clone = comp.clone();
        comp.inner
            .borrow()
            .autocomplete_list
            .connect_row_activated(move |_list, row| {
                let cmd = row.widget_name().to_string();
                let c_clone = comp_clone.clone();

                glib::idle_add_local_once(move || {
                    {
                        let inner = c_clone.inner.borrow();
                        inner.input_buffer.set_text(format!("{} ", cmd));
                        inner.autocomplete_popover.popdown();
                        inner.input_entry.grab_focus();
                        inner.input_entry.set_position(-1);
                    }

                    c_clone.send_message();
                });
            });

        let key_controller = gtk::EventControllerKey::new();
        let comp_clone = comp.clone();
        key_controller.connect_key_pressed(move |_, key, _keycode, _state| {
            let is_visible = comp_clone.inner.borrow().autocomplete_popover.is_visible();
            if is_visible {
                if key == gtk::gdk::Key::Up || key == gtk::gdk::Key::Down {
                    let inner = comp_clone.inner.borrow();
                    let list = &inner.autocomplete_list;
                    let current_idx = list.selected_row().map(|r| r.index()).unwrap_or(0);
                    if key == gtk::gdk::Key::Up && current_idx > 0 {
                        list.select_row(list.row_at_index(current_idx - 1).as_ref());
                        return glib::Propagation::Stop;
                    } else if key == gtk::gdk::Key::Down
                        && let Some(next_row) = list.row_at_index(current_idx + 1)
                    {
                        list.select_row(Some(&next_row));
                        return glib::Propagation::Stop;
                    }
                } else if key == gtk::gdk::Key::Return || key == gtk::gdk::Key::Tab {
                    let row_name = {
                        let inner = comp_clone.inner.borrow();
                        inner
                            .autocomplete_list
                            .selected_row()
                            .map(|r| r.widget_name().to_string())
                    };

                    if let Some(cmd) = row_name {
                        let c_clone = comp_clone.clone();
                        glib::idle_add_local_once(move || {
                            {
                                let inner = c_clone.inner.borrow();
                                inner.input_buffer.set_text(format!("{} ", cmd));
                                inner.autocomplete_popover.popdown();
                                inner.input_entry.grab_focus();
                                inner.input_entry.set_position(-1);
                            }

                            c_clone.send_message();
                        });
                        return glib::Propagation::Stop;
                    }
                } else if key == gtk::gdk::Key::Escape {
                    comp_clone.inner.borrow().autocomplete_popover.popdown();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        input_entry.add_controller(key_controller);

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
            self.inner.borrow().autocomplete_popover.popdown();
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
                    Ok(r) => comp_clone.receive_response(r),
                    Err(e) => comp_clone.receive_response(format!("Error: {e}")),
                }
            }
        });
    }

    fn receive_response(&self, content: String) {
        let mut inner = self.inner.borrow_mut();
        if !inner.is_loading {
            return;
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
