use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub display_name: String,
    pub replacement_text: String,
    pub icon_name: Option<String>,
    pub secondary_text: Option<String>,
}

pub trait CompletionProvider {
    fn trigger_char(&self) -> char;
    fn get_completions(&self, query: &str) -> Vec<CompletionItem>;
}

pub struct AgentCompletionProvider;

impl CompletionProvider for AgentCompletionProvider {
    fn trigger_char(&self) -> char {
        '@'
    }

    fn get_completions(&self, query: &str) -> Vec<CompletionItem> {
        let query_lower = query.to_lowercase();
        let mut items = Vec::new();

        let runtime = boxxy_ai_core::utils::runtime();
        let agents = runtime.block_on(async {
            boxxy_claw::registry::workspace::global_workspace()
                .await
                .get_all_agents()
                .await
        });

        for agent in agents {
            if agent.name.to_lowercase().contains(&query_lower) {
                items.push(CompletionItem {
                    display_name: agent.name.clone(),
                    replacement_text: format!("@{}", agent.name),
                    icon_name: Some("boxxyclaw".to_string()),
                    secondary_text: Some(agent.status),
                });
            }
        }

        items
    }
}

pub struct AutocompleteController {
    entry: gtk::Entry,
    popover: gtk::Popover,
    list: gtk::ListBox,
    providers: Vec<Box<dyn CompletionProvider>>,
    active_trigger: Rc<RefCell<Option<(char, usize)>>>, // (char, start_index)
    on_activated: Option<Box<dyn Fn(String) + 'static>>,
}

impl AutocompleteController {
    #[must_use] 
    pub fn new(
        entry: &gtk::Entry,
        providers: Vec<Box<dyn CompletionProvider>>,
        on_activated: Option<Box<dyn Fn(String) + 'static>>,
    ) -> Rc<Self> {
        let popover = gtk::Popover::new();
        popover.set_parent(entry);
        popover.set_position(gtk::PositionType::Top);
        popover.set_autohide(false);
        popover.set_has_arrow(false);
        popover.add_css_class("autocomplete-popover");

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("completion-list");
        list.set_focusable(false);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(300);
        scroll.set_child(Some(&list));
        popover.set_child(Some(&scroll));
        popover.set_halign(gtk::Align::Start); // Align left instead of center

        let controller = Rc::new(Self {
            entry: entry.clone(),
            popover,
            list,
            providers,
            active_trigger: Rc::new(RefCell::new(None)),
            on_activated,
        });

        controller.setup_signals();
        controller
    }

    fn setup_signals(self: &Rc<Self>) {
        let entry = &self.entry;
        let c_clone = self.clone();

        entry.connect_changed(move |entry| {
            let text = entry.text().to_string();
            let cursor_pos = entry.position() as usize;

            // Simple logic: look backwards from cursor to find a trigger
            let mut found_trigger = None;

            if cursor_pos > 0 {
                let text_before = &text[..cursor_pos];
                for provider in &c_clone.providers {
                    let trigger = provider.trigger_char();
                    if let Some(idx) = text_before.rfind(trigger)
                        && (idx == 0 || text_before.as_bytes()[idx - 1] == b' ') {
                            let query = &text_before[idx + 1..];
                            found_trigger = Some((provider, idx, query));
                            break;
                        }
                }
            }

            if let Some((provider, idx, query)) = found_trigger {
                let completions = provider.get_completions(query);
                if completions.is_empty() {
                    c_clone.popover.popdown();
                    c_clone.active_trigger.replace(None);
                } else {
                    c_clone.update_list(completions);
                    c_clone
                        .active_trigger
                        .replace(Some((provider.trigger_char(), idx)));

                    if !c_clone.popover.is_visible() {
                        c_clone.popover.popup();
                    }
                }
            } else {
                c_clone.popover.popdown();
                c_clone.active_trigger.replace(None);
            }
        });

        let c_clone = self.clone();
        self.list.connect_row_activated(move |_, row| {
            let item_name = row.widget_name();
            c_clone.apply_completion(item_name.as_str());
        });

        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let c_clone = self.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if c_clone.popover.is_visible() {
                match key {
                    gtk::gdk::Key::Up => {
                        c_clone.move_selection(-1);
                        gtk::glib::Propagation::Stop
                    }
                    gtk::gdk::Key::Down => {
                        c_clone.move_selection(1);
                        gtk::glib::Propagation::Stop
                    }
                    gtk::gdk::Key::Return | gtk::gdk::Key::Tab => {
                        if let Some(row) = c_clone.list.selected_row() {
                            c_clone.apply_completion(row.widget_name().as_str());
                            gtk::glib::Propagation::Stop
                        } else {
                            gtk::glib::Propagation::Proceed
                        }
                    }
                    gtk::gdk::Key::Escape => {
                        c_clone.popover.popdown();
                        gtk::glib::Propagation::Stop
                    }
                    _ => gtk::glib::Propagation::Proceed,
                }
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        entry.add_controller(key_ctrl);
    }

    fn update_list(&self, items: Vec<CompletionItem>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        for item in items {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&item.replacement_text);

            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);
            hbox.set_margin_start(6);
            hbox.set_margin_end(6);

            if let Some(icon) = item.icon_name {
                let img = gtk::Image::from_icon_name(&icon);
                img.add_css_class("dim-label");
                hbox.append(&img);
            }

            let label = gtk::Label::new(Some(&item.display_name));
            hbox.append(&label);

            if let Some(secondary) = item.secondary_text {
                let sec_label = gtk::Label::new(Some(&secondary));
                sec_label.add_css_class("caption");
                sec_label.add_css_class("dim-label");
                sec_label.set_hexpand(true);
                sec_label.set_halign(gtk::Align::End);
                hbox.append(&sec_label);
            }

            row.set_child(Some(&hbox));
            self.list.append(&row);
        }

        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
    }

    fn move_selection(&self, delta: i32) {
        let current_idx = self.list.selected_row().map_or(0, |r| r.index());
        let next_idx = (current_idx + delta).max(0);
        if let Some(row) = self.list.row_at_index(next_idx) {
            self.list.select_row(Some(&row));
        }
    }

    fn apply_completion(&self, replacement: &str) {
        let trigger_info = *self.active_trigger.borrow();
        if let Some((_trigger, start_idx)) = trigger_info {
            let text = self.entry.text().to_string();
            let cursor_pos = self.entry.position() as usize;

            let mut new_text = text[..start_idx].to_string();
            new_text.push_str(replacement);
            new_text.push(' ');
            new_text.push_str(&text[cursor_pos..]);

            let new_cursor_pos = start_idx + replacement.len() + 1;

            self.entry.set_text(&new_text);
            self.entry.set_position(new_cursor_pos as i32);

            if let Some(on_activated) = &self.on_activated {
                on_activated(replacement.to_string());
            }
        }
        self.popover.popdown();
    }
}
