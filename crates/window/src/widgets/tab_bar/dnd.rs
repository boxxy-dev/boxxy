use adw::prelude::*;
use gtk::gdk;
use gtk::glib;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::slot::{OPEN_CLOSE_DURATION_MS, REORDER_DURATION_MS, TabSlot};
use super::strip::{ActiveInBarDrag, TabStrip};
use super::tab::{TabCallbacks, TabDragPayload};

const TEAR_OFF_VERTICAL_MARGIN: f64 = 32.0;

struct PressedTabState {
    slot: Rc<TabSlot>,
    grab_offset_x: f64,
    grab_offset_y: f64,
}

pub(super) fn setup_strip_interactions(
    strip: &TabStrip,
    new_tab_btn: &gtk::Button,
    tab_view: &adw::TabView,
    callbacks: &TabCallbacks,
) {
    setup_clicks(strip, tab_view, callbacks);
    setup_drag_and_tear_off(strip, tab_view, callbacks);
    setup_drop_targets(strip, new_tab_btn, tab_view);
}

fn setup_clicks(strip: &TabStrip, tab_view: &adw::TabView, callbacks: &TabCallbacks) {
    let click = gtk::GestureClick::new();
    click.set_button(0);
    let middle_pressed: Rc<RefCell<Option<adw::TabPage>>> = Rc::new(RefCell::new(None));

    {
        let strip_weak = strip.downgrade();
        let tv_weak = tab_view.downgrade();
        let on_ctx = callbacks.on_context_menu.clone();
        let mid_slot = middle_pressed.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            let (Some(strip), Some(tv)) = (strip_weak.upgrade(), tv_weak.upgrade()) else {
                return;
            };
            let content_x = x + strip.imp().scroll_x.get();
            let Some(slot) = strip.find_alive_slot_at_content_x(content_x) else {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            };
            let Some(page) = slot.page.borrow().clone() else {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            };

            let pt = gtk::graphene::Point::new(x as f32, y as f32);
            if let Some(tab_pt) = strip.compute_point(&slot.tab, &pt)
                && slot
                    .tab
                    .is_point_on_close_button(f64::from(tab_pt.x()), f64::from(tab_pt.y()))
            {
                // Let the embedded close `GtkButton` handle its own click without switching tabs.
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }

            let btn = gesture.current_button();
            if btn == gdk::BUTTON_SECONDARY {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                on_ctx(&slot.tab, &page);
            } else if btn == gdk::BUTTON_MIDDLE {
                *mid_slot.borrow_mut() = Some(page);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            } else if btn == gdk::BUTTON_PRIMARY {
                // Select immediately on press (matching AdwTabBar), but do NOT claim the sequence
                // here so `GestureDrag` can seamlessly take over if the pointer moves!
                tv.set_selected_page(&page);
            } else {
                gesture.set_state(gtk::EventSequenceState::Denied);
            }
        });
    }

    {
        let strip_weak = strip.downgrade();
        let tv_weak = tab_view.downgrade();
        let mid_slot = middle_pressed;
        click.connect_released(move |gesture, _, x, y| {
            if gesture.current_button() != gdk::BUTTON_MIDDLE {
                return;
            }
            let expected = mid_slot.borrow_mut().take();
            let (Some(strip), Some(tv), Some(expected_page)) =
                (strip_weak.upgrade(), tv_weak.upgrade(), expected)
            else {
                return;
            };
            if y < 0.0 || y > f64::from(strip.height()) {
                return;
            }
            let content_x = x + strip.imp().scroll_x.get();
            if let Some(slot) = strip.find_alive_slot_at_content_x(content_x)
                && slot.page.borrow().as_ref() == Some(&expected_page)
            {
                tv.close_page(&expected_page);
            }
        });
    }

    strip.add_controller(click);
}

fn setup_drag_and_tear_off(strip: &TabStrip, tab_view: &adw::TabView, callbacks: &TabCallbacks) {
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);

    let pressed: Rc<RefCell<Option<PressedTabState>>> = Rc::new(RefCell::new(None));
    let tore_off: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    {
        let strip_weak = strip.downgrade();
        let pressed_c = pressed.clone();
        let tore_off_c = tore_off.clone();
        drag.connect_drag_begin(move |gesture, start_x, start_y| {
            tore_off_c.set(false);
            *pressed_c.borrow_mut() = None;

            let Some(strip) = strip_weak.upgrade() else {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            };
            let content_x = start_x + strip.imp().scroll_x.get();
            let Some(slot) = strip.find_alive_slot_at_content_x(content_x) else {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            };

            let pt = gtk::graphene::Point::new(start_x as f32, start_y as f32);
            if let Some(tab_pt) = strip.compute_point(&slot.tab, &pt)
                && slot
                    .tab
                    .is_point_on_close_button(f64::from(tab_pt.x()), f64::from(tab_pt.y()))
            {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }

            let cur_slot_x = f64::from(slot.base_x.get()) + slot.visual_offset_x.get();
            *pressed_c.borrow_mut() = Some(PressedTabState {
                slot,
                grab_offset_x: (content_x - cur_slot_x).max(0.0),
                grab_offset_y: start_y.max(0.0),
            });
        });
    }

    {
        let strip_weak = strip.downgrade();
        let tv_weak = tab_view.downgrade();
        let pressed_c = pressed.clone();
        let tore_off_c = tore_off.clone();
        let on_detach = callbacks.on_detach_to_new_window.clone();

        drag.connect_drag_update(move |gesture, offset_x, offset_y| {
            if tore_off_c.get() {
                return;
            }
            let (Some(strip), Some(tv)) = (strip_weak.upgrade(), tv_weak.upgrade()) else {
                return;
            };
            let Some(pressed_info) = pressed_c.borrow().as_ref().map(|p| {
                (
                    p.slot.clone(),
                    p.grab_offset_x,
                    p.grab_offset_y,
                )
            }) else {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            };
            let (slot, grab_x, grab_y) = pressed_info;
            let Some(page) = slot.page.borrow().clone() else {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            };

            let is_already_dragging = strip.imp().active_drag.borrow().is_some();
            if !is_already_dragging {
                let threshold = f64::from(strip.settings().gtk_dnd_drag_threshold().max(4));
                if offset_x.abs() < threshold && offset_y.abs() < threshold {
                    return;
                }
                tv.set_selected_page(&page);
                gesture.set_state(gtk::EventSequenceState::Claimed);
                slot.stop_animations();
                slot.visual_offset_x.set(0.0);
                *strip.imp().top_z_slot.borrow_mut() = Some(Rc::downgrade(&slot));
                *strip.imp().active_drag.borrow_mut() = Some(ActiveInBarDrag {
                    slot: slot.clone(),
                    grab_offset_x: grab_x,
                    grab_offset_y: grab_y,
                    drag_content_x: f64::from(slot.base_x.get()),
                });
            }

            let Some((start_x, start_y)) = gesture.start_point() else {
                return;
            };
            let cur_x = start_x + offset_x;
            let cur_y = start_y + offset_y;

            // Stage 2: Tear off into an OS `GdkDrag` when pulled vertically out of the tab bar
            let outside_vertically = cur_y < -TEAR_OFF_VERTICAL_MARGIN
                || cur_y > f64::from(strip.height()) + TEAR_OFF_VERTICAL_MARGIN;
            if outside_vertically
                && strip.alive_count() > 1
                && let Some(device) = gesture.current_event_device()
                && begin_os_tab_drag(&strip, &tv, &slot, &page, &device, &on_detach)
            {
                tore_off_c.set(true);
                return;
            }

            // Stage 1: Smooth pixel-for-pixel in-bar horizontal reorder
            let content_cursor_x = cur_x + strip.imp().scroll_x.get();
            let slot_w = slot.alloc_w.get().max(1);
            let max_content_x = {
                let per_w = strip.imp().per_tab_width.get().max(1);
                f64::from((strip.animated_content_width(per_w) - slot_w).max(0))
            };
            let drag_cx = (content_cursor_x - grab_x).clamp(0.0, max_content_x);

            if let Some(ref mut active) = *strip.imp().active_drag.borrow_mut() {
                active.drag_content_x = drag_cx;
            }

            maybe_swap_crossed_neighbor(&strip, &tv, &slot, &page, drag_cx, f64::from(slot_w));
            strip.queue_allocate();
            strip.queue_draw();
        });
    }

    {
        let strip_weak = strip.downgrade();
        let pressed_c = pressed;
        let tore_off_c = tore_off;
        drag.connect_drag_end(move |_, _, _| {
            *pressed_c.borrow_mut() = None;
            if tore_off_c.get() {
                tore_off_c.set(false);
                return;
            }
            let Some(strip) = strip_weak.upgrade() else {
                return;
            };
            let active_opt = strip.imp().active_drag.borrow_mut().take();
            if let Some(active) = active_opt {
                let delta = active.drag_content_x - f64::from(active.slot.base_x.get());
                active.slot.visual_offset_x.set(delta);
                *strip.imp().top_z_slot.borrow_mut() = Some(Rc::downgrade(&active.slot));
                active.slot.animate_offset_to_zero(&strip, REORDER_DURATION_MS);
                strip.queue_allocate();
                strip.queue_draw();
            }
        });
    }

    strip.add_controller(drag);
}

pub(super) fn maybe_swap_crossed_neighbor(
    strip: &TabStrip,
    tv: &adw::TabView,
    dragged_slot: &Rc<TabSlot>,
    dragged_page: &adw::TabPage,
    drag_cx: f64,
    drag_w: f64,
) {
    let alive_slots: Vec<Rc<TabSlot>> = strip
        .imp()
        .slots
        .borrow()
        .iter()
        .filter(|s| s.is_alive())
        .cloned()
        .collect();

    let Some(cur_idx) = alive_slots.iter().position(|s| Rc::ptr_eq(s, dragged_slot)) else {
        return;
    };

    // Match `adw-tab-box.c`: moving left swaps as soon as the dragged tab's leading edge
    // crosses left of a neighbor's center; moving right swaps as soon as its trailing edge
    // crosses right of a neighbor's center. `TAB_GAP` provides natural hysteresis.
    let drag_left = drag_cx;
    let drag_right = drag_cx + drag_w;

    let mut target_idx = cur_idx;
    for (idx, neighbor) in alive_slots.iter().enumerate() {
        if idx == cur_idx {
            continue;
        }
        let n_center = f64::from(neighbor.base_x.get()) + f64::from(neighbor.alloc_w.get()) * 0.5;
        if idx < cur_idx && drag_left < n_center {
            target_idx = idx;
            break;
        }
        if idx > cur_idx && drag_right > n_center {
            target_idx = idx;
        }
    }

    if target_idx != cur_idx {
        let dst_pos = tv.n_pinned_pages() + target_idx as i32;
        strip.imp().suppress_reorder_signal.set(true);
        tv.reorder_page(dragged_page, dst_pos);
        strip.imp().suppress_reorder_signal.set(false);
        strip.sync_order_with_flip(tv, Some(dragged_slot));
    }
}

fn begin_os_tab_drag(
    strip: &TabStrip,
    tv: &adw::TabView,
    slot: &Rc<TabSlot>,
    page: &adw::TabPage,
    device: &gdk::Device,
    on_detach: &super::tab::DetachTabCallback,
) -> bool {
    let Some(native) = strip.native() else {
        return false;
    };
    let Some(surface) = native.surface() else {
        return false;
    };

    let (grab_x, grab_y) = strip
        .imp()
        .active_drag
        .borrow()
        .as_ref()
        .map_or((20, 14), |d| (d.grab_offset_x as i32, d.grab_offset_y as i32));

    // Snapshot the live styled BoxxyTab at 1:1 scale before collapsing its slot in the bar.
    let tab_paintable = snapshot_tab_paintable(&slot.tab);

    let handled = Arc::new(AtomicBool::new(false));
    let payload = TabDragPayload {
        source_view: tv.downgrade(),
        page: page.downgrade(),
        handled: handled.clone(),
    };
    let content = gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_value(&payload.to_value()),
        gdk::ContentProvider::for_bytes(
            "application/x-rootwindow-drop",
            &glib::Bytes::from_static(b""),
        ),
    ]);

    let Some(gdk_drag) = gdk::Drag::begin(
        &surface,
        device,
        &content,
        gdk::DragAction::MOVE,
        0.0,
        0.0,
    ) else {
        return false;
    };

    if let Some(paintable) = tab_paintable {
        gtk::DragIcon::set_from_paintable(&gdk_drag, &paintable, grab_x, grab_y);
    }

    // Collapse the torn-off slot in the source strip so remaining tabs & `+` glide closed.
    *strip.imp().active_drag.borrow_mut() = None;
    *strip.imp().torn_off_page.borrow_mut() = Some(page.clone());
    slot.widget.set_opacity(0.0);
    slot.animate_appear(strip, 0.0, OPEN_CLOSE_DURATION_MS, None);

    let drop_performed = Rc::new(Cell::new(false));
    {
        let dp = drop_performed.clone();
        gdk_drag.connect_drop_performed(move |_| {
            dp.set(true);
        });
    }

    {
        let strip_weak = strip.downgrade();
        let tv_weak = tv.downgrade();
        let page_weak = page.downgrade();
        let handled_c = handled.clone();
        let dp = drop_performed;
        let on_detach_c = on_detach.clone();
        gdk_drag.connect_dnd_finished(move |d| {
            if handled_c.load(Ordering::SeqCst) {
                d.drop_done(true);
                return;
            }
            if dp.get()
                && let (Some(tv), Some(p)) = (tv_weak.upgrade(), page_weak.upgrade())
                && (tv.n_pages() - tv.n_pinned_pages()) > 1
            {
                let key = p.child().as_ptr() as usize;
                on_detach_c(key);
                d.drop_done(true);
            } else {
                if let (Some(s), Some(p)) = (strip_weak.upgrade(), page_weak.upgrade()) {
                    s.restore_torn_off_page(&p);
                }
                d.drop_done(false);
            }
        });
    }

    {
        let strip_weak = strip.downgrade();
        let tv_weak = tv.downgrade();
        let page_weak = page.downgrade();
        let handled_c = handled;
        let on_detach_c = on_detach.clone();
        gdk_drag.connect_cancel(move |d, reason| {
            if handled_c.load(Ordering::SeqCst) {
                d.drop_done(true);
                return;
            }
            if reason == gdk::DragCancelReason::NoTarget
                && let (Some(tv), Some(p)) = (tv_weak.upgrade(), page_weak.upgrade())
                && (tv.n_pages() - tv.n_pinned_pages()) > 1
            {
                handled_c.store(true, Ordering::SeqCst);
                let key = p.child().as_ptr() as usize;
                on_detach_c(key);
                d.drop_done(true);
            } else {
                if let (Some(s), Some(p)) = (strip_weak.upgrade(), page_weak.upgrade()) {
                    s.restore_torn_off_page(&p);
                }
                d.drop_done(false);
            }
        });
    }

    true
}

fn snapshot_tab_paintable(tab: &super::tab::BoxxyTab) -> Option<gdk::Paintable> {
    let bounds = tab.compute_bounds(tab)?;
    let w = bounds.width().max(1.0);
    let h = bounds.height().max(1.0);
    let wp = gtk::WidgetPaintable::new(Some(tab));
    let snap = gtk::Snapshot::new();
    gdk::prelude::PaintableExt::snapshot(&wp, &snap, f64::from(w), f64::from(h));
    snap.to_paintable(None)
}

fn setup_drop_targets(strip: &TabStrip, new_tab_btn: &gtk::Button, tab_view: &adw::TabView) {
    // 1. Drop onto `TabStrip` inserts at the exact hovered tab slot position.
    let strip_drop = gtk::DropTarget::new(TabDragPayload::static_type(), gdk::DragAction::MOVE);
    strip_drop.set_preload(true);
    {
        let strip_weak = strip.downgrade();
        let target_tv_weak = tab_view.downgrade();
        strip_drop.connect_drop(move |_, value, x, _| {
            let Ok(payload) = value.get::<TabDragPayload>() else {
                return false;
            };
            let (Some(strip), Some(src_tv), Some(page), Some(target_tv)) = (
                strip_weak.upgrade(),
                payload.source_view.upgrade(),
                payload.page.upgrade(),
                target_tv_weak.upgrade(),
            ) else {
                return false;
            };
            if page.is_pinned() {
                return false;
            }
            payload.handled.store(true, Ordering::SeqCst);
            let unpinned_idx = strip.insertion_index_for_x(x);
            let dst_pos = target_tv.n_pinned_pages() + unpinned_idx;
            complete_tab_drop(&strip, &src_tv, &target_tv, &page, dst_pos);
            true
        });
    }
    strip.add_controller(strip_drop);

    // 2. Drop onto `+` button appends to the end of the strip (same or cross-window).
    {
        let drop = gtk::DropTarget::new(TabDragPayload::static_type(), gdk::DragAction::MOVE);
        let strip_weak = strip.downgrade();
        let target_tv_weak = tab_view.downgrade();
        drop.connect_drop(move |_, value, _, _| {
            let Ok(payload) = value.get::<TabDragPayload>() else {
                return false;
            };
            let (Some(strip), Some(src_tv), Some(page), Some(target_tv)) = (
                strip_weak.upgrade(),
                payload.source_view.upgrade(),
                payload.page.upgrade(),
                target_tv_weak.upgrade(),
            ) else {
                return false;
            };
            if page.is_pinned() {
                return false;
            }
            payload.handled.store(true, Ordering::SeqCst);
            let dst_pos = if src_tv == target_tv {
                (target_tv.n_pages() - 1).max(target_tv.n_pinned_pages())
            } else {
                target_tv.n_pages()
            };
            complete_tab_drop(&strip, &src_tv, &target_tv, &page, dst_pos);
            true
        });
        new_tab_btn.add_controller(drop);
    }

    // 3. Drop onto another Boxxy window's `AdwTabView` content transfers the tab into that
    //    window, while rejecting same-window content drops so dragging a tab down out of the
    //    headerbar detaches it into a new window.
    {
        let view_drop = gtk::DropTarget::new(TabDragPayload::static_type(), gdk::DragAction::MOVE);
        view_drop.set_preload(true);
        let target_tv_motion = tab_view.downgrade();
        view_drop.connect_motion(move |tgt, _, _| {
            let Some(val) = tgt.value() else {
                return gdk::DragAction::MOVE;
            };
            let Ok(payload) = val.get::<TabDragPayload>() else {
                return gdk::DragAction::empty();
            };
            let (Some(src_tv), Some(target_tv)) =
                (payload.source_view.upgrade(), target_tv_motion.upgrade())
            else {
                return gdk::DragAction::empty();
            };
            if src_tv == target_tv {
                gdk::DragAction::empty()
            } else {
                gdk::DragAction::MOVE
            }
        });
        let strip_weak = strip.downgrade();
        let target_tv_weak = tab_view.downgrade();
        view_drop.connect_drop(move |_, value, _, _| {
            let Ok(payload) = value.get::<TabDragPayload>() else {
                return false;
            };
            let (Some(strip), Some(src_tv), Some(page), Some(target_tv)) = (
                strip_weak.upgrade(),
                payload.source_view.upgrade(),
                payload.page.upgrade(),
                target_tv_weak.upgrade(),
            ) else {
                return false;
            };
            if page.is_pinned() || src_tv == target_tv {
                return false;
            }
            payload.handled.store(true, Ordering::SeqCst);
            complete_tab_drop(&strip, &src_tv, &target_tv, &page, target_tv.n_pages());
            true
        });
        tab_view.add_controller(view_drop);
    }
}

fn complete_tab_drop(
    target_strip: &TabStrip,
    src_tv: &adw::TabView,
    target_tv: &adw::TabView,
    page: &adw::TabPage,
    dst_pos: i32,
) {
    if src_tv == target_tv {
        target_strip.restore_torn_off_page(page);
        let max_pos = (target_tv.n_pages() - 1).max(target_tv.n_pinned_pages());
        target_tv.reorder_page(page, dst_pos.min(max_pos));
    } else {
        src_tv.transfer_page(page, target_tv, dst_pos.min(target_tv.n_pages()));
        target_tv.set_selected_page(page);
        if let Some(win) = target_tv.root().and_downcast::<gtk::Window>() {
            win.present();
        }
    }
}
