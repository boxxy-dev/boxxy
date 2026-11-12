use super::app_menu::{AppMenuComponent, AppMenuContext};
use crate::PaneOutput;
use crate::search_bar::SearchBarComponent;
use boxxy_vte::terminal::TerminalWidget;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) fn export_and_open_scrollback(terminal: &TerminalWidget) {
    let selected = terminal.selected_text();
    let term_weak = terminal.downgrade();
    glib::spawn_future_local(async move {
        let Some(term) = term_weak.upgrade() else {
            return;
        };
        let text = match selected {
            Some(sel) if !sel.is_empty() => sel,
            _ => term.dump_buffer(true).await.unwrap_or_default(),
        };
        let path_res = boxxy_sys_utils::runtime()
            .spawn_blocking(move || -> std::io::Result<std::path::PathBuf> {
                let mut path = std::env::temp_dir();
                path.push(format!("boxxy-scrollback-{}.txt", uuid::Uuid::new_v4()));
                std::fs::write(&path, text.as_bytes())?;
                Ok(path)
            })
            .await;

        if let Ok(Ok(path)) = path_res
            && let Some(term) = term_weak.upgrade()
        {
            if let Some(path_str) = path.to_str() {
                term.display().clipboard().set_text(path_str);
            }
            let file = gio::File::for_path(&path);
            let launcher = gtk::FileLauncher::new(Some(&file));
            let root_win = term.root().and_then(|r| r.downcast::<gtk::Window>().ok());
            launcher.launch(root_win.as_ref(), gio::Cancellable::NONE, |_| {});
        }
    });
}

pub(super) fn setup_gestures(
    terminal: &TerminalWidget,
    inner: &Rc<RefCell<super::PaneInner>>,
    search_bar: &Rc<SearchBarComponent>,
    callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    id: String,
) {
    let focus_ctrl = gtk::EventControllerFocus::new();
    let cb_clone = callback.clone();
    focus_ctrl.connect_enter(move |_| {
        cb_clone(PaneOutput::Focused(id.clone()));
    });
    terminal.add_controller(focus_ctrl);

    let action_group = gio::SimpleActionGroup::new();
    let copy_action = gio::SimpleAction::new("copy", None);
    let term_copy = terminal.downgrade();
    copy_action.connect_activate(move |_, _| {
        if let Some(term) = term_copy.upgrade() {
            term.copy_clipboard();
        }
    });
    action_group.add_action(&copy_action);

    let paste_action = gio::SimpleAction::new("paste", None);
    let term_paste = terminal.downgrade();
    paste_action.connect_activate(move |_, _| {
        if let Some(term) = term_paste.upgrade() {
            term.paste_clipboard();
        }
    });
    action_group.add_action(&paste_action);

    let select_all_action = gio::SimpleAction::new("select-all", None);
    let term_select = terminal.downgrade();
    select_all_action.connect_activate(move |_, _| {
        if let Some(term) = term_select.upgrade() {
            term.select_all();
        }
    });
    action_group.add_action(&select_all_action);

    let copy_last_output_action = gio::SimpleAction::new("copy-last-output", None);
    let term_copy_last = terminal.downgrade();
    copy_last_output_action.connect_activate(move |_, _| {
        if let Some(term) = term_copy_last.upgrade() {
            term.copy_last_command_output();
        }
    });
    action_group.add_action(&copy_last_output_action);

    let open_scrollback_action = gio::SimpleAction::new("open-scrollback", None);
    let term_open_scrollback = terminal.downgrade();
    open_scrollback_action.connect_activate(move |_, _| {
        if let Some(term) = term_open_scrollback.upgrade() {
            export_and_open_scrollback(&term);
        }
    });
    action_group.add_action(&open_scrollback_action);

    let clear_scrollback_action = gio::SimpleAction::new("clear-scrollback", None);
    let term_clear_sb = terminal.downgrade();
    clear_scrollback_action.connect_activate(move |_, _| {
        if let Some(term) = term_clear_sb.upgrade() {
            term.clear_scrollback();
        }
    });
    action_group.add_action(&clear_scrollback_action);

    let reset_terminal_action = gio::SimpleAction::new("reset-terminal", None);
    let term_reset = terminal.downgrade();
    reset_terminal_action.connect_activate(move |_, _| {
        if let Some(term) = term_reset.upgrade() {
            term.reset_terminal();
        }
    });
    action_group.add_action(&reset_terminal_action);

    let copy_path_action = gio::SimpleAction::new("copy-path", Some(glib::VariantTy::STRING));
    let term_copy_path = terminal.downgrade();
    copy_path_action.connect_activate(move |_, param: Option<&glib::Variant>| {
        if let Some(path) = param.and_then(|v| v.get::<String>())
            && let Some(term) = term_copy_path.upgrade()
        {
            term.display().clipboard().set_text(&path);
        }
    });
    action_group.add_action(&copy_path_action);

    terminal.insert_action_group("term", Some(&action_group));

    let search_bar_clone_ev = search_bar.clone();
    let term_for_sb_ev = terminal.downgrade();
    let ev_ctrl_search = gtk::EventControllerKey::new();
    ev_ctrl_search.set_propagation_phase(gtk::PropagationPhase::Capture);
    ev_ctrl_search.connect_key_pressed(move |_, keyval, _, state| {
        if (keyval == gtk::gdk::Key::f || keyval == gtk::gdk::Key::F)
            && state.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            && state.contains(gtk::gdk::ModifierType::SHIFT_MASK)
        {
            if !search_bar_clone_ev.is_visible() {
                search_bar_clone_ev.reveal();
            } else {
                search_bar_clone_ev.hide();
            }
            gtk::glib::Propagation::Stop
        } else if (keyval == gtk::gdk::Key::j || keyval == gtk::gdk::Key::J)
            && state.contains(
                gtk::gdk::ModifierType::CONTROL_MASK
                    | gtk::gdk::ModifierType::ALT_MASK
                    | gtk::gdk::ModifierType::SHIFT_MASK,
            )
        {
            if let Some(term) = term_for_sb_ev.upgrade() {
                export_and_open_scrollback(&term);
            }
            gtk::glib::Propagation::Stop
        } else if keyval == gtk::gdk::Key::Escape && search_bar_clone_ev.is_visible() {
            search_bar_clone_ev.hide();
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    terminal.add_controller(ev_ctrl_search);

    let app_menu = AppMenuComponent::new();
    app_menu.widget().set_parent(terminal);

    let app_menu_widget = app_menu.widget().clone();
    terminal.connect_destroy(move |_| {
        app_menu_widget.unparent();
    });

    let middle_gesture = gtk::GestureClick::new();
    middle_gesture.set_button(gdk::BUTTON_MIDDLE);
    middle_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

    let term_for_middle_click = terminal.downgrade();
    middle_gesture.connect_pressed(move |gesture: &gtk::GestureClick, _n_press, _x, _y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(term) = term_for_middle_click.upgrade() {
            term.paste_primary();
        }
    });
    terminal.add_controller(middle_gesture);

    // Right-click context menu is driven by the terminal engine's ownership
    // decision (inside boxxy-vte's click_gesture).  When the engine decides
    // the terminal owns the right-click (no mouse-reporting mode active, or
    // Shift is held), it calls this callback instead of forwarding to the PTY.
    let term_for_menu = terminal.downgrade();
    let inner_for_menu = Rc::downgrade(inner);
    let app_menu_clone = app_menu.clone();
    terminal.on_context_menu(move |x, y| {
        let Some(term_for_menu) = term_for_menu.upgrade() else {
            return;
        };
        let mut path_to_copy = None;

        if let Some(hyperlink) = term_for_menu.check_hyperlink_at(x, y) {
            let hyper_str = hyperlink.to_string();
            if hyper_str.starts_with("file://") {
                if let Some(after_scheme) = hyper_str.strip_prefix("file://") {
                    if let Some(slash_idx) = after_scheme.find('/') {
                        path_to_copy = Some(after_scheme[slash_idx..].to_string());
                    } else {
                        path_to_copy = Some(hyper_str);
                    }
                } else {
                    path_to_copy = Some(hyper_str);
                }
            } else {
                path_to_copy = Some(hyper_str);
            }
        } else {
            let (match_opt, _tag, _) = term_for_menu.check_match_at(x, y);
            if let Some(matched) = match_opt {
                let matched_str = matched.to_string();

                if matched_str.starts_with("http://") || matched_str.starts_with("https://") {
                    path_to_copy = Some(matched_str);
                } else {
                    let cwd = inner_for_menu
                        .upgrade()
                        .and_then(|i| i.borrow().working_dir.clone());
                    if let Some(resolved) =
                        super::preview::resolve_local_path(&matched_str, cwd.as_deref())
                    {
                        path_to_copy = Some(resolved.to_string_lossy().into_owned());
                    }
                }
            }
        }

        let mut is_maximized = false;
        let mut current = term_for_menu.parent();
        while let Some(parent) = current {
            if parent.widget_name() == "maximized-container" {
                is_maximized = true;
                break;
            }
            current = parent.parent();
        }

        let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        let ctx = AppMenuContext {
            is_maximized,
            path_to_copy,
            has_selection: term_for_menu.has_selection(),
        };

        app_menu_clone.show(rect, ctx);
    });

    terminal.set_allow_hyperlink(true);

    let scroll_ctrl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let term_scroll = terminal.downgrade();
    let cb_scroll = callback.clone();
    scroll_ctrl.connect_scroll(move |scroll_ctrl, _, y| {
        let state = scroll_ctrl.current_event_state();
        let is_ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);

        // If Ctrl is held, we always zoom, even if mouse mode is active in the TUI.
        // This is standard behavior in most terminal emulators to allow zooming out of a TUI.
        if is_ctrl {
            if y < 0.0 {
                cb_scroll(PaneOutput::ZoomIn);
            } else if y > 0.0 {
                cb_scroll(PaneOutput::ZoomOut);
            }
            return gtk::glib::Propagation::Stop;
        }

        // Otherwise, if mouse mode is active, the TUI might want the scroll event.
        if let Some(term) = term_scroll.upgrade()
            && term.is_mouse_mode()
        {
            return gtk::glib::Propagation::Proceed;
        }

        gtk::glib::Propagation::Proceed
    });
    terminal.add_controller(scroll_ctrl);

    let mouse_coords = Rc::new(RefCell::new((0.0, 0.0)));
    let motion_ctrl = gtk::EventControllerMotion::new();
    let mc_clone = mouse_coords.clone();
    motion_ctrl.connect_motion(move |_, x, y| {
        *mc_clone.borrow_mut() = (x, y);
    });
    terminal.add_controller(motion_ctrl);
}

/// Dedicated in-process GType for pane split drag-and-drop so external file or
/// text drags never match the split `DropTarget` or flash `.split-drop-overlay`.
#[derive(Clone, Debug, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "BoxxyPaneDragPayload")]
pub struct PaneDragPayload(pub String);

fn calc_drop_direction(width: f64, height: f64, x: f64, y: f64) -> crate::Direction {
    if width <= 0.0 || height <= 0.0 {
        return crate::Direction::Right;
    }
    let l_dist = x / width;
    let t_dist = y / height;
    let r_dist = 1.0 - l_dist;
    let b_dist = 1.0 - t_dist;
    let min = l_dist.min(t_dist).min(r_dist).min(b_dist);

    if (min - l_dist).abs() < f64::EPSILON {
        crate::Direction::Left
    } else if (min - r_dist).abs() < f64::EPSILON {
        crate::Direction::Right
    } else if (min - t_dist).abs() < f64::EPSILON {
        crate::Direction::Up
    } else {
        crate::Direction::Down
    }
}

fn set_drop_overlay_direction(drop_overlay: &gtk::DrawingArea, dir: Option<crate::Direction>) {
    for cls in ["drop-top", "drop-bottom", "drop-left", "drop-right"] {
        drop_overlay.remove_css_class(cls);
    }
    if let Some(d) = dir {
        let cls = match d {
            crate::Direction::Up => "drop-top",
            crate::Direction::Down => "drop-bottom",
            crate::Direction::Left => "drop-left",
            crate::Direction::Right => "drop-right",
        };
        drop_overlay.add_css_class(cls);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn setup_split_drag_and_drop(
    pane_overlay: &gtk::Overlay,
    terminal: &TerminalWidget,
    drop_overlay: &gtk::DrawingArea,
    drag_lane: &gtk::Box,
    drag_handle: &gtk::Box,
    search_bar: &Rc<SearchBarComponent>,
    claw_popover: &boxxy_claw_widget::TerminalOverlay,
    callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    pane_id: String,
) {
    let is_lane_visible = Rc::new(std::cell::Cell::new(false));
    let is_dragging = Rc::new(std::cell::Cell::new(false));

    let hide_lane = {
        let is_lane_visible = is_lane_visible.clone();
        let drag_lane = drag_lane.downgrade();
        let drag_handle = drag_handle.downgrade();
        move || {
            if is_lane_visible.replace(false) {
                if let Some(lane) = drag_lane.upgrade() {
                    lane.remove_css_class("visible");
                }
                if let Some(handle) = drag_handle.upgrade() {
                    handle.remove_css_class("visible");
                    handle.set_can_target(false);
                }
            }
        }
    };

    // Passive motion observer on the pane overlay.
    // Leaving y < 3.0 untouched when hidden ensures hovering the GtkPaned
    // separator border in horizontal splits never interferes with resizing.
    let hover_motion = gtk::EventControllerMotion::new();
    {
        let is_lane_visible = is_lane_visible.clone();
        let is_dragging = is_dragging.clone();
        let drag_lane_w = drag_lane.downgrade();
        let drag_handle_w = drag_handle.downgrade();
        let search_bar = search_bar.clone();
        let claw_popover = claw_popover.clone();
        let hide_lane_c = hide_lane.clone();

        hover_motion.connect_motion(move |ctrl, _x, y| {
            if is_dragging.get() || claw_popover.is_visible() || search_bar.is_visible() {
                hide_lane_c();
                return;
            }

            let buttons_pressed = ctrl.current_event_state().intersects(
                gdk::ModifierType::BUTTON1_MASK
                    | gdk::ModifierType::BUTTON2_MASK
                    | gdk::ModifierType::BUTTON3_MASK,
            );
            if buttons_pressed && !is_lane_visible.get() {
                return;
            }

            let currently_visible = is_lane_visible.get();
            let in_zone = if currently_visible {
                (0.0..=22.0).contains(&y)
            } else {
                (3.0..=18.0).contains(&y)
            };

            if in_zone && !currently_visible {
                is_lane_visible.set(true);
                if let Some(lane) = drag_lane_w.upgrade() {
                    lane.add_css_class("visible");
                }
                if let Some(handle) = drag_handle_w.upgrade() {
                    handle.add_css_class("visible");
                    handle.set_can_target(true);
                }
            } else if !in_zone && currently_visible {
                hide_lane_c();
            }
        });
    }
    {
        let hide_lane_c = hide_lane.clone();
        hover_motion.connect_leave(move |_| {
            hide_lane_c();
        });
    }
    pane_overlay.add_controller(hover_motion);

    // DragSource is attached strictly to the centered three-dots handle,
    // never to the full horizontal line.
    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gdk::DragAction::MOVE);
    {
        let payload = PaneDragPayload(pane_id.clone());
        drag_source.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&payload.to_value()))
        });
    }
    {
        let is_dragging = is_dragging.clone();
        let hide_lane_c = hide_lane.clone();
        let pane_overlay_w = pane_overlay.downgrade();
        let terminal_w = terminal.downgrade();
        drag_source.connect_drag_begin(move |src, _| {
            is_dragging.set(true);
            hide_lane_c();

            let Some(pane_overlay) = pane_overlay_w.upgrade() else {
                return;
            };
            let width = pane_overlay.width();
            let height = pane_overlay.height();
            if width <= 0 || height <= 0 {
                return;
            }

            let preview_scale = 0.22_f32;
            let paintable = gtk::WidgetPaintable::new(Some(&pane_overlay));
            let snap = gtk::Snapshot::new();
            snap.scale(preview_scale, preview_scale);

            let rect = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            let rounded = gtk::gsk::RoundedRect::from_rect(rect, 28.0);
            snap.push_rounded_clip(&rounded);
            let mut bg = terminal_w
                .upgrade()
                .map(|t| t.background_color())
                .unwrap_or_else(|| gdk::RGBA::new(0.08, 0.08, 0.1, 1.0));
            bg.set_alpha(0.94);
            snap.append_color(&bg, &rect);
            gdk::prelude::PaintableExt::snapshot(&paintable, &snap, width as f64, height as f64);
            snap.pop();

            let hot_x = ((width as f32 * preview_scale) / 2.0) as i32;
            let hot_y = ((height as f32 * preview_scale) / 2.0) as i32;
            if let Some(scaled) = snap.to_paintable(None) {
                src.set_icon(Some(&scaled), hot_x, hot_y);
            } else {
                src.set_icon(Some(&paintable), width / 2, height / 2);
            }
        });
    }
    {
        let is_dragging = is_dragging.clone();
        drag_source.connect_drag_end(move |_, _, _| {
            is_dragging.set(false);
        });
    }
    {
        let is_dragging = is_dragging.clone();
        drag_source.connect_drag_cancel(move |_, _, _| {
            is_dragging.set(false);
            false
        });
    }
    drag_handle.add_controller(drag_source);

    // DropTarget on the pane overlay to accept dragged panes and preview the split half.
    // Using `PaneDragPayload::static_type()` ensures external file/text drags never match
    // at GDK format negotiation time and never flash the split overlay during async preload.
    let drop_target = gtk::DropTarget::new(PaneDragPayload::static_type(), gdk::DragAction::MOVE);
    drop_target.set_preload(true);
    {
        let is_self_dragging = is_dragging;
        drop_target.connect_accept(move |_, drop| {
            !is_self_dragging.get() && drop.formats().contains_type(PaneDragPayload::static_type())
        });
    }
    {
        let drop_overlay_w = drop_overlay.downgrade();
        let target_id = pane_id.clone();
        drop_target.connect_value_notify(move |tgt| {
            let valid = tgt
                .value()
                .and_then(|v| v.get::<PaneDragPayload>().ok())
                .is_some_and(|p| p.0 != target_id);
            if !valid {
                if let Some(ov) = drop_overlay_w.upgrade() {
                    set_drop_overlay_direction(&ov, None);
                }
                tgt.reject();
            }
        });
    }
    {
        let pane_overlay_w = pane_overlay.downgrade();
        let drop_overlay_w = drop_overlay.downgrade();
        let target_id = pane_id.clone();
        drop_target.connect_motion(move |tgt, x, y| {
            if let Some(val) = tgt.value() {
                let valid = val
                    .get::<PaneDragPayload>()
                    .ok()
                    .is_some_and(|p| p.0 != target_id);
                if !valid {
                    if let Some(ov) = drop_overlay_w.upgrade() {
                        set_drop_overlay_direction(&ov, None);
                    }
                    return gdk::DragAction::empty();
                }
            }
            if let (Some(pane_ov), Some(drop_ov)) =
                (pane_overlay_w.upgrade(), drop_overlay_w.upgrade())
            {
                let dir =
                    calc_drop_direction(pane_ov.width() as f64, pane_ov.height() as f64, x, y);
                set_drop_overlay_direction(&drop_ov, Some(dir));
            }
            gdk::DragAction::MOVE
        });
    }
    {
        let drop_overlay_w = drop_overlay.downgrade();
        drop_target.connect_leave(move |_| {
            if let Some(ov) = drop_overlay_w.upgrade() {
                set_drop_overlay_direction(&ov, None);
            }
        });
    }
    {
        let pane_overlay_w = pane_overlay.downgrade();
        let drop_overlay_w = drop_overlay.downgrade();
        let target_id = pane_id;
        drop_target.connect_drop(move |_, value, x, y| {
            if let Some(ov) = drop_overlay_w.upgrade() {
                set_drop_overlay_direction(&ov, None);
            }
            let Ok(PaneDragPayload(source_pane_id)) = value.get::<PaneDragPayload>() else {
                return false;
            };
            if source_pane_id == target_id {
                return false;
            }
            let dir = if let Some(pane_ov) = pane_overlay_w.upgrade() {
                calc_drop_direction(pane_ov.width() as f64, pane_ov.height() as f64, x, y)
            } else {
                crate::Direction::Right
            };
            callback(PaneOutput::MovePaneToSplit {
                source_pane_id,
                target_pane_id: target_id.clone(),
                direction: dir,
            });
            true
        });
    }
    pane_overlay.add_controller(drop_target);
}

#[cfg(test)]
mod tests {
    use super::calc_drop_direction;
    use crate::Direction;

    #[test]
    fn drop_direction_identifies_nearest_edge() {
        let w = 400.0;
        let h = 300.0;

        assert_eq!(calc_drop_direction(w, h, 20.0, 150.0), Direction::Left);
        assert_eq!(calc_drop_direction(w, h, 380.0, 150.0), Direction::Right);
        assert_eq!(calc_drop_direction(w, h, 200.0, 20.0), Direction::Up);
        assert_eq!(calc_drop_direction(w, h, 200.0, 280.0), Direction::Down);
    }

    #[test]
    fn drop_direction_handles_zero_dimensions() {
        assert_eq!(calc_drop_direction(0.0, 0.0, 10.0, 10.0), Direction::Right);
    }
}
