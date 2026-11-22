use adw::prelude::*;
use gtk::gdk;
use gtk::glib;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use super::{DEFAULT_TAB_WIDTH, MIN_TAB_WIDTH, TAB_GAP};
use super::slot::{OPEN_CLOSE_DURATION_MS, REORDER_DURATION_MS, TabSlot};
use super::tab::{BoxxyTab, TabCallbacks};

pub(super) struct ActiveInBarDrag {
    pub slot: Rc<TabSlot>,
    pub grab_offset_x: f64,
    pub grab_offset_y: f64,
    pub drag_content_x: f64,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TabStrip {
        pub(in crate::widgets::tab_bar) slots: RefCell<Vec<Rc<TabSlot>>>,
        pub(in crate::widgets::tab_bar) per_tab_width: Cell<i32>,
        pub(in crate::widgets::tab_bar) scroll_x: Cell<f64>,
        pub(in crate::widgets::tab_bar) max_scroll: Cell<f64>,
        pub(in crate::widgets::tab_bar) pending_scroll_page: RefCell<Option<adw::TabPage>>,
        pub(in crate::widgets::tab_bar) active_drag: RefCell<Option<ActiveInBarDrag>>,
        pub(in crate::widgets::tab_bar) top_z_slot: RefCell<Option<Weak<TabSlot>>>,
        pub(in crate::widgets::tab_bar) torn_off_page: RefCell<Option<adw::TabPage>>,
        pub(in crate::widgets::tab_bar) suppress_reorder_signal: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TabStrip {
        const NAME: &'static str = "BoxxyTabStrip";
        type Type = super::TabStrip;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("box");
        }
    }

    impl ObjectImpl for TabStrip {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_overflow(gtk::Overflow::Hidden);

            let scroll_ctrl =
                gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
            let strip_weak = obj.downgrade();
            scroll_ctrl.connect_scroll(move |ctrl, dx, dy| {
                let Some(strip) = strip_weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let max_s = strip.imp().max_scroll.get();
                if max_s <= 0.0 {
                    return glib::Propagation::Proceed;
                }
                let unit = match ctrl.unit() {
                    gdk::ScrollUnit::Surface => 1.0,
                    _ => 48.0,
                };
                let delta = if dx.abs() > dy.abs() { dx } else { dy } * unit;
                let next = (strip.imp().scroll_x.get() + delta).clamp(0.0, max_s);
                if (next - strip.imp().scroll_x.get()).abs() > f64::EPSILON {
                    strip.imp().scroll_x.set(next);
                    strip.queue_allocate();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            obj.add_controller(scroll_ctrl);
        }

        fn dispose(&self) {
            for slot in self.slots.borrow_mut().drain(..) {
                slot.stop_animations();
                if slot.widget.parent().is_some() {
                    slot.widget.unparent();
                }
            }
        }
    }

    impl WidgetImpl for TabStrip {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let mut min_h = 32i32;
            let mut nat_h = 32i32;
            let slots = self.slots.borrow();
            for slot in slots.iter() {
                if slot.widget.should_layout() {
                    let _ = slot.widget.measure(gtk::Orientation::Horizontal, -1);
                    let (mh, nh, _, _) = slot.widget.measure(gtk::Orientation::Vertical, -1);
                    min_h = min_h.max(mh);
                    nat_h = nat_h.max(nh);
                }
            }

            if orientation == gtk::Orientation::Horizontal {
                if slots.is_empty() {
                    (0, 0, -1, -1)
                } else {
                    // Report natural width from `DEFAULT_TAB_WIDTH` rather than the previous
                    // `size_allocate` output (`per_tab_width`) so `AdwHeaderBar` never traps
                    // shrunk tabs inside its half-width center-box after closing overflowed tabs.
                    let nat_w = self.obj().animated_content_width(DEFAULT_TAB_WIDTH);
                    (MIN_TAB_WIDTH.min(nat_w.max(1)), nat_w, -1, -1)
                }
            } else {
                (min_h, nat_h, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let per_w = self.per_tab_width.get().max(MIN_TAB_WIDTH);
            let slots = self.slots.borrow().clone();

            let mut cur_x = 0i32;
            for slot in &slots {
                slot.widget.set_final_width(per_w);
                let w = slot_animated_width(per_w, slot.appear_progress.get());
                slot.base_x.set(cur_x);
                slot.alloc_w.set(w);
                if w > 0 {
                    cur_x += w + TAB_GAP;
                }
            }
            let content_w = (cur_x - TAB_GAP).max(0);
            let max_scroll = f64::from((content_w - width).max(0));
            self.max_scroll.set(max_scroll);

            let mut scroll = self.scroll_x.get().clamp(0.0, max_scroll);
            if let Some(target_page) = self.pending_scroll_page.borrow_mut().take()
                && let Some(slot) = slots
                    .iter()
                    .find(|s| s.page.borrow().as_ref() == Some(&target_page))
            {
                let tab_left = f64::from(slot.base_x.get());
                // Use full `per_w` so newly opening tabs (`appear_progress` near 0) still scroll fully into view.
                let tab_right = tab_left + f64::from(per_w);
                let view_w = f64::from(width);
                if tab_left < scroll {
                    scroll = tab_left;
                } else if tab_right > scroll + view_w {
                    scroll = (tab_right - view_w).max(0.0);
                }
                scroll = scroll.clamp(0.0, max_scroll);
            }
            self.scroll_x.set(scroll);

            let active_drag_info = self
                .active_drag
                .borrow()
                .as_ref()
                .map(|d| (Rc::as_ptr(&d.slot), d.drag_content_x));

            for slot in &slots {
                if !slot.widget.should_layout() {
                    continue;
                }
                let _ = slot.widget.measure(gtk::Orientation::Horizontal, -1);
                let _ = slot.widget.measure(gtk::Orientation::Vertical, -1);

                let w = slot.alloc_w.get().max(0);
                let content_x = if let Some((drag_ptr, drag_cx)) = active_drag_info
                    && Rc::as_ptr(slot) == drag_ptr
                {
                    drag_cx
                } else {
                    f64::from(slot.base_x.get()) + slot.visual_offset_x.get()
                };
                let screen_x = (content_x - scroll).round() as i32;
                slot.widget
                    .size_allocate(&gtk::Allocation::new(screen_x, 0, w, height), -1);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let top_slot = self
                .active_drag
                .borrow()
                .as_ref()
                .map(|d| d.slot.clone())
                .or_else(|| {
                    self.top_z_slot
                        .borrow()
                        .as_ref()
                        .and_then(Weak::upgrade)
                        .filter(|s| s.visual_offset_x.get().abs() > 0.5)
                });
            let top_ptr = top_slot.as_ref().map(Rc::as_ptr);

            let slots = self.slots.borrow();
            for slot in slots.iter() {
                if Some(Rc::as_ptr(slot)) == top_ptr {
                    continue;
                }
                if slot.alloc_w.get() > 0 && slot.widget.is_visible() {
                    obj.snapshot_child(&slot.widget, snapshot);
                }
            }
            if let Some(top) = top_slot
                && top.alloc_w.get() > 0
                && top.widget.is_visible()
            {
                obj.snapshot_child(&top.widget, snapshot);
            }
        }
    }
}

pub(super) fn slot_animated_width(per_tab_w: i32, progress: f64) -> i32 {
    if progress <= 0.001 {
        0
    } else if progress >= 0.999 {
        per_tab_w
    } else {
        (((per_tab_w + TAB_GAP) as f64) * progress).round() as i32 - TAB_GAP
    }
    .max(0)
}

glib::wrapper! {
    pub struct TabStrip(ObjectSubclass<imp::TabStrip>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for TabStrip {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}

impl TabStrip {
    #[must_use]
    pub(super) fn alive_count(&self) -> usize {
        self.imp()
            .slots
            .borrow()
            .iter()
            .filter(|s| s.is_alive())
            .count()
    }

    #[must_use]
    pub(super) fn has_any_slots(&self) -> bool {
        !self.imp().slots.borrow().is_empty()
    }

    #[must_use]
    pub(super) fn animated_progress_sum(&self) -> f64 {
        self.imp()
            .slots
            .borrow()
            .iter()
            .map(|s| s.appear_progress.get())
            .sum()
    }

    #[must_use]
    pub(super) fn animated_content_width(&self, per_tab_w: i32) -> i32 {
        let slots = self.imp().slots.borrow();
        let mut total = 0i32;
        let mut any = false;
        for s in slots.iter() {
            let w = slot_animated_width(per_tab_w, s.appear_progress.get());
            if w > 0 {
                total += w + TAB_GAP;
                any = true;
            }
        }
        if any { total - TAB_GAP } else { 0 }
    }

    #[must_use]
    pub(super) fn tab_for_page(&self, page: &adw::TabPage) -> Option<BoxxyTab> {
        self.imp()
            .slots
            .borrow()
            .iter()
            .find(|s| s.page.borrow().as_ref() == Some(page))
            .map(|s| s.tab.clone())
    }

    pub(super) fn find_alive_slot_at_content_x(&self, content_x: f64) -> Option<Rc<TabSlot>> {
        let imp = self.imp();
        if let Some(ref drag) = *imp.active_drag.borrow() {
            let w = f64::from(drag.slot.alloc_w.get());
            if content_x >= drag.drag_content_x && content_x <= drag.drag_content_x + w {
                return Some(drag.slot.clone());
            }
        }
        for slot in imp.slots.borrow().iter() {
            if !slot.is_alive() || slot.widget.opacity() == 0.0 {
                continue;
            }
            let x = f64::from(slot.base_x.get()) + slot.visual_offset_x.get();
            let w = f64::from(slot.alloc_w.get());
            if content_x >= x && content_x <= x + w {
                return Some(slot.clone());
            }
        }
        None
    }

    pub(super) fn attach_page(
        &self,
        tab_view: &adw::TabView,
        page: &adw::TabPage,
        unpinned_index: usize,
        animate: bool,
        callbacks: &TabCallbacks,
    ) {
        if self.tab_for_page(page).is_some() {
            return;
        }

        let should_animate =
            animate && self.is_mapped() && self.settings().is_gtk_enable_animations();
        let init_progress = if should_animate { 0.0 } else { 1.0 };

        let tab = BoxxyTab::new(tab_view, page, callbacks, false);
        let slot = TabSlot::new(tab, page.clone(), init_progress);
        slot.widget.set_parent(self);

        {
            let mut slots = self.imp().slots.borrow_mut();
            let insert_at = alive_index_to_vec_index(&slots, unpinned_index);
            slots.insert(insert_at, slot.clone());
        }

        self.set_visible(true);
        if should_animate {
            slot.animate_appear(self, 1.0, OPEN_CLOSE_DURATION_MS, None);
        } else {
            self.queue_resize();
            if let Some(p) = self.parent() {
                p.queue_resize();
            }
        }
    }

    pub(super) fn detach_page(&self, page: &adw::TabPage, animate: bool) {
        let imp = self.imp();
        if imp.torn_off_page.borrow().as_ref() == Some(page) {
            *imp.torn_off_page.borrow_mut() = None;
        }

        let slot_opt = imp
            .slots
            .borrow()
            .iter()
            .find(|s| s.page.borrow().as_ref() == Some(page))
            .cloned();
        let Some(slot) = slot_opt else {
            return;
        };

        // Clear active in-bar drag if this tab was mid-drag.
        if imp
            .active_drag
            .borrow()
            .as_ref()
            .is_some_and(|d| Rc::ptr_eq(&d.slot, &slot))
        {
            *imp.active_drag.borrow_mut() = None;
        }

        slot.tab.unbind_page();
        *slot.page.borrow_mut() = None;

        let already_collapsed = slot.appear_progress.get() <= 0.01;
        let should_animate = animate
            && !already_collapsed
            && self.is_mapped()
            && self.settings().is_gtk_enable_animations();

        if should_animate {
            let strip_weak = self.downgrade();
            let slot_weak = Rc::downgrade(&slot);
            slot.animate_appear(
                self,
                0.0,
                OPEN_CLOSE_DURATION_MS,
                Some(Box::new(move || {
                    if let (Some(strip), Some(s)) = (strip_weak.upgrade(), slot_weak.upgrade()) {
                        strip.finalize_remove_slot(&s);
                    }
                })),
            );
        } else {
            self.finalize_remove_slot(&slot);
        }
    }

    pub(super) fn finalize_remove_slot(&self, slot: &Rc<TabSlot>) {
        slot.stop_animations();
        self.imp()
            .slots
            .borrow_mut()
            .retain(|s| !Rc::ptr_eq(s, slot));
        if slot.widget.parent().is_some() {
            slot.widget.unparent();
        }
        let has_any = self.has_any_slots();
        self.set_visible(has_any);
        self.queue_resize();
        if let Some(p) = self.parent() {
            p.queue_resize();
        }
    }

    /// Reconciles alive unpinned slot order with `tab_view` and plays a smooth FLIP slide
    /// animation on every displaced tab.
    pub(super) fn sync_order_with_flip(
        &self,
        tab_view: &adw::TabView,
        skip_slot: Option<&Rc<TabSlot>>,
    ) {
        let imp = self.imp();
        let per_w = imp.per_tab_width.get().max(MIN_TAB_WIDTH);

        // 1. Capture pre-reorder visual content X for every alive slot.
        let old_positions: Vec<(usize, f64)> = imp
            .slots
            .borrow()
            .iter()
            .map(|s| {
                (
                    Rc::as_ptr(s) as usize,
                    f64::from(s.base_x.get()) + s.visual_offset_x.get(),
                )
            })
            .collect();

        // 2. Reorder alive slots to match `tab_view`'s unpinned page order while preserving
        //    any closing slots at their current relative indices.
        {
            let mut slots = imp.slots.borrow_mut();
            let mut desired_alive = Vec::new();
            for i in tab_view.n_pinned_pages()..tab_view.n_pages() {
                let p = tab_view.nth_page(i);
                if let Some(s) = slots
                    .iter()
                    .find(|s| s.page.borrow().as_ref() == Some(&p))
                    .cloned()
                {
                    desired_alive.push(s);
                }
            }
            let mut alive_iter = desired_alive.into_iter();
            for entry in slots.iter_mut() {
                if entry.is_alive()
                    && let Some(next_alive) = alive_iter.next()
                {
                    *entry = next_alive;
                }
            }
        }

        // 3. Recompute new `base_x` and invert `visual_offset_x` so nothing jumps, then play!
        let slots_snapshot = imp.slots.borrow().clone();
        let mut cur_x = 0i32;
        for slot in &slots_snapshot {
            let w = slot_animated_width(per_w, slot.appear_progress.get());
            slot.base_x.set(cur_x);
            slot.alloc_w.set(w);
            if w > 0 {
                cur_x += w + TAB_GAP;
            }
        }

        let skip_ptr = skip_slot.map(Rc::as_ptr);
        for slot in &slots_snapshot {
            if Some(Rc::as_ptr(slot)) == skip_ptr || !slot.is_alive() {
                continue;
            }
            if let Some((_, old_x)) = old_positions
                .iter()
                .find(|(ptr, _)| *ptr == Rc::as_ptr(slot) as usize)
            {
                let delta = old_x - f64::from(slot.base_x.get());
                slot.visual_offset_x.set(delta);
                slot.animate_offset_to_zero(self, REORDER_DURATION_MS);
            }
        }
        self.queue_allocate();
    }

    pub(super) fn sync_selected(&self, selected: Option<&adw::TabPage>, scroll_into_view: bool) {
        let imp = self.imp();
        for slot in imp.slots.borrow().iter() {
            let is_sel = slot.page.borrow().as_ref() == selected && selected.is_some();
            slot.tab.set_selected(is_sel);
        }
        if scroll_into_view && let Some(sel) = selected && !sel.is_pinned() {
            *imp.pending_scroll_page.borrow_mut() = Some(sel.clone());
            self.queue_allocate();
        }
    }

    #[must_use]
    pub(super) fn insertion_index_for_x(&self, strip_local_x: f64) -> i32 {
        let imp = self.imp();
        let content_x = strip_local_x + imp.scroll_x.get();
        let mut alive_idx = 0i32;
        for slot in imp.slots.borrow().iter() {
            if !slot.is_alive() || slot.widget.opacity() == 0.0 {
                continue;
            }
            let center = f64::from(slot.base_x.get()) + f64::from(slot.alloc_w.get()) * 0.5;
            if content_x < center {
                return alive_idx;
            }
            alive_idx += 1;
        }
        alive_idx
    }

    pub(super) fn restore_torn_off_page(&self, page: &adw::TabPage) {
        let imp = self.imp();
        if imp.torn_off_page.borrow().as_ref() == Some(page) {
            *imp.torn_off_page.borrow_mut() = None;
        }
        if let Some(slot) = imp
            .slots
            .borrow()
            .iter()
            .find(|s| s.page.borrow().as_ref() == Some(page))
            .cloned()
        {
            slot.widget.set_opacity(1.0);
            slot.animate_appear(self, 1.0, OPEN_CLOSE_DURATION_MS, None);
        }
    }
}

fn alive_index_to_vec_index(slots: &[Rc<TabSlot>], target_alive_idx: usize) -> usize {
    let mut seen_alive = 0usize;
    for (vec_idx, slot) in slots.iter().enumerate() {
        if slot.is_alive() {
            if seen_alive == target_alive_idx {
                return vec_idx;
            }
            seen_alive += 1;
        }
    }
    slots.len()
}
