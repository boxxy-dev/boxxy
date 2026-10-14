use crate::cache::{
    CachedThumbnail, ThumbnailCacheKey, compute_disk_cache_key, get_cached_thumbnail,
    insert_cached_thumbnail, load_disk_cached_thumbnail, metadata_signature,
    save_disk_cached_thumbnail,
};
use crate::parser::blocks::{ContentBlock, GalleryItem};
use crate::renderer::BlockRenderer;
use boxxy_assets::image::{
    MAX_THUMBNAIL_INPUT_BYTES, ThumbnailOutput, ThumbnailSpec, generate_thumbnail,
};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, gio, glib, graphene, gsk, pango};
use std::cell::Cell;
use std::path::Path;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

const STANDALONE_SPEC: ThumbnailSpec = ThumbnailSpec::new(640, 400);
const GALLERY_SPEC: ThumbnailSpec = ThumbnailSpec::new(640, 400);

const GALLERY_SPACING: i32 = 12;
const TWO_COL_BREAKPOINT: i32 = 420;
const THREE_COL_BREAKPOINT: i32 = 920;
const CARD_FOOTER_HEIGHT: i32 = 34;

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
/// Preserves the ~16:10 card preview proportion across all window scales:
/// - `< 420px`: 1 column
/// - `420px .. 920px`: 2 columns
/// - `>= 920px` (with 3+ items): 3 columns
fn compute_grid_metrics(avail_width: i32, n_children: i32) -> (i32, i32, i32) {
    let cols = if avail_width >= THREE_COL_BREAKPOINT && n_children >= 3 {
        3
    } else if avail_width >= TWO_COL_BREAKPOINT && n_children >= 2 {
        2
    } else {
        1
    };
    let total_spacing = (cols - 1) * GALLERY_SPACING;
    let col_w = ((avail_width - total_spacing).max(120)) / cols;
    let preview_h = (col_w * 10 / 16).max(110);
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
                    let avail_w = if for_size > 0 { for_size } else { 420 };
                    let (cols, col_w, card_h) = compute_grid_metrics(avail_w, n_children);

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
                }
                _ => (140, 420, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let mut children = Vec::new();
            let mut child_opt = self.obj().first_child();
            while let Some(child) = child_opt {
                if child.is_visible() {
                    children.push(child.clone());
                }
                child_opt = child.next_sibling();
            }

            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let n_children = children.len() as i32;
            if n_children == 0 {
                return;
            }

            let (cols, col_w, card_h) = compute_grid_metrics(width, n_children);

            for (idx, child) in children.into_iter().enumerate() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                let idx_i32 = idx as i32;
                let row = idx_i32 / cols;
                let col = idx_i32 % cols;
                let x = col * (col_w + GALLERY_SPACING);
                let y = row * (card_h + GALLERY_SPACING);

                let _ = child.measure(gtk::Orientation::Vertical, col_w);
                #[allow(clippy::cast_precision_loss)]
                let transform =
                    gsk::Transform::new().translate(&graphene::Point::new(x as f32, y as f32));
                child.allocate(col_w, card_h, -1, Some(transform));
            }

            if self.last_cols.get() != cols || (self.last_card_h.get() - card_h).abs() > 2 {
                self.last_cols.set(cols);
                self.last_card_h.set(card_h);
                let obj = self.obj().clone();
                glib::idle_add_local_once(move || {
                    obj.queue_resize();
                });
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

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) BoxxyTerminal/0.1")
        .build()
        .unwrap_or_default()
});

/// Extracts a single PNG frame from a local video (at 1.0s, falling back to 0.0s)
/// or embedded cover art from an audio file using `ffmpeg` with fast input keyframe seeking.
async fn extract_ffmpeg_frame_png(path: &str, kind: MediaKind) -> Option<Vec<u8>> {
    if kind == MediaKind::Video {
        for seek in ["00:00:01.00", "00:00:00.00"] {
            if let Ok(out) = tokio::process::Command::new("ffmpeg")
                .args([
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
                .output()
                .await
                && out.status.success()
                && !out.stdout.is_empty()
            {
                return Some(out.stdout);
            }
        }
        None
    } else {
        // Audio: attempt to extract embedded cover art stream if present
        let out = tokio::process::Command::new("ffmpeg")
            .args([
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
            .output()
            .await
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
        let stripped = strip_simple_pango_tags(cap);
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

fn strip_simple_pango_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
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

/// Builds a `.viewer-image-card` widget supporting Images, Videos, and Audio with
/// Tier-1 synchronous cache lookup, click-to-open in default viewer/player, and `Open With…`.
fn build_image_card(
    url: &str,
    title: &str,
    alt: &str,
    caption: Option<&str>,
    in_gallery: bool,
) -> gtk::Widget {
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

    if in_gallery {
        card.set_hexpand(true);
        card.set_vexpand(true);
        card.set_halign(gtk::Align::Fill);
        card.set_valign(gtk::Align::Fill);
    } else {
        card.set_hexpand(false);
        card.set_halign(gtk::Align::Start);
        card.set_margin_bottom(12);
    }

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

    let overlay = gtk::Overlay::new();
    overlay.add_css_class("viewer-image-preview-area");
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay.set_child(Some(&picture));
    overlay.set_cursor_from_name(Some("pointer"));

    if let Some(play_badge) = create_media_badge_overlay(kind) {
        overlay.add_overlay(&play_badge);
    }

    let display_caption = derive_display_caption(url, title, alt, caption);
    let base_tooltip = clean_local_path(url);
    overlay.set_tooltip_text(Some(&base_tooltip));

    // Footer bar sits directly on the card surface (no nested background box)
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    footer.add_css_class("viewer-image-card-footer");
    footer.set_vexpand(false);
    footer.set_valign(gtk::Align::End);

    let caption_label = gtk::Label::new(Some(&display_caption));
    caption_label.set_xalign(0.0);
    caption_label.set_hexpand(true);
    caption_label.set_width_chars(6);
    caption_label.set_max_width_chars(14);
    caption_label.set_ellipsize(pango::EllipsizeMode::Middle);
    caption_label.set_single_line_mode(true);
    caption_label.add_css_class("caption");
    caption_label.add_css_class("dim-label");
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

    let menu_btn = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .css_classes(["flat", "circular", "viewer-card-menu-btn"])
        .tooltip_text("Media Options")
        .valign(gtk::Align::Center)
        .menu_model(&menu)
        .build();
    footer.append(&menu_btn);

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
    overlay.add_controller(primary_click);

    // Secondary (right) click on the preview opens the action menu
    let secondary_click = gtk::GestureClick::new();
    secondary_click.set_button(gdk::BUTTON_SECONDARY);
    let menu_btn_for_ctx = menu_btn.clone();
    secondary_click.connect_pressed(move |gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        menu_btn_for_ctx.popup();
    });
    overlay.add_controller(secondary_click);

    card.append(&overlay);
    card.append(&footer);

    // Fast-path: synchronous Tier-1 texture cache check (0ms on GtkListView scroll-back!)
    let cache_key = ThumbnailCacheKey {
        url: url.to_string(),
        spec,
    };

    if let Some(cached) = get_cached_thumbnail(&cache_key) {
        if !in_gallery {
            picture.set_size_request(cached.disp_w.min(360), cached.disp_h.min(240));
        }
        picture.set_paintable(Some(&cached.texture));
        picture.set_visible(true);
        let rich_tooltip = if kind == MediaKind::Image {
            format!(
                "{} ({}×{})",
                base_tooltip, cached.original_width, cached.original_height
            )
        } else {
            base_tooltip.clone()
        };
        overlay.set_tooltip_text(Some(&rich_tooltip));
        caption_label.set_tooltip_text(Some(&rich_tooltip));
        return card.upcast();
    }

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
    let picture_clone = picture.clone();
    let overlay_clone = overlay.clone();
    let spinner_clone = spinner.clone();
    let caption_label_clone = caption_label.clone();

    gtk::glib::spawn_future_local(async move {
        let url_for_task = url_async.clone();
        let thumb_opt = boxxy_sys_utils::runtime()
            .spawn(async move { fetch_or_load_thumbnail(url_for_task, spec, kind).await })
            .await
            .ok()
            .flatten();

        spinner_clone.stop();
        overlay_clone.remove_overlay(&spinner_clone);

        if let Some(thumb) = thumb_opt {
            #[allow(clippy::cast_possible_wrap)]
            let disp_w = thumb.thumb_width as i32;
            #[allow(clippy::cast_possible_wrap)]
            let disp_h = thumb.thumb_height as i32;

            let glib_bytes = glib::Bytes::from_owned(thumb.rgba_bytes);
            let texture = gdk::MemoryTexture::new(
                disp_w,
                disp_h,
                gdk::MemoryFormat::R8g8b8a8,
                &glib_bytes,
                (thumb.thumb_width * 4) as usize,
            );

            insert_cached_thumbnail(
                cache_key,
                CachedThumbnail {
                    texture: texture.clone(),
                    disp_w,
                    disp_h,
                    original_width: thumb.original_width,
                    original_height: thumb.original_height,
                },
            );

            if !in_gallery {
                picture_clone.set_size_request(disp_w.min(360), disp_h.min(240));
            }
            picture_clone.set_paintable(Some(&texture));
            picture_clone.set_visible(true);
            let rich_tooltip = if kind == MediaKind::Image {
                format!(
                    "{} ({}×{})",
                    base_tooltip, thumb.original_width, thumb.original_height
                )
            } else {
                base_tooltip.clone()
            };
            overlay_clone.set_tooltip_text(Some(&rich_tooltip));
            caption_label_clone.set_tooltip_text(Some(&rich_tooltip));
        } else if kind != MediaKind::Image {
            // For videos/audio without an extractable cover/frame, keep the clean card
            // with the centered Play/Audio badge instead of collapsing into a text link.
            if !in_gallery {
                picture_clone.set_size_request(320, 180);
            }
            picture_clone.set_visible(true);
        } else {
            fallback_to_link(&overlay_clone, &picture_clone, &url_async, &alt_async);
        }
    });

    card.upcast()
}

fn render_gallery(items: &[GalleryItem]) -> gtk::Widget {
    if items.len() == 1 {
        let item = &items[0];
        return build_image_card(
            &item.url,
            &item.title,
            &item.alt,
            item.caption.as_deref(),
            false,
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
            true,
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
        _registry: &crate::registry::ViewerRegistry,
    ) -> gtk::Widget {
        match block {
            ContentBlock::Image { url, title, alt } => {
                build_image_card(url, title, alt, None, false)
            }
            ContentBlock::ImageGallery { items } => render_gallery(items),
            _ => unreachable!(),
        }
    }
}

fn fallback_to_link(overlay: &gtk::Overlay, picture: &gtk::Picture, url: &str, alt: &str) {
    picture.set_visible(false);
    let label = if alt.is_empty() { url } else { alt };
    let link = gtk::LinkButton::with_label(url, label);
    link.set_halign(gtk::Align::Center);
    link.set_valign(gtk::Align::Center);
    overlay.set_child(Some(&link));
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
    fn compute_grid_metrics_scales_columns_and_preserves_16_10_ratio() {
        // Narrow (< 420px): 1 column, 16:10 preview height
        let (cols_narrow, col_w_narrow, card_h_narrow) = compute_grid_metrics(280, 4);
        assert_eq!(cols_narrow, 1);
        assert_eq!(col_w_narrow, 280);
        assert_eq!(card_h_narrow, (280 * 10 / 16) + CARD_FOOTER_HEIGHT);

        // Medium/Wide (420px .. 920px): 2 columns, 16:10 preview height per column
        let (cols_med, col_w_med, card_h_med) = compute_grid_metrics(800, 4);
        assert_eq!(cols_med, 2);
        assert_eq!(col_w_med, (800 - 12) / 2);
        assert_eq!(card_h_med, (col_w_med * 10 / 16) + CARD_FOOTER_HEIGHT);

        // Very wide (>= 920px): 3 columns
        let (cols_wide, _, _) = compute_grid_metrics(1000, 4);
        assert_eq!(cols_wide, 3);
    }
}
