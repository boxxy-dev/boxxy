use adw::prelude::*;
use boxxy_core_widgets::ObjectExtSafe;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use sourceview5::prelude::*;

use crate::Bookmark;
use crate::editor::BookmarkEditor;
use crate::manager::{BOOKMARKS_EVENT_BUS, BookmarkEvent, BookmarksManager};
use crate::parser::BookmarkTemplate;
use std::cell::RefCell;
use std::rc::Rc;

struct CardWidgets {
    script_icon: gtk::Image,
    name_label: gtk::Label,
    filename_label: gtk::Label,
    vars_box: gtk::Box,
    run_btn: gtk::Button,
    copy_btn: gtk::Button,
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
    main_stack: gtk::Stack,
    results_stack: gtk::Stack,
    count_badge: gtk::Label,
    list_store: gtk::gio::ListStore,
    filter: gtk::CustomFilter,
    filter_model: gtk::FilterListModel,
    on_run: Rc<dyn Fn(String, String, String)>,
}

impl std::fmt::Debug for BookmarksTabComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookmarksTabComponent").finish()
    }
}

impl BookmarksTabComponent {
    pub fn new<F: Fn(String, String, String) + 'static>(on_run: F) -> Self {
        let builder =
            gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/bookmarks_tab.ui");

        let widget: gtk::Box = builder
            .object("root_box")
            .expect("root_box in bookmarks_tab.ui");
        let main_stack: gtk::Stack = builder
            .object("main_stack")
            .expect("main_stack in bookmarks_tab.ui");
        let results_stack: gtk::Stack = builder
            .object("results_stack")
            .expect("results_stack in bookmarks_tab.ui");
        let count_badge: gtk::Label = builder
            .object("count_badge")
            .expect("count_badge in bookmarks_tab.ui");
        let search_entry: gtk::SearchEntry = builder
            .object("search_entry")
            .expect("search_entry in bookmarks_tab.ui");
        let empty_create_btn: gtk::Button = builder
            .object("empty_create_btn")
            .expect("empty_create_btn in bookmarks_tab.ui");
        let header_new_btn: gtk::Button = builder
            .object("header_new_btn")
            .expect("header_new_btn in bookmarks_tab.ui");
        let list_container: gtk::Box = builder
            .object("list_container")
            .expect("list_container in bookmarks_tab.ui");

        // Register shared preview font CSS from terminal settings
        let settings = boxxy_preferences::Settings::load();
        let palette = boxxy_themes::load_palette(&settings.theme);
        let is_dark = adw::StyleManager::default().is_dark();

        let preview_provider = gtk::CssProvider::new();
        let apply_preview_font = {
            let provider = preview_provider.clone();
            move |font_name: &str, font_size: u16| {
                let css = format!(
                    "textview.bookmark-preview-tv {{ font-family: \"{font_name}\"; font-size: {font_size}pt; background-color: @window_bg_color; }}"
                );
                provider.load_from_string(&css);
            }
        };
        apply_preview_font(&settings.font_name, settings.font_size);

        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &preview_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            let provider_cleanup = preview_provider;
            widget.connect_destroy(move |_| {
                if let Some(display) = gtk::gdk::Display::default() {
                    gtk::style_context_remove_provider_for_display(&display, &provider_cleanup);
                }
            });
        }

        {
            let mut settings_rx = boxxy_preferences::SETTINGS_EVENT_BUS.subscribe();
            glib::spawn_future_local(async move {
                loop {
                    match settings_rx.recv().await {
                        Ok(new_settings) => {
                            apply_preview_font(&new_settings.font_name, new_settings.font_size);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let latest = boxxy_preferences::Settings::load();
                            apply_preview_font(&latest.font_name, latest.font_size);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        let list_store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();

        let search_query = Rc::new(RefCell::new(String::new()));
        let query_for_filter = search_query.clone();
        let filter = gtk::CustomFilter::new(move |item| {
            let q = query_for_filter.borrow();
            if q.is_empty() {
                return true;
            }
            if let Some(obj) = item.downcast_ref::<glib::BoxedAnyObject>() {
                let bm = obj.borrow::<Bookmark>();
                if bm.name.to_ascii_lowercase().contains(q.as_str())
                    || bm.filename.to_ascii_lowercase().contains(q.as_str())
                {
                    return true;
                }
                if let Some(script) = BookmarksManager::get_script(&bm.filename) {
                    return script.to_ascii_lowercase().contains(q.as_str());
                }
            }
            false
        });

        let filter_model = gtk::FilterListModel::new(Some(list_store.clone()), Some(filter.clone()));
        let selection_model = gtk::NoSelection::new(Some(filter_model.clone()));

        let factory = gtk::SignalListItemFactory::new();

        let inner = Rc::new(RefCell::new(BookmarksTabInner {
            main_stack,
            results_stack: results_stack.clone(),
            count_badge,
            list_store,
            filter: filter.clone(),
            filter_model: filter_model.clone(),
            on_run: Rc::new(on_run),
        }));

        let inner_weak = Rc::downgrade(&inner);

        factory.connect_setup(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();

            let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
            card.add_css_class("card");
            card.add_css_class("bookmark-card");
            card.set_margin_bottom(14);

            let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            header.set_margin_top(12);
            header.set_margin_bottom(10);
            header.set_margin_start(14);
            header.set_margin_end(14);

            let script_icon = gtk::Image::new();
            script_icon.set_pixel_size(24);
            script_icon.set_valign(gtk::Align::Center);
            header.append(&script_icon);

            // Title + metadata column
            let title_col = gtk::Box::new(gtk::Orientation::Vertical, 3);
            title_col.set_hexpand(true);
            title_col.set_valign(gtk::Align::Center);

            let name_label = gtk::Label::new(None);
            name_label.add_css_class("heading");
            name_label.set_halign(gtk::Align::Start);
            name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title_col.append(&name_label);

            let meta_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            meta_row.set_halign(gtk::Align::Start);

            let filename_label = gtk::Label::new(None);
            filename_label.add_css_class("caption");
            filename_label.add_css_class("dim-label");
            filename_label.add_css_class("monospace");
            meta_row.append(&filename_label);

            let vars_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            meta_row.append(&vars_box);

            title_col.append(&meta_row);
            header.append(&title_col);

            // Action buttons
            let actions_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            actions_box.set_valign(gtk::Align::Center);

            let run_btn = gtk::Button::with_label("Run");
            run_btn.add_css_class("flat");
            run_btn.set_tooltip_text(Some("Run in New Tab"));

            let list_item_run = list_item.clone();
            let inner_weak_run = inner_weak.clone();
            run_btn.connect_clicked(move |_| {
                if let Some(obj) = list_item_run.item().and_downcast::<glib::BoxedAnyObject>() {
                    let bm = obj.borrow::<Bookmark>();
                    if let Some(script) = BookmarksManager::get_script(&bm.filename)
                        && let Some(inner) = inner_weak_run.upgrade()
                    {
                        (inner.borrow().on_run)(bm.name.clone(), bm.filename.clone(), script);
                    }
                }
            });
            actions_box.append(&run_btn);

            let copy_btn = gtk::Button::from_icon_name("boxxy-edit-copy-symbolic");
            copy_btn.add_css_class("flat");
            copy_btn.add_css_class("circular");
            copy_btn.set_tooltip_text(Some("Copy Script to Clipboard"));

            let list_item_copy = list_item.clone();
            copy_btn.connect_clicked(move |btn| {
                if let Some(obj) = list_item_copy.item().and_downcast::<glib::BoxedAnyObject>() {
                    let bm = obj.borrow::<Bookmark>();
                    if let Some(script) = BookmarksManager::get_script(&bm.filename) {
                        btn.clipboard().set_text(&script);
                    }
                }
            });
            actions_box.append(&copy_btn);

            let edit_btn = gtk::Button::from_icon_name("boxxy-document-edit-symbolic");
            edit_btn.add_css_class("flat");
            edit_btn.add_css_class("circular");
            edit_btn.set_tooltip_text(Some("Edit Bookmark"));

            let list_item_edit = list_item.clone();
            edit_btn.connect_clicked(move |btn| {
                if let Some(obj) = list_item_edit.item().and_downcast::<glib::BoxedAnyObject>() {
                    let bm = obj.borrow::<Bookmark>();
                    if let Some(root) = btn.root() {
                        BookmarkEditor::show(&root, Some(bm.clone()), None);
                    }
                }
            });
            actions_box.append(&edit_btn);

            let delete_btn = gtk::Button::from_icon_name("boxxy-user-trash-symbolic");
            delete_btn.add_css_class("flat");
            delete_btn.add_css_class("circular");
            delete_btn.set_tooltip_text(Some("Delete Bookmark"));

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
                            .body(format!("Are you sure you want to delete '{bm_name}'?"))
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
            actions_box.append(&delete_btn);

            header.append(&actions_box);
            card.append(&header);

            // Preview area wrapper
            let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
            preview_box.set_margin_start(14);
            preview_box.set_margin_end(14);
            preview_box.set_margin_bottom(14);

            let preview_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
            preview_container.add_css_class("bookmark-preview-frame");
            preview_container.set_overflow(gtk::Overflow::Hidden);

            let buffer = sourceview5::Buffer::new(None);
            let lang_manager = sourceview5::LanguageManager::default();
            buffer.set_language(lang_manager.language("sh").as_ref());
            boxxy_themes::apply_sourceview_palette(&buffer, palette.as_ref(), is_dark);

            let source_view = sourceview5::View::with_buffer(&buffer);
            source_view.set_monospace(true);
            source_view.set_editable(false);
            source_view.set_cursor_visible(false);
            source_view.set_show_line_numbers(false);
            source_view.set_top_margin(10);
            source_view.set_bottom_margin(10);
            source_view.set_left_margin(12);
            source_view.set_right_margin(12);
            source_view.add_css_class("bookmark-preview-tv");

            preview_container.append(&source_view);

            let more_label = gtk::Label::new(None);
            more_label.set_halign(gtk::Align::Start);
            more_label.set_margin_start(12);
            more_label.set_margin_bottom(8);
            more_label.add_css_class("caption");
            more_label.add_css_class("dim-label");
            more_label.add_css_class("monospace");
            preview_container.append(&more_label);

            preview_box.append(&preview_container);

            let missing_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            missing_box.set_margin_top(8);
            missing_box.set_margin_bottom(8);
            missing_box.set_margin_start(8);
            missing_box.set_margin_end(8);
            missing_box.set_halign(gtk::Align::Center);

            let icon = gtk::Image::from_icon_name("boxxy-dialog-warning-symbolic");
            icon.add_css_class("error");
            missing_box.append(&icon);

            let missing_label =
                gtk::Label::new(Some("File Missing. The script file could not be found."));
            missing_label.add_css_class("dim-label");
            missing_box.append(&missing_label);

            preview_box.append(&missing_box);
            card.append(&preview_box);

            list_item.set_child(Some(&card));
            list_item.set_safe_data(
                "card_widgets",
                CardWidgets {
                    script_icon,
                    name_label,
                    filename_label,
                    vars_box,
                    run_btn,
                    copy_btn,
                    buffer,
                    more_label,
                    preview_container,
                    missing_box,
                },
            );
        });

        factory.connect_bind(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(obj) = list_item.item().and_downcast::<glib::BoxedAnyObject>() {
                let bm = obj.borrow::<Bookmark>();
                if let Some(widgets) = list_item.get_safe_data::<CardWidgets>("card_widgets") {
                    widgets.name_label.set_text(&bm.name);
                    widgets.filename_label.set_text(&bm.filename);

                    while let Some(child) = widgets.vars_box.first_child() {
                        widgets.vars_box.remove(&child);
                    }

                    if let Some(script) = BookmarksManager::get_script(&bm.filename) {
                        let lang = bm.language(&script);
                        widgets.script_icon.set_icon_name(Some(lang.icon_name()));

                        let lang_mgr = sourceview5::LanguageManager::default();
                        widgets
                            .buffer
                            .set_language(lang_mgr.language(lang.sourceview_lang_id()).as_ref());

                        let tpl = BookmarkTemplate::parse(&script);
                        for var in tpl.placeholders.iter().take(4) {
                            let chip = gtk::Label::new(Some(&format!("{{{{{{{var}}}}}}}")));
                            chip.add_css_class("bookmark-var-chip");
                            chip.add_css_class("monospace");
                            chip.add_css_class("caption");
                            widgets.vars_box.append(&chip);
                        }
                        if tpl.placeholders.len() > 4 {
                            let extra =
                                gtk::Label::new(Some(&format!("+{}", tpl.placeholders.len() - 4)));
                            extra.add_css_class("caption");
                            extra.add_css_class("dim-label");
                            widgets.vars_box.append(&extra);
                        }

                        let total_lines = script.lines().count();
                        let preview_lines = 6;
                        let preview_text: String = script
                            .lines()
                            .take(preview_lines)
                            .collect::<Vec<_>>()
                            .join("\n");
                        widgets.buffer.set_text(&preview_text);

                        if total_lines > preview_lines {
                            let hidden = total_lines - preview_lines;
                            widgets
                                .more_label
                                .set_text(&format!("… +{hidden} more line{}", if hidden == 1 { "" } else { "s" }));
                            widgets.more_label.set_visible(true);
                        } else {
                            widgets.more_label.set_visible(false);
                        }

                        widgets.preview_container.set_visible(true);
                        widgets.missing_box.set_visible(false);
                        widgets.run_btn.set_sensitive(true);
                        widgets.copy_btn.set_sensitive(true);
                    } else {
                        widgets
                            .script_icon
                            .set_icon_name(Some("boxxy-dialog-warning-symbolic"));
                        widgets.preview_container.set_visible(false);
                        widgets.missing_box.set_visible(true);
                        widgets.run_btn.set_sensitive(false);
                        widgets.copy_btn.set_sensitive(false);
                    }
                }
            }
        });

        factory.connect_unbind(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(widgets) = list_item.get_safe_data::<CardWidgets>("card_widgets") {
                widgets.buffer.set_text("");
            }
        });

        let list_view = gtk::ListView::new(Some(selection_model), Some(factory));
        list_view.set_show_separators(false);
        list_view.add_css_class("bookmarks-gallery");
        list_container.append(&list_view);

        let comp = Self {
            widget,
            inner,
        };

        // Wire search filter
        let query_cell = search_query;
        let comp_search = comp.clone();
        search_entry.connect_search_changed(move |entry| {
            *query_cell.borrow_mut() = entry.text().trim().to_ascii_lowercase();
            comp_search.refresh_filter();
        });

        // Wire both Create buttons (empty state + gallery header)
        empty_create_btn.connect_clicked(move |btn| {
            if let Some(root) = btn.root() {
                BookmarkEditor::show(&root, None, None);
            }
        });

        header_new_btn.connect_clicked(move |btn| {
            if let Some(root) = btn.root() {
                BookmarkEditor::show(&root, None, None);
            }
        });

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

    fn refresh_filter(&self) {
        let inner = self.inner.borrow();
        inner.filter.changed(gtk::FilterChange::Different);
        if inner.filter_model.n_items() == 0 {
            inner.results_stack.set_visible_child_name("no-results");
        } else {
            inner.results_stack.set_visible_child_name("list");
        }
    }

    fn load_bookmarks(&self) {
        let inner = self.inner.borrow();
        inner.list_store.remove_all();

        let bookmarks = BookmarksManager::list();
        let count = bookmarks.len();
        inner.count_badge.set_label(&count.to_string());

        if bookmarks.is_empty() {
            inner.main_stack.set_visible_child_name("empty");
        } else {
            for bm in bookmarks {
                inner.list_store.append(&glib::BoxedAnyObject::new(bm));
            }
            inner.main_stack.set_visible_child_name("gallery");
            if inner.filter_model.n_items() == 0 {
                inner.results_stack.set_visible_child_name("no-results");
            } else {
                inner.results_stack.set_visible_child_name("list");
            }
        }
    }
}
