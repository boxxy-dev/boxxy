use crate::cache::{
    CachedThumbnail, ThumbnailCacheKey, compute_disk_cache_key, get_cached_thumbnail,
    insert_cached_thumbnail, load_disk_cached_thumbnail, metadata_signature,
    save_disk_cached_thumbnail,
};
use crate::parser::blocks::{ContentBlock, GalleryItem};
use crate::parser::markdown::strip_pango_markup;
use crate::renderer::{BlockRenderer, RenderContext};
use boxxy_assets::image::{
    MAX_THUMBNAIL_INPUT_BYTES, ThumbnailOutput, ThumbnailSpec, generate_thumbnail,
};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, gio, glib, graphene, gsk, pango};
use parking_lot::Mutex;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

const STANDALONE_SPEC: ThumbnailSpec = ThumbnailSpec::new(640, 400);
const GALLERY_SPEC: ThumbnailSpec = ThumbnailSpec::new(560, 350);

const GALLERY_SPACING: i32 = 12;
const GALLERY_CARD_MIN_W: i32 = 200;
const GALLERY_CARD_MAX_W: i32 = 280;
const GALLERY_MIN_PREVIEW_H: i32 = 125;
const GALLERY_MAX_PREVIEW_H: i32 = 175;
const CARD_FOOTER_HEIGHT: i32 = 28;

const STANDALONE_MAX_W: i32 = 320;
const STANDALONE_MAX_H: i32 = 200;
const STANDALONE_LOADING_W: i32 = 260;
const STANDALONE_LOADING_H: i32 = 162;

const TABLE_CELL_MAX_W: i32 = 200;
const TABLE_CELL_MAX_H: i32 = 125;
const TABLE_CELL_LOADING_W: i32 = 176;
const TABLE_CELL_LOADING_H: i32 = 110;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardLayout {
    Gallery,
    Standalone,
    TableCell,
}

/// Scales `(img_w, img_h)` proportionally to fit within `(max_w, max_h)`,
/// enforcing sensible minimum card dimensions so the footer never clips.
fn compute_bounded_card_dims(img_w: i32, img_h: i32, max_w: i32, max_h: i32) -> (i32, i32) {
    if img_w <= 0 || img_h <= 0 {
        return (max_w, max_h + CARD_FOOTER_HEIGHT);
    }
    let scale = (f64::from(max_w) / f64::from(img_w))
        .min(f64::from(max_h) / f64::from(img_h))
        .min(1.0);
    #[allow(clippy::cast_possible_truncation)]
    let pw = ((f64::from(img_w) * scale).round() as i32).clamp(140, max_w);
    #[allow(clippy::cast_possible_truncation)]
    let ph = ((f64::from(img_h) * scale).round() as i32).clamp(90, max_h);
    (pw, ph + CARD_FOOTER_HEIGHT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
}

#[must_use]
pub fn detect_media_kind(url: &str) -> MediaKind {
    let lower = clean_local_path(url).to_lowercase();
    if lower.ends_with(".mp4")
        || lower.ends_with(".webm")
        || lower.ends_with(".mkv")
        || lower.ends_with(".mov")
        || lower.ends_with(".avi")
    {
        MediaKind::Video
    } else if lower.ends_with(".mp3")
        || lower.ends_with(".flac")
        || lower.ends_with(".wav")
        || lower.ends_with(".ogg")
        || lower.ends_with(".m4a")
        || lower.ends_with(".opus")
        || lower.ends_with(".aac")
    {
        MediaKind::Audio
    } else {
        MediaKind::Image
    }
}

/// Computes `(cols, col_width, card_height)` for a given container width and item count.
///
/// Packs ~16:10 thumbnail cards (`200..=280 px` wide, `153..=203 px` tall including caption strip)
/// into as many columns as fit `avail_width` (capped at `n_children`) without stretching small
/// galleries into giant billboards on wide/ultrawide displays.
fn compute_grid_metrics(avail_width: i32, n_children: i32) -> (i32, i32, i32) {
    let n = n_children.max(1);
    let fit_cols = ((avail_width + GALLERY_SPACING) / (GALLERY_CARD_MIN_W + GALLERY_SPACING)).max(1);
    let cols = fit_cols.min(n);
    let total_spacing = (cols - 1) * GALLERY_SPACING;
    let avail_col_w = ((avail_width - total_spacing).max(120)) / cols;
    let col_w = avail_col_w.min(GALLERY_CARD_MAX_W);
    let preview_h = (col_w * 10 / 16).clamp(GALLERY_MIN_PREVIEW_H, GALLERY_MAX_PREVIEW_H);
    let card_h = preview_h + CARD_FOOTER_HEIGHT;
    (cols, col_w, card_h)
}

// ---------------------------------------------------------------------------
// Deterministic Responsive Aspect-Ratio Gallery Grid
// ---------------------------------------------------------------------------

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GalleryGrid {
        pub(super) last_cols: Cell<i32>,
        pub(super) last_card_h: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GalleryGrid {
        const NAME: &'static str = "BoxxyViewerGalleryGrid";
        type Type = super::GalleryGrid;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for GalleryGrid {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for GalleryGrid {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let mut n_children = 0;
            let mut child_opt = self.obj().first_child();
            while let Some(child) = child_opt {
                if child.is_visible() {
                    n_children += 1;
                }
                child_opt = child.next_sibling();
            }

            if n_children == 0 {
                return (0, 0, -1, -1);
            }

            match orientation {
                gtk::Orientation::Horizontal => {
                    let mut c = self.obj().first_child();
                    while let Some(child) = c {
                        if child.is_visible() {
                            let _ = child.measure(gtk::Orientation::Horizontal, -1);
                        }
                        c = child.next_sibling();
                    }
                    (140, 420, -1, -1)
                }
                gtk::Orientation::Vertical => {
                    if for_size > 0 {
                        let (cols, col_w, card_h) = compute_grid_metrics(for_size, n_children);
                        let mut c = self.obj().first_child();
                        while let Some(child) = c {
                            if child.is_visible() {
                                let _ = child.measure(gtk::Orientation::Vertical, col_w);
                            }
                            c = child.next_sibling();
                        }
                        let rows = (n_children + cols - 1) / cols;
                        let total_h = rows * card_h + (rows - 1) * GALLERY_SPACING;
                        (total_h, total_h, -1, -1)
                    } else {
                        // Unconstrained vertical query (used by GtkBox(Horizontal) for minimum height):
                        // at wide widths all items can fit in a single compact row, so `min_h` must
                        // equal 1 row at minimum card height (`<= measure(Vertical, width)` everywhere).
                        let min_h = GALLERY_MIN_PREVIEW_H + CARD_FOOTER_HEIGHT;

                        let (cols, col_w, card_h) = compute_grid_metrics(480, n_children);
                        let mut c = self.obj().first_child();
                        while let Some(child) = c {
                            if child.is_visible() {
                                let _ = child.measure(gtk::Orientation::Vertical, col_w);
                            }
                            c = child.next_sibling();
                        }
                        let rows = (n_children + cols - 1) / cols;
                        let nat_h = (rows * card_h + (rows - 1) * GALLERY_SPACING).max(min_h);
                        (min_h, nat_h, -1, -1)
                    }
                }
                _ => (140, 420, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let mut n_children = 0_i32;
            let mut count_opt = self.obj().first_child();
            while let Some(child) = count_opt {
                if child.is_visible() {
                    n_children += 1;
                }
                count_opt = child.next_sibling();
            }

            if n_children == 0 {
                return;
            }

            let (cols, col_w, card_h) = compute_grid_metrics(width, n_children);

            let mut idx_i32 = 0_i32;
            let mut child_opt = self.obj().first_child();
            while let Some(child) = child_opt {
                let next = child.next_sibling();
                if child.is_visible() {
                    let row = idx_i32 / cols;
                    let col = idx_i32 % cols;
                    let x = col * (col_w + GALLERY_SPACING);
                    let y = row * (card_h + GALLERY_SPACING);

                    let _ = child.measure(gtk::Orientation::Vertical, col_w);
                    #[allow(clippy::cast_precision_loss)]
                    let transform =
                        gsk::Transform::new().translate(&graphene::Point::new(x as f32, y as f32));
                    child.allocate(col_w, card_h, -1, Some(transform));
                    idx_i32 += 1;
                }
                child_opt = next;
            }

            let rows = (n_children + cols - 1) / cols;
            let expected_h = rows * card_h + (rows - 1) * GALLERY_SPACING;
            self.last_cols.set(cols);
            self.last_card_h.set(card_h);

            // Only schedule a resize if GTK allocated a stale height that differs from
            // `measure(Vertical, width)` (e.g. during live window/pane drag-resizing).
            if (height - expected_h).abs() > 2 {
                let obj_weak = self.obj().downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(obj) = obj_weak.upgrade() {
                        obj.queue_resize();
                    }
                });
            }
        }
    }
    pub struct BoundedCard {
        pub(super) card_w: Cell<i32>,
        pub(super) card_h: Cell<i32>,
    }

    impl Default for BoundedCard {
        fn default() -> Self {
            Self {
                card_w: Cell::new(STANDALONE_LOADING_W),
                card_h: Cell::new(STANDALONE_LOADING_H + CARD_FOOTER_HEIGHT),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BoundedCard {
        const NAME: &'static str = "BoxxyViewerBoundedCard";
        type Type = super::BoundedCard;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for BoundedCard {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for BoundedCard {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            // ConstantSize prevents GtkPicture from scaling its natural height up to
            // a wide parent container's full width (which otherwise creates a huge vertical gap).
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let w = self.card_w.get().max(120);
            let h = self.card_h.get().max(64);
            if let Some(child) = self.obj().first_child() {
                match orientation {
                    gtk::Orientation::Horizontal => {
                        let _ = child.measure(gtk::Orientation::Horizontal, -1);
                    }
                    gtk::Orientation::Vertical => {
                        let _ = child.measure(gtk::Orientation::Vertical, w);
                    }
                    _ => {}
                }
            }
            match orientation {
                gtk::Orientation::Horizontal => (w, w, -1, -1),
                gtk::Orientation::Vertical => (h, h, -1, -1),
                _ => (w, w, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                let target_w = width.min(self.card_w.get().max(120));
                let target_h = height.min(self.card_h.get().max(64));
                let _ = child.measure(gtk::Orientation::Vertical, target_w);
                child.allocate(target_w, target_h, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct GalleryGrid(ObjectSubclass<imp::GalleryGrid>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl GalleryGrid {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn append(&self, child: &impl IsA<gtk::Widget>) {
        child.set_parent(self);
    }
}

impl Default for GalleryGrid {
    fn default() -> Self {
        Self::new()
    }
}

glib::wrapper! {
    pub struct BoundedCard(ObjectSubclass<imp::BoundedCard>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl BoundedCard {
    pub fn new(w: i32, h: i32, child: &impl IsA<gtk::Widget>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().card_w.set(w);
        obj.imp().card_h.set(h);
        child.set_parent(&obj);
        obj
    }

    pub fn set_dimensions(&self, w: i32, h: i32) {
        let imp = self.imp();
        if imp.card_w.get() != w || imp.card_h.get() != h {
            imp.card_w.set(w);
            imp.card_h.set(h);
            self.queue_resize();
        }
    }
}

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) BoxxyTerminal/0.1")
        .build()
        .unwrap_or_default()
});

const FFMPEG_TIMEOUT: Duration = Duration::from_secs(5);

type SharedThumbOutcome = Option<Arc<ThumbnailOutput>>;
type InFlightThumbMap =
    HashMap<ThumbnailCacheKey, tokio::sync::watch::Sender<Option<SharedThumbOutcome>>>;

static IN_FLIGHT_THUMBS: LazyLock<Mutex<InFlightThumbMap>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct InFlightCleanup(ThumbnailCacheKey);

impl Drop for InFlightCleanup {
    fn drop(&mut self) {
        IN_FLIGHT_THUMBS.lock().remove(&self.0);
    }
}

/// Deduplicates concurrent thumbnail loads/decodes/downloads for the same `(url, spec)` key.
async fn fetch_or_load_thumbnail_deduped(
    url: String,
    spec: ThumbnailSpec,
    kind: MediaKind,
) -> Option<Arc<ThumbnailOutput>> {
    let key = ThumbnailCacheKey {
        url: url.clone(),
        spec,
    };

    let existing_rx = {
        let mut map = IN_FLIGHT_THUMBS.lock();
        if let Some(tx) = map.get(&key) {
            Some(tx.subscribe())
        } else {
            let (tx, _rx) = tokio::sync::watch::channel(None);
            map.insert(key.clone(), tx);
            None
        }
    };

    if let Some(mut rx) = existing_rx {
        let _ = rx.wait_for(Option::is_some).await;
        return rx.borrow().clone().flatten();
    }

    let _cleanup = InFlightCleanup(key.clone());
    let loaded = fetch_or_load_thumbnail(url, spec, kind).await.map(Arc::new);

    if let Some(tx) = IN_FLIGHT_THUMBS.lock().get(&key) {
        let _ = tx.send(Some(loaded.clone()));
    }

    loaded
}

/// Extracts a single PNG frame from a local video (at 1.0s, falling back to 0.0s)
/// or embedded cover art from an audio file using `ffmpeg` with fast input keyframe seeking,
/// bounded by `FFMPEG_TIMEOUT` and `kill_on_drop(true)`.
async fn extract_ffmpeg_frame_png(path: &str, kind: MediaKind) -> Option<Vec<u8>> {
    if kind == MediaKind::Video {
        for seek in ["00:00:01.00", "00:00:00.00"] {
            let mut cmd = tokio::process::Command::new("ffmpeg");
            cmd.args([
                "-ss",
                seek,
                "-i",
                path,
                "-frames:v",
                "1",
                "-vf",
                "scale=640:-1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "-v",
                "error",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

            if let Ok(Ok(out)) = tokio::time::timeout(FFMPEG_TIMEOUT, cmd.output()).await
                && out.status.success()
                && !out.stdout.is_empty()
            {
                return Some(out.stdout);
            }
        }
        None
    } else {
        // Audio: attempt to extract embedded cover art stream if present
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.args([
            "-i",
            path,
            "-an",
            "-frames:v",
            "1",
            "-vf",
            "scale=640:-1",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "-v",
            "error",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

        let out = tokio::time::timeout(FFMPEG_TIMEOUT, cmd.output())
            .await
            .ok()?
            .ok()?;

        if out.status.success() && !out.stdout.is_empty() {
            Some(out.stdout)
        } else {
            None
        }
    }
}

/// Fetches (or reads locally) and downscales an image, video frame, or audio cover into a
/// bounded `ThumbnailOutput`, consulting and populating the persistent Tier-2 disk cache.
async fn fetch_or_load_thumbnail(
    url: String,
    spec: ThumbnailSpec,
    kind: MediaKind,
) -> Option<ThumbnailOutput> {
    let is_remote = url.starts_with("http://") || url.starts_with("https://");

    if is_remote {
        if kind != MediaKind::Image {
            return None;
        }
        let disk_key = compute_disk_cache_key(&url, 0, 0, spec);
        if let Some(cached) = load_disk_cached_thumbnail(&disk_key).await {
            return Some(cached);
        }

        let mut resp = match HTTP_CLIENT.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                log::error!("Network error fetching image {url}: {e}");
                return None;
            }
        };

        if !resp.status().is_success() {
            log::error!("Failed to fetch image {}: HTTP {}", url, resp.status());
            return None;
        }

        if let Some(len) = resp.content_length()
            && len > MAX_THUMBNAIL_INPUT_BYTES as u64
        {
            log::error!("Remote image {url} is too large ({len} bytes)");
            return None;
        }

        let mut acc = Vec::new();
        while let Ok(Some(chunk)) = resp.chunk().await {
            if acc.len().saturating_add(chunk.len()) > MAX_THUMBNAIL_INPUT_BYTES {
                log::error!("Remote image {url} exceeded size limit during streaming");
                return None;
            }
            acc.extend_from_slice(&chunk);
        }

        let thumb = tokio::task::spawn_blocking(move || generate_thumbnail(&acc, spec))
            .await
            .ok()?
            .ok()?;

        save_disk_cached_thumbnail(&disk_key, &thumb).await;
        Some(thumb)
    } else {
        let path = clean_local_path(&url);
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) => {
                log::warn!("Failed to stat local media {path}: {e}");
                return None;
            }
        };

        if kind == MediaKind::Image && meta.len() > MAX_THUMBNAIL_INPUT_BYTES as u64 {
            log::warn!(
                "Local image {} exceeds 50 MB limit ({} bytes)",
                path,
                meta.len()
            );
            return None;
        }

        let (mtime, file_len) = metadata_signature(&meta);
        let disk_key = compute_disk_cache_key(&path, mtime, file_len, spec);
        if let Some(cached) = load_disk_cached_thumbnail(&disk_key).await {
            return Some(cached);
        }

        let bytes = if kind == MediaKind::Image {
            match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("Failed to read local image {path}: {e}");
                    return None;
                }
            }
        } else {
            extract_ffmpeg_frame_png(&path, kind).await?
        };

        let url_for_log = path.clone();
        let thumb =
            match tokio::task::spawn_blocking(move || generate_thumbnail(&bytes, spec)).await {
                Ok(Ok(t)) => t,
                Ok(Err(e)) => {
                    log::warn!("Thumbnail decode rejected for {url_for_log}: {e}");
                    return None;
                }
                Err(e) => {
                    log::error!("Thumbnail decode task panicked for {url_for_log}: {e}");
                    return None;
                }
            };

        save_disk_cached_thumbnail(&disk_key, &thumb).await;
        Some(thumb)
    }
}

fn clean_local_path(url: &str) -> String {
    let trimmed = url.trim();
    if let Some(stripped) = trimmed.strip_prefix("file://") {
        glib::Uri::unescape_string(stripped, None)
            .map_or_else(|| stripped.to_string(), |g| g.to_string())
    } else {
        trimmed.to_string()
    }
}

fn derive_display_caption(
    url: &str,
    title: &str,
    alt: &str,
    explicit_caption: Option<&str>,
) -> String {
    if let Some(cap) = explicit_caption {
        let stripped = strip_pango_markup(cap, false);
        if !stripped.trim().is_empty() {
            return stripped;
        }
    }
    if !title.trim().is_empty() {
        return title.trim().to_string();
    }
    let path_str = clean_local_path(url);
    if let Some(name) = Path::new(&path_str).file_name().and_then(|n| n.to_str())
        && !name.is_empty()
    {
        return name.to_string();
    }
    if !alt.trim().is_empty() {
        return alt.trim().to_string();
    }
    url.to_string()
}

/// Opens the media file directly in the user's default viewer/player without prompting "Open With…".
fn open_default_viewer(widget: &impl IsA<gtk::Widget>, url: &str) {
    let is_remote = url.starts_with("http://") || url.starts_with("https://");
    let target_uri = if is_remote {
        url.to_string()
    } else {
        let path = clean_local_path(url);
        gio::File::for_path(&path).uri().to_string()
    };

    if gio::AppInfo::launch_default_for_uri(&target_uri, None::<&gio::AppLaunchContext>).is_ok() {
        return;
    }

    let parent_win = widget
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok());
    if is_remote {
        let launcher = gtk::UriLauncher::new(url);
        gtk::glib::spawn_future_local(async move {
            let _ = launcher.launch_future(parent_win.as_ref()).await;
        });
    } else {
        let path = clean_local_path(url);
        let file = gio::File::for_path(&path);
        let launcher = gtk::FileLauncher::new(Some(&file));
        launcher.set_always_ask(false);
        gtk::glib::spawn_future_local(async move {
            let _ = launcher.launch_future(parent_win.as_ref()).await;
        });
    }
}

/// Explicitly opens the "Open With…" application chooser dialog via XDG Desktop Portal.
fn open_with_dialog(widget: &impl IsA<gtk::Widget>, url: &str) {
    let parent_win = widget
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok());
    let path = clean_local_path(url);
    let file = gio::File::for_path(&path);
    let launcher = gtk::FileLauncher::new(Some(&file));
    launcher.set_always_ask(true);
    gtk::glib::spawn_future_local(async move {
        if let Err(e) = launcher.launch_future(parent_win.as_ref()).await {
            log::warn!("FileLauncher (Open With) failed for {path}: {e}");
        }
    });
}

/// Opens the containing directory of a local file in the system file manager.
fn show_in_folder(widget: &impl IsA<gtk::Widget>, url: &str) {
    let path = clean_local_path(url);
    let file = gio::File::for_path(&path);
    if let Some(parent) = file.parent()
        && gio::AppInfo::launch_default_for_uri(&parent.uri(), None::<&gio::AppLaunchContext>)
            .is_ok()
    {
        return;
    }

    let parent_win = widget
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok());
    let launcher = gtk::FileLauncher::new(Some(&file));
    gtk::glib::spawn_future_local(async move {
        if let Err(e) = launcher
            .open_containing_folder_future(parent_win.as_ref())
            .await
        {
            log::warn!("open_containing_folder failed for {path}: {e}");
        }
    });
}

fn create_media_badge_overlay(kind: MediaKind) -> Option<gtk::Box> {
    if kind == MediaKind::Image {
        return None;
    }
    let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    badge.add_css_class("viewer-video-play-badge");
    badge.set_halign(gtk::Align::Center);
    badge.set_valign(gtk::Align::Center);
    badge.set_can_target(false);

    let icon_name = if kind == MediaKind::Video {
        "boxxy-media-playback-start-symbolic"
    } else {
        "audio-x-generic-symbolic"
    };
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(24);
    badge.append(&icon);
    Some(badge)
}

/// Builds a compact `.viewer-image-card` suitable for embedding inside a Markdown table cell.
#[must_use]
pub(crate) fn build_table_image_card(item: &GalleryItem, ctx: &RenderContext) -> gtk::Widget {
    build_image_card(
        &item.url,
        &item.title,
        &item.alt,
        item.caption.as_deref(),
        CardLayout::TableCell,
        ctx,
    )
}

/// Builds a `.viewer-image-card` widget supporting Images, Videos, and Audio with
/// Tier-1 synchronous cache lookup, click-to-open in default viewer/player, and `Open With…`.
#[allow(clippy::too_many_lines)]
fn build_image_card(
    url: &str,
    title: &str,
    alt: &str,
    caption: Option<&str>,
    layout: CardLayout,
    ctx: &RenderContext,
) -> gtk::Widget {
    let in_gallery = layout == CardLayout::Gallery;
    let (max_w, max_h, loading_w, loading_h) = match layout {
        CardLayout::Gallery => (GALLERY_CARD_MAX_W, GALLERY_MAX_PREVIEW_H, 240, 150),
        CardLayout::Standalone => (
            STANDALONE_MAX_W,
            STANDALONE_MAX_H,
            STANDALONE_LOADING_W,
            STANDALONE_LOADING_H,
        ),
        CardLayout::TableCell => (
            TABLE_CELL_MAX_W,
            TABLE_CELL_MAX_H,
            TABLE_CELL_LOADING_W,
            TABLE_CELL_LOADING_H,
        ),
    };

    let kind = detect_media_kind(url);
    let spec = if in_gallery {
        GALLERY_SPEC
    } else {
        STANDALONE_SPEC
    };

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.add_css_class("viewer-image-card");
    card.set_overflow(gtk::Overflow::Hidden);
    card.set_hexpand(true);
    card.set_vexpand(true);
    card.set_halign(gtk::Align::Fill);
    card.set_valign(gtk::Align::Fill);

    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    picture.set_content_fit(if in_gallery || kind != MediaKind::Image {
        gtk::ContentFit::Cover
    } else {
        gtk::ContentFit::Contain
    });
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.set_hexpand(true);
    picture.set_vexpand(true);

    // Base preview surface owns primary/secondary click gestures so the floating
    // top-right `⋮` MenuButton overlay never triggers `open_default_viewer`.
    let preview_surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview_surface.add_css_class("viewer-image-preview-area");
    preview_surface.set_hexpand(true);
    preview_surface.set_vexpand(true);
    preview_surface.set_cursor_from_name(Some("pointer"));
    preview_surface.append(&picture);

    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay.set_child(Some(&preview_surface));

    if let Some(play_badge) = create_media_badge_overlay(kind) {
        overlay.add_overlay(&play_badge);
    }

    let display_caption = derive_display_caption(url, title, alt, caption);
    let base_tooltip = clean_local_path(url);
    preview_surface.set_tooltip_text(Some(&base_tooltip));

    // Full-width bottom title strip (no competing button, no artificial max_width_chars cap)
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    footer.add_css_class("viewer-image-card-footer");
    footer.set_vexpand(false);
    footer.set_valign(gtk::Align::End);

    let caption_label = gtk::Label::new(Some(&display_caption));
    caption_label.set_xalign(0.0);
    caption_label.set_halign(gtk::Align::Fill);
    caption_label.set_hexpand(true);
    caption_label.set_width_chars(1);
    caption_label.set_ellipsize(pango::EllipsizeMode::Middle);
    caption_label.set_single_line_mode(true);
    caption_label.add_css_class("caption");
    caption_label.add_css_class("viewer-image-card-title");
    caption_label.set_tooltip_text(Some(&base_tooltip));
    footer.append(&caption_label);

    // Build action group & menu for Open / Open With... / Show in Folder / Copy
    let is_remote = url.starts_with("http://") || url.starts_with("https://");
    let action_group = gio::SimpleActionGroup::new();

    let url_open = url.to_string();
    let card_weak_open = card.downgrade();
    let act_open = gio::SimpleAction::new("open", None);
    act_open.connect_activate(move |_, _| {
        if let Some(c) = card_weak_open.upgrade() {
            open_default_viewer(&c, &url_open);
        }
    });
    action_group.add_action(&act_open);

    if !is_remote {
        let url_open_with = url.to_string();
        let card_weak_ow = card.downgrade();
        let act_open_with = gio::SimpleAction::new("open-with", None);
        act_open_with.connect_activate(move |_, _| {
            if let Some(c) = card_weak_ow.upgrade() {
                open_with_dialog(&c, &url_open_with);
            }
        });
        action_group.add_action(&act_open_with);

        let url_folder = url.to_string();
        let card_weak_folder = card.downgrade();
        let act_folder = gio::SimpleAction::new("show-in-folder", None);
        act_folder.connect_activate(move |_, _| {
            if let Some(c) = card_weak_folder.upgrade() {
                show_in_folder(&c, &url_folder);
            }
        });
        action_group.add_action(&act_folder);
    }

    if kind == MediaKind::Image {
        let pic_weak_copy = picture.downgrade();
        let act_copy_img = gio::SimpleAction::new("copy-image", None);
        act_copy_img.connect_activate(move |_, _| {
            if let Some(pic) = pic_weak_copy.upgrade()
                && let Some(paintable) = pic.paintable()
                && let Ok(texture) = paintable.downcast::<gdk::Texture>()
                && let Some(display) = gdk::Display::default()
            {
                display.clipboard().set_texture(&texture);
            }
        });
        action_group.add_action(&act_copy_img);
    }

    let copy_target = clean_local_path(url);
    let act_copy_path = gio::SimpleAction::new("copy-path", None);
    act_copy_path.connect_activate(move |_, _| {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&copy_target);
        }
    });
    action_group.add_action(&act_copy_path);

    card.insert_action_group("img", Some(&action_group));

    let menu = gio::Menu::new();
    let open_section = gio::Menu::new();
    open_section.append(Some("Open"), Some("img.open"));
    if !is_remote {
        open_section.append(Some("Open With…"), Some("img.open-with"));
        open_section.append(Some("Show in Folder"), Some("img.show-in-folder"));
    }
    menu.append_section(None, &open_section);

    let copy_section = gio::Menu::new();
    if kind == MediaKind::Image {
        copy_section.append(Some("Copy Image"), Some("img.copy-image"));
    }
    copy_section.append(
        Some(if is_remote { "Copy URL" } else { "Copy Path" }),
        Some("img.copy-path"),
    );
    menu.append_section(None, &copy_section);

    // Float the `⋮` options button over the top-right corner of the image (fades in on hover)
    let menu_btn = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .css_classes(["circular", "viewer-card-menu-btn"])
        .tooltip_text("Media Options")
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .margin_top(6)
        .margin_end(6)
        .menu_model(&menu)
        .build();
    overlay.add_overlay(&menu_btn);

    // Primary click on the preview opens immediately in default viewer/player (no "Open With" prompt)
    let primary_click = gtk::GestureClick::new();
    primary_click.set_button(gdk::BUTTON_PRIMARY);
    let url_click = url.to_string();
    let card_weak_click = card.downgrade();
    primary_click.connect_released(move |gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(c) = card_weak_click.upgrade() {
            open_default_viewer(&c, &url_click);
        }
    });
    preview_surface.add_controller(primary_click);

    // Secondary (right) click on the preview opens the action menu
    let secondary_click = gtk::GestureClick::new();
    secondary_click.set_button(gdk::BUTTON_SECONDARY);
    let menu_btn_for_ctx = menu_btn;
    secondary_click.connect_pressed(move |gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        menu_btn_for_ctx.popup();
    });
    preview_surface.add_controller(secondary_click);

    card.append(&overlay);
    card.append(&footer);

    // Fast-path: synchronous Tier-1 texture cache check (0ms on GtkListView scroll-back!)
    let cache_key = ThumbnailCacheKey {
        url: url.to_string(),
        spec,
    };

    if let Some(cached) = get_cached_thumbnail(&cache_key) {
        picture.set_paintable(Some(&cached.texture));
        picture.set_visible(true);
        let rich_tooltip = if kind == MediaKind::Image {
            format!(
                "{} ({}×{})",
                base_tooltip, cached.original_width, cached.original_height
            )
        } else {
            base_tooltip
        };
        preview_surface.set_tooltip_text(Some(&rich_tooltip));
        caption_label.set_tooltip_text(Some(&rich_tooltip));

        if in_gallery {
            return card.upcast();
        }
        let (cw, ch) = compute_bounded_card_dims(cached.disp_w, cached.disp_h, max_w, max_h);
        let bounded = BoundedCard::new(cw, ch, &card);
        bounded.set_halign(gtk::Align::Start);
        if layout == CardLayout::Standalone {
            bounded.set_margin_bottom(12);
        }
        return bounded.upcast();
    }

    let bounded_opt = if in_gallery {
        None
    } else {
        let bounded = BoundedCard::new(loading_w, loading_h + CARD_FOOTER_HEIGHT, &card);
        bounded.set_halign(gtk::Align::Start);
        if layout == CardLayout::Standalone {
            bounded.set_margin_bottom(12);
        }
        Some(bounded)
    };

    // Slow-path (cache miss): show spinner and decode/load off-thread
    picture.set_visible(false);
    let spinner = gtk::Spinner::new();
    spinner.set_size_request(24, 24);
    spinner.set_halign(gtk::Align::Center);
    spinner.set_valign(gtk::Align::Center);
    spinner.set_margin_top(24);
    spinner.set_margin_bottom(24);
    spinner.start();
    overlay.add_overlay(&spinner);
    overlay.set_measure_overlay(&spinner, true);

    let url_async = url.to_string();
    let alt_async = alt.to_string();
    let gen_handle = ctx.generation.clone();
    let start_gen = ctx.current_generation();
    let picture_weak = picture.downgrade();
    let preview_weak = preview_surface.downgrade();
    let overlay_weak = overlay.downgrade();
    let spinner_weak = spinner.downgrade();
    let caption_weak = caption_label.downgrade();
    let bounded_weak = bounded_opt.as_ref().map(ObjectExt::downgrade);

    gtk::glib::spawn_future_local(async move {
        let url_for_task = url_async.clone();
        let thumb_opt = boxxy_sys_utils::runtime()
            .spawn(async move { fetch_or_load_thumbnail_deduped(url_for_task, spec, kind).await })
            .await
            .ok()
            .flatten();

        // Populate Tier-1 MemoryTexture cache even if the row scrolled out of view mid-load,
        // so scrolling back to this row hits Tier-1 synchronously in 0 ms!
        let cached_entry = thumb_opt.map(|thumb_arc| {
            #[allow(clippy::cast_possible_wrap)]
            let disp_w = thumb_arc.thumb_width as i32;
            #[allow(clippy::cast_possible_wrap)]
            let disp_h = thumb_arc.thumb_height as i32;
            let orig_w = thumb_arc.original_width;
            let orig_h = thumb_arc.original_height;
            let stride = (thumb_arc.thumb_width * 4) as usize;

            let rgba_vec = Arc::try_unwrap(thumb_arc)
                .map_or_else(|shared| shared.rgba_bytes.clone(), |owned| owned.rgba_bytes);
            let glib_bytes = glib::Bytes::from_owned(rgba_vec);
            let texture = gdk::MemoryTexture::new(
                disp_w,
                disp_h,
                gdk::MemoryFormat::R8g8b8a8,
                &glib_bytes,
                stride,
            );

            let entry = CachedThumbnail {
                texture,
                disp_w,
                disp_h,
                original_width: orig_w,
                original_height: orig_h,
            };
            insert_cached_thumbnail(cache_key, entry.clone());
            entry
        });

        if gen_handle.load(Ordering::Relaxed) != start_gen {
            return;
        }
        let (Some(picture_live), Some(preview_live), Some(overlay_live)) = (
            picture_weak.upgrade(),
            preview_weak.upgrade(),
            overlay_weak.upgrade(),
        ) else {
            return;
        };

        if let Some(spinner_live) = spinner_weak.upgrade() {
            spinner_live.stop();
            overlay_live.remove_overlay(&spinner_live);
        }

        if let Some(entry) = cached_entry {
            if let Some(ref b_weak) = bounded_weak
                && let Some(b) = b_weak.upgrade()
            {
                let (cw, ch) = compute_bounded_card_dims(entry.disp_w, entry.disp_h, max_w, max_h);
                b.set_dimensions(cw, ch);
            }
            picture_live.set_paintable(Some(&entry.texture));
            picture_live.set_visible(true);
            let rich_tooltip = if kind == MediaKind::Image {
                format!(
                    "{} ({}×{})",
                    base_tooltip, entry.original_width, entry.original_height
                )
            } else {
                base_tooltip
            };
            preview_live.set_tooltip_text(Some(&rich_tooltip));
            if let Some(cap_live) = caption_weak.upgrade() {
                cap_live.set_tooltip_text(Some(&rich_tooltip));
            }
        } else if kind != MediaKind::Image {
            if let Some(ref b_weak) = bounded_weak
                && let Some(b) = b_weak.upgrade()
            {
                b.set_dimensions(loading_w, loading_h + CARD_FOOTER_HEIGHT);
            }
            picture_live.set_visible(true);
        } else {
            if let Some(ref b_weak) = bounded_weak
                && let Some(b) = b_weak.upgrade()
            {
                b.set_dimensions(220, 44 + CARD_FOOTER_HEIGHT);
            }
            fallback_to_link(&preview_live, &picture_live, &url_async, &alt_async);
        }
    });

    if let Some(bounded) = bounded_opt {
        bounded.upcast()
    } else {
        card.upcast()
    }
}

fn render_gallery(items: &[GalleryItem], ctx: &RenderContext) -> gtk::Widget {
    if items.len() == 1 {
        let item = &items[0];
        return build_image_card(
            &item.url,
            &item.title,
            &item.alt,
            item.caption.as_deref(),
            CardLayout::Standalone,
            ctx,
        );
    }

    let grid = GalleryGrid::new();
    grid.add_css_class("viewer-gallery");
    grid.set_hexpand(true);
    grid.set_halign(gtk::Align::Fill);
    grid.set_margin_top(6);
    grid.set_margin_bottom(14);

    for item in items {
        let card = build_image_card(
            &item.url,
            &item.title,
            &item.alt,
            item.caption.as_deref(),
            CardLayout::Gallery,
            ctx,
        );
        grid.append(&card);
    }

    grid.upcast()
}

pub struct ImageRenderer;

impl BlockRenderer for ImageRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(
            block,
            ContentBlock::Image { .. } | ContentBlock::ImageGallery { .. }
        )
    }

    fn render(
        &self,
        block: &ContentBlock,
        registry: &crate::registry::ViewerRegistry,
    ) -> gtk::Widget {
        self.render_with_context(block, registry, &RenderContext::default())
    }

    fn render_with_context(
        &self,
        block: &ContentBlock,
        _registry: &crate::registry::ViewerRegistry,
        ctx: &RenderContext,
    ) -> gtk::Widget {
        match block {
            ContentBlock::Image { url, title, alt } => {
                build_image_card(url, title, alt, None, CardLayout::Standalone, ctx)
            }
            ContentBlock::ImageGallery { items } => render_gallery(items, ctx),
            _ => unreachable!(),
        }
    }
}

fn fallback_to_link(preview_surface: &gtk::Box, picture: &gtk::Picture, url: &str, alt: &str) {
    picture.set_visible(false);
    let label = if alt.is_empty() { url } else { alt };
    let link = gtk::LinkButton::with_label(url, label);
    link.set_halign(gtk::Align::Center);
    link.set_valign(gtk::Align::Center);
    preview_surface.append(&link);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_local_path_unescapes_file_uris() {
        assert_eq!(
            clean_local_path("file:///tmp/my%20wallpaper.png"),
            "/tmp/my wallpaper.png"
        );
        assert_eq!(
            clean_local_path("/tmp/plain-file.jpg"),
            "/tmp/plain-file.jpg"
        );
    }

    #[test]
    fn detect_media_kind_classifies_extensions() {
        assert_eq!(detect_media_kind("/tmp/clip.mp4"), MediaKind::Video);
        assert_eq!(detect_media_kind("/tmp/clip.WEBM"), MediaKind::Video);
        assert_eq!(detect_media_kind("/tmp/song.flac"), MediaKind::Audio);
        assert_eq!(detect_media_kind("/tmp/photo.png"), MediaKind::Image);
    }

    #[test]
    fn derive_display_caption_prefers_explicit_then_basename() {
        assert_eq!(
            derive_display_caption(
                "file:///tmp/folder/wall-01.jpg",
                "",
                "alt text",
                Some("<b>Custom Caption</b>")
            ),
            "Custom Caption"
        );
        assert_eq!(
            derive_display_caption("file:///tmp/folder/wall-01.jpg", "", "alt text", None),
            "wall-01.jpg"
        );
    }

    #[test]
    fn compute_grid_metrics_caps_card_size_and_packs_wide_columns() {
        // Narrow (< 412px): 1 column, capped at GALLERY_CARD_MAX_W (280px)
        let (cols_narrow, col_w_narrow, card_h_narrow) = compute_grid_metrics(320, 4);
        assert_eq!(cols_narrow, 1);
        assert_eq!(col_w_narrow, GALLERY_CARD_MAX_W);
        assert_eq!(
            card_h_narrow,
            (GALLERY_CARD_MAX_W * 10 / 16) + CARD_FOOTER_HEIGHT
        );

        // Medium (600px): 2 columns capped at 280px
        let (cols_med, col_w_med, _) = compute_grid_metrics(600, 4);
        assert_eq!(cols_med, 2);
        assert_eq!(col_w_med, GALLERY_CARD_MAX_W);

        // Wide (1200px) with 4 items: all 4 fit in a single row, each 280px wide
        let (cols_wide, col_w_wide, card_h_wide) = compute_grid_metrics(1200, 4);
        assert_eq!(cols_wide, 4);
        assert_eq!(col_w_wide, GALLERY_CARD_MAX_W);
        assert_eq!(card_h_wide, GALLERY_MAX_PREVIEW_H + CARD_FOOTER_HEIGHT);
    }

    #[test]
    fn compute_bounded_card_dims_preserves_aspect_ratio() {
        // 16:9 image (640x360) scaled into 320x200 standalone box -> 320x180 + footer
        let (cw, ch) = compute_bounded_card_dims(640, 360, STANDALONE_MAX_W, STANDALONE_MAX_H);
        assert_eq!(cw, 320);
        assert_eq!(ch, 180 + CARD_FOOTER_HEIGHT);

        // 16:9 image (640x360) scaled into 200x125 table-cell box -> 200x113 + footer
        let (tcw, tch) = compute_bounded_card_dims(640, 360, TABLE_CELL_MAX_W, TABLE_CELL_MAX_H);
        assert_eq!(tcw, 200);
        assert_eq!(tch, 113 + CARD_FOOTER_HEIGHT);
    }
}
