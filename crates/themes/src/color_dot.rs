use crate::app_theme::sanitize_hex_color;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

mod imp {
    use super::*;
    use std::cell::Cell;

    pub struct ColorDot {
        pub color: Cell<gdk::RGBA>,
        pub size: Cell<i32>,
    }

    impl Default for ColorDot {
        fn default() -> Self {
            Self {
                color: Cell::new(gdk::RGBA::new(0.5, 0.5, 0.5, 1.0)),
                size: Cell::new(14),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ColorDot {
        const NAME: &'static str = "BoxxyColorDot";
        type Type = super::ColorDot;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ColorDot {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("theme-preview-dot");
            obj.set_valign(gtk::Align::Center);
            obj.set_halign(gtk::Align::Center);
        }
    }

    impl WidgetImpl for ColorDot {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let width = self.obj().width() as f32;
            let height = self.obj().height() as f32;
            if width <= 0.0 || height <= 0.0 {
                return;
            }

            let rect = gtk::graphene::Rect::new(0.0, 0.0, width, height);
            let radius = gtk::graphene::Size::new(width * 0.5, height * 0.5);
            let rounded = gtk::gsk::RoundedRect::new(rect, radius, radius, radius, radius);

            snapshot.push_rounded_clip(&rounded);
            snapshot.append_color(&self.color.get(), &rect);
            snapshot.pop();
        }

        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let s = self.size.get().max(4);
            (s, s, -1, -1)
        }
    }
}

glib::wrapper! {
    /// Lightweight GPU-snapshotted circular color swatch widget.
    ///
    /// Draws its fill directly via `GskRoundedRect` in `snapshot()` while delegating its
    /// border (`@boxxy_ink`) to `.theme-preview-dot` in CSS—requiring zero per-dot
    /// global `GtkCssProvider` registrations on `gdk::Display`.
    pub struct ColorDot(ObjectSubclass<imp::ColorDot>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ColorDot {
    #[must_use]
    pub fn new(hex: &str, size: i32) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().size.set(size.max(4));
        obj.set_hex(hex);
        obj
    }

    pub fn set_hex(&self, hex: &str) {
        let safe = sanitize_hex_color(hex).unwrap_or_else(|| "#808080".to_string());
        let rgba = gdk::RGBA::parse(&safe).unwrap_or_else(|_| gdk::RGBA::new(0.5, 0.5, 0.5, 1.0));
        self.imp().color.set(rgba);
        self.queue_draw();
    }
}
