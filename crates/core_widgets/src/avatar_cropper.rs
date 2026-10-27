use adw::prelude::*;
use boxxy_assets::image::{
    AvatarOutput, AvatarSource, MIN_AVATAR_DIMENSION, RegionCrop, crop_avatar_source,
    load_avatar_source,
};
use gtk4 as gtk;
use gtk4:: cairo;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// A clickable circular avatar widget with a bottom-right pencil-edit badge and an
/// optional top-right hover `×` reset/remove badge, both protruding ~35% outside
/// the avatar circle so the image itself remains unobstructed.
///
/// Clicking the avatar opens a file chooser followed by the interactive square
/// crop/resize dialog (`AvatarCropDialog`), returning the processed 256×256 PNG bytes.
#[derive(Clone)]
pub struct EditableAvatar {
    container: gtk::Overlay,
    button: gtk::Button,
    edit_btn: gtk::Button,
    remove_btn: gtk::Button,
    avatar: adw::Avatar,
}

impl EditableAvatar {
    #[must_use]
    pub fn new(size: i32, text: Option<&str>, show_initials: bool) -> Self {
        let avatar = adw::Avatar::new(size, text, show_initials);

        let button = gtk::Button::new();
        button.add_css_class("editable-avatar-btn");
        button.set_valign(gtk::Align::Center);
        button.set_halign(gtk::Align::Center);
        button.set_focus_on_click(false);
        button.set_tooltip_text(Some("Click to change avatar"));
        button.set_cursor_from_name(Some("pointer"));
        button.set_child(Some(&avatar));

        let container = gtk::Overlay::new();
        container.add_css_class("editable-avatar-container");
        container.set_valign(gtk::Align::Center);
        container.set_halign(gtk::Align::Center);
        container.set_child(Some(&button));

        // Bottom-right pencil badge button (protrudes ~35% outside the avatar circle)
        let edit_btn = gtk::Button::new();
        edit_btn.add_css_class("editable-avatar-badge");
        edit_btn.set_halign(gtk::Align::End);
        edit_btn.set_valign(gtk::Align::End);
        edit_btn.set_focus_on_click(false);
        edit_btn.set_tooltip_text(Some("Click to change avatar"));
        edit_btn.set_cursor_from_name(Some("pointer"));

        let pencil_icon = gtk::Image::from_icon_name("boxxy-document-edit-symbolic");
        pencil_icon.set_pixel_size(13);
        pencil_icon.set_halign(gtk::Align::Center);
        pencil_icon.set_valign(gtk::Align::Center);
        edit_btn.set_child(Some(&pencil_icon));

        container.add_overlay(&edit_btn);

        // Top-right `×` remove/reset badge button (always displayed when `removable` is enabled)
        let remove_btn = gtk::Button::new();
        remove_btn.add_css_class("editable-avatar-remove-badge");
        remove_btn.set_halign(gtk::Align::End);
        remove_btn.set_valign(gtk::Align::Start);
        remove_btn.set_focus_on_click(false);
        remove_btn.set_tooltip_text(Some("Reset avatar"));
        remove_btn.set_cursor_from_name(Some("pointer"));
        remove_btn.set_visible(false);

        let remove_icon = gtk::Image::from_icon_name("boxxy-cross-small-symbolic");
        remove_icon.set_pixel_size(14);
        remove_icon.set_halign(gtk::Align::Center);
        remove_icon.set_valign(gtk::Align::Center);
        remove_btn.set_child(Some(&remove_icon));

        container.add_overlay(&remove_btn);

        Self {
            container,
            button,
            edit_btn,
            remove_btn,
            avatar,
        }
    }

    /// Underlying `adw::Avatar` for setting custom textures or fallback initials.
    #[must_use]
    pub const fn avatar(&self) -> &adw::Avatar {
        &self.avatar
    }

    /// Top-level widget to add to rows or containers.
    #[must_use]
    pub const fn widget(&self) -> &gtk::Overlay {
        &self.container
    }

    /// Controls whether the top-right `×` reset/remove badge is displayed.
    pub fn set_removable(&self, removable: bool) {
        self.remove_btn.set_visible(removable);
    }

    /// Customises the tooltip on the top-right `×` reset/remove badge.
    pub fn set_remove_tooltip(&self, tooltip: &str) {
        self.remove_btn.set_tooltip_text(Some(tooltip));
    }

    /// Connects the click handler (on both the avatar and the pencil badge) to open the
    /// file chooser and square crop dialog.
    pub fn connect_avatar_cropped<F>(&self, dialog_title: &str, on_cropped: F)
    where
        F: Fn(Result<AvatarOutput, String>) + 'static,
    {
        let title = dialog_title.to_string();
        let on_cropped = Rc::new(on_cropped);

        let title_main = title.clone();
        let cb_main = on_cropped.clone();
        self.button.connect_clicked(move |btn| {
            let cb = cb_main.clone();
            prompt_and_crop_avatar(btn, &title_main, move |res| {
                cb(res);
            });
        });

        let title_edit = title;
        let cb_edit = on_cropped;
        self.edit_btn.connect_clicked(move |btn| {
            let cb = cb_edit.clone();
            prompt_and_crop_avatar(btn, &title_edit, move |res| {
                cb(res);
            });
        });
    }

    /// Connects the click handler for the top-right `×` reset/remove badge.
    pub fn connect_avatar_removed<F>(&self, on_removed: F)
    where
        F: Fn() + 'static,
    {
        self.remove_btn.connect_clicked(move |_| {
            on_removed();
        });
    }
}

/// Opens a file chooser for PNG/JPEG/WebP images, validates & decodes the image on a
/// background thread, and presents the interactive `AvatarCropDialog`.
pub fn prompt_and_crop_avatar<W, F>(parent: &W, dialog_title: &str, on_result: F)
where
    W: IsA<gtk::Widget>,
    F: FnOnce(Result<AvatarOutput, String>) + 'static,
{
    let window = parent.root().and_downcast::<gtk::Window>();
    let dialog = gtk::FileDialog::new();
    dialog.set_title(dialog_title);

    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Images (PNG, JPEG, WebP)"));
    filter.add_mime_type("image/png");
    filter.add_mime_type("image/jpeg");
    filter.add_mime_type("image/webp");
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));

    let parent_widget: gtk::Widget = parent.clone().upcast();
    let title = dialog_title.to_string();
    let on_result = Rc::new(RefCell::new(Some(on_result)));

    dialog.open(
        window.as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(file) = result else {
                // User cancelled the file chooser
                return;
            };
            let Some(path) = file.path() else {
                return;
            };

            let parent_widget = parent_widget.clone();
            let title = title.clone();
            let on_result = on_result.clone();

            glib::spawn_future_local(async move {
                let bytes = match tokio::fs::read(&path).await {
                    Ok(b) => b,
                    Err(e) => {
                        log::error!("Failed to read selected avatar image: {e}");
                        if let Some(cb) = on_result.borrow_mut().take() {
                            cb(Err("Failed to read image file.".to_string()));
                        }
                        return;
                    }
                };

                let decode_res =
                    tokio::task::spawn_blocking(move || load_avatar_source(&bytes)).await;

                match decode_res {
                    Ok(Ok(source)) => {
                        if let Some(cb) = on_result.borrow_mut().take() {
                            AvatarCropDialog::present(&parent_widget, &title, source, cb);
                        }
                    }
                    Ok(Err(asset_err)) => {
                        log::warn!("Avatar image validation failed: {asset_err}");
                        if let Some(cb) = on_result.borrow_mut().take() {
                            cb(Err(asset_err.to_string()));
                        }
                    }
                    Err(join_err) => {
                        log::error!("Avatar decode task panicked: {join_err}");
                        if let Some(cb) = on_result.borrow_mut().take() {
                            cb(Err("Failed to decode image.".to_string()));
                        }
                    }
                }
            });
        },
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    ResizeNW,
    ResizeNE,
    ResizeSW,
    ResizeSE,
    ResizeN,
    ResizeS,
    ResizeE,
    ResizeW,
}

#[derive(Debug, Clone)]
struct CropState {
    img_w: f64,
    img_h: f64,
    min_size: f64,
    max_size: f64,
    crop_x: f64,
    crop_y: f64,
    crop_size: f64,
    drag_mode: Option<DragMode>,
    drag_start_crop_x: f64,
    drag_start_crop_y: f64,
    drag_start_crop_size: f64,
    drag_origin_canvas_x: f64,
    drag_origin_canvas_y: f64,
}

impl CropState {
    fn new(width: u32, height: u32) -> Self {
        let img_w = f64::from(width.max(1));
        let img_h = f64::from(height.max(1));
        let max_size = img_w.min(img_h);
        let min_size = f64::from(MIN_AVATAR_DIMENSION).max(24.0).min(max_size);

        let mut state = Self {
            img_w,
            img_h,
            min_size,
            max_size,
            crop_x: 0.0,
            crop_y: 0.0,
            crop_size: max_size,
            drag_mode: None,
            drag_start_crop_x: 0.0,
            drag_start_crop_y: 0.0,
            drag_start_crop_size: max_size,
            drag_origin_canvas_x: 0.0,
            drag_origin_canvas_y: 0.0,
        };
        state.reset_centered();
        state
    }

    fn reset_centered(&mut self) {
        self.crop_size = self.max_size;
        self.crop_x = (self.img_w - self.crop_size) * 0.5;
        self.crop_y = (self.img_h - self.crop_size) * 0.5;
    }

    fn set_size_around_center(&mut self, new_size: f64) {
        let clamped_size = new_size.clamp(self.min_size, self.max_size);
        let center_x = self.crop_x + self.crop_size * 0.5;
        let center_y = self.crop_y + self.crop_size * 0.5;

        self.crop_size = clamped_size;
        self.crop_x = (center_x - clamped_size * 0.5).clamp(0.0, self.img_w - clamped_size);
        self.crop_y = (center_y - clamped_size * 0.5).clamp(0.0, self.img_h - clamped_size);
    }

    fn apply_drag(&mut self, mode: DragMode, delta_img_x: f64, delta_img_y: f64) {
        let sx = self.drag_start_crop_x;
        let sy = self.drag_start_crop_y;
        let ss = self.drag_start_crop_size;

        match mode {
            DragMode::Move => {
                self.crop_x = (sx + delta_img_x).clamp(0.0, self.img_w - ss);
                self.crop_y = (sy + delta_img_y).clamp(0.0, self.img_h - ss);
            }
            DragMode::ResizeSE => {
                let anchor_x = sx;
                let anchor_y = sy;
                let px = sx + ss + delta_img_x;
                let py = sy + ss + delta_img_y;
                let raw = (px - anchor_x).max(py - anchor_y);
                let max_s = (self.img_w - anchor_x).min(self.img_h - anchor_y);
                self.crop_size = raw.clamp(self.min_size, max_s);
                self.crop_x = anchor_x;
                self.crop_y = anchor_y;
            }
            DragMode::ResizeNW => {
                let anchor_x = sx + ss;
                let anchor_y = sy + ss;
                let px = sx + delta_img_x;
                let py = sy + delta_img_y;
                let raw = (anchor_x - px).max(anchor_y - py);
                let max_s = anchor_x.min(anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                self.crop_size = new_size;
                self.crop_x = anchor_x - new_size;
                self.crop_y = anchor_y - new_size;
            }
            DragMode::ResizeNE => {
                let anchor_x = sx;
                let anchor_y = sy + ss;
                let px = sx + ss + delta_img_x;
                let py = sy + delta_img_y;
                let raw = (px - anchor_x).max(anchor_y - py);
                let max_s = (self.img_w - anchor_x).min(anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                self.crop_size = new_size;
                self.crop_x = anchor_x;
                self.crop_y = anchor_y - new_size;
            }
            DragMode::ResizeSW => {
                let anchor_x = sx + ss;
                let anchor_y = sy;
                let px = sx + delta_img_x;
                let py = sy + ss + delta_img_y;
                let raw = (anchor_x - px).max(py - anchor_y);
                let max_s = anchor_x.min(self.img_h - anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                self.crop_size = new_size;
                self.crop_x = anchor_x - new_size;
                self.crop_y = anchor_y;
            }
            DragMode::ResizeE => {
                let anchor_x = sx;
                let center_y = sy + ss * 0.5;
                let raw = ss + delta_img_x;
                let max_s = (self.img_w - anchor_x).min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                self.crop_size = new_size;
                self.crop_x = anchor_x;
                self.crop_y = (center_y - new_size * 0.5).clamp(0.0, self.img_h - new_size);
            }
            DragMode::ResizeW => {
                let anchor_x = sx + ss;
                let center_y = sy + ss * 0.5;
                let raw = ss - delta_img_x;
                let max_s = anchor_x.min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                self.crop_size = new_size;
                self.crop_x = anchor_x - new_size;
                self.crop_y = (center_y - new_size * 0.5).clamp(0.0, self.img_h - new_size);
            }
            DragMode::ResizeS => {
                let anchor_y = sy;
                let center_x = sx + ss * 0.5;
                let raw = ss + delta_img_y;
                let max_s = (self.img_h - anchor_y).min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                self.crop_size = new_size;
                self.crop_y = anchor_y;
                self.crop_x = (center_x - new_size * 0.5).clamp(0.0, self.img_w - new_size);
            }
            DragMode::ResizeN => {
                let anchor_y = sy + ss;
                let center_x = sx + ss * 0.5;
                let raw = ss - delta_img_y;
                let max_s = anchor_y.min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                self.crop_size = new_size;
                self.crop_y = anchor_y - new_size;
                self.crop_x = (center_x - new_size * 0.5).clamp(0.0, self.img_w - new_size);
            }
        }
    }

    fn to_region_crop(&self) -> RegionCrop {
        let size = (self.crop_size.round() as u32).max(1);
        let x = (self.crop_x.round() as u32).min((self.img_w as u32).saturating_sub(1));
        let y = (self.crop_y.round() as u32).min((self.img_h as u32).saturating_sub(1));
        RegionCrop {
            x,
            y,
            width: size,
            height: size,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Viewport {
    offset_x: f64,
    offset_y: f64,
    draw_w: f64,
    draw_h: f64,
    scale: f64,
}

impl Viewport {
    fn compute(canvas_w: f64, canvas_h: f64, img_w: f64, img_h: f64) -> Self {
        let pad = 22.0;
        let avail_w = (canvas_w - pad * 2.0).max(32.0);
        let avail_h = (canvas_h - pad * 2.0).max(32.0);
        let scale = (avail_w / img_w).min(avail_h / img_h);
        let draw_w = img_w * scale;
        let draw_h = img_h * scale;
        let offset_x = (canvas_w - draw_w) * 0.5;
        let offset_y = (canvas_h - draw_h) * 0.5;
        Self {
            offset_x,
            offset_y,
            draw_w,
            draw_h,
            scale,
        }
    }
}

/// Interactive modal dialog for selecting a square crop region from an `AvatarSource`.
pub struct AvatarCropDialog;

impl AvatarCropDialog {
    pub fn present<W, F>(parent: &W, title: &str, source: AvatarSource, on_apply: F)
    where
        W: IsA<gtk::Widget>,
        F: FnOnce(Result<AvatarOutput, String>) + 'static,
    {
        let builder =
            gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/widgets/avatar_crop_dialog.ui");

        let dialog: adw::Dialog = builder
            .object("crop_dialog")
            .expect("crop_dialog must exist in avatar_crop_dialog.ui");
        let window_title: adw::WindowTitle = builder
            .object("window_title")
            .expect("window_title must exist");
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

        let state = Rc::new(RefCell::new(CropState::new(source.width, source.height)));
        let source_rc = Rc::new(source);
        let surface_rc = Rc::new(create_cairo_surface(&source_rc));
        let syncing_scale = Rc::new(Cell::new(false));

        // Configure scale bounds
        {
            let st = state.borrow();
            size_scale.set_range(st.min_size, st.max_size);
            size_scale.set_value(st.crop_size);
            size_scale.set_sensitive((st.max_size - st.min_size).abs() > 0.5);
        }

        // Helper to refresh canvas, info label, slider, and live avatar preview
        let update_ui = {
            let canvas = canvas.clone();
            let preview_avatar = preview_avatar.clone();
            let crop_info_label = crop_info_label.clone();
            let size_scale = size_scale.clone();
            let syncing_scale = syncing_scale.clone();
            let state = state.clone();
            let source_rc = source_rc.clone();

            Rc::new(move || {
                let region = {
                    let st = state.borrow();
                    syncing_scale.set(true);
                    size_scale.set_value(st.crop_size);
                    syncing_scale.set(false);
                    st.to_region_crop()
                };

                crop_info_label.set_label(&format!("{} × {} px", region.width, region.height));
                update_preview_avatar(&preview_avatar, &source_rc, region);
                canvas.queue_draw();
            })
        };

        // Initial preview render
        update_ui();

        // Drawing function for the interactive crop canvas
        let state_draw = state.clone();
        let surface_draw = surface_rc;
        canvas.set_draw_func(move |_area, cr, width, height| {
            let cw = f64::from(width);
            let ch = f64::from(height);
            let st = state_draw.borrow();
            let vp = Viewport::compute(cw, ch, st.img_w, st.img_h);

            // 1. Subtle checkerboard background inside the image bounds (for transparent PNGs)
            draw_checkerboard(cr, vp.offset_x, vp.offset_y, vp.draw_w, vp.draw_h);

            // 2. Draw scaled source image
            if let Some(surface) = surface_draw.as_ref() {
                let _ = cr.save();
                cr.translate(vp.offset_x, vp.offset_y);
                cr.scale(vp.scale, vp.scale);
                let _ = cr.set_source_surface(surface, 0.0, 0.0);
                let _ = cr.paint();
                let _ = cr.restore();
            }

            // Compute crop square in canvas coordinates
            let sx = vp.offset_x + st.crop_x * vp.scale;
            let sy = vp.offset_y + st.crop_y * vp.scale;
            let sw = st.crop_size * vp.scale;

            // 3. Darkened scrim outside the selected square (within image bounds)
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.62);
            // Top strip
            cr.rectangle(vp.offset_x, vp.offset_y, vp.draw_w, (sy - vp.offset_y).max(0.0));
            // Bottom strip
            let bottom_y = sy + sw;
            cr.rectangle(
                vp.offset_x,
                bottom_y,
                vp.draw_w,
                (vp.offset_y + vp.draw_h - bottom_y).max(0.0),
            );
            // Left strip
            cr.rectangle(vp.offset_x, sy, (sx - vp.offset_x).max(0.0), sw);
            // Right strip
            let right_x = sx + sw;
            cr.rectangle(
                right_x,
                sy,
                (vp.offset_x + vp.draw_w - right_x).max(0.0),
                sw,
            );
            let _ = cr.fill();

            // 4. Subtle circular avatar guide inscribed inside the square
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.32);
            cr.set_line_width(1.0);
            cr.arc(
                sx + sw * 0.5,
                sy + sw * 0.5,
                (sw * 0.5 - 1.0).max(1.0),
                0.0,
                std::f64::consts::TAU,
            );
            let _ = cr.stroke();

            // 5. Rule-of-thirds grid inside the crop square
            let grid_alpha = if st.drag_mode.is_some() { 0.38 } else { 0.18 };
            cr.set_source_rgba(1.0, 1.0, 1.0, grid_alpha);
            cr.set_line_width(1.0);
            for i in 1..=2 {
                let offset = sw * (f64::from(i) / 3.0);
                // Vertical
                cr.move_to(sx + offset, sy);
                cr.line_to(sx + offset, sy + sw);
                // Horizontal
                cr.move_to(sx, sy + offset);
                cr.line_to(sx + sw, sy + offset);
            }
            let _ = cr.stroke();

            // 6. Outer shadow + crisp white border around the crop square
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
            cr.set_line_width(3.0);
            cr.rectangle(sx, sy, sw, sw);
            let _ = cr.stroke();

            cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
            cr.set_line_width(1.6);
            cr.rectangle(sx, sy, sw, sw);
            let _ = cr.stroke();

            // 7. Corner & edge resize handles
            let handles = [
                (sx, sy),
                (sx + sw, sy),
                (sx, sy + sw),
                (sx + sw, sy + sw),
                (sx + sw * 0.5, sy),
                (sx + sw * 0.5, sy + sw),
                (sx, sy + sw * 0.5),
                (sx + sw, sy + sw * 0.5),
            ];
            for (i, (hx, hy)) in handles.iter().enumerate() {
                let radius = if i < 4 { 5.5 } else { 4.2 };
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
                cr.arc(*hx, *hy, radius + 1.2, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();

                cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
                cr.arc(*hx, *hy, radius, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
        });

        // Motion controller for dynamic resize/move cursors
        let motion = gtk::EventControllerMotion::new();
        let state_motion = state.clone();
        let canvas_motion = canvas.clone();
        motion.connect_motion(move |_, x, y| {
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
            let mode = hit_test(&st, &vp, x, y);
            canvas_motion.set_cursor_from_name(Some(cursor_for_mode(mode, false)));
        });
        canvas.add_controller(motion);

        // Drag gesture for moving and resizing the square
        let drag = gtk::GestureDrag::new();
        let state_drag_begin = state.clone();
        let canvas_drag_begin = canvas.clone();
        let update_drag_begin = update_ui.clone();
        drag.connect_drag_begin(move |_, start_x, start_y| {
            let vp = {
                let st = state_drag_begin.borrow();
                Viewport::compute(
                    f64::from(canvas_drag_begin.width()),
                    f64::from(canvas_drag_begin.height()),
                    st.img_w,
                    st.img_h,
                )
            };

            let mut mode = {
                let st = state_drag_begin.borrow();
                hit_test(&st, &vp, start_x, start_y)
            };

            // Clicking outside the square re-centers the square on that point and begins Move
            if mode.is_none() {
                let mut st = state_drag_begin.borrow_mut();
                let img_x = (start_x - vp.offset_x) / vp.scale;
                let img_y = (start_y - vp.offset_y) / vp.scale;
                let half = st.crop_size * 0.5;
                st.crop_x = (img_x - half).clamp(0.0, st.img_w - st.crop_size);
                st.crop_y = (img_y - half).clamp(0.0, st.img_h - st.crop_size);
                mode = Some(DragMode::Move);
            }

            {
                let mut st = state_drag_begin.borrow_mut();
                st.drag_mode = mode;
                st.drag_start_crop_x = st.crop_x;
                st.drag_start_crop_y = st.crop_y;
                st.drag_start_crop_size = st.crop_size;
                st.drag_origin_canvas_x = start_x;
                st.drag_origin_canvas_y = start_y;
            }

            canvas_drag_begin.set_cursor_from_name(Some(cursor_for_mode(mode, true)));
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
                st.apply_drag(mode, delta_img_x, delta_img_y);
                true
            };
            if changed {
                update_drag();
            }
        });

        let state_drag_end = state.clone();
        let canvas_drag_end = canvas.clone();
        drag.connect_drag_end(move |_, offset_x, offset_y| {
            let (end_x, end_y, vp, mode) = {
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
                let hover_mode = hit_test(&st, &vp, ex, ey);
                (ex, ey, vp, hover_mode)
            };
            let _ = (end_x, end_y, vp);
            canvas_drag_end.set_cursor_from_name(Some(cursor_for_mode(mode, false)));
            canvas_drag_end.queue_draw();
        });
        canvas.add_controller(drag);

        // Scroll controller for smooth mouse-wheel resizing around center
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let state_scroll = state.clone();
        let update_scroll = update_ui.clone();
        scroll.connect_scroll(move |_, _dx, dy| {
            {
                let mut st = state_scroll.borrow_mut();
                let step = (st.max_size - st.min_size).max(32.0) * 0.05;
                let new_size = st.crop_size - dy * step;
                st.set_size_around_center(new_size);
            }
            update_scroll();
            glib::Propagation::Stop
        });
        canvas.add_controller(scroll);

        // Scale slider value changed
        let state_scale = state.clone();
        let syncing_scale_cb = syncing_scale;
        let update_scale = update_ui.clone();
        size_scale.connect_value_changed(move |scale| {
            if syncing_scale_cb.get() {
                return;
            }
            {
                let mut st = state_scale.borrow_mut();
                st.set_size_around_center(scale.value());
            }
            update_scale();
        });

        // Reset / Center button
        let state_reset = state.clone();
        let update_reset = update_ui;
        reset_btn.connect_clicked(move |_| {
            state_reset.borrow_mut().reset_centered();
            update_reset();
        });

        // Cancel button
        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

        // Apply button
        let dialog_apply = dialog.clone();
        let on_apply = Rc::new(RefCell::new(Some(on_apply)));
        apply_btn.connect_clicked(move |btn| {
            let Some(cb) = on_apply.borrow_mut().take() else {
                return;
            };
            btn.set_sensitive(false);
            let region = state.borrow().to_region_crop();
            let source_clone = (*source_rc).clone();
            let dialog_apply = dialog_apply.clone();

            glib::spawn_future_local(async move {
                let res = tokio::task::spawn_blocking(move || {
                    crop_avatar_source(&source_clone, region)
                })
                .await;

                dialog_apply.close();

                match res {
                    Ok(Ok(output)) => cb(Ok(output)),
                    Ok(Err(e)) => cb(Err(e.to_string())),
                    Err(e) => cb(Err(format!("Failed to process avatar: {e}"))),
                }
            });
        });

        dialog.present(Some(parent));
    }
}

fn hit_test(st: &CropState, vp: &Viewport, cx: f64, cy: f64) -> Option<DragMode> {
    let sx = vp.offset_x + st.crop_x * vp.scale;
    let sy = vp.offset_y + st.crop_y * vp.scale;
    let sw = st.crop_size * vp.scale;

    let corner_r = 14.0;
    let dist_sq = |x1: f64, y1: f64, x2: f64, y2: f64| (x1 - x2).powi(2) + (y1 - y2).powi(2);

    if dist_sq(cx, cy, sx, sy) <= corner_r * corner_r {
        return Some(DragMode::ResizeNW);
    }
    if dist_sq(cx, cy, sx + sw, sy) <= corner_r * corner_r {
        return Some(DragMode::ResizeNE);
    }
    if dist_sq(cx, cy, sx, sy + sw) <= corner_r * corner_r {
        return Some(DragMode::ResizeSW);
    }
    if dist_sq(cx, cy, sx + sw, sy + sw) <= corner_r * corner_r {
        return Some(DragMode::ResizeSE);
    }

    let edge_r = 8.0;
    if (cy - sy).abs() <= edge_r && cx >= sx && cx <= sx + sw {
        return Some(DragMode::ResizeN);
    }
    if (cy - (sy + sw)).abs() <= edge_r && cx >= sx && cx <= sx + sw {
        return Some(DragMode::ResizeS);
    }
    if (cx - sx).abs() <= edge_r && cy >= sy && cy <= sy + sw {
        return Some(DragMode::ResizeW);
    }
    if (cx - (sx + sw)).abs() <= edge_r && cy >= sy && cy <= sy + sw {
        return Some(DragMode::ResizeE);
    }

    if cx >= sx && cx <= sx + sw && cy >= sy && cy <= sy + sw {
        return Some(DragMode::Move);
    }

    None
}

const fn cursor_for_mode(mode: Option<DragMode>, dragging: bool) -> &'static str {
    match mode {
        Some(DragMode::ResizeNW | DragMode::ResizeSE) => "nwse-resize",
        Some(DragMode::ResizeNE | DragMode::ResizeSW) => "nesw-resize",
        Some(DragMode::ResizeN | DragMode::ResizeS) => "ns-resize",
        Some(DragMode::ResizeE | DragMode::ResizeW) => "ew-resize",
        Some(DragMode::Move) => {
            if dragging {
                "grabbing"
            } else {
                "grab"
            }
        }
        None => "crosshair",
    }
}

fn create_cairo_surface(source: &AvatarSource) -> Option<cairo::ImageSurface> {
    let w = i32::try_from(source.width).ok()?;
    let h = i32::try_from(source.height).ok()?;
    let stride = cairo::Format::ARgb32.stride_for_width(source.width).ok()?;
    let mut data = vec![0u8; (stride as usize) * (source.height as usize)];

    // Convert straight RGBA8 to Cairo's premultiplied native ARGB32 (BGRA on little-endian)
    for y in 0..(source.height as usize) {
        let src_row = y * (source.width as usize) * 4;
        let dst_row = y * (stride as usize);
        for x in 0..(source.width as usize) {
            let si = src_row + x * 4;
            let di = dst_row + x * 4;
            let r = u16::from(source.rgba[si]);
            let g = u16::from(source.rgba[si + 1]);
            let b = u16::from(source.rgba[si + 2]);
            let a = u16::from(source.rgba[si + 3]);

            let pr = ((r * a + 127) / 255) as u8;
            let pg = ((g * a + 127) / 255) as u8;
            let pb = ((b * a + 127) / 255) as u8;
            let pa = a as u8;

            data[di] = pb;
            data[di + 1] = pg;
            data[di + 2] = pr;
            data[di + 3] = pa;
        }
    }

    cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, w, h, stride).ok()
}

fn update_preview_avatar(avatar: &adw::Avatar, source: &AvatarSource, crop: RegionCrop) {
    // Nearest-neighbor sample an 88x88 RGBA buffer from the selected crop region for instant UI preview
    const PREVIEW_DIM: u32 = 88;
    let mut preview_rgba = vec![0u8; (PREVIEW_DIM * PREVIEW_DIM * 4) as usize];

    let img_w = source.width.max(1);
    let img_h = source.height.max(1);
    let cx = crop.x.min(img_w - 1);
    let cy = crop.y.min(img_h - 1);
    let cw = crop.width.max(1).min(img_w - cx);
    let ch = crop.height.max(1).min(img_h - cy);

    for py in 0..PREVIEW_DIM {
        let sy = (cy + (py * ch) / PREVIEW_DIM).min(img_h - 1);
        for px in 0..PREVIEW_DIM {
            let sx = (cx + (px * cw) / PREVIEW_DIM).min(img_w - 1);
            let src_idx = ((sy * img_w + sx) * 4) as usize;
            let dst_idx = ((py * PREVIEW_DIM + px) * 4) as usize;
            preview_rgba[dst_idx..dst_idx + 4].copy_from_slice(&source.rgba[src_idx..src_idx + 4]);
        }
    }

    let bytes = glib::Bytes::from_owned(preview_rgba);
    let texture = gdk::MemoryTexture::new(
        PREVIEW_DIM as i32,
        PREVIEW_DIM as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        (PREVIEW_DIM * 4) as usize,
    );
    avatar.set_custom_image(Some(&texture));
}

fn draw_checkerboard(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let _ = cr.save();
    cr.rectangle(x, y, w, h);
    cr.clip();

    cr.set_source_rgb(0.18, 0.18, 0.20);
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();

    let tile = 12.0;
    cr.set_source_rgb(0.24, 0.24, 0.27);
    let cols = (w / tile).ceil() as i32;
    let rows = (h / tile).ceil() as i32;
    for r in 0..rows {
        for c in 0..cols {
            if (r + c) % 2 == 0 {
                cr.rectangle(
                    x + f64::from(c) * tile,
                    y + f64::from(r) * tile,
                    tile,
                    tile,
                );
            }
        }
    }
    let _ = cr.fill();
    let _ = cr.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crop_state_initial_and_move_clamping() {
        let mut st = CropState::new(400, 200);
        assert_eq!(st.crop_size, 200.0);
        assert_eq!(st.crop_x, 100.0);
        assert_eq!(st.crop_y, 0.0);

        st.drag_start_crop_x = st.crop_x;
        st.drag_start_crop_y = st.crop_y;
        st.drag_start_crop_size = st.crop_size;

        // Drag far right — should clamp to x = 200
        st.apply_drag(DragMode::Move, 500.0, 50.0);
        assert_eq!(st.crop_x, 200.0);
        assert_eq!(st.crop_y, 0.0);
    }

    #[test]
    fn test_crop_state_corner_resize_preserves_square_and_bounds() {
        let mut st = CropState::new(300, 300);
        st.set_size_around_center(100.0);
        assert_eq!(st.crop_x, 100.0);
        assert_eq!(st.crop_y, 100.0);
        assert_eq!(st.crop_size, 100.0);

        st.drag_start_crop_x = st.crop_x;
        st.drag_start_crop_y = st.crop_y;
        st.drag_start_crop_size = st.crop_size;

        // Resize NW corner outward by (-50, -30) -> size grows by 50 while SE corner stays at (200, 200)
        st.apply_drag(DragMode::ResizeNW, -50.0, -30.0);
        assert_eq!(st.crop_size, 150.0);
        assert_eq!(st.crop_x, 50.0);
        assert_eq!(st.crop_y, 50.0);
    }
}
