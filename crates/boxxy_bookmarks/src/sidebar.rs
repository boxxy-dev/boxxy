use crate::Bookmark;
use crate::editor::BookmarkEditor;
use crate::manager::{BOOKMARKS_EVENT_BUS, BookmarkEvent, BookmarksManager};
use crate::parser::BookmarkTemplate;
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
    sidebar_stack: gtk::Stack,
    list_box: gtk::ListBox,
    on_execute: Rc<dyn Fn(String, String, String)>,
}

impl std::fmt::Debug for BookmarksSidebarComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookmarksSidebarComponent").finish()
    }
}

impl BookmarksSidebarComponent {
    pub fn new<FE, FO>(on_execute: FE, on_open_tab: FO) -> Self
    where
        FE: Fn(String, String, String) + 'static,
        FO: Fn() + 'static,
    {
        let builder =
            gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/bookmarks_sidebar.ui");

        let widget: gtk::Box = builder
            .object("root_box")
            .expect("root_box in bookmarks_sidebar.ui");
        let sidebar_stack: gtk::Stack = builder
            .object("sidebar_stack")
            .expect("sidebar_stack in bookmarks_sidebar.ui");
        let list_box: gtk::ListBox = builder
            .object("list_box")
            .expect("list_box in bookmarks_sidebar.ui");
        let empty_open_tab_btn: gtk::Label = builder
            .object("empty_open_tab_btn")
            .expect("empty_open_tab_btn in bookmarks_sidebar.ui");
        let empty_new_btn: gtk::Label = builder
            .object("empty_new_btn")
            .expect("empty_new_btn in bookmarks_sidebar.ui");
        let footer_open_tab_btn: gtk::Label = builder
            .object("footer_open_tab_btn")
            .expect("footer_open_tab_btn in bookmarks_sidebar.ui");
        let footer_new_btn: gtk::Label = builder
            .object("footer_new_btn")
            .expect("footer_new_btn in bookmarks_sidebar.ui");

        let on_open_tab: Rc<dyn Fn()> = Rc::new(on_open_tab);

        for open_lbl in [&empty_open_tab_btn, &footer_open_tab_btn] {
            open_lbl.set_cursor_from_name(Some("pointer"));
            let open_cb = on_open_tab.clone();
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(move |_, _, _, _| {
                open_cb();
            });
            open_lbl.add_controller(gesture);
        }

        for new_lbl in [&empty_new_btn, &footer_new_btn] {
            new_lbl.set_cursor_from_name(Some("pointer"));
            let lbl_weak = new_lbl.downgrade();
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(move |_, _, _, _| {
                if let Some(lbl) = lbl_weak.upgrade()
                    && let Some(root) = lbl.root()
                {
                    BookmarkEditor::show(&root, None, None);
                }
            });
            new_lbl.add_controller(gesture);
        }

        let inner = Rc::new(RefCell::new(BookmarksSidebarInner {
            sidebar_stack,
            list_box,
            on_execute: Rc::new(on_execute),
        }));

        let comp = Self { widget, inner };

        comp.load_bookmarks();

        // Listen for bookmark events
        let c = comp.clone();
        let mut rx = BOOKMARKS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            while let Ok(
                BookmarkEvent::Added(_)
                | BookmarkEvent::Removed(_)
                | BookmarkEvent::Reloaded
                | BookmarkEvent::Reordered(_)
                | BookmarkEvent::Updated(_),
            )
            | Err(crate::manager::BroadcastRecvError::Lagged(_)) = rx.recv().await
            {
                c.load_bookmarks();
            }
        });

        comp
    }

    #[must_use]
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    fn load_bookmarks(&self) {
        let inner = self.inner.borrow();
        while let Some(child) = inner.list_box.first_child() {
            inner.list_box.remove(&child);
        }

        let bookmarks = BookmarksManager::list();
        if bookmarks.is_empty() {
            inner.sidebar_stack.set_visible_child_name("empty");
        } else {
            for bm in bookmarks {
                let row = self.create_row(&bm);
                inner.list_box.append(&row);
            }
            inner.sidebar_stack.set_visible_child_name("list");
        }
    }

    fn create_row(&self, bm: &Bookmark) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("activatable");

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        hbox.set_margin_top(8);
        hbox.set_margin_bottom(8);
        hbox.set_margin_start(10);
        hbox.set_margin_end(10);

        let script_icon = gtk::Image::new();
        script_icon.set_pixel_size(16);

        let mut placeholder_count = 0;
        if let Some(script) = BookmarksManager::get_script(&bm.filename) {
            let lang = bm.language(&script);
            script_icon.set_icon_name(Some(lang.icon_name()));
            placeholder_count = BookmarkTemplate::parse(&script).placeholders.len();
        } else {
            script_icon.set_icon_name(Some("boxxy-dialog-warning-symbolic"));
        }

        hbox.append(&script_icon);

        let label = gtk::Label::new(Some(&bm.name));
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hbox.append(&label);

        if placeholder_count > 0 {
            let var_badge = gtk::Label::new(Some(&format!("{{{placeholder_count}}}")));
            var_badge.add_css_class("bookmark-var-chip");
            var_badge.add_css_class("monospace");
            var_badge.add_css_class("caption");
            var_badge.set_tooltip_text(Some(&format!(
                "{placeholder_count} template variable{}",
                if placeholder_count == 1 { "" } else { "s" }
            )));
            hbox.append(&var_badge);
        }

        let drag_icon = gtk::Image::from_icon_name("list-drag-handle-symbolic");
        drag_icon.add_css_class("dim-label");
        hbox.append(&drag_icon);

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

        // Drag and Drop Reordering
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
                && let Ok(source_id) = Uuid::parse_str(&source_id_str)
            {
                if source_id != target_id {
                    Self::reorder_bookmarks(source_id, target_id);
                    let _ = &c;
                }
                return true;
            }
            false
        });
        row.add_controller(drop_target);

        row
    }

    fn reorder_bookmarks(source_id: Uuid, target_id: Uuid) {
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
