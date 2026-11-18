mod dnd;
mod slot;
mod strip;
mod tab;

pub use tab::{BoxxyTab, TabDragPayload};

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use strip::TabStrip;
use tab::TabCallbacks;

/// Default non-expanded tab width in pixels (replaces Libadwaita's hardcoded 207px cap, #64).
pub const DEFAULT_TAB_WIDTH: i32 = 250;
/// Minimum unpinned tab width before horizontal scrolling engages.
pub const MIN_TAB_WIDTH: i32 = 120;
/// Compact square width for pinned icon-only tabs (e.g. Bookmarks).
const PINNED_TAB_WIDTH: i32 = 36;
/// Horizontal gap between tabs and before the trailing `+` button.
pub(super) const TAB_GAP: i32 = 4;

mod imp {
    use super::*;

    pub struct BoxxyTabBar {
        pub pinned_box: gtk::Box,
        pub strip: TabStrip,
        pub new_tab_btn: gtk::Button,
        pub expand_tabs: Cell<bool>,
        pub target_tab_width: Cell<i32>,
        pub tab_view: RefCell<Option<adw::TabView>>,
        pub view_handlers: RefCell<Vec<glib::SignalHandlerId>>,
        pub pinned_tabs: RefCell<HashMap<adw::TabPage, BoxxyTab>>,
        pub on_context_menu: RefCell<Option<tab::ContextMenuCallback>>,
        pub on_pane_drop: RefCell<Option<tab::PaneDropCallback>>,
        pub on_detach_to_new_window: RefCell<Option<tab::DetachTabCallback>>,
    }

    impl Default for BoxxyTabBar {
        fn default() -> Self {
            Self {
                pinned_box: gtk::Box::new(gtk::Orientation::Horizontal, TAB_GAP),
                strip: TabStrip::default(),
                new_tab_btn: gtk::Button::new(),
                expand_tabs: Cell::new(false),
                target_tab_width: Cell::new(DEFAULT_TAB_WIDTH),
                tab_view: RefCell::new(None),
                view_handlers: RefCell::new(Vec::new()),
                pinned_tabs: RefCell::new(HashMap::new()),
                on_context_menu: RefCell::new(None),
                on_pane_drop: RefCell::new(None),
                on_detach_to_new_window: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BoxxyTabBar {
        const NAME: &'static str = "BoxxyTabBar";
        type Type = super::BoxxyTabBar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("tabbar");
            klass.set_accessible_role(gtk::AccessibleRole::TabList);
        }
    }

    impl ObjectImpl for BoxxyTabBar {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            self.pinned_box.set_visible(false);
            self.pinned_box.set_parent(&*obj);

            self.strip.set_visible(false);
            self.strip.set_parent(&*obj);

            self.new_tab_btn.set_icon_name("boxxy-plus-symbolic");
            self.new_tab_btn.set_action_name(Some("win.new-tab"));
            self.new_tab_btn.set_tooltip_text(Some("New Tab"));
            self.new_tab_btn.set_focus_on_click(false);
            self.new_tab_btn
                .set_css_classes(&["flat", "image-button", "new-tab-btn"]);
            self.new_tab_btn.set_parent(&*obj);
        }

        fn dispose(&self) {
            if let Some(tv) = self.tab_view.borrow_mut().take() {
                for h in self.view_handlers.borrow_mut().drain(..) {
                    tv.disconnect(h);
                }
            }
            for (_, tab) in self.pinned_tabs.borrow_mut().drain() {
                if tab.parent().is_some() {
                    tab.unparent();
                }
            }
            if self.pinned_box.parent().is_some() {
                self.pinned_box.unparent();
            }
            if self.strip.parent().is_some() {
                self.strip.unparent();
            }
            if self.new_tab_btn.parent().is_some() {
                self.new_tab_btn.unparent();
            }
        }
    }

    impl WidgetImpl for BoxxyTabBar {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let (btn_min_w, btn_nat_w, _, _) =
                self.new_tab_btn.measure(gtk::Orientation::Horizontal, -1);
            let (btn_min_h, btn_nat_h, _, _) =
                self.new_tab_btn.measure(gtk::Orientation::Vertical, -1);

            let (pin_min_w, pin_nat_w, pin_min_h, pin_nat_h) = if self.pinned_box.is_visible() {
                let (mw, nw, _, _) = self.pinned_box.measure(gtk::Orientation::Horizontal, -1);
                let (mh, nh, _, _) = self.pinned_box.measure(gtk::Orientation::Vertical, -1);
                (mw + TAB_GAP, nw + TAB_GAP, mh, nh)
            } else {
                (0, 0, 0, 0)
            };

            let (strip_min_w, strip_nat_w, strip_min_h, strip_nat_h) = if self.strip.is_visible() {
                let (mw, nw, _, _) = self.strip.measure(gtk::Orientation::Horizontal, -1);
                let (mh, nh, _, _) = self.strip.measure(gtk::Orientation::Vertical, -1);
                (mw + TAB_GAP, nw + TAB_GAP, mh, nh)
            } else {
                (0, 0, 0, 0)
            };

            if orientation == gtk::Orientation::Horizontal {
                let min_w = pin_min_w + strip_min_w.min(MIN_TAB_WIDTH + TAB_GAP) + btn_min_w;
                let nat_w = pin_nat_w + strip_nat_w + btn_nat_w;
                (min_w, nat_w.max(min_w), -1, -1)
            } else {
                let min_h = btn_min_h.max(pin_min_h).max(strip_min_h).max(32);
                let nat_h = btn_nat_h.max(pin_nat_h).max(strip_nat_h).max(32);
                (min_h, nat_h.max(min_h), -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let (_, btn_nat_w, _, _) = self.new_tab_btn.measure(gtk::Orientation::Horizontal, -1);
            let (_, btn_nat_h, _, _) = self.new_tab_btn.measure(gtk::Orientation::Vertical, -1);
            let btn_w = btn_nat_w.max(28);
            let btn_h = btn_nat_h.max(28).min(height.max(28));

            let pin_dims = self.pinned_box.is_visible().then(|| {
                let (_, nw, _, _) = self.pinned_box.measure(gtk::Orientation::Horizontal, -1);
                let (_, nh, _, _) = self.pinned_box.measure(gtk::Orientation::Vertical, -1);
                (nw, nh)
            });
            let strip_visible = self.strip.is_visible() && self.strip.has_any_slots();
            let strip_nat_h = if strip_visible {
                let (_, nh, _, _) = self.strip.measure(gtk::Orientation::Vertical, -1);
                nh
            } else {
                0
            };

            let mut cur_x = 0i32;
            let tab_h = 32
                .max(pin_dims.map_or(0, |(_, nh)| nh))
                .max(strip_nat_h)
                .min(height.max(32));

            if let Some((pin_nat_w, _)) = pin_dims {
                let pin_y = ((height - tab_h) / 2).max(0);
                let pin_w = pin_nat_w.min((width - btn_w).max(0));
                self.pinned_box
                    .size_allocate(&gtk::Allocation::new(cur_x, pin_y, pin_w, tab_h), -1);
                cur_x += pin_w + TAB_GAP;
            }

            if strip_visible {
                let avail_unpinned = (width - cur_x - btn_w - TAB_GAP).max(0);
                let progress_sum = self.strip.animated_progress_sum();
                let n_eff = progress_sum.max(1.0);
                let max_per_tab = if self.expand_tabs.get() {
                    avail_unpinned.max(MIN_TAB_WIDTH)
                } else {
                    self.target_tab_width.get().max(MIN_TAB_WIDTH)
                };
                let fit_w =
                    ((f64::from(avail_unpinned) - (n_eff - 1.0) * f64::from(TAB_GAP)) / n_eff)
                        .floor() as i32;
                let per_tab_w = fit_w.clamp(MIN_TAB_WIDTH, max_per_tab);
                self.strip.imp().per_tab_width.set(per_tab_w);

                let _ = self.strip.measure(gtk::Orientation::Horizontal, -1);

                let content_w = self.strip.animated_content_width(per_tab_w);
                let strip_w = content_w.min(avail_unpinned);
                let strip_y = ((height - tab_h) / 2).max(0);
                if strip_w > 0 {
                    self.strip
                        .size_allocate(&gtk::Allocation::new(cur_x, strip_y, strip_w, tab_h), -1);
                    cur_x += strip_w + TAB_GAP;
                } else {
                    self.strip
                        .size_allocate(&gtk::Allocation::new(cur_x, strip_y, 0, tab_h), -1);
                }
            }

            let btn_x = cur_x.min((width - btn_w).max(0));
            let btn_y = ((height - btn_h) / 2).max(0);
            self.new_tab_btn
                .size_allocate(&gtk::Allocation::new(btn_x, btn_y, btn_w, btn_h), -1);
        }

        fn contains(&self, _x: f64, _y: f64) -> bool {
            // Only child widgets (tabs and the `+` button) are hit-testable; empty trailing
            // space passes straight through to `AdwHeaderBar`'s `GtkWindowHandle`.
            false
        }
    }
}

glib::wrapper! {
    pub struct BoxxyTabBar(ObjectSubclass<imp::BoxxyTabBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for BoxxyTabBar {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}

impl BoxxyTabBar {
    #[must_use]
    pub fn new(tab_view: &adw::TabView) -> Self {
        let obj = Self::default();
        obj.set_view(tab_view);
        obj
    }

    #[must_use]
    pub fn expands_tabs(&self) -> bool {
        self.imp().expand_tabs.get()
    }

    pub fn set_expand_tabs(&self, expand: bool) {
        if self.imp().expand_tabs.get() != expand {
            self.imp().expand_tabs.set(expand);
            self.queue_resize();
        }
    }

    #[must_use]
    pub fn tab_for_page(&self, page: &adw::TabPage) -> Option<BoxxyTab> {
        if let Some(tab) = self.imp().pinned_tabs.borrow().get(page).cloned() {
            return Some(tab);
        }
        self.imp().strip.tab_for_page(page)
    }

    #[must_use]
    pub fn new_tab_button(&self) -> &gtk::Button {
        &self.imp().new_tab_btn
    }

    pub fn set_on_context_menu(&self, cb: impl Fn(&BoxxyTab, &adw::TabPage) + 'static) {
        *self.imp().on_context_menu.borrow_mut() = Some(Rc::new(cb));
    }

    pub fn set_on_pane_drop(&self, cb: impl Fn(String, usize) + 'static) {
        *self.imp().on_pane_drop.borrow_mut() = Some(Rc::new(cb));
    }

    pub fn set_on_detach_to_new_window(&self, cb: impl Fn(usize) + 'static) {
        *self.imp().on_detach_to_new_window.borrow_mut() = Some(Rc::new(cb));
    }

    /// Prevents ancestor `GtkWindowHandle`s (`AdwHeaderBar` and `AdwToolbarView`) from stealing
    /// pointer drags initiated on a tab or button, while keeping empty headerbar space draggable
    /// and resetting child gestures after Wayland `xdg_toplevel_move`.
    pub fn install_header_drag_guard(&self) {
        let mut window_handles: Vec<glib::WeakRef<gtk::Widget>> = Vec::new();
        let mut ancestor = self.parent();

        while let Some(w) = ancestor {
            if w.downcast_ref::<gtk::WindowHandle>().is_some() || w.css_name() == "windowhandle" {
                window_handles.push(w.downgrade());

                let controllers = w.observe_controllers();
                for i in 0..controllers.n_items() {
                    if let Some(drag_gesture) = controllers.item(i).and_downcast::<gtk::GestureDrag>()
                    {
                        drag_gesture.connect_drag_begin(|gesture, start_x, start_y| {
                            if let Some(wh) = gesture.widget()
                                && is_point_on_interactive_tab_bar_child(&wh, start_x, start_y)
                            {
                                gesture.set_state(gtk::EventSequenceState::Denied);
                            }
                        });

                        drag_gesture.connect_sequence_state_changed(|gesture, _, state| {
                            if state == gtk::EventSequenceState::Claimed
                                && let Some(wh) = gesture.widget()
                            {
                                reset_inactive_gestures(&wh, true);
                            }
                        });
                    }
                }
            }
            ancestor = w.parent();
        }

        let motion_guard = gtk::EventControllerMotion::new();
        motion_guard.set_propagation_phase(gtk::PropagationPhase::Capture);
        motion_guard.connect_enter(move |ctrl, _, _| {
            let buttons = ctrl.current_event_state();
            if !buttons.intersects(
                gtk::gdk::ModifierType::BUTTON1_MASK
                    | gtk::gdk::ModifierType::BUTTON2_MASK
                    | gtk::gdk::ModifierType::BUTTON3_MASK,
            ) {
                if let Some(tb) = ctrl.widget() {
                    reset_inactive_gestures(&tb, true);
                }
                for wh in window_handles.iter().filter_map(glib::WeakRef::upgrade) {
                    reset_inactive_gestures(&wh, false);
                }
            }
        });
        self.add_controller(motion_guard);
    }

    fn make_callbacks(&self) -> TabCallbacks {
        let bar_ctx = self.downgrade();
        let bar_pane = self.downgrade();
        let bar_detach = self.downgrade();
        TabCallbacks {
            on_context_menu: Rc::new(move |tab, page| {
                if let Some(bar) = bar_ctx.upgrade()
                    && let Some(ref cb) = *bar.imp().on_context_menu.borrow()
                {
                    cb(tab, page);
                }
            }),
            on_pane_drop: Rc::new(move |pane_id, target_key| {
                if let Some(bar) = bar_pane.upgrade()
                    && let Some(ref cb) = *bar.imp().on_pane_drop.borrow()
                {
                    cb(pane_id, target_key);
                }
            }),
            on_detach_to_new_window: Rc::new(move |page_key| {
                if let Some(bar) = bar_detach.upgrade()
                    && let Some(ref cb) = *bar.imp().on_detach_to_new_window.borrow()
                {
                    cb(page_key);
                }
            }),
        }
    }

    fn set_view(&self, tab_view: &adw::TabView) {
        let imp = self.imp();
        *imp.tab_view.borrow_mut() = Some(tab_view.clone());

        let callbacks = self.make_callbacks();
        dnd::setup_strip_interactions(&imp.strip, &imp.new_tab_btn, tab_view, &callbacks);

        // Seed any already-present pages without open animation.
        for i in 0..tab_view.n_pages() {
            let page = tab_view.nth_page(i);
            if page.is_pinned() {
                self.sync_pinned_tabs();
            } else {
                let unpinned_pos = (i - tab_view.n_pinned_pages()).max(0) as usize;
                imp.strip
                    .attach_page(tab_view, &page, unpinned_pos, false, &callbacks);
            }
        }
        self.sync_selected_page(false);

        let mut handlers = Vec::with_capacity(5);
        {
            let bar_weak = self.downgrade();
            handlers.push(tab_view.connect_page_attached(move |tv, page, pos| {
                let Some(bar) = bar_weak.upgrade() else {
                    return;
                };
                if page.is_pinned() {
                    bar.sync_pinned_tabs();
                } else {
                    let unpinned_pos = (pos - tv.n_pinned_pages()).max(0) as usize;
                    let cbs = bar.make_callbacks();
                    bar.imp()
                        .strip
                        .attach_page(tv, page, unpinned_pos, true, &cbs);
                }
                bar.sync_selected_page(true);
                bar.queue_resize();
            }));
        }
        {
            let bar_weak = self.downgrade();
            handlers.push(tab_view.connect_page_detached(move |_, page, _| {
                let Some(bar) = bar_weak.upgrade() else {
                    return;
                };
                if bar.imp().pinned_tabs.borrow().contains_key(page) {
                    bar.sync_pinned_tabs();
                } else {
                    bar.imp().strip.detach_page(page, true);
                }
                bar.sync_selected_page(false);
                bar.queue_resize();
            }));
        }
        {
            let bar_weak = self.downgrade();
            handlers.push(tab_view.connect_page_reordered(move |tv, page, _| {
                let Some(bar) = bar_weak.upgrade() else {
                    return;
                };
                if page.is_pinned() {
                    bar.sync_pinned_tabs();
                } else if !bar.imp().strip.imp().suppress_reorder_signal.get() {
                    bar.imp().strip.sync_order_with_flip(tv, None);
                }
                bar.queue_resize();
            }));
        }
        {
            let bar_weak = self.downgrade();
            handlers.push(tab_view.connect_selected_page_notify(move |_| {
                if let Some(bar) = bar_weak.upgrade() {
                    bar.sync_selected_page(true);
                }
            }));
        }
        {
            let bar_weak = self.downgrade();
            handlers.push(
                tab_view.connect_notify_local(Some("n-pinned-pages"), move |tv, _| {
                    let Some(bar) = bar_weak.upgrade() else {
                        return;
                    };
                    bar.sync_pinned_tabs();
                    // Move any newly pinned/unpinned pages across `pinned_box` <-> `strip`.
                    let cbs = bar.make_callbacks();
                    for i in 0..tv.n_pages() {
                        let p = tv.nth_page(i);
                        if p.is_pinned() {
                            bar.imp().strip.detach_page(&p, false);
                        } else {
                            let unpinned_pos = (i - tv.n_pinned_pages()).max(0) as usize;
                            bar.imp().strip.attach_page(tv, &p, unpinned_pos, false, &cbs);
                        }
                    }
                    bar.sync_selected_page(true);
                    bar.queue_resize();
                }),
            );
        }

        *imp.view_handlers.borrow_mut() = handlers;
    }

    fn sync_pinned_tabs(&self) {
        let imp = self.imp();
        let Some(tv) = imp.tab_view.borrow().clone() else {
            return;
        };
        let callbacks = self.make_callbacks();

        let n_pinned = tv.n_pinned_pages();
        let mut live_pinned = HashSet::with_capacity(n_pinned as usize);
        let mut ordered = Vec::with_capacity(n_pinned as usize);
        for i in 0..n_pinned {
            let p = tv.nth_page(i);
            live_pinned.insert(p.clone());
            ordered.push(p);
        }

        imp.pinned_tabs.borrow_mut().retain(|page, tab| {
            if live_pinned.contains(page) {
                true
            } else {
                if tab.parent().is_some() {
                    tab.unparent();
                }
                false
            }
        });

        let mut prev: Option<BoxxyTab> = None;
        for page in &ordered {
            let tab = {
                let mut map = imp.pinned_tabs.borrow_mut();
                map.entry(page.clone())
                    .or_insert_with(|| BoxxyTab::new(&tv, page, &callbacks, true))
                    .clone()
            };
            tab.sync_from_page();
            tab.set_width_request(PINNED_TAB_WIDTH);
            let target_parent: &gtk::Widget = imp.pinned_box.upcast_ref();
            if tab.parent().as_ref() != Some(target_parent) {
                if tab.parent().is_some() {
                    tab.unparent();
                }
                imp.pinned_box.append(&tab);
            }
            tab.insert_after(&imp.pinned_box, prev.as_ref());
            prev = Some(tab);
        }

        imp.pinned_box.set_visible(!ordered.is_empty());
    }

    fn sync_selected_page(&self, scroll_into_view: bool) {
        let imp = self.imp();
        let Some(tv) = imp.tab_view.borrow().clone() else {
            return;
        };
        let selected = tv.selected_page();

        for (page, tab) in imp.pinned_tabs.borrow().iter() {
            tab.set_selected(selected.as_ref() == Some(page));
        }
        imp.strip.sync_selected(selected.as_ref(), scroll_into_view);
    }
}

fn is_interactive_child(mut picked: Option<gtk::Widget>, stop_at: &gtk::Widget) -> bool {
    while let Some(w) = picked {
        if &w == stop_at {
            break;
        }
        let css = w.css_name();
        if css == "tab" || css == "button" {
            return true;
        }
        picked = w.parent();
    }
    false
}

fn is_point_on_interactive_tab_bar_child(window_handle: &gtk::Widget, x: f64, y: f64) -> bool {
    let flags = gtk::PickFlags::DEFAULT;
    let Some(hit) = window_handle.pick(x, y, flags) else {
        return false;
    };
    if is_interactive_child(Some(hit), window_handle) {
        return true;
    }
    is_interactive_child(window_handle.pick(x - 3.0, y, flags), window_handle)
        || is_interactive_child(window_handle.pick(x + 3.0, y, flags), window_handle)
}

fn reset_inactive_gestures(widget: &gtk::Widget, recursive: bool) {
    let controllers = widget.observe_controllers();
    for i in 0..controllers.n_items() {
        if let Some(gesture) = controllers.item(i).and_downcast::<gtk::Gesture>()
            && !gesture.is_active()
        {
            gesture.reset();
        }
    }
    if recursive {
        let mut child = widget.first_child();
        while let Some(c) = child {
            reset_inactive_gestures(&c, true);
            child = c.next_sibling();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk::test]
    fn fixed_width_tabs_allocate_250px_with_trailing_plus_and_header_drag_guard() {
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_enable_animations(false);
        }

        let tab_view = adw::TabView::new();
        let p1 = tab_view.append(&gtk::Box::new(gtk::Orientation::Vertical, 0));
        p1.set_title("alpha");
        let p2 = tab_view.append(&gtk::Box::new(gtk::Orientation::Vertical, 0));
        p2.set_title("beta");

        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::builder()
            .show_title(false)
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        let tab_bar = BoxxyTabBar::new(&tab_view);
        tab_bar.set_hexpand(true);
        tab_bar.set_margin_start(36);
        header.pack_start(&tab_bar);
        if let Some(start_box) = tab_bar.parent() {
            start_box.set_halign(gtk::Align::Fill);
            start_box.set_hexpand(true);
        }
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&tab_view));
        tab_bar.install_header_drag_guard();

        let win = gtk::Window::builder()
            .default_width(900)
            .default_height(160)
            .child(&toolbar)
            .build();
        win.set_visible(true);
        let (_, nat_h, _, _) = toolbar.measure(gtk::Orientation::Vertical, 900);
        toolbar.allocate(900, nat_h.max(160), -1, None);

        let t1 = tab_bar.tab_for_page(&p1).expect("tab 1 exists");
        let t2 = tab_bar.tab_for_page(&p2).expect("tab 2 exists");
        let t1_bounds = t1.compute_bounds(&tab_bar).unwrap();
        let t2_bounds = t2.compute_bounds(&tab_bar).unwrap();
        assert_eq!(t1_bounds.width(), DEFAULT_TAB_WIDTH as f32);
        assert_eq!(t2_bounds.width(), DEFAULT_TAB_WIDTH as f32);

        let plus_bounds = tab_bar.new_tab_button().compute_bounds(&tab_bar).unwrap();
        assert!(
            (plus_bounds.x() - (t2_bounds.x() + t2_bounds.width() + TAB_GAP as f32)).abs() <= 1.0,
            "Expected + button right after last tab, got plus.x={} after t2_end={}",
            plus_bounds.x(),
            t2_bounds.x() + t2_bounds.width()
        );

        let wh = header.first_child().expect("AdwHeaderBar WindowHandle");
        let t1_center = t1
            .compute_point(&wh, &gtk::graphene::Point::new(130.0, 15.0))
            .unwrap();
        assert!(
            is_point_on_interactive_tab_bar_child(
                &wh,
                f64::from(t1_center.x()),
                f64::from(t1_center.y())
            ),
            "Pressing on a tab must deny GtkWindowHandle drag"
        );
        assert!(
            !is_point_on_interactive_tab_bar_child(&wh, 820.0, 20.0),
            "Empty space after + must remain draggable for moving the window"
        );

        // Reordering via drag-edge crossing works into both first (index 0) and last slots
        let slot2 = tab_bar.imp().strip.imp().slots.borrow()[1].clone();
        dnd::maybe_swap_crossed_neighbor(
            &tab_bar.imp().strip,
            &tab_view,
            &slot2,
            &p2,
            100.0, // leading edge crosses left of tab 0's center (130.0)
            DEFAULT_TAB_WIDTH as f64,
        );
        assert_eq!(tab_view.page_position(&p2), 0);
        assert_eq!(tab_view.page_position(&p1), 1);

        toolbar.allocate(900, nat_h.max(160), -1, None);
        let t1_after = t1.compute_bounds(&tab_bar).unwrap();
        let t2_after = t2.compute_bounds(&tab_bar).unwrap();
        assert!(t2_after.x() < t1_after.x());

        // Dragging first tab back right past second tab's center (379.0) swaps to last slot
        dnd::maybe_swap_crossed_neighbor(
            &tab_bar.imp().strip,
            &tab_view,
            &slot2,
            &p2,
            150.0, // trailing edge (150 + 250 = 400) crosses right of tab 1's center (379.0)
            DEFAULT_TAB_WIDTH as f64,
        );
        assert_eq!(tab_view.page_position(&p1), 0);
        assert_eq!(tab_view.page_position(&p2), 1);

        win.destroy();
    }

    #[gtk::test]
    fn overflowing_tabs_clamp_to_min_width_and_keep_plus_visible() {
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_enable_animations(false);
        }

        let tab_view = adw::TabView::new();
        for i in 0..12 {
            let p = tab_view.append(&gtk::Box::new(gtk::Orientation::Vertical, 0));
            p.set_title(&format!("tab-{i}"));
        }

        let tab_bar = BoxxyTabBar::new(&tab_view);
        let (_, nat_h, _, _) = tab_bar.measure(gtk::Orientation::Vertical, 800);
        tab_bar.allocate(800, nat_h.max(40), -1, None);

        let last_page = tab_view.nth_page(11);
        let last_tab = tab_bar.tab_for_page(&last_page).unwrap();
        let last_bounds = last_tab.compute_bounds(&tab_bar).unwrap();
        assert_eq!(last_bounds.width(), MIN_TAB_WIDTH as f32);

        let plus_bounds = tab_bar.new_tab_button().compute_bounds(&tab_bar).unwrap();
        assert!(
            plus_bounds.x() + plus_bounds.width() <= 800.0,
            "+ button must remain fully inside the allocated tab bar width"
        );
    }

    #[gtk::test]
    fn four_left_indicators_align_in_stability_order_and_bell_gpa_parses() {
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_enable_animations(false);
        }

        let bell_bytes = glib::Bytes::from_static(include_bytes!(
            "../../../../../resources/icons/boxxy-bell.gpa"
        ));
        let bell_svg = gtk::Svg::from_bytes(&bell_bytes);
        assert_eq!(bell_svg.state(), 0);
        bell_svg.set_state(1);
        assert_eq!(bell_svg.state(), 1);
        let snap = gtk::Snapshot::new();
        gtk::prelude::SymbolicPaintableExt::snapshot_symbolic(
            &bell_svg,
            &snap,
            16.0,
            16.0,
            &[gtk::gdk::RGBA::WHITE],
        );
        bell_svg.set_state(0);
        assert_eq!(bell_svg.state(), 0);

        let tab_view = adw::TabView::new();
        let page = tab_view.append(&gtk::Box::new(gtk::Orientation::Vertical, 0));
        page.set_title("vim");

        let tab_bar = BoxxyTabBar::new(&tab_view);
        let tab = tab_bar.tab_for_page(&page).unwrap();
        assert!(!tab.color_dot().is_visible());

        tab.set_tab_color(crate::state::TabColor::Orange);
        assert!(tab.color_dot().is_visible());
        tab.color_dot().set_size_request(8, 8);
        tab.indicator_image().set_visible(true);
        tab.category_image().set_paintable(Some(&bell_svg));
        tab.bell_image().set_paintable(Some(&bell_svg));
        tab.bell_image().set_visible(true);

        let (_, nat_h, _, _) = tab_bar.measure(gtk::Orientation::Vertical, 600);
        tab_bar.allocate(600, nat_h.max(40), -1, None);

        let dot_x = tab.color_dot().compute_bounds(&tab).unwrap().x();
        let claw_x = tab.indicator_image().compute_bounds(&tab).unwrap().x();
        let bell_x = tab.bell_image().compute_bounds(&tab).unwrap().x();
        let app_x = tab.category_image().compute_bounds(&tab).unwrap().x();
        let label_x = tab.title_label().compute_bounds(&tab).unwrap().x();
        assert!(
            dot_x < claw_x && claw_x < bell_x && bell_x < app_x && app_x < label_x,
            "Expected [color_dot < claw < bell < app_icon < title], got [{dot_x}, {claw_x}, {bell_x}, {app_x}, {label_x}]"
        );

        // Clearing the app icon paintable after fade-out must move neither the bell nor the title!
        tab.category_image()
            .set_paintable(None::<&gtk::gdk::Paintable>);
        tab_bar.allocate(600, nat_h.max(40), -1, None);
        let bell_x_after = tab.bell_image().compute_bounds(&tab).unwrap().x();
        let label_x_after = tab.title_label().compute_bounds(&tab).unwrap().x();
        assert_eq!(bell_x_after, bell_x);
        assert_eq!(label_x_after, label_x);

        let tab_wp = gtk::WidgetPaintable::new(Some(&tab));
        let tab_snap = gtk::Snapshot::new();
        gtk::gdk::prelude::PaintableExt::snapshot(&tab_wp, &tab_snap, 250.0, 36.0);

        tab.set_tab_color(crate::state::TabColor::Default);
        assert!(!tab.color_dot().is_visible());
    }
}
