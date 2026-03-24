use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use sourceview5::prelude::*;

use crate::Bookmark;
use crate::editor::BookmarkEditor;
use crate::manager::{BOOKMARKS_EVENT_BUS, BookmarkEvent, BookmarksManager};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

struct CardWidgets {
    script_icon: gtk::Image,
    name_label: gtk::Label,
    run_btn: gtk::Button,
    buffer: sourceview5::Buffer,
    more_label: gtk::Label,
    preview_container: gtk::Box,
    missing_box: gtk::Box,
}

#[derive(Clone)]
pub struct BookmarksTabComponent {
    widget: gtk::Box,
    inner: Rc<RefCell<BookmarksTabInner>>,
}

struct BookmarksTabInner {
    list_store: gtk::gio::ListStore,
    on_run: Rc<dyn Fn(String, String, String)>,
}

impl std::fmt::Debug for BookmarksTabComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookmarksTabComponent").finish()
    }
}

impl BookmarksTabComponent {
    pub fn new<F: Fn(String, String, String) + 'static>(on_run: F) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let content_scroll = gtk::ScrolledWindow::new();
        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content_box.set_margin_top(24);
        content_box.set_margin_bottom(24);
        content_box.set_margin_start(12);
        content_box.set_margin_end(12);
        content_box.set_hexpand(true);
        content_box.set_vexpand(true);
        content_scroll.set_child(Some(&content_box));

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(800);
        clamp.set_child(Some(&content_scroll));
        widget.append(&clamp);

        let new_btn = gtk::Button::builder()
            .label("Create New Bookmark")
            .halign(gtk::Align::Start)
            .build();
        new_btn.add_css_class("flat");
        content_box.append(&new_btn);

        let list_store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection_model = gtk::NoSelection::new(Some(list_store.clone()));

        let factory = gtk::SignalListItemFactory::new();
        let widgets_map: Rc<RefCell<HashMap<gtk::ListItem, CardWidgets>>> =
            Rc::new(RefCell::new(HashMap::new()));

        let inner = Rc::new(RefCell::new(BookmarksTabInner {
            list_store: list_store.clone(),
            on_run: Rc::new(on_run),
        }));

        let inner_weak = Rc::downgrade(&inner);
        let widgets_map_setup = widgets_map.clone();

        factory.connect_setup(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();

            let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
            card.add_css_class("card");
            card.set_margin_bottom(12);

            let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            header.set_margin_top(12);
            header.set_margin_bottom(12);
            header.set_margin_start(12);
            header.set_margin_end(12);

            let script_icon = gtk::Image::new();
            script_icon.set_pixel_size(24);
            header.append(&script_icon);

            let name_label = gtk::Label::new(None);
            name_label.add_css_class("heading");
            name_label.set_halign(gtk::Align::Start);
            name_label.set_hexpand(true);
            header.append(&name_label);

            let run_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
            run_btn.add_css_class("flat");
            run_btn.add_css_class("circular");
            run_btn.set_tooltip_text(Some("Run in New Tab"));

            let list_item_run = list_item.clone();
            let inner_weak_run = inner_weak.clone();
            run_btn.connect_clicked(move |_| {
                if let Some(obj) = list_item_run.item().and_downcast::<glib::BoxedAnyObject>() {
                    let bm = obj.borrow::<Bookmark>();
                    if let Some(script) = BookmarksManager::get_script(&bm.filename)
                        && let Some(inner) = inner_weak_run.upgrade() {
                            (inner.borrow().on_run)(bm.name.clone(), bm.filename.clone(), script);
                        }
                }
            });
            header.append(&run_btn);

            let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
            edit_btn.add_css_class("flat");
            edit_btn.add_css_class("circular");

            let list_item_edit = list_item.clone();
            edit_btn.connect_clicked(move |btn| {
                if let Some(obj) = list_item_edit.item().and_downcast::<glib::BoxedAnyObject>() {
                    let bm = obj.borrow::<Bookmark>();
                    if let Some(root) = btn.root() {
                        BookmarkEditor::show(&root, Some(bm.clone()), None);
                    }
                }
            });
            header.append(&edit_btn);

            let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            delete_btn.add_css_class("flat");
            delete_btn.add_css_class("circular");

            let list_item_delete = list_item.clone();
            delete_btn.connect_clicked(move |btn| {
                if let Some(obj) = list_item_delete
                    .item()
                    .and_downcast::<glib::BoxedAnyObject>()
                {
                    let bm = obj.borrow::<Bookmark>();
                    let id = bm.id;
                    let bm_name = bm.name.clone();
                    if let Some(root) = btn.root().and_downcast::<gtk::Window>() {
                        let dialog = adw::AlertDialog::builder()
                            .heading("Delete Bookmark")
                            .body(format!("Are you sure you want to delete '{}'?", bm_name))
                            .build();
                        dialog.add_response("cancel", "Cancel");
                        dialog.add_response("delete", "Delete");
                        dialog.set_response_appearance(
                            "delete",
                            adw::ResponseAppearance::Destructive,
                        );

                        dialog.choose(Some(&root), gtk::gio::Cancellable::NONE, move |response| {
                            if response == "delete" {
                                BookmarksManager::delete(id);
                            }
                        });
                    }
                }
            });
            header.append(&delete_btn);

            card.append(&header);

            // Preview area wrapper
            let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
            preview_box.set_margin_start(12);
            preview_box.set_margin_end(12);
            preview_box.set_margin_bottom(12);

            let preview_container = gtk::Box::new(gtk::Orientation::Vertical, 0);

            let buffer = sourceview5::Buffer::new(None);
            let lang_manager = sourceview5::LanguageManager::default();
            buffer.set_language(lang_manager.language("sh").as_ref());

            let settings = boxxy_preferences::Settings::load();
            let palette = boxxy_themes::load_palette(&settings.theme);
            let is_dark = adw::StyleManager::default().is_dark();
            boxxy_themes::apply_sourceview_palette(&buffer, palette.as_ref(), is_dark);

            let source_view = sourceview5::View::with_buffer(&buffer);
            source_view.set_editable(false);
            source_view.set_cursor_visible(false);
            source_view.set_show_line_numbers(false);
            source_view.set_top_margin(8);
            source_view.set_bottom_margin(8);
            source_view.set_left_margin(8);
            source_view.set_right_margin(8);

            let provider = gtk::CssProvider::new();
            let css = format!(
                "textview {{ font-family: \"{}\"; background-color: @window_bg_color; }}",
                settings.font_name
            );
            provider.load_from_string(&css);
            #[allow(deprecated)]
            source_view
                .style_context()
                .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

            preview_container.append(&source_view);

            let more_label = gtk::Label::new(Some("..."));
            more_label.set_halign(gtk::Align::Start);
            more_label.set_margin_start(8);
            more_label.set_margin_bottom(4);
            more_label.set_opacity(0.5);
            preview_container.append(&more_label);

            preview_box.append(&preview_container);

            let missing_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            missing_box.set_margin_top(8);
            missing_box.set_margin_bottom(8);
            missing_box.set_margin_start(8);
            missing_box.set_margin_end(8);
            missing_box.set_halign(gtk::Align::Center);

            let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
            icon.add_css_class("error");
            missing_box.append(&icon);

            let missing_label =
                gtk::Label::new(Some("File Missing. The script file could not be found."));
            missing_label.add_css_class("dim-label");
            missing_box.append(&missing_label);

            preview_box.append(&missing_box);

            card.append(&preview_box);

            list_item.set_child(Some(&card));

            widgets_map_setup.borrow_mut().insert(
                list_item.clone(),
                CardWidgets {
                    script_icon,
                    name_label,
                    run_btn,
                    buffer,
                    more_label,
                    preview_container,
                    missing_box,
                },
            );
        });

        let widgets_map_bind = widgets_map.clone();
        factory.connect_bind(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(obj) = list_item.item().and_downcast::<glib::BoxedAnyObject>() {
                let bm = obj.borrow::<Bookmark>();
                let map = widgets_map_bind.borrow();
                if let Some(widgets) = map.get(list_item) {
                    widgets.name_label.set_text(&bm.name);

                    if let Some(script) = BookmarksManager::get_script(&bm.filename) {
                        if bm.filename.ends_with(".py")
                            || script.starts_with("#!/usr/bin/env python")
                            || script.starts_with("#!/usr/bin/python")
                            || script.starts_with("```python")
                        {
                            widgets.script_icon.set_icon_name(Some("python"));
                        } else {
                            widgets.script_icon.set_icon_name(Some("console"));
                        }

                        let preview_text: String =
                            script.lines().take(5).collect::<Vec<_>>().join("\n");
                        widgets.buffer.set_text(&preview_text);
                        widgets.more_label.set_visible(script.lines().count() > 5);

                        widgets.preview_container.set_visible(true);
                        widgets.missing_box.set_visible(false);
                        widgets.run_btn.set_sensitive(true);
                    } else {
                        widgets
                            .script_icon
                            .set_icon_name(Some("dialog-warning-symbolic"));
                        widgets.preview_container.set_visible(false);
                        widgets.missing_box.set_visible(true);
                        widgets.run_btn.set_sensitive(false);
                    }
                }
            }
        });

        let factory_unbind_map = widgets_map.clone();
        factory.connect_unbind(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(widgets) = factory_unbind_map.borrow().get(list_item) {
                // Clear the buffer to free memory when recycled
                widgets.buffer.set_text("");
            }
        });

        let list_view = gtk::ListView::new(Some(selection_model), Some(factory));
        list_view.set_show_separators(false);
        list_view.add_css_class("bookmarks-gallery");

        content_box.append(&list_view);

        let comp = Self {
            widget,
            inner: inner.clone(),
        };

        new_btn.connect_clicked(move |btn| {
            if let Some(root) = btn.root() {
                BookmarkEditor::show(&root, None, None);
            }
        });

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
        inner.list_store.remove_all();

        let bookmarks = BookmarksManager::list();
        for bm in bookmarks {
            inner.list_store.append(&glib::BoxedAnyObject::new(bm));
        }
    }
}
