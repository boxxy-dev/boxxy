//! Interactive [`AvatarCropDialog`] modal shell, async loading state machine, and input controllers.

use super::geometry::{CropState, DragMode, Viewport, cursor_for_mode, hit_test};
use super::read_selected_file_bytes;
use super::render::{
    BackdropBuffer, build_backdrop_buffer, create_checkerboard_pattern, draw_crop_canvas,
    update_preview_avatar,
};
use adw::prelude::*;
use boxxy_assets::image::{
    AvatarOutput, AvatarSource, RegionCrop, crop_avatar_source, load_avatar_source,
};
use gtk4 as gtk;
use gtk4::cairo;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Grace period before surfacing a loading spinner so fast (<180ms) local decodes open
/// straight into the interactive cropper without a single-frame spinner flash.
const SPINNER_DELAY: Duration = Duration::from_millis(180);

/// Minimum duration the spinner stays visible *if* a slow load crosses `SPINNER_DELAY`,
/// preventing jarring 10-20ms flashes on borderline loads.
const SPINNER_MIN_HOLD: Duration = Duration::from_millis(250);

type CropResultCb = Box<dyn FnOnce(Result<AvatarOutput, String>)>;
type MountSourceFn = Rc<dyn Fn(AvatarSource, Option<BackdropBuffer>, bool)>;

struct ShellHandle {
    dialog: adw::Dialog,
    closed_flag: Rc<Cell<bool>>,
    mount: MountSourceFn,
}

/// Interactive modal dialog for selecting a square crop region from an [`AvatarSource`].
pub struct AvatarCropDialog;

impl AvatarCropDialog {
    /// Presents the interactive square cropper for an already-loaded [`AvatarSource`], building
    /// its Cairo backdrop off the GTK main thread.
    ///
    /// `on_apply` is invoked when the user clicks **Apply Avatar** (or if encoding fails), and is
    /// dropped without being called if the user cancels or closes the dialog.
    pub fn present<W, F>(parent: &W, title: &str, source: AvatarSource, on_apply: F)
    where
        W: IsA<gtk::Widget>,
        F: FnOnce(Result<AvatarOutput, String>) + 'static,
    {
        Self::present_with_loader(parent, title, on_apply, move |_closed_flag| async move {
            let join_res = boxxy_sys_utils::runtime().spawn_blocking(move || {
                let backdrop = build_backdrop_buffer(&source);
                (source, backdrop)
            });
            join_res.await.map_err(|join_err| {
                log::error!("Avatar backdrop task panicked: {join_err}");
                "Failed to prepare image preview.".to_string()
            })
        });
    }

    pub(super) fn present_from_file<W, F>(parent: &W, title: &str, file: gio::File, on_result: F)
    where
        W: IsA<gtk::Widget>,
        F: FnOnce(Result<AvatarOutput, String>) + 'static,
    {
        Self::present_with_loader(parent, title, on_result, move |closed_flag| async move {
            let bytes = read_selected_file_bytes(&file).await?;
            if closed_flag.get() {
                return Err(String::new());
            }

            let decode_res = boxxy_sys_utils::runtime()
                .spawn_blocking(move || {
                    let source = load_avatar_source(&bytes)?;
                    let backdrop = build_backdrop_buffer(&source);
                    Ok::<_, boxxy_assets::AssetError>((source, backdrop))
                })
                .await;

            match decode_res {
                Ok(Ok(pair)) => Ok(pair),
                Ok(Err(asset_err)) => {
                    log::warn!("Avatar image validation failed: {asset_err}");
                    Err(asset_err.to_string())
                }
                Err(join_err) => {
                    log::error!("Avatar decode task panicked: {join_err}");
                    Err("Failed to decode image.".to_string())
                }
            }
        });
    }

    fn present_with_loader<W, F, L, Fut>(parent: &W, title: &str, on_result: F, loader: L)
    where
        W: IsA<gtk::Widget>,
        F: FnOnce(Result<AvatarOutput, String>) + 'static,
        L: FnOnce(Rc<Cell<bool>>) -> Fut + 'static,
        Fut: Future<Output = Result<(AvatarSource, Option<BackdropBuffer>), String>> + 'static,
    {
        let parent_widget: gtk::Widget = parent.clone().upcast();
        let title_str = title.to_string();
        let on_result_slot: Rc<RefCell<Option<CropResultCb>>> =
            Rc::new(RefCell::new(Some(Box::new(on_result))));
        let shell_slot: Rc<RefCell<Option<ShellHandle>>> = Rc::new(RefCell::new(None));
        let shared_closed = Rc::new(Cell::new(false));
        let load_finished = Rc::new(Cell::new(false));
        let spinner_shown_at: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));

        // Only open the loading spinner dialog if loading exceeds `SPINNER_DELAY` (180ms).
        {
            let parent_timer = parent_widget.clone();
            let title_timer = title_str.clone();
            let on_result_timer = on_result_slot.clone();
            let shell_timer = shell_slot.clone();
            let closed_timer = shared_closed.clone();
            let finished_timer = load_finished.clone();
            let shown_timer = spinner_shown_at.clone();

            glib::spawn_future_local(async move {
                glib::timeout_future(SPINNER_DELAY).await;
                if finished_timer.get() || closed_timer.get() || parent_timer.root().is_none() {
                    return;
                }
                let shell = Self::build_shell(&title_timer, on_result_timer, closed_timer);
                shell.dialog.present(Some(&parent_timer));
                shown_timer.set(Some(Instant::now()));
                *shell_timer.borrow_mut() = Some(shell);
            });
        }

        glib::spawn_future_local(async move {
            let outcome = loader(shared_closed.clone()).await;
            load_finished.set(true);

            let existing_shell = shell_slot.borrow_mut().take();
            match (existing_shell, outcome) {
                // Fast path (< 180ms): open directly in the ready state with zero spinner flash
                (None, Ok((source, backdrop))) => {
                    if parent_widget.root().is_none() {
                        return;
                    }
                    let shell = Self::build_shell(&title_str, on_result_slot, shared_closed);
                    (shell.mount)(source, backdrop, false);
                    shell.dialog.present(Some(&parent_widget));
                }
                // Fast path error (< 180ms): report error directly without flashing an empty dialog
                (None, Err(msg)) => {
                    if !msg.is_empty()
                        && let Some(cb) = on_result_slot.borrow_mut().take()
                    {
                        cb(Err(msg));
                    }
                }
                // Slow path (>= 180ms): enforce minimum spinner hold before crossfading to ready
                (Some(shell), Ok((source, backdrop))) => {
                    if shell.closed_flag.get() {
                        return;
                    }
                    if let Some(shown_at) = spinner_shown_at.get()
                        && let Some(remaining) = SPINNER_MIN_HOLD.checked_sub(shown_at.elapsed())
                    {
                        glib::timeout_future(remaining).await;
                    }
                    if !shell.closed_flag.get() {
                        (shell.mount)(source, backdrop, true);
                    }
                }
                // Slow path error (>= 180ms): close the loading dialog and surface the error
                (Some(shell), Err(msg)) => {
                    if !shell.closed_flag.get() {
                        shell.closed_flag.set(true);
                        shell.dialog.close();
                        if !msg.is_empty()
                            && let Some(cb) = on_result_slot.borrow_mut().take()
                        {
                            cb(Err(msg));
                        }
                    }
                }
            }
        });
    }

    #[allow(clippy::too_many_lines)]
    fn build_shell(
        title: &str,
        on_apply: Rc<RefCell<Option<CropResultCb>>>,
        closed_flag: Rc<Cell<bool>>,
    ) -> ShellHandle {
        let builder = gtk::Builder::from_resource(
            "/dev/boxxy/BoxxyTerminal/ui/widgets/avatar_crop_dialog.ui",
        );

        let dialog: adw::Dialog = builder
            .object("crop_dialog")
            .expect("crop_dialog must exist in avatar_crop_dialog.ui");
        let window_title: adw::WindowTitle = builder
            .object("window_title")
            .expect("window_title must exist");
        let canvas_stack: gtk::Stack = builder
            .object("canvas_stack")
            .expect("canvas_stack must exist");
        let loading_label: gtk::Label = builder
            .object("loading_label")
            .expect("loading_label must exist");
        let controls_box: gtk::Box = builder
            .object("controls_box")
            .expect("controls_box must exist");
        let canvas: gtk::DrawingArea = builder
            .object("crop_canvas")
            .expect("crop_canvas must exist");
        let preview_avatar: adw::Avatar = builder
            .object("preview_avatar")
            .expect("preview_avatar must exist");
        let crop_info_label: gtk::Label = builder
            .object("crop_info_label")
            .expect("crop_info_label must exist");
        let size_scale: gtk::Scale = builder.object("size_scale").expect("size_scale must exist");
        let reset_btn: gtk::Button = builder.object("reset_btn").expect("reset_btn must exist");
        let cancel_btn: gtk::Button = builder.object("cancel_btn").expect("cancel_btn must exist");
        let apply_btn: gtk::Button = builder.object("apply_btn").expect("apply_btn must exist");

        dialog.set_title(title);
        window_title.set_title(title);
        dialog.set_default_widget(Some(&apply_btn));
        canvas.set_focusable(true);

        // Start in loading state until `mount` populates the decoded source
        canvas_stack.set_visible_child_name("loading");
        controls_box.set_sensitive(false);
        apply_btn.set_sensitive(false);

        let closed_on_dialog = closed_flag.clone();
        dialog.connect_closed(move |_| {
            closed_on_dialog.set(true);
        });

        let state = Rc::new(RefCell::new(CropState::new(256, 256)));
        let loaded_source: Rc<RefCell<Option<AvatarSource>>> = Rc::new(RefCell::new(None));
        let loaded_surface: Rc<RefCell<Option<(cairo::ImageSurface, f64, f64)>>> =
            Rc::new(RefCell::new(None));
        let checker_pattern = create_checkerboard_pattern();
        let syncing_scale = Rc::new(Cell::new(false));
        let last_cursor: Rc<Cell<&'static str>> = Rc::new(Cell::new(""));
        let last_preview_key: Rc<Cell<Option<(RegionCrop, i32)>>> = Rc::new(Cell::new(None));

        // Refreshes canvas on every sub-pixel motion while deduplicating integer `RegionCrop`
        // preview texture uploads and label allocations during 120 Hz drags.
        let update_ui = {
            let canvas = canvas.clone();
            let size_scale = size_scale.clone();
            let syncing_scale = syncing_scale.clone();
            let state = state.clone();
            let source_slot = loaded_source.clone();
            let last_preview_key = last_preview_key.clone();

            Rc::new(move || {
                let Some(src) = source_slot.borrow().clone() else {
                    return;
                };
                let region = {
                    let st = state.borrow();
                    syncing_scale.set(true);
                    size_scale.set_value(st.crop_size);
                    syncing_scale.set(false);
                    st.to_region_crop()
                };

                let scale_factor = preview_avatar.scale_factor();
                let preview_key = (region, scale_factor);
                if last_preview_key.get() != Some(preview_key) {
                    last_preview_key.set(Some(preview_key));
                    crop_info_label.set_label(&format!("{} × {} px", region.width, region.height));
                    update_preview_avatar(&preview_avatar, &src, region);
                }
                canvas.queue_draw();
            })
        };

        let mount = {
            let state = state.clone();
            let loaded_source = loaded_source.clone();
            let loaded_surface = loaded_surface.clone();
            let size_scale = size_scale.clone();
            let controls_box = controls_box.clone();
            let apply_btn = apply_btn.clone();
            let canvas_stack = canvas_stack.clone();
            let canvas = canvas.clone();
            let update_ui = update_ui.clone();

            Rc::new(
                move |source: AvatarSource,
                      backdrop: Option<BackdropBuffer>,
                      animate_transition: bool| {
                    let new_state = CropState::new(source.width(), source.height());
                    size_scale.set_range(new_state.min_size, new_state.max_size);
                    size_scale.set_increments(1.0, 16.0);
                    size_scale.set_value(new_state.crop_size);
                    size_scale.set_sensitive((new_state.max_size - new_state.min_size).abs() > 0.5);
                    *state.borrow_mut() = new_state;

                    let surface_opt = backdrop.and_then(|b| {
                        let surf = cairo::ImageSurface::create_for_data(
                            b.data,
                            cairo::Format::ARgb32,
                            b.width,
                            b.height,
                            b.stride,
                        )
                        .ok()?;
                        Some((surf, f64::from(b.width), f64::from(b.height)))
                    });
                    *loaded_surface.borrow_mut() = surface_opt;
                    *loaded_source.borrow_mut() = Some(source);
                    last_preview_key.set(None);

                    update_ui();
                    controls_box.set_sensitive(true);
                    apply_btn.set_sensitive(true);

                    if animate_transition {
                        canvas_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
                        canvas_stack.set_visible_child_name("ready");
                    } else {
                        canvas_stack.set_transition_type(gtk::StackTransitionType::None);
                        canvas_stack.set_visible_child_name("ready");
                        canvas_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
                    }
                    let _ = canvas.grab_focus();
                },
            )
        };

        // Drawing function for the interactive crop canvas
        let state_draw = state.clone();
        let surface_draw = loaded_surface;
        canvas.set_draw_func(move |_area, cr, width, height| {
            let st = state_draw.borrow();
            let surface_guard = surface_draw.borrow();
            draw_crop_canvas(
                cr,
                width,
                height,
                &st,
                checker_pattern.as_ref(),
                surface_guard.as_ref(),
            );
        });

        // Motion controller for dynamic resize/move cursors (deduplicated per cursor transition)
        let motion = gtk::EventControllerMotion::new();
        let state_motion = state.clone();
        let canvas_motion = canvas.clone();
        let cursor_motion = last_cursor.clone();
        motion.connect_motion(move |_, x, y| {
            let mode = {
                let st = state_motion.borrow();
                if st.drag_mode.is_some() {
                    return;
                }
                let vp = Viewport::compute(
                    f64::from(canvas_motion.width()),
                    f64::from(canvas_motion.height()),
                    st.img_w,
                    st.img_h,
                );
                hit_test(&st, &vp, x, y)
            };
            set_cursor_if_changed(&canvas_motion, &cursor_motion, cursor_for_mode(mode, false));
        });
        canvas.add_controller(motion);

        // Primary-button drag gesture for moving and resizing the square
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let state_drag_begin = state.clone();
        let canvas_drag_begin = canvas.clone();
        let cursor_drag_begin = last_cursor.clone();
        let update_drag_begin = update_ui.clone();
        drag.connect_drag_begin(move |_, start_x, start_y| {
            let _ = canvas_drag_begin.grab_focus();
            let mode = {
                let mut st = state_drag_begin.borrow_mut();
                let vp = Viewport::compute(
                    f64::from(canvas_drag_begin.width()),
                    f64::from(canvas_drag_begin.height()),
                    st.img_w,
                    st.img_h,
                );
                let mut detected = hit_test(&st, &vp, start_x, start_y);
                // Clicking outside the square re-centers the square on that point and begins Move
                if detected.is_none() {
                    let img_x = (start_x - vp.offset_x) / vp.scale;
                    let img_y = (start_y - vp.offset_y) / vp.scale;
                    st.recenter_at(img_x, img_y);
                    detected = Some(DragMode::Move);
                }
                st.begin_drag(detected, start_x, start_y);
                detected
            };

            set_cursor_if_changed(
                &canvas_drag_begin,
                &cursor_drag_begin,
                cursor_for_mode(mode, true),
            );
            update_drag_begin();
        });

        let state_drag_update = state.clone();
        let canvas_drag_update = canvas.clone();
        let update_drag = update_ui.clone();
        drag.connect_drag_update(move |_, offset_x, offset_y| {
            let changed = {
                let mut st = state_drag_update.borrow_mut();
                let Some(mode) = st.drag_mode else {
                    return;
                };
                let vp = Viewport::compute(
                    f64::from(canvas_drag_update.width()),
                    f64::from(canvas_drag_update.height()),
                    st.img_w,
                    st.img_h,
                );
                let delta_img_x = offset_x / vp.scale;
                let delta_img_y = offset_y / vp.scale;
                st.apply_drag(mode, delta_img_x, delta_img_y)
            };
            if changed {
                update_drag();
            }
        });

        let state_drag_end = state.clone();
        let canvas_drag_end = canvas.clone();
        let cursor_drag_end = last_cursor;
        drag.connect_drag_end(move |_, offset_x, offset_y| {
            let mode = {
                let mut st = state_drag_end.borrow_mut();
                let ex = st.drag_origin_canvas_x + offset_x;
                let ey = st.drag_origin_canvas_y + offset_y;
                st.drag_mode = None;
                let vp = Viewport::compute(
                    f64::from(canvas_drag_end.width()),
                    f64::from(canvas_drag_end.height()),
                    st.img_w,
                    st.img_h,
                );
                hit_test(&st, &vp, ex, ey)
            };
            set_cursor_if_changed(
                &canvas_drag_end,
                &cursor_drag_end,
                cursor_for_mode(mode, false),
            );
            canvas_drag_end.queue_draw();
        });
        canvas.add_controller(drag);

        // Scroll controller supporting both discrete mouse wheels and smooth Wayland touchpads
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let state_scroll = state.clone();
        let update_scroll = update_ui.clone();
        scroll.connect_scroll(move |ctrl, _dx, dy| {
            let ticks = if ctrl.unit() == gdk::ScrollUnit::Surface {
                dy / 60.0
            } else {
                dy
            };
            let changed = {
                let mut st = state_scroll.borrow_mut();
                let step = (st.max_size - st.min_size).max(32.0) * 0.05;
                let new_size = f64::mul_add(ticks, -step, st.crop_size);
                st.set_size_around_center(new_size)
            };
            if changed {
                update_scroll();
            }
            glib::Propagation::Stop
        });
        canvas.add_controller(scroll);

        // Keyboard controller for pixel-accurate nudging and zoom shortcuts
        let key_ctrl = gtk::EventControllerKey::new();
        let state_key = state.clone();
        let update_key = update_ui.clone();
        key_ctrl.connect_key_pressed(move |_, keyval, _keycode, modifiers| {
            let step = if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                10.0
            } else {
                1.0
            };
            let changed = {
                let mut st = state_key.borrow_mut();
                match keyval {
                    gdk::Key::Left => st.nudge(-step, 0.0),
                    gdk::Key::Right => st.nudge(step, 0.0),
                    gdk::Key::Up => st.nudge(0.0, -step),
                    gdk::Key::Down => st.nudge(0.0, step),
                    gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => {
                        let s = f64::mul_add(step, 4.0, st.crop_size);
                        st.set_size_around_center(s)
                    }
                    gdk::Key::minus | gdk::Key::KP_Subtract => {
                        let s = f64::mul_add(step, -4.0, st.crop_size);
                        st.set_size_around_center(s)
                    }
                    gdk::Key::_0 | gdk::Key::KP_0 | gdk::Key::Home => st.reset_centered(),
                    _ => return glib::Propagation::Proceed,
                }
            };
            if changed {
                update_key();
            }
            glib::Propagation::Stop
        });
        canvas.add_controller(key_ctrl);

        // Scale slider value changed
        let state_scale = state.clone();
        let syncing_scale_cb = syncing_scale;
        let update_scale = update_ui.clone();
        size_scale.connect_value_changed(move |scale| {
            if syncing_scale_cb.get() {
                return;
            }
            let changed = {
                let mut st = state_scale.borrow_mut();
                st.set_size_around_center(scale.value())
            };
            if changed {
                update_scale();
            }
        });

        // Reset / Center button
        let state_reset = state.clone();
        let update_reset = update_ui;
        reset_btn.connect_clicked(move |_| {
            if state_reset.borrow_mut().reset_centered() {
                update_reset();
            }
        });

        // Cancel button
        let dialog_cancel = dialog.clone();
        let closed_cancel = closed_flag.clone();
        cancel_btn.connect_clicked(move |_| {
            closed_cancel.set(true);
            dialog_cancel.close();
        });

        // Apply button (`AvatarSource` is Arc-backed, so `.clone()` is O(1))
        let dialog_apply = dialog.clone();
        let closed_apply = closed_flag.clone();
        apply_btn.connect_clicked(move |btn| {
            let Some(source_for_crop) = loaded_source.borrow().clone() else {
                return;
            };
            let Some(cb) = on_apply.borrow_mut().take() else {
                return;
            };
            btn.set_sensitive(false);
            controls_box.set_sensitive(false);

            let apply_done = Rc::new(Cell::new(false));
            let apply_spinner_at: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));

            // Only swap to the "Applying avatar…" spinner if encoding exceeds `SPINNER_DELAY`
            {
                let done_timer = apply_done.clone();
                let closed_timer = closed_apply.clone();
                let shown_timer = apply_spinner_at.clone();
                let label_timer = loading_label.clone();
                let stack_timer = canvas_stack.clone();
                glib::spawn_future_local(async move {
                    glib::timeout_future(SPINNER_DELAY).await;
                    if !done_timer.get() && !closed_timer.get() {
                        label_timer.set_label("Applying avatar…");
                        stack_timer.set_visible_child_name("loading");
                        shown_timer.set(Some(Instant::now()));
                    }
                });
            }

            let region = state.borrow().to_region_crop();
            let dialog_apply = dialog_apply.clone();
            let closed_apply = closed_apply.clone();

            glib::spawn_future_local(async move {
                let res = boxxy_sys_utils::runtime()
                    .spawn_blocking(move || crop_avatar_source(&source_for_crop, region))
                    .await;

                apply_done.set(true);
                if closed_apply.get() {
                    return;
                }

                if let Some(shown_at) = apply_spinner_at.get()
                    && let Some(remaining) = SPINNER_MIN_HOLD.checked_sub(shown_at.elapsed())
                {
                    glib::timeout_future(remaining).await;
                }

                // Do not invoke `cb` if the user pressed Cancel or Escape while encoding ran.
                if closed_apply.get() {
                    return;
                }

                closed_apply.set(true);
                dialog_apply.close();

                match res {
                    Ok(Ok(output)) => cb(Ok(output)),
                    Ok(Err(e)) => cb(Err(e.to_string())),
                    Err(e) => cb(Err(format!("Failed to process avatar: {e}"))),
                }
            });
        });

        ShellHandle {
            dialog,
            closed_flag,
            mount,
        }
    }
}

fn set_cursor_if_changed(widget: &gtk::DrawingArea, last: &Cell<&'static str>, next: &'static str) {
    if last.get() != next {
        last.set(next);
        widget.set_cursor_from_name(Some(next));
    }
}
