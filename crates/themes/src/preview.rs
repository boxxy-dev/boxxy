use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::sync::OnceLock;

    fn bold_font_desc() -> &'static gtk::pango::FontDescription {
        static DESC: OnceLock<gtk::pango::FontDescription> = OnceLock::new();
        DESC.get_or_init(|| {
            let mut font_desc = gtk::pango::FontDescription::new();
            font_desc.set_weight(gtk::pango::Weight::Bold);
            font_desc
        })
    }

    pub struct ThemePreview {
        pub name: Cell<&'static str>,
        pub bg_color: Cell<gdk::RGBA>,
        pub fg_color: Cell<gdk::RGBA>,
        pub colors: Cell<[gdk::RGBA; 6]>,
        pub has_theme: Cell<bool>,
        pub layout: RefCell<Option<gtk::pango::Layout>>,
        pub layout_width_pango: Cell<i32>,
        pub text_height: Cell<f32>,
    }

    impl Default for ThemePreview {
        fn default() -> Self {
            Self {
                name: Cell::new(""),
                bg_color: Cell::new(gdk::RGBA::BLACK),
                fg_color: Cell::new(gdk::RGBA::WHITE),
                colors: Cell::new([gdk::RGBA::BLACK; 6]),
                has_theme: Cell::new(false),
                layout: RefCell::new(None),
                layout_width_pango: Cell::new(-1),
                text_height: Cell::new(0.0),
            }
        }
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
            if !self.has_theme.get() {
                return;
            }

            let width = self.obj().width() as f32;
            let height = self.obj().height() as f32;

            // Draw background
            let bg = self.bg_color.get();
            let rect = gtk::graphene::Rect::new(0.0, 0.0, width, height);
            let radius = gtk::graphene::Size::new(8.0, 8.0);
            let rounded = gtk::gsk::RoundedRect::new(rect, radius, radius, radius, radius);
            snapshot.push_rounded_clip(&rounded);
            snapshot.append_color(&bg, &rect);
            snapshot.pop();

            // Draw text using cached Pango layout
            let name = self.name.get();
            if !name.is_empty() {
                let target_width = (width - 150.0).max(0.0) as i32 * gtk::pango::SCALE;
                let mut layout_ref = self.layout.borrow_mut();
                let layout = layout_ref.get_or_insert_with(|| {
                    let pango_ctx = self.obj().pango_context();
                    let l = gtk::pango::Layout::new(&pango_ctx);
                    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    l.set_font_description(Some(bold_font_desc()));
                    l.set_text(name);
                    l
                });

                if self.layout_width_pango.get() != target_width {
                    layout.set_width(target_width);
                    let (_, logical) = layout.extents();
                    self.text_height
                        .set(logical.height() as f32 / gtk::pango::SCALE as f32);
                    self.layout_width_pango.set(target_width);
                }

                let fg = self.fg_color.get();
                let text_height = self.text_height.get();

                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(
                    12.0,
                    (height - text_height) / 2.0,
                ));
                snapshot.append_layout(layout, &fg);
                snapshot.restore();
            }

            // Draw color swatches
            let colors = self.colors.get();
            let square_size = 14.0_f32;
            let spacing = 6.0_f32;
            let total_width =
                (square_size * colors.len() as f32) + (spacing * (colors.len() - 1) as f32);
            let mut x = width - total_width - 12.0; // 12px right margin
            let y = (height - square_size) / 2.0;
            let swatch_radius = gtk::graphene::Size::new(4.0, 4.0);

            for color in &colors {
                let rect = gtk::graphene::Rect::new(x, y, square_size, square_size);
                let rounded = gtk::gsk::RoundedRect::new(
                    rect,
                    swatch_radius,
                    swatch_radius,
                    swatch_radius,
                    swatch_radius,
                );

                snapshot.push_rounded_clip(&rounded);
                snapshot.append_color(color, &rect);
                snapshot.pop();

                x += square_size + spacing;
            }
        }

        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
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

    pub fn set_theme(
        &self,
        name: &'static str,
        bg: gdk::RGBA,
        fg: gdk::RGBA,
        colors: [gdk::RGBA; 6],
    ) {
        let imp = self.imp();
        if imp.name.get() != name {
            imp.name.set(name);
            if let Some(layout) = imp.layout.borrow().as_ref() {
                layout.set_text(name);
                let (_, logical) = layout.extents();
                imp.text_height
                    .set(logical.height() as f32 / gtk::pango::SCALE as f32);
            }
        }
        imp.bg_color.set(bg);
        imp.fg_color.set(fg);
        imp.colors.set(colors);
        imp.has_theme.set(true);
        self.queue_draw();
    }
}
