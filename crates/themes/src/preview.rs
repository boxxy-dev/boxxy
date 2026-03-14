use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct ThemePreview {
        pub name: RefCell<String>,
        pub bg_color: RefCell<Option<gdk::RGBA>>,
        pub fg_color: RefCell<Option<gdk::RGBA>>,
        pub colors: RefCell<Vec<gdk::RGBA>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThemePreview {
        const NAME: &'static str = "ThemePreview";
        type Type = super::ThemePreview;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ThemePreview {}

    impl WidgetImpl for ThemePreview {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let width = self.obj().width() as f32;
            let height = self.obj().height() as f32;

            // Draw background
            if let Some(bg) = self.bg_color.borrow().as_ref() {
                let rect = gtk::graphene::Rect::new(0.0, 0.0, width, height);
                let radius = gtk::graphene::Size::new(8.0, 8.0);
                let rounded = gtk::gsk::RoundedRect::new(rect, radius, radius, radius, radius);
                snapshot.push_rounded_clip(&rounded);
                snapshot.append_color(bg, &rect);
                snapshot.pop();
            }

            // Draw text
            let name = self.name.borrow();
            if !name.is_empty() {
                let pango_ctx = self.obj().pango_context();
                let layout = gtk::pango::Layout::new(&pango_ctx);
                layout.set_text(&name);
                layout.set_ellipsize(gtk::pango::EllipsizeMode::End);
                layout.set_width((width - 150.0).max(0.0) as i32 * gtk::pango::SCALE);
                
                // Set bold font
                let mut font_desc = gtk::pango::FontDescription::new();
                font_desc.set_weight(gtk::pango::Weight::Bold);
                layout.set_font_description(Some(&font_desc));
                
                let (_, logical) = layout.extents();
                let text_height = logical.height() as f32 / gtk::pango::SCALE as f32;
                
                let fg = self.fg_color.borrow().unwrap_or_else(|| gdk::RGBA::new(1.0, 1.0, 1.0, 1.0));
                
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(12.0, (height - text_height) / 2.0));
                snapshot.append_layout(&layout, &fg);
                snapshot.restore();
            }

            // Draw color swatches
            let colors = self.colors.borrow();
            if !colors.is_empty() {
                let square_size = 14.0_f32;
                let spacing = 6.0_f32;
                let total_width = (square_size * colors.len() as f32) + (spacing * (colors.len().saturating_sub(1)) as f32);
                let mut x = width - total_width - 12.0; // 12px right margin

                for color in colors.iter() {
                    let rect = gtk::graphene::Rect::new(x, (height - square_size) / 2.0, square_size, square_size);
                    let radius = gtk::graphene::Size::new(4.0, 4.0);
                    let rounded = gtk::gsk::RoundedRect::new(rect, radius, radius, radius, radius);
                    
                    snapshot.push_rounded_clip(&rounded);
                    snapshot.append_color(color, &rect);
                    snapshot.pop();
                    
                    x += square_size + spacing;
                }
            }
        }

        fn measure(
            &self,
            orientation: gtk::Orientation,
            _for_size: i32,
        ) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (200, 200, -1, -1),
                gtk::Orientation::Vertical => (36, 36, -1, -1), // Give the row a decent height
                _ => (0, 0, -1, -1),
            }
        }
    }
}

glib::wrapper! {
    pub struct ThemePreview(ObjectSubclass<imp::ThemePreview>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ThemePreview {
    fn default() -> Self {
        Self::new()
    }
}

impl ThemePreview {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_theme(&self, name: String, bg: gdk::RGBA, fg: gdk::RGBA, colors: Vec<gdk::RGBA>) {
        self.imp().name.replace(name);
        self.imp().bg_color.replace(Some(bg));
        self.imp().fg_color.replace(Some(fg));
        self.imp().colors.replace(colors);
        self.queue_draw();
    }
}
