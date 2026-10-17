use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Width threshold (in logical pixels) at which a Modified file diff switches
/// automatically between Two-Column Side-by-Side mode and Compact Drawer mode.
pub const SPLIT_VIEW_BREAKPOINT: i32 = 460;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLayoutMode {
    Split,
    Drawer,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ResponsiveDiffContainer {
        pub(super) last_mode: Cell<Option<DiffLayoutMode>>,
        pub(super) user_override: Cell<Option<DiffLayoutMode>>,
        pub(super) pending_switch: Cell<bool>,
        pub(super) on_mode_change: RefCell<Option<Rc<dyn Fn(DiffLayoutMode, bool)>>>,
        pub(super) drawer_panel: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ResponsiveDiffContainer {
        const NAME: &'static str = "BoxxyViewerResponsiveDiffContainer";
        type Type = super::ResponsiveDiffContainer;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ResponsiveDiffContainer {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for ResponsiveDiffContainer {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().first_child() else {
                return (0, 0, -1, -1);
            };

            let (min_w, nat_w, _, _) = child.measure(gtk::Orientation::Horizontal, -1);

            match orientation {
                gtk::Orientation::Horizontal => {
                    let clamped_min = min_w.min(220);
                    let clamped_nat = nat_w.max(clamped_min);
                    (clamped_min, clamped_nat, -1, -1)
                }
                gtk::Orientation::Vertical => {
                    let target_w = if for_size < 0 {
                        nat_w.max(720)
                    } else {
                        for_size.max(220)
                    };
                    let (min_h, nat_h, _, _) =
                        child.measure(gtk::Orientation::Vertical, target_w);
                    let full_h = nat_h.max(min_h);
                    (full_h, full_h, -1, -1)
                }
                _ => (min_w, nat_w, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if width > 0
                && let Some(ref panel) = *self.drawer_panel.borrow()
            {
                // Give the slide-over drawer ~58% of the card width (at least 300px,
                // never exceeding card width minus a 40px peek margin).
                let max_allowed = (width - 40).max(200);
                let target_drawer_w = ((width * 58) / 100).clamp(300.min(max_allowed), max_allowed);
                if panel.width_request() != target_drawer_w {
                    panel.set_width_request(target_drawer_w);
                }
            }

            if let Some(child) = self.obj().first_child() {
                let _ = child.measure(gtk::Orientation::Vertical, width);
                child.allocate(width, height, baseline, None);
            }

            if width <= 0 {
                return;
            }

            let is_narrow = width < SPLIT_VIEW_BREAKPOINT;
            let desired = self.user_override.get().unwrap_or(if is_narrow {
                DiffLayoutMode::Drawer
            } else {
                DiffLayoutMode::Split
            });

            if self.last_mode.get() != Some(desired) && !self.pending_switch.get() {
                self.pending_switch.set(true);
                let obj_weak = self.obj().downgrade();
                glib::idle_add_local_once(move || {
                    let Some(obj) = obj_weak.upgrade() else {
                        return;
                    };
                    let imp = obj.imp();
                    imp.pending_switch.set(false);
                    let w = obj.width();
                    let narrow = w > 0 && w < SPLIT_VIEW_BREAKPOINT;
                    let target = imp.user_override.get().unwrap_or(if narrow {
                        DiffLayoutMode::Drawer
                    } else {
                        DiffLayoutMode::Split
                    });
                    if imp.last_mode.replace(Some(target)) != Some(target)
                        && let Some(cb) = imp.on_mode_change.borrow().as_ref()
                    {
                        cb(target, narrow);
                    }
                });
            }
        }
    }
}

glib::wrapper! {
    pub struct ResponsiveDiffContainer(ObjectSubclass<imp::ResponsiveDiffContainer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ResponsiveDiffContainer {
    #[must_use]
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let obj: Self = glib::Object::new();
        obj.set_hexpand(true);
        obj.set_halign(gtk::Align::Fill);
        child.set_parent(&obj);
        obj
    }

    pub fn set_drawer_panel(&self, panel: &impl IsA<gtk::Widget>) {
        *self.imp().drawer_panel.borrow_mut() = Some(panel.clone().upcast());
    }

    pub fn connect_mode_changed(&self, cb: impl Fn(DiffLayoutMode, bool) + 'static) {
        *self.imp().on_mode_change.borrow_mut() = Some(Rc::new(cb));
    }

    pub fn current_mode(&self) -> DiffLayoutMode {
        self.imp().last_mode.get().unwrap_or(DiffLayoutMode::Split)
    }

    pub fn set_user_override(&self, mode: Option<DiffLayoutMode>) {
        let imp = self.imp();
        imp.user_override.set(mode);
        let w = self.width();
        let narrow = w > 0 && w < SPLIT_VIEW_BREAKPOINT;
        let target = mode.unwrap_or(if narrow {
            DiffLayoutMode::Drawer
        } else {
            DiffLayoutMode::Split
        });
        imp.last_mode.set(Some(target));
        if let Some(cb) = imp.on_mode_change.borrow().as_ref() {
            cb(target, narrow);
        }
        self.queue_resize();
    }
}
