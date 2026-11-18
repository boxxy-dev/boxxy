use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::tab::BoxxyTab;

pub(super) const OPEN_CLOSE_DURATION_MS: u32 = 200;
pub(super) const REORDER_DURATION_MS: u32 = 220;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TabSlotWidget {
        pub child: RefCell<Option<BoxxyTab>>,
        pub final_width: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TabSlotWidget {
        const NAME: &'static str = "BoxxyTabSlotWidget";
        type Type = super::TabSlotWidget;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("tabboxchild");
        }
    }

    impl ObjectImpl for TabSlotWidget {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_overflow(gtk::Overflow::Hidden);
        }

        fn dispose(&self) {
            if let Some(c) = self.child.borrow_mut().take()
                && c.parent().is_some()
            {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for TabSlotWidget {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(ref child) = *self.child.borrow() else {
                return (0, 0, -1, -1);
            };
            let (min, nat, min_b, nat_b) = child.measure(orientation, for_size);
            if orientation == gtk::Orientation::Horizontal {
                // Allow the clip slot to animate smoothly down to 0px during open/close.
                (0, nat, min_b, nat_b)
            } else {
                (min, nat, min_b, nat_b)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(ref child) = *self.child.borrow() else {
                return;
            };
            let _ = child.measure(gtk::Orientation::Horizontal, -1);
            let _ = child.measure(gtk::Orientation::Vertical, -1);

            // Mirror `adw-tab-box.c:allocate_tab`: always allocate the inner `BoxxyTab` at its
            // full target width centered inside this `overflow: hidden` slot so text and icons
            // unfold/fold smoothly without reflowing or emitting negative-width warnings.
            let full_w = self.final_width.get().max(width).max(1);
            let width_diff = (full_w - width).max(0);
            let transform = if width_diff > 0 {
                Some(
                    gtk::gsk::Transform::new()
                        .translate(&gtk::graphene::Point::new(-(width_diff as f32) * 0.5, 0.0)),
                )
            } else {
                None
            };
            child.allocate(full_w, height, baseline, transform);
        }
    }
}

glib::wrapper! {
    pub struct TabSlotWidget(ObjectSubclass<imp::TabSlotWidget>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TabSlotWidget {
    pub(super) fn new(tab: &BoxxyTab) -> Self {
        let obj: Self = glib::Object::builder().build();
        tab.set_parent(&obj);
        *obj.imp().child.borrow_mut() = Some(tab.clone());
        obj
    }

    pub(super) fn set_final_width(&self, width: i32) {
        self.imp().final_width.set(width);
    }
}

pub(super) struct TabSlot {
    pub widget: TabSlotWidget,
    pub tab: BoxxyTab,
    /// `Some(page)` while alive; `None` while playing the close animation.
    pub page: RefCell<Option<adw::TabPage>>,
    pub appear_progress: Cell<f64>,
    pub appear_anim: RefCell<Option<adw::TimedAnimation>>,
    pub visual_offset_x: Cell<f64>,
    pub offset_anim: RefCell<Option<adw::TimedAnimation>>,
    pub base_x: Cell<i32>,
    pub alloc_w: Cell<i32>,
}

impl TabSlot {
    pub fn new(tab: BoxxyTab, page: adw::TabPage, initial_progress: f64) -> Rc<Self> {
        let widget = TabSlotWidget::new(&tab);
        Rc::new(Self {
            widget,
            tab,
            page: RefCell::new(Some(page)),
            appear_progress: Cell::new(initial_progress),
            appear_anim: RefCell::new(None),
            visual_offset_x: Cell::new(0.0),
            offset_anim: RefCell::new(None),
            base_x: Cell::new(0),
            alloc_w: Cell::new(0),
        })
    }

    pub fn is_alive(&self) -> bool {
        self.page.borrow().is_some()
    }

    pub fn stop_animations(&self) {
        if let Some(anim) = self.appear_anim.borrow_mut().take() {
            anim.pause();
        }
        if let Some(anim) = self.offset_anim.borrow_mut().take() {
            anim.pause();
        }
    }

    pub fn animate_appear(
        self: &Rc<Self>,
        host_widget: &impl IsA<gtk::Widget>,
        to: f64,
        duration_ms: u32,
        on_done: Option<Box<dyn FnOnce()>>,
    ) {
        if let Some(prev) = self.appear_anim.borrow_mut().take() {
            prev.pause();
        }
        let from = self.appear_progress.get();
        if (from - to).abs() < 1e-4 {
            self.appear_progress.set(to);
            host_widget.queue_resize();
            if let Some(cb) = on_done {
                cb();
            }
            return;
        }

        let slot_weak = Rc::downgrade(self);
        let host_weak = host_widget.as_ref().downgrade();
        let target = adw::CallbackAnimationTarget::new(move |val| {
            if let Some(slot) = slot_weak.upgrade() {
                slot.appear_progress.set(val);
            }
            if let Some(host) = host_weak.upgrade() {
                host.queue_resize();
                if let Some(parent) = host.parent() {
                    parent.queue_resize();
                }
            }
        });

        let anim = adw::TimedAnimation::builder()
            .widget(host_widget)
            .value_from(from)
            .value_to(to)
            .duration(duration_ms)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();

        let slot_done = Rc::downgrade(self);
        let on_done_cell = RefCell::new(on_done);
        anim.connect_done(move |_| {
            if let Some(slot) = slot_done.upgrade() {
                let _ = slot.appear_anim.borrow_mut().take();
            }
            if let Some(cb) = on_done_cell.borrow_mut().take() {
                cb();
            }
        });

        *self.appear_anim.borrow_mut() = Some(anim.clone());
        anim.play();
    }

    pub fn animate_offset_to_zero(
        self: &Rc<Self>,
        host_widget: &impl IsA<gtk::Widget>,
        duration_ms: u32,
    ) {
        if let Some(prev) = self.offset_anim.borrow_mut().take() {
            prev.pause();
        }
        let from = self.visual_offset_x.get();
        if from.abs() < 0.5 {
            self.visual_offset_x.set(0.0);
            host_widget.queue_allocate();
            return;
        }

        let slot_weak = Rc::downgrade(self);
        let host_weak = host_widget.as_ref().downgrade();
        let target = adw::CallbackAnimationTarget::new(move |val| {
            if let Some(slot) = slot_weak.upgrade() {
                slot.visual_offset_x.set(val);
            }
            if let Some(host) = host_weak.upgrade() {
                host.queue_allocate();
            }
        });

        let anim = adw::TimedAnimation::builder()
            .widget(host_widget)
            .value_from(from)
            .value_to(0.0)
            .duration(duration_ms)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();

        let slot_done = Rc::downgrade(self);
        anim.connect_done(move |_| {
            if let Some(slot) = slot_done.upgrade() {
                slot.visual_offset_x.set(0.0);
                let _ = slot.offset_anim.borrow_mut().take();
            }
        });

        *self.offset_anim.borrow_mut() = Some(anim.clone());
        anim.play();
    }
}
