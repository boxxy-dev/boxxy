use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub display_name: String,
    pub replacement_text: String,
    pub icon_name: Option<String>,
    /// File-system path to an image (PNG/JPEG). Takes priority over `icon_name`.
    pub icon_path: Option<String>,
    pub secondary_text: Option<String>,
    pub badge_text: Option<String>,
    pub badge_color: Option<String>,
    pub disabled: bool,
}

pub trait CompletionProvider {
    fn trigger(&self) -> String;
    fn get_completions(&self, query: &str) -> Vec<CompletionItem>;
}
pub struct AutocompleteController {
    entry: gtk::Entry,
    popover: gtk::Popover,
    list: gtk::ListBox,
    providers: Vec<Box<dyn CompletionProvider>>,
    active_trigger: Rc<RefCell<Option<(String, usize)>>>, // (trigger, start_index)
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
        list.set_valign(gtk::Align::Start);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(300);
        scroll.set_child(Some(&list));

        popover.set_child(Some(&scroll));
        popover.set_halign(gtk::Align::Start);

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

    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
    }

    pub fn hide(&self) {
        self.popover.popdown();
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
                    let trigger = provider.trigger();
                    if let Some(idx) = text_before.rfind(&trigger) {
                        // Trigger must be preceded by space or be at the start
                        let is_at_start = idx == 0;
                        let followed_by_space = if is_at_start {
                            false
                        } else {
                            text_before.as_bytes().get(idx - 1) == Some(&b' ')
                        };

                        if is_at_start || followed_by_space {
                            let query = &text_before[idx + trigger.len()..];

                            // Allow multi-word queries for commands that end with a space (e.g., "/resume ")
                            let allow_spaces = trigger.ends_with(' ');

                            if allow_spaces || !query.contains(' ') {
                                found_trigger = Some((provider, idx, query, trigger));
                                break;
                            }
                        }
                    }
                }
            }

            if let Some((provider, idx, query, trigger)) = found_trigger {
                let completions = provider.get_completions(query);
                if completions.is_empty() {
                    c_clone.popover.popdown();
                    c_clone.active_trigger.replace(None);
                } else {
                    // GTK4 won't resize an already visible popover to fit new content.
                    // By popping down, updating the list, and immediately popping back up,
                    // GTK measures the new natural height of the ScrolledWindow perfectly.
                    c_clone.popover.popdown();
                    c_clone.update_list(completions);
                    c_clone.active_trigger.replace(Some((trigger, idx)));
                    c_clone.popover.popup();
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

        let mut first_selectable = None;
        for item in items {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&item.replacement_text);
            
            if item.disabled {
                row.set_sensitive(false);
                row.set_activatable(false);
                row.set_selectable(false);
            }

            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);
            hbox.set_margin_start(6);
            hbox.set_margin_end(6);

            if let Some(path) = item.icon_path.as_deref().filter(|p| !p.is_empty()) {
                if let Ok(texture) = gtk::gdk::Texture::from_filename(path) {
                    let avatar = adw::Avatar::new(32, None, false);
                    avatar.set_custom_image(Some(&texture));
                    hbox.append(&avatar);
                } else if let Some(icon) = item.icon_name.as_deref() {
                    let img = gtk::Image::from_icon_name(icon);
                    img.add_css_class("dim-label");
                    hbox.append(&img);
                }
            } else if let Some(icon) = item.icon_name.as_deref() {
                let img = gtk::Image::from_icon_name(icon);
                img.add_css_class("dim-label");
                hbox.append(&img);
            }

            let label = gtk::Label::new(Some(&item.display_name));
            label.set_hexpand(true);
            label.set_halign(gtk::Align::Fill);
            label.set_xalign(0.0);

            // Only force a wide width for rich items (like session resume)
            // to avoid making simple command suggestions ('/resume') weirdly wide.
            if item.badge_text.is_some() {
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_width_request(400);
            } else {
                label.set_ellipsize(gtk::pango::EllipsizeMode::None);
                label.set_width_request(-1);
            }

            hbox.append(&label);

            let right_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            right_box.set_halign(gtk::Align::End);
            right_box.set_valign(gtk::Align::Center);

            if let Some(secondary) = item.secondary_text {
                let sec_label = gtk::Label::new(Some(&secondary));
                sec_label.add_css_class("caption");
                sec_label.add_css_class("dim-label");
                // Fixed minimum width and right alignment inside the label for perfect column stacking
                sec_label.set_width_request(40);
                sec_label.set_xalign(1.0);
                sec_label.set_halign(gtk::Align::End);
                right_box.append(&sec_label);
            }

            if let Some(badge) = item.badge_text {
                let badge_label = gtk::Label::new(Some(&badge));
                badge_label.add_css_class("caption");

                let badge_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                badge_box.set_margin_top(0);
                badge_box.set_margin_bottom(0);
                // Fixed width for the badge container so they stack perfectly
                badge_box.set_width_request(140);
                badge_box.set_halign(gtk::Align::End);

                // We want the actual badge to center inside its allocated column space
                let inner_badge_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                inner_badge_box.set_halign(gtk::Align::Center);

                if let Some(color) = item.badge_color {
                    let provider = gtk::CssProvider::new();
                    let css = format!(
                        ".autocomplete-badge {{ background-color: {}; color: white; border-radius: 12px; padding: 2px 8px; font-weight: bold; font-size: 0.9em; }}",
                        color
                    );
                    provider.load_from_string(&css);
                    inner_badge_box
                        .style_context()
                        .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                    inner_badge_box.add_css_class("autocomplete-badge");
                }

                inner_badge_box.append(&badge_label);
                badge_box.append(&inner_badge_box);
                right_box.append(&badge_box);
            }

            hbox.append(&right_box);

            row.set_child(Some(&hbox));
            self.list.append(&row);

            if !item.disabled && first_selectable.is_none() {
                first_selectable = Some(row);
            }
        }

        if let Some(first) = first_selectable {
            self.list.select_row(Some(&first));
        }
    }

    fn move_selection(&self, delta: i32) {
        let current_idx = self.list.selected_row().map_or(0, |r| r.index());
        let mut next_idx = current_idx + delta;
        let mut found_row = None;

        // Keep stepping by delta until we find a selectable row or hit the bounds.
        while next_idx >= 0 {
            if let Some(row) = self.list.row_at_index(next_idx) {
                if row.is_selectable() {
                    found_row = Some(row);
                    break;
                }
                next_idx += delta;
            } else {
                break;
            }
        }

        if let Some(row) = found_row {
            self.list.select_row(Some(&row));

            // Ensure the newly selected row is visible within the scrolled window.
            // In GTK4, ListBox doesn't automatically scroll to the selected row
            // if focus doesn't move. We can trigger a scroll by getting the row's bounds.
            let mut current_parent = self.list.parent();
            while let Some(p) = current_parent {
                if let Some(scroll) = p.downcast_ref::<gtk::ScrolledWindow>() {
                    let vadj = scroll.vadjustment();
                    let row_h = row.height() as f64;
                    // Compute coordinate relative to the ListBox
                    let (_, row_y) = row
                        .compute_point(&self.list, &gtk::graphene::Point::new(0.0, 0.0))
                        .map(|pt| (pt.x() as f64, pt.y() as f64))
                        .unwrap_or((0.0, 0.0));

                    let page_size = vadj.page_size();
                    let value = vadj.value();

                    if row_y < value {
                        vadj.set_value(row_y);
                    } else if row_y + row_h > value + page_size {
                        vadj.set_value(row_y + row_h - page_size);
                    }
                    break;
                }
                current_parent = p.parent();
            }
        }
    }

    fn apply_completion(&self, replacement: &str) {
        self.popover.popdown();

        let trigger_info = self.active_trigger.borrow().clone();
        if let Some((_trigger, start_idx)) = trigger_info {
            let text = self.entry.text().to_string();
            let cursor_pos = self.entry.position() as usize;

            let mut new_text = text[..start_idx].to_string();
            new_text.push_str(replacement);

            let is_command = replacement.starts_with('/');
            if !is_command {
                new_text.push(' ');
            }
            new_text.push_str(&text[cursor_pos..]);

            let new_cursor_pos = start_idx + replacement.len() + if is_command { 0 } else { 1 };

            self.entry.set_text(&new_text);
            self.entry.set_position(new_cursor_pos as i32);

            if let Some(on_activated) = &self.on_activated {
                on_activated(replacement.to_string());
            }
        }
    }
}
