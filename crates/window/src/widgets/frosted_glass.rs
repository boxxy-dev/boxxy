use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FrostedGlassContainer {
        pub child: RefCell<Option<gtk::Widget>>,
        pub frosted_glass: Cell<bool>,
        pub opacity: Cell<f64>,
        pub tint_color: RefCell<Option<gdk::RGBA>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FrostedGlassContainer {
        const NAME: &'static str = "BoxxyFrostedGlassContainer";
        type Type = super::FrostedGlassContainer;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for FrostedGlassContainer {
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for FrostedGlassContainer {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let width = self.obj().width() as f32;
            let height = self.obj().height() as f32;

            if self.frosted_glass.get() && self.opacity.get() < 1.0 && width > 0.0 && height > 0.0 {
                let rect = gtk::graphene::Rect::new(0.0, 0.0, width, height);

                // AdwApplicationWindow already clips its content to the actual window shape.
                // Do not add a second, hardcoded rounded clip around the blur marker: the
                // native clip also accounts for CSD offsets, tiled edges and fullscreen.

                let mut tint = self
                    .tint_color
                    .borrow()
                    .unwrap_or_else(|| gdk::RGBA::new(0.08, 0.08, 0.1, 1.0));
                tint.set_alpha(self.opacity.get() as f32);

                let pad = 60.0f32;
                let padded_rect =
                    gtk::graphene::Rect::new(-pad, -pad, width + 2.0 * pad, height + 2.0 * pad);

                snapshot.push_copy();
                // GTK 4.25 halves this legacy radius to sigma; keep sigma >= 20.
                snapshot.push_blur(40.0);
                snapshot.append_paste(&padded_rect, 0);
                snapshot.pop(); // blur
                snapshot.pop(); // copy

                // Tint is a flat overlay, not part of the backdrop filter. Blurring it
                // couples edge alpha to the renderer's intermediate surface bounds.
                snapshot.append_color(&tint, &rect);

                if let Some(ref child) = *self.child.borrow() {
                    self.obj().snapshot_child(child, snapshot);
                }
            } else if let Some(ref child) = *self.child.borrow() {
                self.obj().snapshot_child(child, snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct FrostedGlassContainer(ObjectSubclass<imp::FrostedGlassContainer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for FrostedGlassContainer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrostedGlassContainer {
    pub fn new() -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().opacity.set(1.0);
        obj
    }

    pub fn set_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        let imp = self.imp();
        if let Some(old_child) = imp.child.borrow_mut().take() {
            old_child.unparent();
        }
        if let Some(new_child) = child {
            let child_widget = new_child.as_ref();
            child_widget.set_parent(self);
            imp.child.replace(Some(child_widget.clone()));
        }
        self.queue_draw();
    }

    pub fn set_frosted_glass(&self, enabled: bool) {
        if self.imp().frosted_glass.get() != enabled {
            self.imp().frosted_glass.set(enabled);
            self.queue_draw();
        }
    }

    pub fn set_opacity(&self, opacity: f64) {
        let clamped = opacity.clamp(0.0, 1.0);
        if (self.imp().opacity.get() - clamped).abs() > 0.001 {
            self.imp().opacity.set(clamped);
            self.queue_draw();
        }
    }

    pub fn set_tint_color(&self, color: gdk::RGBA) {
        self.imp().tint_color.replace(Some(color));
        self.queue_draw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk::test]
    fn backdrop_marker_and_tint_are_separate() {
        let glass = FrostedGlassContainer::new();
        glass.set_frosted_glass(true);
        glass.set_opacity(0.8);
        glass.allocate(600, 400, -1, None);

        let snapshot = gtk::Snapshot::new();
        glass.imp().snapshot(&snapshot);
        let node = snapshot.to_node().unwrap();
        let layers = node.downcast_ref::<gtk::gsk::ContainerNode>().unwrap();
        assert_eq!(layers.n_children(), 2);

        // No widget-local rounded clip or blurred color: only the backdrop is filtered.
        let marker = layers.child(0);
        let copy = marker.downcast_ref::<gtk::gsk::CopyNode>().unwrap();
        let filter = copy.child();
        let blur = filter.downcast_ref::<gtk::gsk::BlurNode>().unwrap();
        let backdrop = blur.child();
        let paste = backdrop.downcast_ref::<gtk::gsk::PasteNode>().unwrap();
        assert_eq!(
            paste.bounds(),
            gtk::graphene::Rect::new(-60.0, -60.0, 720.0, 520.0)
        );

        let tint = layers.child(1);
        let color = tint.downcast_ref::<gtk::gsk::ColorNode>().unwrap();
        assert_eq!(
            color.bounds(),
            gtk::graphene::Rect::new(0.0, 0.0, 600.0, 400.0)
        );
        assert!((color.color().alpha() - 0.8).abs() < 0.001);
    }

    #[gtk::test]
    fn inactive_or_opaque_glass_has_no_backdrop_marker() {
        let glass = FrostedGlassContainer::new();
        glass.allocate(600, 400, -1, None);

        for (enabled, opacity) in [(false, 0.8), (true, 1.0)] {
            glass.set_frosted_glass(enabled);
            glass.set_opacity(opacity);
            let snapshot = gtk::Snapshot::new();
            glass.imp().snapshot(&snapshot);
            assert!(snapshot.to_node().is_none());
        }
    }
}
