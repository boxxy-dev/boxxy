use crate::Bookmark;
use crate::manager::{BOOKMARKS_EVENT_BUS, BookmarkEvent, BookmarksManager};
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;
use uuid::Uuid;

#[derive(Clone)]
pub struct BookmarksSidebarComponent {
    widget: gtk::Box,
    inner: Rc<RefCell<BookmarksSidebarInner>>,
}

struct BookmarksSidebarInner {
    list_box: gtk::ListBox,
    on_execute: Rc<dyn Fn(String, String, String)>,
}

impl std::fmt::Debug for BookmarksSidebarComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookmarksSidebarComponent").finish()
    }
}

impl BookmarksSidebarComponent {
    pub fn new<F: Fn(String, String, String) + 'static>(on_execute: F) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.add_css_class("navigation-sidebar");
        scroll.set_child(Some(&list_box));

        widget.append(&scroll);

        let inner = Rc::new(RefCell::new(BookmarksSidebarInner {
            list_box: list_box.clone(),
            on_execute: Rc::new(on_execute),
        }));

        let comp = Self { widget, inner };

        comp.load_bookmarks();

        // Listen for events
        let c = comp.clone();
        let mut rx = BOOKMARKS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    BookmarkEvent::Added(_)
                    | BookmarkEvent::Removed(_)
                    | BookmarkEvent::Reloaded
                    | BookmarkEvent::Reordered(_)
                    | BookmarkEvent::Updated(_) => {
                        c.load_bookmarks();
                    }
                }
            }
        });

        comp
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    fn load_bookmarks(&self) {
        let inner = self.inner.borrow();
        while let Some(child) = inner.list_box.first_child() {
            inner.list_box.remove(&child);
        }

        let bookmarks = BookmarksManager::list();
        for bm in bookmarks {
            let row = self.create_row(bm);
            inner.list_box.append(&row);
        }
    }

    fn create_row(&self, bm: Bookmark) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("activatable");

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hbox.set_margin_top(8);
        hbox.set_margin_bottom(8);
        hbox.set_margin_start(12);
        hbox.set_margin_end(12);

        let script_icon = gtk::Image::new();
        script_icon.set_pixel_size(16);

        if let Some(script) = BookmarksManager::get_script(&bm.filename) {
            if bm.filename.ends_with(".py")
                || script.starts_with("#!/usr/bin/env python")
                || script.starts_with("#!/usr/bin/python")
                || script.starts_with("```python")
            {
                script_icon.set_icon_name(Some("python"));
            } else {
                script_icon.set_icon_name(Some("console"));
            }
        } else {
            script_icon.set_icon_name(Some("dialog-warning-symbolic"));
        }

        hbox.append(&script_icon);

        let label = gtk::Label::new(Some(&bm.name));
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hbox.append(&label);

        row.set_child(Some(&hbox));

        // Execution
        let gesture = gtk::GestureClick::new();
        let name = bm.name.clone();
        let filename = bm.filename.clone();
        let on_exec = self.inner.borrow().on_execute.clone();
        gesture.connect_released(move |_, _, _, _| {
            if let Some(script) = BookmarksManager::get_script(&filename) {
                on_exec(name.clone(), filename.clone(), script);
            }
        });
        row.add_controller(gesture);

        // Drag and Drop
        let drag_source = gtk::DragSource::new();
        drag_source.set_actions(gtk::gdk::DragAction::MOVE);
        let id_str = bm.id.to_string();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gtk::gdk::ContentProvider::for_value(&id_str.to_value()))
        });
        row.add_controller(drag_source);

        let drop_target = gtk::DropTarget::new(gtk::glib::Type::STRING, gtk::gdk::DragAction::MOVE);
        let target_id = bm.id;
        let c = self.clone();
        drop_target.connect_drop(move |_, value, _, _| {
            if let Ok(source_id_str) = value.get::<String>()
                && let Ok(source_id) = Uuid::parse_str(&source_id_str) {
                    if source_id != target_id {
                        c.reorder_bookmarks(source_id, target_id);
                    }
                    return true;
                }
            false
        });
        row.add_controller(drop_target);

        row
    }

    fn reorder_bookmarks(&self, source_id: Uuid, target_id: Uuid) {
        let bookmarks = BookmarksManager::list();
        let mut order: Vec<Uuid> = bookmarks.iter().map(|b| b.id).collect();

        if let (Some(from_idx), Some(to_idx)) = (
            order.iter().position(|id| id == &source_id),
            order.iter().position(|id| id == &target_id),
        ) {
            let item = order.remove(from_idx);
            order.insert(to_idx, item);
            BookmarksManager::reorder(order);
        }
    }
}
