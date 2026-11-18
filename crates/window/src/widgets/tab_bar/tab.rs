use adw::prelude::*;
use gtk::gdk;
use gtk::glib;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::state::TabColor;

#[derive(Clone, Debug, glib::Boxed)]
#[boxed_type(name = "BoxxyTabDragPayload")]
pub struct TabDragPayload {
    pub source_view: glib::WeakRef<adw::TabView>,
    pub page: glib::WeakRef<adw::TabPage>,
    pub handled: Arc<AtomicBool>,
}

pub(super) type ContextMenuCallback = Rc<dyn Fn(&BoxxyTab, &adw::TabPage)>;
pub(super) type PaneDropCallback = Rc<dyn Fn(String, usize)>;
pub(super) type DetachTabCallback = Rc<dyn Fn(usize)>;

#[derive(Clone)]
pub(super) struct TabCallbacks {
    pub on_context_menu: ContextMenuCallback,
    pub on_pane_drop: PaneDropCallback,
    pub on_detach_to_new_window: DetachTabCallback,
}

const APP_ICON_SIZE: i32 = 16;
const APP_ICON_TO_LABEL_GAP: i32 = 5;
const BADGES_TO_APP_ICON_GAP: i32 = 6;

mod regular_imp {
    use super::*;

    #[derive(Default)]
    pub struct RegularTabContent {
        pub start_box: gtk::Box,
        pub color_dot: gtk::Box,
        pub indicator_image: gtk::Image,
        pub bell_image: gtk::Image,
        pub category_image: gtk::Image,
        pub title_label: gtk::Label,
        pub close_btn: gtk::Button,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RegularTabContent {
        const NAME: &'static str = "BoxxyRegularTabContent";
        type Type = super::RegularTabContent;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for RegularTabContent {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            self.start_box.set_orientation(gtk::Orientation::Horizontal);
            self.start_box.set_spacing(5);
            self.start_box.set_margin_start(5);
            self.start_box.set_valign(gtk::Align::Center);

            self.color_dot.add_css_class("tab-color-dot");
            self.color_dot.set_valign(gtk::Align::Center);
            self.color_dot.set_halign(gtk::Align::Center);
            self.color_dot.set_visible(false);

            self.indicator_image.set_pixel_size(16);
            self.indicator_image.set_visible(false);
            self.bell_image.set_pixel_size(16);
            self.bell_image.add_css_class("tab-bell-icon");
            self.bell_image.set_visible(false);

            // Left edge holds tab-level status badges only: [Color Dot · Claw/Timer · Bell].
            self.start_box.append(&self.color_dot);
            self.start_box.append(&self.indicator_image);
            self.start_box.append(&self.bell_image);
            self.start_box.set_parent(&*obj);

            // Permanently visible 16x16 slot anchored right before `title_label` so app icons
            // (`vim`, `lazygit`, `sleep` loader) read as part of the title without ever
            // shifting the centered title or left badges when appearing or fading out.
            self.category_image.set_pixel_size(APP_ICON_SIZE);
            self.category_image.set_valign(gtk::Align::Center);
            self.category_image.set_parent(&*obj);

            self.title_label.set_single_line_mode(true);
            self.title_label
                .set_ellipsize(gtk::pango::EllipsizeMode::End);
            self.title_label.set_valign(gtk::Align::Center);
            self.title_label.set_parent(&*obj);

            self.close_btn.set_icon_name("boxxy-window-close-symbolic");
            self.close_btn
                .set_css_classes(&["flat", "image-button", "tab-close-button", "tab-close-btn"]);
            self.close_btn.set_valign(gtk::Align::Center);
            self.close_btn.set_focus_on_click(false);
            self.close_btn.set_tooltip_text(Some("Close Tab"));
            self.close_btn.set_parent(&*obj);
        }

        fn dispose(&self) {
            for child in [
                self.start_box.upcast_ref::<gtk::Widget>(),
                self.category_image.upcast_ref::<gtk::Widget>(),
                self.title_label.upcast_ref::<gtk::Widget>(),
                self.close_btn.upcast_ref::<gtk::Widget>(),
            ] {
                if child.parent().is_some() {
                    child.unparent();
                }
            }
        }
    }

    impl WidgetImpl for RegularTabContent {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let (start_min_w, start_nat_w, _, _) =
                self.start_box.measure(gtk::Orientation::Horizontal, -1);
            let (start_min_h, start_nat_h, _, _) =
                self.start_box.measure(gtk::Orientation::Vertical, -1);

            let (_, icon_nat_w, _, _) = self
                .category_image
                .measure(gtk::Orientation::Horizontal, -1);
            let (icon_min_h, icon_nat_h, _, _) =
                self.category_image.measure(gtk::Orientation::Vertical, -1);

            let (lbl_min_w, lbl_nat_w, _, _) =
                self.title_label.measure(gtk::Orientation::Horizontal, -1);
            let (lbl_min_h, lbl_nat_h, _, _) =
                self.title_label.measure(gtk::Orientation::Vertical, -1);

            let (end_min_w, end_nat_w, _, _) =
                self.close_btn.measure(gtk::Orientation::Horizontal, -1);
            let (end_min_h, end_nat_h, _, _) =
                self.close_btn.measure(gtk::Orientation::Vertical, -1);

            if orientation == gtk::Orientation::Horizontal {
                let icon_slot_w = icon_nat_w.max(APP_ICON_SIZE) + APP_ICON_TO_LABEL_GAP;
                let min_w = start_min_w + icon_slot_w + lbl_min_w + end_min_w;
                let nat_w = start_nat_w
                    + BADGES_TO_APP_ICON_GAP
                    + icon_slot_w
                    + lbl_nat_w
                    + 6
                    + end_nat_w;
                (min_w, nat_w.max(min_w), -1, -1)
            } else {
                let min_h = start_min_h.max(icon_min_h).max(lbl_min_h).max(end_min_h);
                let nat_h = start_nat_h.max(icon_nat_h).max(lbl_nat_h).max(end_nat_h);
                (min_h, nat_h.max(min_h), -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let (_, start_nat_w, _, _) = self.start_box.measure(gtk::Orientation::Horizontal, -1);
            let (_, start_nat_h, _, _) = self.start_box.measure(gtk::Orientation::Vertical, -1);
            let (_, icon_nat_w, _, _) = self
                .category_image
                .measure(gtk::Orientation::Horizontal, -1);
            let (_, icon_nat_h, _, _) =
                self.category_image.measure(gtk::Orientation::Vertical, -1);
            let (_, lbl_nat_w, _, _) = self.title_label.measure(gtk::Orientation::Horizontal, -1);
            let (_, lbl_nat_h, _, _) = self.title_label.measure(gtk::Orientation::Vertical, -1);
            let (_, end_nat_w, _, _) = self.close_btn.measure(gtk::Orientation::Horizontal, -1);
            let (_, end_nat_h, _, _) = self.close_btn.measure(gtk::Orientation::Vertical, -1);

            let start_w = start_nat_w.min(width.max(0));
            let start_h = start_nat_h.min(height.max(0));
            let start_y = ((height - start_h) / 2).max(0);
            self.start_box
                .size_allocate(&gtk::Allocation::new(0, start_y, start_w, start_h), -1);

            let end_w = end_nat_w.min((width - start_w).max(0));
            let end_h = end_nat_h.min(height.max(0));
            let end_x = (width - end_w).max(0);
            let end_y = ((height - end_h) / 2).max(0);
            self.close_btn
                .size_allocate(&gtk::Allocation::new(end_x, end_y, end_w, end_h), -1);

            let has_badges = self.color_dot.is_visible()
                || self.indicator_image.is_visible()
                || self.bell_image.is_visible();
            let min_icon_x = if has_badges {
                start_w + BADGES_TO_APP_ICON_GAP
            } else {
                5
            };
            let reserved_icon_w = icon_nat_w.max(APP_ICON_SIZE);
            let min_label_x = min_icon_x + reserved_icon_w + APP_ICON_TO_LABEL_GAP;
            let max_label_right = (end_x - 4).max(min_label_x);

            let avail_label_w = (max_label_right - min_label_x).max(0);
            let lbl_w = lbl_nat_w.min(avail_label_w);
            let centered_lbl_x = (width - lbl_w) / 2;
            let lbl_x =
                centered_lbl_x.clamp(min_label_x, (max_label_right - lbl_w).max(min_label_x));

            let icon_w = reserved_icon_w.min((lbl_x - min_icon_x).max(0));
            let icon_x = (lbl_x - APP_ICON_TO_LABEL_GAP - icon_w).max(0);
            let icon_h = icon_nat_h.max(APP_ICON_SIZE).min(height.max(0));
            let icon_y = ((height - icon_h) / 2).max(0);
            self.category_image
                .size_allocate(&gtk::Allocation::new(icon_x, icon_y, icon_w, icon_h), -1);

            let lbl_h = lbl_nat_h.min(height.max(0));
            let lbl_y = ((height - lbl_h) / 2).max(0);
            self.title_label
                .size_allocate(&gtk::Allocation::new(lbl_x, lbl_y, lbl_w, lbl_h), -1);
        }
    }
}

glib::wrapper! {
    pub struct RegularTabContent(ObjectSubclass<regular_imp::RegularTabContent>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for RegularTabContent {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BoxxyTab {
        pub root_stack: gtk::Stack,
        pub pinned_image: gtk::Image,
        pub regular: RegularTabContent,
        pub page: RefCell<Option<adw::TabPage>>,
        pub page_handlers: RefCell<Vec<glib::SignalHandlerId>>,
        pub hover_switch_source: Cell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BoxxyTab {
        const NAME: &'static str = "BoxxyTab";
        type Type = super::BoxxyTab;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("tab");
            klass.set_accessible_role(gtk::AccessibleRole::Tab);
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for BoxxyTab {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            self.root_stack
                .set_transition_type(gtk::StackTransitionType::None);
            self.root_stack.set_hexpand(true);
            self.root_stack.set_vexpand(true);

            self.pinned_image.set_pixel_size(16);
            self.pinned_image.set_halign(gtk::Align::Center);
            self.pinned_image.set_valign(gtk::Align::Center);
            self.root_stack.add_named(&self.pinned_image, Some("pinned"));

            self.regular.set_hexpand(true);
            self.regular.set_vexpand(true);
            self.root_stack.add_named(&self.regular, Some("regular"));

            self.root_stack.set_parent(&*obj);
        }

        fn dispose(&self) {
            if let Some(src) = self.hover_switch_source.take() {
                src.remove();
            }
            self.obj().unbind_page();
            if self.root_stack.parent().is_some() {
                self.root_stack.unparent();
            }
        }
    }

    impl WidgetImpl for BoxxyTab {}
}

glib::wrapper! {
    pub struct BoxxyTab(ObjectSubclass<imp::BoxxyTab>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl BoxxyTab {
    pub(super) fn new(
        tab_view: &adw::TabView,
        page: &adw::TabPage,
        callbacks: &TabCallbacks,
        attach_pinned_gestures: bool,
    ) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.bind_page(tab_view, page, callbacks, attach_pinned_gestures);
        obj
    }

    #[must_use]
    pub fn page(&self) -> Option<adw::TabPage> {
        self.imp().page.borrow().clone()
    }

    pub(super) fn unbind_page(&self) {
        let imp = self.imp();
        if let Some(page) = imp.page.borrow_mut().take() {
            for handler in imp.page_handlers.borrow_mut().drain(..) {
                page.disconnect(handler);
            }
        }
    }

    #[must_use]
    pub fn category_image(&self) -> &gtk::Image {
        &self.imp().regular.imp().category_image
    }

    #[must_use]
    pub fn indicator_image(&self) -> &gtk::Image {
        &self.imp().regular.imp().indicator_image
    }

    #[must_use]
    pub fn bell_image(&self) -> &gtk::Image {
        &self.imp().regular.imp().bell_image
    }

    #[must_use]
    pub fn color_dot(&self) -> &gtk::Box {
        &self.imp().regular.imp().color_dot
    }

    #[must_use]
    pub fn title_label(&self) -> &gtk::Label {
        &self.imp().regular.imp().title_label
    }

    #[must_use]
    pub fn is_point_on_close_button(&self, x: f64, y: f64) -> bool {
        let close_btn = &self.imp().regular.imp().close_btn;
        if !close_btn.is_visible() {
            return false;
        }
        self.pick(x, y, gtk::PickFlags::DEFAULT)
            .is_some_and(|hit| hit == *close_btn || hit.is_ancestor(close_btn))
    }

    pub fn set_selected(&self, selected: bool) {
        if selected {
            self.set_state_flags(gtk::StateFlags::SELECTED, false);
        } else {
            self.unset_state_flags(gtk::StateFlags::SELECTED);
        }
    }

    pub fn set_tab_color(&self, color: TabColor) {
        let target_cls = color.as_css_class();
        for c in TabColor::all() {
            if let Some(cls) = c.as_css_class()
                && Some(cls) != target_cls
                && self.has_css_class(cls)
            {
                self.remove_css_class(cls);
            }
        }
        if let Some(cls) = target_cls
            && !self.has_css_class(cls)
        {
            self.add_css_class(cls);
        }
        let show_dot = target_cls.is_some();
        let dot = &self.imp().regular.imp().color_dot;
        if dot.is_visible() != show_dot {
            dot.set_visible(show_dot);
        }
    }

    pub fn sync_from_page(&self) {
        let imp = self.imp();
        let Some(page) = imp.page.borrow().clone() else {
            return;
        };

        if page.is_pinned() {
            imp.root_stack.set_visible_child_name("pinned");
            if let Some(icon) = page.icon() {
                imp.pinned_image.set_from_gicon(&icon);
            }
            let tooltip = page.tooltip();
            if let Some(ref tt) = tooltip
                && !tt.is_empty()
            {
                self.set_tooltip_markup(Some(tt.as_str()));
            } else {
                self.set_tooltip_text(Some(page.title().as_str()));
            }
        } else {
            imp.root_stack.set_visible_child_name("regular");
            imp.regular
                .imp()
                .title_label
                .set_label(page.title().as_str());
            let tooltip = page.tooltip();
            if let Some(ref tt) = tooltip
                && !tt.is_empty()
            {
                self.set_tooltip_markup(Some(tt.as_str()));
            } else {
                self.set_tooltip_markup(None);
            }
        }
    }

    fn bind_page(
        &self,
        tab_view: &adw::TabView,
        page: &adw::TabPage,
        callbacks: &TabCallbacks,
        attach_pinned_gestures: bool,
    ) {
        let imp = self.imp();
        *imp.page.borrow_mut() = Some(page.clone());

        let mut handlers = Vec::with_capacity(4);
        for prop in ["title", "tooltip", "icon", "pinned"] {
            let tab_weak = self.downgrade();
            let id = page.connect_notify_local(Some(prop), move |_, _| {
                if let Some(tab) = tab_weak.upgrade() {
                    tab.sync_from_page();
                }
            });
            handlers.push(id);
        }
        *imp.page_handlers.borrow_mut() = handlers;
        self.sync_from_page();

        {
            let tv_weak = tab_view.downgrade();
            let page_weak = page.downgrade();
            imp.regular.imp().close_btn.connect_clicked(move |_| {
                if let (Some(tv), Some(p)) = (tv_weak.upgrade(), page_weak.upgrade()) {
                    tv.close_page(&p);
                }
            });
        }

        // Pinned tabs live in `pinned_box` outside `TabStrip`, so they carry direct click
        // and context-menu controllers. Unpinned tabs are driven centrally by `TabStrip`.
        if attach_pinned_gestures {
            let primary = gtk::GestureClick::new();
            primary.set_button(gdk::BUTTON_PRIMARY);
            let tv_weak = tab_view.downgrade();
            let page_weak = page.downgrade();
            primary.connect_pressed(move |gesture, _, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let (Some(tv), Some(p)) = (tv_weak.upgrade(), page_weak.upgrade()) {
                    tv.set_selected_page(&p);
                }
            });
            self.add_controller(primary);

            let secondary = gtk::GestureClick::new();
            secondary.set_button(gdk::BUTTON_SECONDARY);
            let tab_weak = self.downgrade();
            let page_weak = page.downgrade();
            let on_ctx = callbacks.on_context_menu.clone();
            secondary.connect_pressed(move |gesture, _, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let (Some(tab), Some(p)) = (tab_weak.upgrade(), page_weak.upgrade()) {
                    on_ctx(&tab, &p);
                }
            });
            self.add_controller(secondary);
        } else {
            self.setup_pane_drop(tab_view, page, callbacks);
        }
    }

    fn setup_pane_drop(
        &self,
        tab_view: &adw::TabView,
        page: &adw::TabPage,
        callbacks: &TabCallbacks,
    ) {
        let pane_drop = gtk::DropTarget::new(
            boxxy_terminal::PaneDragPayload::static_type(),
            gdk::DragAction::MOVE,
        );
        pane_drop.set_preload(true);

        {
            let page_weak = page.downgrade();
            let on_pane_drop = callbacks.on_pane_drop.clone();
            pane_drop.connect_drop(move |_, value, _, _| {
                let Ok(boxxy_terminal::PaneDragPayload(source_pane_id)) =
                    value.get::<boxxy_terminal::PaneDragPayload>()
                else {
                    return false;
                };
                let Some(p) = page_weak.upgrade() else {
                    return false;
                };
                if p.is_pinned() {
                    return false;
                }
                let target_page_key = p.child().as_ptr() as usize;
                on_pane_drop(source_pane_id, target_page_key);
                true
            });
        }
        self.add_controller(pane_drop);

        let drop_motion = gtk::DropControllerMotion::new();
        {
            let tab_weak = self.downgrade();
            let tv_weak = tab_view.downgrade();
            let page_weak = page.downgrade();
            drop_motion.connect_enter(move |ctrl, _, _| {
                let is_pane_drag = ctrl.drop().is_some_and(|d| {
                    d.formats()
                        .contains_type(boxxy_terminal::PaneDragPayload::static_type())
                });
                if !is_pane_drag {
                    return;
                }
                let Some(tab) = tab_weak.upgrade() else {
                    return;
                };
                if let Some(prev) = tab.imp().hover_switch_source.take() {
                    prev.remove();
                }
                let tab_for_cb = tab.downgrade();
                let tv_for_cb = tv_weak.clone();
                let page_for_cb = page_weak.clone();
                let src = glib::timeout_add_local_once(Duration::from_millis(350), move || {
                    if let Some(t) = tab_for_cb.upgrade() {
                        t.imp().hover_switch_source.set(None);
                    }
                    if let (Some(tv), Some(p)) = (tv_for_cb.upgrade(), page_for_cb.upgrade())
                        && !p.is_pinned()
                    {
                        tv.set_selected_page(&p);
                    }
                });
                tab.imp().hover_switch_source.set(Some(src));
            });
        }
        {
            let tab_weak = self.downgrade();
            drop_motion.connect_leave(move |_| {
                if let Some(tab) = tab_weak.upgrade()
                    && let Some(prev) = tab.imp().hover_switch_source.take()
                {
                    prev.remove();
                }
            });
        }
        self.add_controller(drop_motion);
    }
}
