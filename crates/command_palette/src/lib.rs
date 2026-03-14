use gtk4::prelude::*;
use gtk4::glib;

#[derive(Clone)]
struct CommandItem {
    title: String,
    action: String,
    shortcut: Option<String>,
}

pub struct CommandPaletteComponent {
    popover: gtk4::Popover,
    _search_entry: gtk4::SearchEntry,
}

impl CommandPaletteComponent {
    pub fn new() -> Self {
        let popover = gtk4::Popover::builder()
            .has_arrow(false)
            .autohide(true)
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Start)
            .build();
        popover.add_css_class("command-palette");

        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        let search_entry = gtk4::SearchEntry::builder()
            .placeholder_text("Search commands...")
            .width_request(360)
            .activates_default(false)
            .build();
        vbox.append(&search_entry);

        let listbox = gtk4::ListBox::builder()
            .selection_mode(gtk4::SelectionMode::Single)
            .build();
        listbox.add_css_class("navigation-sidebar");
        listbox.set_margin_top(6);

        // Let the search entry capture typing events when the user navigates the listbox
        search_entry.set_key_capture_widget(Some(&listbox));

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .min_content_height(300)
            .max_content_height(500)
            .child(&listbox)
            .build();
        vbox.append(&scrolled);

        popover.set_child(Some(&vbox));

        // Add Escape key handler
        let ev_ctrl = gtk4::EventControllerKey::new();
        ev_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let popover_esc = popover.clone();
        ev_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk4::gdk::Key::Escape {
                popover_esc.popdown();
                gtk4::glib::Propagation::Stop
            } else {
                gtk4::glib::Propagation::Proceed
            }
        });
        popover.add_controller(ev_ctrl);

        let commands = vec![
            CommandItem { title: "New Window".to_string(), action: "win.new-window".to_string(), shortcut: Some("<Primary><Shift>N".to_string()) },
            CommandItem { title: "New Tab".to_string(), action: "win.new-tab".to_string(), shortcut: Some("<Primary><Shift>T".to_string()) },
            CommandItem { title: "AI Chat".to_string(), action: "win.ai-chat".to_string(), shortcut: Some("<Primary><Shift>E".to_string()) },
            CommandItem { title: "Models Selection".to_string(), action: "win.model-selection".to_string(), shortcut: None },
            CommandItem { title: "Themes".to_string(), action: "win.themes".to_string(), shortcut: Some("<Primary><Shift>K".to_string()) },
            CommandItem { title: "Preferences".to_string(), action: "win.preferences".to_string(), shortcut: Some("<Primary>comma".to_string()) },
            CommandItem { title: "Keyboard Shortcuts".to_string(), action: "win.shortcuts".to_string(), shortcut: Some("<Primary><Shift>question".to_string()) },
            CommandItem { title: "About".to_string(), action: "win.about".to_string(), shortcut: None },
            CommandItem { title: "GTK Inspector".to_string(), action: "app.inspector".to_string(), shortcut: None },
        ];

        for cmd in &commands {
            let row = gtk4::ListBoxRow::new();
            let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
            hbox.set_margin_start(12);
            hbox.set_margin_end(12);
            hbox.set_margin_top(8);
            hbox.set_margin_bottom(8);

            // Left side: Title and Action
            let left_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            let title_label = gtk4::Label::builder()
                .label(&cmd.title)
                .halign(gtk4::Align::Start)
                .build();
            let action_label = gtk4::Label::builder()
                .label(&cmd.action)
                .halign(gtk4::Align::Start)
                .css_classes(vec!["dim-label".to_string(), "caption-heading".to_string(), "monospace".to_string()])
                .build();
            
            left_vbox.append(&title_label);
            left_vbox.append(&action_label);
            
            left_vbox.set_hexpand(true);
            hbox.append(&left_vbox);

            // Right side: Shortcut Keys
            if let Some(ref sc) = cmd.shortcut {
                let shortcut_label = libadwaita::ShortcutLabel::new(sc);
                shortcut_label.set_valign(gtk4::Align::Center);
                shortcut_label.add_css_class("palette-shortcut");
                hbox.append(&shortcut_label);
            }

            row.set_child(Some(&hbox));
            listbox.append(&row);
        }

        let _search_entry_clone = search_entry.clone();
        let listbox_clone = listbox.clone();
        let cmds_for_search = commands.clone();
        search_entry.connect_search_changed(move |entry| {
            listbox_clone.invalidate_filter();
            let text = entry.text().to_string().to_lowercase();
            
            let mut found = false;
            // Auto-select the first visible item
            for (i, cmd) in cmds_for_search.iter().enumerate() {
                if text.is_empty() || cmd.title.to_lowercase().contains(&text) {
                    if let Some(row) = listbox_clone.row_at_index(i as i32) {
                        listbox_clone.select_row(Some(&row));
                        found = true;
                    }
                    break;
                }
            }
            
            if !found {
                listbox_clone.unselect_all();
            }
        });

        // Use the native search entry activation to trigger the selected row action
        let listbox_activate = listbox.clone();
        let cmds_for_activate = commands.clone();
        let popover_for_activate = popover.clone();
        search_entry.connect_activate(move |_| {
            if let Some(row) = listbox_activate.selected_row() {
                let idx = row.index() as usize;
                if idx < cmds_for_activate.len() {
                    let action_name = cmds_for_activate[idx].action.clone();
                    let pop_clone = popover_for_activate.clone();
                    gtk4::glib::idle_add_local(move || {
                        if let Some(window) = pop_clone.root().and_then(|r| r.downcast::<gtk4::Window>().ok()) {
                            let _ = window.activate_action(&action_name, None);
                        }
                        gtk4::glib::ControlFlow::Break
                    });
                    popover_for_activate.popdown();
                }
            }
        });

        let cmds2 = commands.clone();
        let search_entry_filter = search_entry.clone();
        listbox.set_filter_func(move |row| {
            let index = row.index() as usize;
            if index >= cmds2.len() {
                return false;
            }
            let title = cmds2[index].title.to_lowercase();
            let text = search_entry_filter.text().to_string().to_lowercase();
            text.is_empty() || title.contains(&text)
        });

        let popover_clone = popover.clone();
        let cmds3 = commands.clone();
        listbox.connect_row_activated(move |_, row| {
            let index = row.index() as usize;
            if index < cmds3.len() {
                let action_name = &cmds3[index].action;
                // Defer action execution slightly to let popover close cleanly
                let action = action_name.clone();
                let pop_clone = popover_clone.clone();
                gtk4::glib::idle_add_local(move || {
                    if let Some(window) = pop_clone.root().and_then(|r| r.downcast::<gtk4::Window>().ok()) {
                        let _ = window.activate_action(&action, None);
                    }
                    gtk4::glib::ControlFlow::Break
                });
            }
            popover_clone.popdown();
        });

        // On show, focus search entry and select first row
        let search_entry_focus = search_entry.clone();
        let listbox_focus = listbox.clone();
        let scrolled_focus = scrolled.clone();
        popover.connect_map(move |_| {
            search_entry_focus.set_text("");
            // ensure all items visible again
            listbox_focus.invalidate_filter();

            // Reset scroll position to top
            scrolled_focus.vadjustment().set_value(0.0);

            // Defer focus and selection slightly to ensure popover is fully ready
            let entry = search_entry_focus.clone();
            let list = listbox_focus.clone();
            gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                entry.grab_focus();
                if let Some(first_row) = list.row_at_index(0) {
                    list.select_row(Some(&first_row));
                }
                gtk4::glib::ControlFlow::Break
            });
        });

        // Add Up/Down key handling for the search entry to navigate the listbox
        let entry_key_ctrl = gtk4::EventControllerKey::new();
        let cmds_for_nav = commands.clone();
        let search_entry_nav = search_entry.clone();
        entry_key_ctrl.connect_key_pressed(gtk4::glib::clone!(
            #[weak]
            listbox,
            #[upgrade_or]
            gtk4::glib::Propagation::Proceed,
            move |_, keyval, _, _| {
                let text = search_entry_nav.text().to_string().to_lowercase();
                let is_visible = |idx: usize| -> bool {
                    if idx >= cmds_for_nav.len() { return false; }
                    text.is_empty() || cmds_for_nav[idx].title.to_lowercase().contains(&text)
                };

                match keyval {
                    gtk4::gdk::Key::Up => {
                        if let Some(row) = listbox.selected_row() {
                            let mut idx = row.index() - 1;
                            while idx >= 0 {
                                if is_visible(idx as usize) {
                                    if let Some(prev) = listbox.row_at_index(idx) {
                                        listbox.select_row(Some(&prev));
                                        
                                        // Scroll to show the row, then return focus to search entry immediately
                                        prev.grab_focus();
                                        search_entry_nav.grab_focus();
                                    }
                                    break;
                                }
                                idx -= 1;
                            }
                        }
                        gtk4::glib::Propagation::Stop
                    }
                    gtk4::gdk::Key::Down => {
                        let mut idx = if let Some(row) = listbox.selected_row() {
                            row.index() + 1
                        } else {
                            0
                        };
                        let max_idx = cmds_for_nav.len() as i32;
                        while idx < max_idx {
                            if is_visible(idx as usize) {
                                if let Some(next) = listbox.row_at_index(idx) {
                                    listbox.select_row(Some(&next));
                                    
                                    // Scroll to show the row, then return focus to search entry immediately
                                    next.grab_focus();
                                    search_entry_nav.grab_focus();
                                }
                                break;
                            }
                            idx += 1;
                        }
                        gtk4::glib::Propagation::Stop
                    }
                    _ => gtk4::glib::Propagation::Proceed,
                }
            }
        ));
        search_entry.add_controller(entry_key_ctrl);

        Self { popover, _search_entry: search_entry }
    }

    pub fn widget(&self) -> &gtk4::Popover {
        &self.popover
    }

    pub fn show(&self, parent: &impl IsA<gtk4::Widget>) {
        if self.popover.parent().as_ref() != Some(parent.upcast_ref()) {
            if self.popover.parent().is_some() {
                self.popover.unparent();
            }
            self.popover.set_parent(parent);
        }

        // Ensure center alignment relative to the pointing rect
        self.popover.set_halign(gtk4::Align::Center);
        self.popover.set_valign(gtk4::Align::Start);
        self.popover.set_position(gtk4::PositionType::Bottom);

        // Center horizontally in parent
        let width = parent.width();
        let rect = gtk4::gdk::Rectangle::new(width / 2, 60, 0, 0);
        self.popover.set_pointing_to(Some(&rect));

        self.popover.popup();
    }
    pub fn show_as_menu(&self, button: &gtk4::Button) {
        if self.popover.parent().as_ref() != Some(button.upcast_ref()) {
            if self.popover.parent().is_some() {
                self.popover.unparent();
            }
            self.popover.set_parent(button);
        }

        // Reset alignments so it acts like a normal popover under the button
        self.popover.set_halign(gtk4::Align::Fill);
        self.popover.set_valign(gtk4::Align::Fill);
        self.popover.set_position(gtk4::PositionType::Bottom);

        // Clear custom pointing rect so it points at the whole button
        self.popover.set_pointing_to(None::<&gtk4::gdk::Rectangle>);

        self.popover.popup();
    }
}

impl Default for CommandPaletteComponent {
    fn default() -> Self {
        Self::new()
    }
}
