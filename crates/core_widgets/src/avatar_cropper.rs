use adw::prelude::*;
use boxxy_assets::image::{
    AvatarOutput, AvatarSource, MAX_INPUT_BYTES, MIN_AVATAR_DIMENSION, RegionCrop,
    crop_avatar_source, load_avatar_source,
};
use gtk4 as gtk;
use gtk4::cairo;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Grace period before surfacing a loading spinner so fast (<180ms) local decodes open
/// straight into the interactive cropper without a single-frame spinner flash.
const SPINNER_DELAY: Duration = Duration::from_millis(180);

/// Minimum duration the spinner stays visible *if* a slow load crosses `SPINNER_DELAY`,
/// preventing jarring 10-20ms flashes on borderline loads.
const SPINNER_MIN_HOLD: Duration = Duration::from_millis(250);

/// Maximum pixel dimension on the longest axis for the interactive Cairo canvas backdrop.
/// Keeps the software-composited `GtkDrawingArea` surface <= 4 MB (instead of 64 MB for 4K images)
/// for 120 Hz drag responsiveness while final export still crops from the full-res `AvatarSource`.
const MAX_BACKDROP_DIM: u32 = 1024;

/// Live circular `AdwAvatar` preview texture size (`44px` logical × 2x HiDPI).
const PREVIEW_DIM: u32 = 88;

/// Pre-scaled premultiplied native-endian ARGB32 buffer prepared off the GTK main thread.
struct BackdropBuffer {
    width: i32,
    height: i32,
    stride: i32,
    data: Vec<u8>,
}

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
        button.add_css_class("circular");
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
        edit_btn.add_css_class("circular");
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

        // Top-right `×` remove/reset badge button (displayed when `removable` is enabled)
        let remove_btn = gtk::Button::new();
        remove_btn.add_css_class("circular");
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

/// Opens a file chooser for PNG/JPEG/WebP images, immediately presents `AvatarCropDialog`
/// in its loading state once a file is chosen, and decodes the image + backdrop off-thread.
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
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));

    let parent_widget: gtk::Widget = parent.clone().upcast();
    let title = dialog_title.to_string();
    let on_result = Rc::new(RefCell::new(Some(on_result)));

    dialog.open(window.as_ref(), None::<&gio::Cancellable>, move |result| {
        let Ok(file) = result else {
            // User cancelled the file chooser
            return;
        };

        if let Some(cb) = on_result.borrow_mut().take() {
            AvatarCropDialog::present_from_file(&parent_widget, &title, file, cb);
        }
    });
}

/// Reads a selected `gio::File` while enforcing `MAX_INPUT_BYTES` *before* allocating
/// multi-hundred-MB buffers, supporting both local paths and Flatpak/GVFS portal URIs.
async fn read_selected_file_bytes(file: &gio::File) -> Result<Vec<u8>, String> {
    if let Some(path) = file.path() {
        let meta = tokio::fs::metadata(&path).await.map_err(|e| {
            log::error!("Failed to stat selected avatar image: {e}");
            "Failed to read image file.".to_string()
        })?;
        if meta.len() > MAX_INPUT_BYTES as u64 {
            return Err(format!(
                "input too large: {} bytes (max {MAX_INPUT_BYTES})",
                meta.len()
            ));
        }
        return tokio::fs::read(&path).await.map_err(|e| {
            log::error!("Failed to read selected avatar image: {e}");
            "Failed to read image file.".to_string()
        });
    }

    // Fallback for portal / GVFS virtual files without a host filesystem path
    if let Ok(info) = file
        .query_info_future(
            gio::FILE_ATTRIBUTE_STANDARD_SIZE,
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await
    {
        let size = info.size();
        if size > 0 && (size as u64) > MAX_INPUT_BYTES as u64 {
            return Err(format!(
                "input too large: {size} bytes (max {MAX_INPUT_BYTES})"
            ));
        }
    }

    let (bytes, _) = file.load_bytes_future().await.map_err(|e| {
        log::error!("Failed to load selected portal avatar image: {e}");
        "Failed to read image file.".to_string()
    })?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "input too large: {} bytes (max {MAX_INPUT_BYTES})",
            bytes.len()
        ));
    }
    Ok(bytes.to_vec())
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

    fn reset_centered(&mut self) -> bool {
        let next_size = self.max_size;
        let next_x = (self.img_w - next_size) * 0.5;
        let next_y = (self.img_h - next_size) * 0.5;
        self.set_geometry(next_x, next_y, next_size)
    }

    fn set_size_around_center(&mut self, new_size: f64) -> bool {
        let clamped_size = new_size.clamp(self.min_size, self.max_size);
        let center_x = self.crop_x + self.crop_size * 0.5;
        let center_y = self.crop_y + self.crop_size * 0.5;

        let next_x = (center_x - clamped_size * 0.5).clamp(0.0, self.img_w - clamped_size);
        let next_y = (center_y - clamped_size * 0.5).clamp(0.0, self.img_h - clamped_size);
        self.set_geometry(next_x, next_y, clamped_size)
    }

    fn nudge(&mut self, dx: f64, dy: f64) -> bool {
        let next_x = (self.crop_x + dx).clamp(0.0, self.img_w - self.crop_size);
        let next_y = (self.crop_y + dy).clamp(0.0, self.img_h - self.crop_size);
        self.set_geometry(next_x, next_y, self.crop_size)
    }

    fn apply_drag(&mut self, mode: DragMode, delta_img_x: f64, delta_img_y: f64) -> bool {
        let sx = self.drag_start_crop_x;
        let sy = self.drag_start_crop_y;
        let ss = self.drag_start_crop_size;

        let (next_x, next_y, next_size) = match mode {
            DragMode::Move => (
                (sx + delta_img_x).clamp(0.0, self.img_w - ss),
                (sy + delta_img_y).clamp(0.0, self.img_h - ss),
                ss,
            ),
            // Project corner drag vectors onto the 45° outward diagonal so dragging inward
            // along either axis shrinks smoothly instead of fighting single-axis motion.
            DragMode::ResizeSE => {
                let anchor_x = sx;
                let anchor_y = sy;
                let raw = ss + (delta_img_x + delta_img_y) * 0.5;
                let max_s = (self.img_w - anchor_x).min(self.img_h - anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                (anchor_x, anchor_y, new_size)
            }
            DragMode::ResizeNW => {
                let anchor_x = sx + ss;
                let anchor_y = sy + ss;
                let raw = ss - (delta_img_x + delta_img_y) * 0.5;
                let max_s = anchor_x.min(anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                (anchor_x - new_size, anchor_y - new_size, new_size)
            }
            DragMode::ResizeNE => {
                let anchor_x = sx;
                let anchor_y = sy + ss;
                let raw = ss + (delta_img_x - delta_img_y) * 0.5;
                let max_s = (self.img_w - anchor_x).min(anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                (anchor_x, anchor_y - new_size, new_size)
            }
            DragMode::ResizeSW => {
                let anchor_x = sx + ss;
                let anchor_y = sy;
                let raw = ss + (-delta_img_x + delta_img_y) * 0.5;
                let max_s = anchor_x.min(self.img_h - anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                (anchor_x - new_size, anchor_y, new_size)
            }
            DragMode::ResizeE => {
                let anchor_x = sx;
                let center_y = sy + ss * 0.5;
                let raw = ss + delta_img_x;
                let max_s = (self.img_w - anchor_x).min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                let y = (center_y - new_size * 0.5).clamp(0.0, self.img_h - new_size);
                (anchor_x, y, new_size)
            }
            DragMode::ResizeW => {
                let anchor_x = sx + ss;
                let center_y = sy + ss * 0.5;
                let raw = ss - delta_img_x;
                let max_s = anchor_x.min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                let y = (center_y - new_size * 0.5).clamp(0.0, self.img_h - new_size);
                (anchor_x - new_size, y, new_size)
            }
            DragMode::ResizeS => {
                let anchor_y = sy;
                let center_x = sx + ss * 0.5;
                let raw = ss + delta_img_y;
                let max_s = (self.img_h - anchor_y).min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                let x = (center_x - new_size * 0.5).clamp(0.0, self.img_w - new_size);
                (x, anchor_y, new_size)
            }
            DragMode::ResizeN => {
                let anchor_y = sy + ss;
                let center_x = sx + ss * 0.5;
                let raw = ss - delta_img_y;
                let max_s = anchor_y.min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                let x = (center_x - new_size * 0.5).clamp(0.0, self.img_w - new_size);
                (x, anchor_y - new_size, new_size)
            }
        };

        self.set_geometry(next_x, next_y, next_size)
    }

    fn set_geometry(&mut self, next_x: f64, next_y: f64, next_size: f64) -> bool {
        let changed = (self.crop_x - next_x).abs() > 1e-4
            || (self.crop_y - next_y).abs() > 1e-4
            || (self.crop_size - next_size).abs() > 1e-4;
        self.crop_x = next_x;
        self.crop_y = next_y;
        self.crop_size = next_size;
        changed
    }

    fn to_region_crop(&self) -> RegionCrop {
        let img_w = self.img_w as u32;
        let img_h = self.img_h as u32;
        let size = (self.crop_size.round() as u32).max(1);
        let x = self.crop_x.round().max(0.0) as u32;
        let y = self.crop_y.round().max(0.0) as u32;
        RegionCrop::new(x, y, size, size).clamp_square_to_bounds(img_w, img_h)
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

type CropResultCb = Box<dyn FnOnce(Result<AvatarOutput, String>)>;
type MountSourceFn = Rc<dyn Fn(AvatarSource, Option<BackdropBuffer>, bool)>;

struct ShellHandle {
    dialog: adw::Dialog,
    closed_flag: Rc<Cell<bool>>,
    mount: MountSourceFn,
}

impl AvatarCropDialog {
    pub fn present<W, F>(parent: &W, title: &str, source: AvatarSource, on_apply: F)
    where
        W: IsA<gtk::Widget>,
        F: FnOnce(Result<AvatarOutput, String>) + 'static,
    {
        let backdrop = build_backdrop_buffer(&source);
        let on_apply_slot: Rc<RefCell<Option<CropResultCb>>> =
            Rc::new(RefCell::new(Some(Box::new(on_apply))));
        let shell = Self::build_shell(title, on_apply_slot);
        (shell.mount)(source, backdrop, false);
        shell.dialog.present(Some(parent));
    }

    fn present_from_file<W, F>(parent: &W, title: &str, file: gio::File, on_result: F)
    where
        W: IsA<gtk::Widget>,
        F: FnOnce(Result<AvatarOutput, String>) + 'static,
    {
        let parent_widget: gtk::Widget = parent.clone().upcast();
        let title_str = title.to_string();
        let on_result_slot: Rc<RefCell<Option<CropResultCb>>> =
            Rc::new(RefCell::new(Some(Box::new(on_result))));
        let shell_slot: Rc<RefCell<Option<ShellHandle>>> = Rc::new(RefCell::new(None));
        let load_finished = Rc::new(Cell::new(false));
        let spinner_shown_at: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));

        // Only open the loading spinner dialog if reading + decoding exceeds `SPINNER_DELAY` (180ms).
        {
            let parent_timer = parent_widget.clone();
            let title_timer = title_str.clone();
            let on_result_timer = on_result_slot.clone();
            let shell_timer = shell_slot.clone();
            let finished_timer = load_finished.clone();
            let shown_timer = spinner_shown_at.clone();

            glib::spawn_future_local(async move {
                glib::timeout_future(SPINNER_DELAY).await;
                if finished_timer.get() || parent_timer.root().is_none() {
                    return;
                }
                let shell = Self::build_shell(&title_timer, on_result_timer);
                shell.dialog.present(Some(&parent_timer));
                shown_timer.set(Some(Instant::now()));
                *shell_timer.borrow_mut() = Some(shell);
            });
        }

        glib::spawn_future_local(async move {
            let outcome: Result<(AvatarSource, Option<BackdropBuffer>), String> = async {
                let bytes = read_selected_file_bytes(&file).await?;

                if shell_slot
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.closed_flag.get())
                {
                    return Err(String::new());
                }

                let decode_res = tokio::task::spawn_blocking(move || {
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
            }
            .await;

            load_finished.set(true);

            let existing_shell = shell_slot.borrow_mut().take();
            match (existing_shell, outcome) {
                // Fast path (< 180ms): open directly in the ready state with zero spinner flash
                (None, Ok((source, backdrop))) => {
                    if parent_widget.root().is_none() {
                        return;
                    }
                    let shell = Self::build_shell(&title_str, on_result_slot);
                    (shell.mount)(source, backdrop, false);
                    shell.dialog.present(Some(&parent_widget));
                }
                // Fast path error (< 180ms): report toast directly without flashing an empty dialog
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
                    if let Some(shown_at) = spinner_shown_at.get() {
                        let elapsed = shown_at.elapsed();
                        if elapsed < SPINNER_MIN_HOLD {
                            glib::timeout_future(SPINNER_MIN_HOLD - elapsed).await;
                        }
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

    fn build_shell(title: &str, on_apply: Rc<RefCell<Option<CropResultCb>>>) -> ShellHandle {
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

        let closed_flag = Rc::new(Cell::new(false));
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

        // Helper to refresh canvas, info label, slider, and live avatar preview
        let update_ui = {
            let canvas = canvas.clone();
            let preview_avatar = preview_avatar.clone();
            let crop_info_label = crop_info_label.clone();
            let size_scale = size_scale.clone();
            let syncing_scale = syncing_scale.clone();
            let state = state.clone();
            let source_slot = loaded_source.clone();

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

                crop_info_label.set_label(&format!("{} × {} px", region.width, region.height));
                update_preview_avatar(&preview_avatar, &src, region);
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
            let cw = f64::from(width);
            let ch = f64::from(height);
            let st = state_draw.borrow();
            let vp = Viewport::compute(cw, ch, st.img_w, st.img_h);

            // 1. Tiled checkerboard background inside the image bounds (single fill via SurfacePattern)
            draw_checkerboard(
                cr,
                checker_pattern.as_ref(),
                vp.offset_x,
                vp.offset_y,
                vp.draw_w,
                vp.draw_h,
            );

            // 2. Draw pre-scaled source backdrop
            if let Some((surface, surf_w, surf_h)) = surface_draw.borrow().as_ref() {
                let _ = cr.save();
                cr.rectangle(vp.offset_x, vp.offset_y, vp.draw_w, vp.draw_h);
                cr.clip();
                cr.translate(vp.offset_x, vp.offset_y);
                cr.scale(vp.draw_w / *surf_w, vp.draw_h / *surf_h);
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
            cr.rectangle(
                vp.offset_x,
                vp.offset_y,
                vp.draw_w,
                (sy - vp.offset_y).max(0.0),
            );
            let bottom_y = sy + sw;
            cr.rectangle(
                vp.offset_x,
                bottom_y,
                vp.draw_w,
                (vp.offset_y + vp.draw_h - bottom_y).max(0.0),
            );
            cr.rectangle(vp.offset_x, sy, (sx - vp.offset_x).max(0.0), sw);
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
                cr.move_to(sx + offset, sy);
                cr.line_to(sx + offset, sy + sw);
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

        // Motion controller for dynamic resize/move cursors (deduplicated per cursor transition)
        let motion = gtk::EventControllerMotion::new();
        let state_motion = state.clone();
        let canvas_motion = canvas.clone();
        let cursor_motion = last_cursor.clone();
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
            set_cursor_if_changed(&canvas_motion, &cursor_motion, cursor_for_mode(mode, false));
        });
        canvas.add_controller(motion);

        // Drag gesture for moving and resizing the square
        let drag = gtk::GestureDrag::new();
        let state_drag_begin = state.clone();
        let canvas_drag_begin = canvas.clone();
        let cursor_drag_begin = last_cursor.clone();
        let update_drag_begin = update_ui.clone();
        drag.connect_drag_begin(move |_, start_x, start_y| {
            let _ = canvas_drag_begin.grab_focus();
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
                let new_size = st.crop_size - ticks * step;
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
                        let s = st.crop_size + step * 4.0;
                        st.set_size_around_center(s)
                    }
                    gdk::Key::minus | gdk::Key::KP_Subtract => {
                        let s = st.crop_size - step * 4.0;
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
                let res = tokio::task::spawn_blocking(move || {
                    crop_avatar_source(&source_for_crop, region)
                })
                .await;

                apply_done.set(true);
                if let Some(shown_at) = apply_spinner_at.get() {
                    let elapsed = shown_at.elapsed();
                    if elapsed < SPINNER_MIN_HOLD {
                        glib::timeout_future(SPINNER_MIN_HOLD - elapsed).await;
                    }
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

/// Builds a bounded (`<= 1024px`) premultiplied native-endian ARGB32 backdrop buffer.
/// Safe to execute inside `tokio::task::spawn_blocking` (contains no GTK/Cairo types).
fn build_backdrop_buffer(source: &AvatarSource) -> Option<BackdropBuffer> {
    let src_w = source.width();
    let src_h = source.height();
    if src_w == 0 || src_h == 0 {
        return None;
    }

    let (dst_w, dst_h) = if src_w <= MAX_BACKDROP_DIM && src_h <= MAX_BACKDROP_DIM {
        (src_w, src_h)
    } else {
        let scale = (f64::from(MAX_BACKDROP_DIM) / f64::from(src_w))
            .min(f64::from(MAX_BACKDROP_DIM) / f64::from(src_h));
        let dw = ((f64::from(src_w) * scale).round() as u32).clamp(1, MAX_BACKDROP_DIM);
        let dh = ((f64::from(src_h) * scale).round() as u32).clamp(1, MAX_BACKDROP_DIM);
        (dw, dh)
    };

    let w_i32 = i32::try_from(dst_w).ok()?;
    let h_i32 = i32::try_from(dst_h).ok()?;
    let stride = cairo::Format::ARgb32.stride_for_width(dst_w).ok()?;
    let stride_usize = stride as usize;
    let mut data = vec![0u8; stride_usize * (dst_h as usize)];
    let rgba = source.rgba();

    if dst_w == src_w && dst_h == src_h {
        for y in 0..(dst_h as usize) {
            let src_row = y * (src_w as usize) * 4;
            let dst_row = y * stride_usize;
            for x in 0..(dst_w as usize) {
                let si = src_row + x * 4;
                let di = dst_row + x * 4;
                let word =
                    premultiply_argb32_word(rgba[si], rgba[si + 1], rgba[si + 2], rgba[si + 3]);
                data[di..di + 4].copy_from_slice(&word.to_ne_bytes());
            }
        }
    } else {
        // 4-tap sub-pixel box sample when downscaling 2K/4K sources to <=1024px
        let max_sx = src_w - 1;
        let max_sy = src_h - 1;
        for dy in 0..dst_h {
            let sy0 = ((dy * 4 + 1) * src_h / (dst_h * 4)).min(max_sy);
            let sy1 = ((dy * 4 + 3) * src_h / (dst_h * 4)).min(max_sy);
            let dst_row = (dy as usize) * stride_usize;
            for dx in 0..dst_w {
                let sx0 = ((dx * 4 + 1) * src_w / (dst_w * 4)).min(max_sx);
                let sx1 = ((dx * 4 + 3) * src_w / (dst_w * 4)).min(max_sx);
                let [r, g, b, a] = average_4_rgba(rgba, src_w, sx0, sy0, sx1, sy1);
                let di = dst_row + (dx as usize) * 4;
                let word = premultiply_argb32_word(r, g, b, a);
                data[di..di + 4].copy_from_slice(&word.to_ne_bytes());
            }
        }
    }

    Some(BackdropBuffer {
        width: w_i32,
        height: h_i32,
        stride,
        data,
    })
}

#[inline]
fn premultiply_argb32_word(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let a16 = u16::from(a);
    let pr = u32::from((u16::from(r) * a16 + 127) / 255);
    let pg = u32::from((u16::from(g) * a16 + 127) / 255);
    let pb = u32::from((u16::from(b) * a16 + 127) / 255);
    let pa = u32::from(a);
    (pa << 24) | (pr << 16) | (pg << 8) | pb
}

#[inline]
fn average_4_rgba(rgba: &[u8], img_w: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> [u8; 4] {
    let i00 = ((y0 * img_w + x0) * 4) as usize;
    let i10 = ((y0 * img_w + x1) * 4) as usize;
    let i01 = ((y1 * img_w + x0) * 4) as usize;
    let i11 = ((y1 * img_w + x1) * 4) as usize;

    let r = (u16::from(rgba[i00])
        + u16::from(rgba[i10])
        + u16::from(rgba[i01])
        + u16::from(rgba[i11])
        + 2)
        >> 2;
    let g = (u16::from(rgba[i00 + 1])
        + u16::from(rgba[i10 + 1])
        + u16::from(rgba[i01 + 1])
        + u16::from(rgba[i11 + 1])
        + 2)
        >> 2;
    let b = (u16::from(rgba[i00 + 2])
        + u16::from(rgba[i10 + 2])
        + u16::from(rgba[i01 + 2])
        + u16::from(rgba[i11 + 2])
        + 2)
        >> 2;
    let a = (u16::from(rgba[i00 + 3])
        + u16::from(rgba[i10 + 3])
        + u16::from(rgba[i01 + 3])
        + u16::from(rgba[i11 + 3])
        + 2)
        >> 2;

    [r as u8, g as u8, b as u8, a as u8]
}

fn update_preview_avatar(avatar: &adw::Avatar, source: &AvatarSource, crop: RegionCrop) {
    // 4-tap anti-aliased sub-pixel sample into an 88x88 RGBA buffer (~0.04 ms, shimmer-free)
    let mut preview_rgba = vec![0u8; (PREVIEW_DIM * PREVIEW_DIM * 4) as usize];

    let img_w = source.width().max(1);
    let img_h = source.height().max(1);
    let max_x = img_w - 1;
    let max_y = img_h - 1;
    let rgba = source.rgba();
    let sq = crop.clamp_square_to_bounds(img_w, img_h);

    for py in 0..PREVIEW_DIM {
        let sy0 = (sq.y + ((py * 4 + 1) * sq.height) / (PREVIEW_DIM * 4)).min(max_y);
        let sy1 = (sq.y + ((py * 4 + 3) * sq.height) / (PREVIEW_DIM * 4)).min(max_y);
        for px in 0..PREVIEW_DIM {
            let sx0 = (sq.x + ((px * 4 + 1) * sq.width) / (PREVIEW_DIM * 4)).min(max_x);
            let sx1 = (sq.x + ((px * 4 + 3) * sq.width) / (PREVIEW_DIM * 4)).min(max_x);
            let sample = average_4_rgba(rgba, img_w, sx0, sy0, sx1, sy1);
            let dst_idx = ((py * PREVIEW_DIM + px) * 4) as usize;
            preview_rgba[dst_idx..dst_idx + 4].copy_from_slice(&sample);
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

/// Builds a tiny 24×24 repeating 2×2 checkerboard tile once so redraws execute a single `fill()`.
fn create_checkerboard_pattern() -> Option<cairo::SurfacePattern> {
    const TILE: i32 = 12;
    let surface = cairo::ImageSurface::create(cairo::Format::Rgb24, TILE * 2, TILE * 2).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;

    cr.set_source_rgb(0.18, 0.18, 0.20);
    cr.rectangle(0.0, 0.0, f64::from(TILE * 2), f64::from(TILE * 2));
    let _ = cr.fill();

    cr.set_source_rgb(0.24, 0.24, 0.27);
    cr.rectangle(0.0, 0.0, f64::from(TILE), f64::from(TILE));
    cr.rectangle(
        f64::from(TILE),
        f64::from(TILE),
        f64::from(TILE),
        f64::from(TILE),
    );
    let _ = cr.fill();
    drop(cr);

    let pattern = cairo::SurfacePattern::create(&surface);
    pattern.set_extend(cairo::Extend::Repeat);
    pattern.set_filter(cairo::Filter::Nearest);
    Some(pattern)
}

fn draw_checkerboard(
    cr: &cairo::Context,
    pattern: Option<&cairo::SurfacePattern>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) {
    let _ = cr.save();
    if let Some(pat) = pattern {
        let mut matrix = cairo::Matrix::identity();
        matrix.translate(-x, -y);
        pat.set_matrix(matrix);
        let _ = cr.set_source(pat);
    } else {
        cr.set_source_rgb(0.18, 0.18, 0.20);
    }
    cr.rectangle(x, y, w, h);
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

        // Drag far right — should clamp to x = 200 and report changed = true
        assert!(st.apply_drag(DragMode::Move, 500.0, 50.0));
        assert_eq!(st.crop_x, 200.0);
        assert_eq!(st.crop_y, 0.0);

        // Dragging further right against the same wall reports changed = false
        assert!(!st.apply_drag(DragMode::Move, 600.0, 50.0));
    }

    #[test]
    fn test_crop_state_corner_resize_projects_diagonally_both_outward_and_inward() {
        let mut st = CropState::new(300, 300);
        st.set_size_around_center(120.0);
        assert_eq!(st.crop_x, 90.0);
        assert_eq!(st.crop_y, 90.0);
        assert_eq!(st.crop_size, 120.0);

        st.drag_start_crop_x = st.crop_x;
        st.drag_start_crop_y = st.crop_y;
        st.drag_start_crop_size = st.crop_size;

        // Resize NW corner outward by (-40, -40) -> size grows to 160 while SE anchor stays at (210, 210)
        assert!(st.apply_drag(DragMode::ResizeNW, -40.0, -40.0));
        assert_eq!(st.crop_size, 160.0);
        assert_eq!(st.crop_x, 50.0);
        assert_eq!(st.crop_y, 50.0);

        // Dragging SE corner inward purely horizontally (-60, 0) smoothly shrinks by 30px
        st.drag_start_crop_x = st.crop_x;
        st.drag_start_crop_y = st.crop_y;
        st.drag_start_crop_size = st.crop_size;
        assert!(st.apply_drag(DragMode::ResizeSE, -60.0, 0.0));
        assert_eq!(st.crop_size, 130.0);
        assert_eq!(st.crop_x, 50.0);
        assert_eq!(st.crop_y, 50.0);
    }

    #[test]
    fn test_premultiply_argb32_word_handles_extremes_and_half_alpha() {
        assert_eq!(premultiply_argb32_word(255, 128, 64, 0), 0);
        assert_eq!(
            premultiply_argb32_word(255, 128, 64, 255),
            (255 << 24) | (255 << 16) | (128 << 8) | 64
        );
    }
}
