use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FrostedGlassContainer {
        pub child: RefCell<Option<gtk::Widget>>,
        pub frosted_glass: Cell<bool>,
        pub opacity: Cell<f64>,
        pub is_maximized: Cell<bool>,
        pub has_sidebar: Cell<bool>,
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

            if self.frosted_glass.get()
                && self.opacity.get() < 1.0
                && width > 0.0
                && height > 0.0
            {
                let rect = gtk::graphene::Rect::new(0.0, 0.0, width, height);

                // Corner radii matching Libadwaita window radius (12px).
                // When maximized: 0px.
                // When unmaximized with sidebar visible: left edge is straight (0px) along the divider.
                // When unmaximized without sidebar: all 4 corners are rounded (12px).
                let r = 12.0f32;
                let is_max = self.is_maximized.get();
                let (tl, bl) = if is_max || self.has_sidebar.get() {
                    (0.0, 0.0)
                } else {
                    (r, r)
                };
                let (tr, br) = if is_max {
                    (0.0, 0.0)
                } else {
                    (r, r)
                };

                let rounded = gtk::gsk::RoundedRect::new(
                    rect,
                    gtk::graphene::Size::new(tl, tl),
                    gtk::graphene::Size::new(tr, tr),
                    gtk::graphene::Size::new(br, br),
                    gtk::graphene::Size::new(bl, bl),
                );

                // Push rounded clip so GTK 4.24 Wayland blur detector produces a rounded region
                // and child widgets (headerbar, window controls) cannot draw into square corners.
                snapshot.push_rounded_clip(&rounded);

                // Wayland ext-background-effect-v1 marker tree:
                // Copy -> Blur (>= 20.0px) -> Paste -> Color tint
                snapshot.push_copy();
                snapshot.push_blur(20.0);
                snapshot.append_paste(&rect, 0);

                let mut tint = self
                    .tint_color
                    .borrow()
                    .unwrap_or_else(|| gdk::RGBA::new(0.08, 0.08, 0.1, 1.0));
                tint.set_alpha(self.opacity.get() as f32);
                snapshot.append_color(&tint, &rect);

                snapshot.pop(); // blur
                snapshot.pop(); // copy

                if let Some(ref child) = *self.child.borrow() {
                    self.obj().snapshot_child(child, snapshot);
                }

                snapshot.pop(); // rounded_clip
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

    pub fn set_is_maximized(&self, maximized: bool) {
        if self.imp().is_maximized.get() != maximized {
            self.imp().is_maximized.set(maximized);
            self.queue_draw();
        }
    }

    pub fn set_has_sidebar(&self, has_sidebar: bool) {
        if self.imp().has_sidebar.get() != has_sidebar {
            self.imp().has_sidebar.set(has_sidebar);
            self.queue_draw();
        }
    }

    pub fn set_tint_color(&self, color: gdk::RGBA) {
        self.imp().tint_color.replace(Some(color));
        self.queue_draw();
    }
}
