use gtk4 as gtk;
use gtk::glib;
use gtk::gio;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::rc::Rc;
use std::cell::Cell;
use crate::{ParsedPaletteStatic, THEMES};
use crate::preview::ThemePreview;

mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct PaletteObject {
        pub palette: Cell<Option<ParsedPaletteStatic>>,
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
        obj.imp().palette.set(Some(palette));
        obj
    }

    pub fn get_palette(&self) -> Option<ParsedPaletteStatic> {
        self.imp().palette.get()
    }
}

pub struct ThemeSelectorComponent {
    widget: gtk::Box,
    selection_model: gtk::SingleSelection,
}

impl ThemeSelectorComponent {
    pub fn new<F: Fn(ParsedPaletteStatic) + 'static>(on_select: F) -> Self {
        let store = gio::ListStore::new::<PaletteObject>();
        
        for theme in THEMES.iter() {
            store.append(&PaletteObject::new(*theme));
        }

        let filter = gtk::CustomFilter::new(|_item| {
            true // Initial state accepts all
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
            let preview = list_item.child().unwrap().downcast::<ThemePreview>().unwrap();

            let item = list_item.item().unwrap().downcast::<PaletteObject>().unwrap();
            if let Some(palette) = item.get_palette() {
                let bg = gtk::gdk::RGBA::parse(palette.light.background).unwrap_or(gtk::gdk::RGBA::BLACK);
                let fg = gtk::gdk::RGBA::parse(palette.light.foreground).unwrap_or(gtk::gdk::RGBA::WHITE);

                let mut colors = Vec::new();
                for i in 1..=6 {
                    if let Ok(c) = gtk::gdk::RGBA::parse(palette.light.colors[i]) {
                        colors.push(c);
                    }
                }
                preview.set_theme(palette.name.to_string(), bg, fg, colors);
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

            let Some(item) = sel.selected_item() else { return };
            let Ok(obj) = item.downcast::<PaletteObject>() else { return };
            let Some(palette) = obj.get_palette() else { return };

            let on_select_clone = on_select_rc.clone();
            let debounce_id_clone = debounce_id.clone();

            let id = glib::timeout_add_local_once(
                std::time::Duration::from_millis(150),
                move || {
                    debounce_id_clone.set(None);
                    on_select_clone(palette);
                },
            );
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
            let query = entry.text().to_string().to_lowercase();
            filter.set_filter_func(move |item| {
                if query.is_empty() {
                    return true;
                }
                if let Some(obj) = item.downcast_ref::<PaletteObject>()
                    && let Some(palette) = obj.get_palette() {
                        return palette.name.to_lowercase().contains(&query) ||
                               palette.id.to_lowercase().contains(&query);
                    }
                false
            });
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
                        && palette.id == id {
                            self.selection_model.set_selected(i);
                            break;
                        }
        }
    }
}
