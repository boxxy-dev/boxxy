use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct CommandItem {
    pub title: String,
    pub action: String,
    pub parameter: Option<gtk4::glib::Variant>,
    pub shortcut: Option<String>,
}

#[derive(Clone)]
pub struct CommandPaletteComponent {
    popover: gtk4::Popover,
    inner: Rc<RefCell<CommandPaletteInner>>,
}

struct CommandPaletteInner {
    search_entry: gtk4::SearchEntry,
    listbox: gtk4::ListBox,
    scrolled_window: gtk4::ScrolledWindow,
    static_commands: Vec<CommandItem>,
    dynamic_commands: Vec<CommandItem>,
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
            .width_request(450)
            .activates_default(false)
            .build();
        vbox.append(&search_entry);

        let listbox = gtk4::ListBox::builder()
            .selection_mode(gtk4::SelectionMode::Single)
            .build();
        listbox.add_css_class("navigation-sidebar");
        listbox.set_margin_top(6);

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

        let static_commands = vec![
            CommandItem {
                title: "New Window".to_string(),
                action: "win.new-window".to_string(),
                parameter: None,
                shortcut: Some("<Primary><Shift>N".to_string()),
            },
            CommandItem {
                title: "New Tab".to_string(),
                action: "win.new-tab".to_string(),
                parameter: None,
                shortcut: Some("<Primary><Shift>T".to_string()),
            },
            CommandItem {
                title: "AI Chat".to_string(),
                action: "win.ai-chat".to_string(),
                parameter: None,
                shortcut: Some("<Primary><Shift>E".to_string()),
            },
            CommandItem {
                title: "Claw".to_string(),
                action: "win.claw".to_string(),
                parameter: None,
                shortcut: None,
            },
            CommandItem {
                title: "Bookmarks".to_string(),
                action: "win.bookmarks".to_string(),
                parameter: None,
                shortcut: None,
            },
            CommandItem {
                title: "Models Selection".to_string(),
                action: "win.model-selection".to_string(),
                parameter: None,
                shortcut: None,
            },
            CommandItem {
                title: "Themes".to_string(),
                action: "win.themes".to_string(),
                parameter: None,
                shortcut: Some("<Primary><Shift>K".to_string()),
            },
            CommandItem {
                title: "Preferences".to_string(),
                action: "win.preferences".to_string(),
                parameter: None,
                shortcut: Some("<Primary>comma".to_string()),
            },
            CommandItem {
                title: "Keyboard Shortcuts".to_string(),
                action: "win.shortcuts".to_string(),
                parameter: None,
                shortcut: Some("<Primary><Shift>question".to_string()),
            },
            CommandItem {
                title: "About".to_string(),
                action: "win.about".to_string(),
                parameter: None,
                shortcut: None,
            },
            CommandItem {
                title: "GTK Inspector".to_string(),
                action: "app.inspector".to_string(),
                parameter: None,
                shortcut: None,
            },
        ];

        let inner = Rc::new(RefCell::new(CommandPaletteInner {
            search_entry: search_entry.clone(),
            listbox: listbox.clone(),
            scrolled_window: scrolled.clone(),
            static_commands,
            dynamic_commands: Vec::new(),
        }));

        let comp = Self {
            popover: popover.clone(),
            inner: inner.clone(),
        };

        comp.rebuild_list();

        let inner_search = inner.clone();
        search_entry.connect_search_changed(move |entry| {
            let inner = inner_search.borrow();
            inner.listbox.invalidate_filter();
            let text = entry.text().to_string().to_lowercase();

            let mut found = false;
            let all_cmds = inner.all_commands();
            for (i, cmd) in all_cmds.iter().enumerate() {
                if text.is_empty() || cmd.title.to_lowercase().contains(&text) {
                    if let Some(row) = inner.listbox.row_at_index(i as i32) {
                        inner.listbox.select_row(Some(&row));
                        found = true;
                    }
                    break;
                }
            }

            if !found {
                inner.listbox.unselect_all();
            }
        });

        let inner_activate = inner.clone();
        let pop_activate = popover.clone();
        search_entry.connect_activate(move |_| {
            let inner = inner_activate.borrow();
            if let Some(row) = inner.listbox.selected_row() {
                let idx = row.index() as usize;
                let all_cmds = inner.all_commands();
                if idx < all_cmds.len() {
                    let cmd = &all_cmds[idx];
                    let action_name = cmd.action.clone();
                    let param = cmd.parameter.clone();
                    let pop_clone = pop_activate.clone();
                    gtk4::glib::idle_add_local(move || {
                        if let Some(window) = pop_clone
                            .root()
                            .and_then(|r| r.downcast::<gtk4::Window>().ok())
                        {
                            let _ = window.activate_action(&action_name, param.as_ref());
                        }
                        gtk4::glib::ControlFlow::Break
                    });
                    pop_activate.popdown();
                }
            }
        });

        let inner_filter = inner.clone();
        listbox.set_filter_func(move |row| {
            let inner = inner_filter.borrow();
            let index = row.index() as usize;
            let all_cmds = inner.all_commands();
            if index >= all_cmds.len() {
                return false;
            }
            let title = all_cmds[index].title.to_lowercase();
            let text = inner.search_entry.text().to_string().to_lowercase();
            text.is_empty() || title.contains(&text)
        });

        let inner_row_act = inner.clone();
        let pop_row_act = popover.clone();
        listbox.connect_row_activated(move |_, row| {
            let inner = inner_row_act.borrow();
            let index = row.index() as usize;
            let all_cmds = inner.all_commands();
            if index < all_cmds.len() {
                let action_name = all_cmds[index].action.clone();
                let param = all_cmds[index].parameter.clone();
                let pop_clone = pop_row_act.clone();
                gtk4::glib::idle_add_local(move || {
                    if let Some(window) = pop_clone
                        .root()
                        .and_then(|r| r.downcast::<gtk4::Window>().ok())
                    {
                        let _ = window.activate_action(&action_name, param.as_ref());
                    }
                    gtk4::glib::ControlFlow::Break
                });
            }
            pop_row_act.popdown();
        });

        popover.connect_map(move |_| {
            let inner = inner.borrow();
            inner.search_entry.set_text("");
            inner.listbox.invalidate_filter();
            inner.scrolled_window.vadjustment().set_value(0.0);

            let entry = inner.search_entry.clone();
            let list = inner.listbox.clone();
            gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                entry.grab_focus();
                if let Some(first_row) = list.row_at_index(0) {
                    list.select_row(Some(&first_row));
                }
                gtk4::glib::ControlFlow::Break
            });
        });

        let entry_key_ctrl = gtk4::EventControllerKey::new();
        let inner_nav = comp.inner.clone();
        entry_key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            let inner = inner_nav.borrow();
            let text = inner.search_entry.text().to_string().to_lowercase();
            let all_cmds = inner.all_commands();
            let is_visible = |idx: usize| -> bool {
                if idx >= all_cmds.len() {
                    return false;
                }
                text.is_empty() || all_cmds[idx].title.to_lowercase().contains(&text)
            };

            match keyval {
                gtk4::gdk::Key::Up if let Some(row) = inner.listbox.selected_row() => {
                    let mut idx = row.index() - 1;
                    while idx >= 0 {
                        if is_visible(idx as usize) {
                            if let Some(prev) = inner.listbox.row_at_index(idx) {
                                inner.listbox.select_row(Some(&prev));
                                prev.grab_focus();
                                inner.search_entry.grab_focus();
                            }
                            break;
                        }
                        idx -= 1;
                    }
                    gtk4::glib::Propagation::Stop
                }
                gtk4::gdk::Key::Up => gtk4::glib::Propagation::Stop,
                gtk4::gdk::Key::Down => {
                    let mut idx = if let Some(row) = inner.listbox.selected_row() {
                        row.index() + 1
                    } else {
                        0
                    };
                    let max_idx = all_cmds.len() as i32;
                    while idx < max_idx {
                        if is_visible(idx as usize) {
                            if let Some(next) = inner.listbox.row_at_index(idx) {
                                inner.listbox.select_row(Some(&next));
                                next.grab_focus();
                                inner.search_entry.grab_focus();
                            }
                            break;
                        }
                        idx += 1;
                    }
                    gtk4::glib::Propagation::Stop
                }
                _ => gtk4::glib::Propagation::Proceed,
            }
        });
        search_entry.add_controller(entry_key_ctrl);

        comp
    }

    fn rebuild_list(&self) {
        let inner = self.inner.borrow();
        while let Some(child) = inner.listbox.first_child() {
            inner.listbox.remove(&child);
        }

        let all_cmds = inner.all_commands();
        for cmd in all_cmds {
            let row = gtk4::ListBoxRow::new();
            let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
            hbox.set_margin_start(12);
            hbox.set_margin_end(12);
            hbox.set_margin_top(8);
            hbox.set_margin_bottom(8);

            let left_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            let title_label = gtk4::Label::builder()
                .label(&cmd.title)
                .halign(gtk4::Align::Start)
                .build();
            let action_label = gtk4::Label::builder()
                .label(&cmd.action)
                .halign(gtk4::Align::Start)
                .css_classes(vec![
                    "dim-label".to_string(),
                    "caption-heading".to_string(),
                    "monospace".to_string(),
                ])
                .build();

            left_vbox.append(&title_label);
            left_vbox.append(&action_label);

            left_vbox.set_hexpand(true);
            hbox.append(&left_vbox);

            if let Some(ref sc) = cmd.shortcut {
                let shortcut_label = libadwaita::ShortcutLabel::new(sc);
                shortcut_label.set_valign(gtk4::Align::Center);
                shortcut_label.add_css_class("palette-shortcut");
                hbox.append(&shortcut_label);
            }

            row.set_child(Some(&hbox));
            inner.listbox.append(&row);
        }
    }

    pub fn set_dynamic_commands(&self, commands: Vec<CommandItem>) {
        self.inner.borrow_mut().dynamic_commands = commands;
        self.rebuild_list();
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

        self.popover.set_halign(gtk4::Align::Center);
        self.popover.set_valign(gtk4::Align::Start);
        self.popover.set_position(gtk4::PositionType::Bottom);

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

        self.popover.set_halign(gtk4::Align::Fill);
        self.popover.set_valign(gtk4::Align::Fill);
        self.popover.set_position(gtk4::PositionType::Bottom);
        self.popover.set_pointing_to(None::<&gtk4::gdk::Rectangle>);
        self.popover.popup();
    }
}

impl CommandPaletteInner {
    fn all_commands(&self) -> Vec<CommandItem> {
        let mut all = self.static_commands.clone();
        all.extend(self.dynamic_commands.clone());
        all
    }
}

impl Default for CommandPaletteComponent {
    fn default() -> Self {
        Self::new()
    }
}
