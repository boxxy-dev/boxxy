use crate::preview::ThemePreview;
use crate::{ParsedPaletteStatic, THEMES};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Zero-allocation case-insensitive ASCII substring check.
#[inline]
fn contains_ignore_ascii_case(haystack: &str, needle_lower: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle_lower.as_bytes();
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len())
        .any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

mod imp {
    use super::*;
    use std::cell::Cell;

    pub struct PaletteObject {
        pub palette: Cell<Option<ParsedPaletteStatic>>,
        pub bg: Cell<gdk::RGBA>,
        pub fg: Cell<gdk::RGBA>,
        pub swatches: Cell<[gdk::RGBA; 6]>,
    }

    impl Default for PaletteObject {
        fn default() -> Self {
            Self {
                palette: Cell::new(None),
                bg: Cell::new(gdk::RGBA::BLACK),
                fg: Cell::new(gdk::RGBA::WHITE),
                swatches: Cell::new([gdk::RGBA::BLACK; 6]),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PaletteObject {
        const NAME: &'static str = "PaletteObject";
        type Type = super::PaletteObject;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for PaletteObject {}
}

glib::wrapper! {
    pub struct PaletteObject(ObjectSubclass<imp::PaletteObject>);
}

impl PaletteObject {
    pub fn new(palette: ParsedPaletteStatic) -> Self {
        let obj: Self = glib::Object::builder().build();
        let imp = obj.imp();
        imp.palette.set(Some(palette));
        imp.bg.set(
            gdk::RGBA::parse(palette.light.background).unwrap_or(gdk::RGBA::BLACK),
        );
        imp.fg.set(
            gdk::RGBA::parse(palette.light.foreground).unwrap_or(gdk::RGBA::WHITE),
        );
        let swatches = std::array::from_fn(|i| {
            gdk::RGBA::parse(palette.light.colors[i + 1]).unwrap_or(gdk::RGBA::BLACK)
        });
        imp.swatches.set(swatches);
        obj
    }

    #[inline]
    pub fn get_palette(&self) -> Option<ParsedPaletteStatic> {
        self.imp().palette.get()
    }

    #[inline]
    pub fn preview_data(&self) -> Option<(&'static str, gdk::RGBA, gdk::RGBA, [gdk::RGBA; 6])> {
        let imp = self.imp();
        let palette = imp.palette.get()?;
        Some((
            palette.name,
            imp.bg.get(),
            imp.fg.get(),
            imp.swatches.get(),
        ))
    }
}

pub struct ThemeSelectorComponent {
    widget: gtk::Box,
    selection_model: gtk::SingleSelection,
}

impl ThemeSelectorComponent {
    pub fn new<F: Fn(ParsedPaletteStatic) + 'static>(on_select: F) -> Self {
        let store = gio::ListStore::new::<PaletteObject>();

        // Batch-insert all palettes in a single GListModel emission.
        let items: Vec<PaletteObject> = THEMES.iter().map(|t| PaletteObject::new(*t)).collect();
        store.extend_from_slice(&items);

        let query_state: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let query_for_filter = query_state.clone();

        let filter = gtk::CustomFilter::new(move |item| {
            let q = query_for_filter.borrow();
            if q.is_empty() {
                return true;
            }
            if let Some(obj) = item.downcast_ref::<PaletteObject>()
                && let Some(palette) = obj.get_palette()
            {
                return contains_ignore_ascii_case(palette.name, &q)
                    || contains_ignore_ascii_case(palette.id, &q);
            }
            false
        });

        let filter_model = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));

        let selection_model = gtk::SingleSelection::new(Some(filter_model.clone()));
        selection_model.set_autoselect(false);

        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let preview = ThemePreview::new();
            preview.set_margin_top(4);
            preview.set_margin_bottom(4);
            preview.set_margin_start(8);
            preview.set_margin_end(8);
            list_item.set_child(Some(&preview));
        });

        factory.connect_bind(move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let preview = list_item
                .child()
                .unwrap()
                .downcast::<ThemePreview>()
                .unwrap();

            let item = list_item
                .item()
                .unwrap()
                .downcast::<PaletteObject>()
                .unwrap();
            if let Some((name, bg, fg, colors)) = item.preview_data() {
                preview.set_theme(name, bg, fg, colors);
            }
        });
        let list_view = gtk::ListView::builder()
            .model(&selection_model)
            .factory(&factory)
            .margin_start(8)
            .margin_end(8)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        list_view.add_css_class("navigation-sidebar");

        let on_select_rc = Rc::new(on_select);

        // Debounce source ID: cancel any pending timer when selection changes
        // again, so rapid arrow-key repeats don't queue up heavy theme work.
        let debounce_id: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));

        selection_model.connect_selected_item_notify(move |sel| {
            // Cancel any previously scheduled apply.
            if let Some(id) = debounce_id.take() {
                id.remove();
            }

            let Some(item) = sel.selected_item() else {
                return;
            };
            let Ok(obj) = item.downcast::<PaletteObject>() else {
                return;
            };
            let Some(palette) = obj.get_palette() else {
                return;
            };

            let on_select_clone = on_select_rc.clone();
            let debounce_id_clone = debounce_id.clone();

            let id =
                glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                    debounce_id_clone.set(None);
                    on_select_clone(palette);
                });
            debounce_id.set(Some(id));
        });

        let scrolled_window = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list_view)
            .vexpand(true)
            .build();

        let search_entry = gtk::SearchEntry::builder()
            .margin_top(8)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .placeholder_text("Filter colors...")
            .width_request(249)
            .halign(gtk::Align::Center)
            .hexpand(false)
            .build();

        search_entry.connect_search_changed(move |entry| {
            let new_query = entry.text().to_ascii_lowercase();
            let change = {
                let mut prev = query_state.borrow_mut();
                if *prev == new_query {
                    return;
                }
                let change = if new_query.starts_with(prev.as_str()) {
                    gtk::FilterChange::MoreStrict
                } else if prev.starts_with(new_query.as_str()) {
                    gtk::FilterChange::LessStrict
                } else {
                    gtk::FilterChange::Different
                };
                *prev = new_query;
                change
            };
            filter.changed(change);
        });

        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        widget.append(&search_entry);
        widget.append(&scrolled_window);

        Self {
            widget,
            selection_model,
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn select_theme(&self, id: &str) {
        let model = self.selection_model.clone();
        for i in 0..model.n_items() {
            if let Some(item) = model.item(i)
                && let Ok(obj) = item.downcast::<PaletteObject>()
                && let Some(palette) = obj.get_palette()
                && palette.id == id
            {
                self.selection_model.set_selected(i);
                break;
            }
        }
    }
}
