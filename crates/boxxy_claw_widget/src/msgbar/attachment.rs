use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Maximum raw file size accepted for attached images and text/code files (10 MiB,
/// matching `boxxy_claw::engine::turn::file_refs::MAX_IMAGE_BYTES`).
pub const MAX_ATTACHMENT_FILE_BYTES: usize = 10 * 1024 * 1024;
/// Maximum UTF-8 text/code slice retained inline per attachment/snippet (512 KiB).
const MAX_TEXT_SNIPPET_BYTES: usize = 512 * 1024;
/// Header window inspected for NUL bytes to reject binary files (PDFs, archives, media).
const BINARY_PROBE_BYTES: usize = 8 * 1024;
/// Chunk size used when streaming portal / GVFS files without a local host path.
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

type NotifyHook = Rc<RefCell<Option<Rc<dyn Fn(String)>>>>;

/// RAII guard resetting `picker_in_flight` when dropped (on open, error, or cancel).
struct InFlightGuard(Rc<Cell<bool>>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileReadReject {
    Directory,
    Empty,
    TooLarge { limit_bytes: usize },
    IoError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileIngestSource {
    Picker,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub label: String,
    pub content: String,
    pub is_image: bool,
}

#[derive(Clone)]
pub struct AttachmentManager {
    pub tags_box: gtk::FlowBox,
    pub attachments: Rc<RefCell<Vec<Attachment>>>,
    notify_hook: NotifyHook,
    picker_in_flight: Rc<Cell<bool>>,
}

impl Default for AttachmentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachmentManager {
    #[must_use]
    pub fn new() -> Self {
        let tags_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(8)
            .row_spacing(6)
            .max_children_per_line(12)
            .min_children_per_line(1)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .visible(false)
            .css_classes(["msgbar-tags-flow"])
            .build();

        Self {
            tags_box,
            attachments: Rc::new(RefCell::new(Vec::new())),
            notify_hook: Rc::new(RefCell::new(None)),
            picker_in_flight: Rc::new(Cell::new(false)),
        }
    }

    #[must_use]
    pub const fn widget(&self) -> &gtk::FlowBox {
        &self.tags_box
    }

    /// Registers a callback (typically routed to `ClawHost::notify`) for surfacing user-visible
    /// toasts when an attached, pasted, or dropped file is oversized or unsupported.
    pub fn set_notify_hook<F: Fn(String) + 'static>(&self, hook: F) {
        *self.notify_hook.borrow_mut() = Some(Rc::new(hook));
    }

    fn notify(&self, message: impl Into<String>) {
        if let Some(cb) = self.notify_hook.borrow().as_ref().cloned() {
            cb(message.into());
        }
    }

    pub fn clear(&self) {
        self.attachments.borrow_mut().clear();
        while let Some(child) = self.tags_box.first_child() {
            self.tags_box.remove(&child);
        }
        self.tags_box.set_visible(false);
    }

    #[must_use]
    pub fn get_attachments(&self) -> Vec<Attachment> {
        self.attachments.borrow().clone()
    }

    pub fn load_attachments(&self, attachments: Vec<Attachment>) {
        self.clear();
        for att in attachments {
            self.add_attachment(att.label, att.content, att.is_image);
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn add_attachment(&self, label: String, content: String, is_image: bool) {
        let id = uuid::Uuid::new_v4().to_string();
        let tooltip = if is_image {
            label.clone()
        } else {
            build_text_preview_tooltip(&label, &content)
        };

        let att = Attachment {
            id: id.clone(),
            label: label.clone(),
            content,
            is_image,
        };

        self.attachments.borrow_mut().push(att);

        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chip.add_css_class("msgbar-tag");
        chip.set_tooltip_text(Some(&tooltip));

        let lbl = gtk::Label::builder()
            .label(&label)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(28)
            .build();
        chip.append(&lbl);

        let close_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .css_classes(["flat", "circular"])
            .valign(gtk::Align::Center)
            .tooltip_text("Remove attachment")
            .can_focus(false)
            .build();
        chip.append(&close_btn);

        let flow_child = gtk::FlowBoxChild::builder()
            .child(&chip)
            .can_focus(false)
            .build();

        let atts_clone = self.attachments.clone();
        let tags_clone = self.tags_box.clone();
        let flow_child_clone = flow_child.clone();
        let id_clone = id;
        close_btn.connect_clicked(move |_| {
            let is_empty = {
                let mut atts = atts_clone.borrow_mut();
                atts.retain(|a| a.id != id_clone);
                atts.is_empty()
            };
            tags_clone.remove(&flow_child_clone);
            tags_clone.set_visible(!is_empty);
        });

        self.tags_box.append(&flow_child);
        self.tags_box.set_visible(true);
    }

    #[must_use]
    pub fn build_payload(&self, base_text: &str) -> (String, Vec<String>) {
        let mut text = base_text.to_string();
        let mut images = Vec::new();

        let atts = self.attachments.borrow();
        if !atts.is_empty() {
            let mut text_attachments_present = false;
            for att in atts.iter() {
                if att.is_image {
                    images.push(att.content.clone());
                } else {
                    if !text_attachments_present {
                        text.push_str("\n\n--- ATTACHMENTS ---");
                        text_attachments_present = true;
                    }
                    let fence = fence_for_content(&att.content);
                    let _ = write!(
                        text,
                        "\n\n**{}**\n{fence}text\n{}\n{fence}",
                        att.label, att.content
                    );
                }
            }
        }

        (text, images)
    }

    /// Opens a single-flight native multi-file chooser for attaching photos (PNG, JPEG, WebP, GIF)
    /// and UTF-8 text/code files as chips above the input bar.
    pub fn prompt_and_attach_files<W: IsA<gtk::Widget>>(
        &self,
        parent: &W,
        entry: &gtk::TextView,
        working_dir: Option<&str>,
    ) {
        if self.picker_in_flight.replace(true) {
            return;
        }
        let guard = InFlightGuard(self.picker_in_flight.clone());

        let window = parent.root().and_downcast::<gtk::Window>();
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Add Photos & Files");

        if let Some(cwd) = working_dir
            && !cwd.trim().is_empty()
        {
            let initial_folder = gio::File::for_path(cwd);
            dialog.set_initial_folder(Some(&initial_folder));
        }

        let filters = build_attachment_file_filters();
        if let Some(default_filter) = filters.item(0).and_downcast::<gtk::FileFilter>() {
            dialog.set_default_filter(Some(&default_filter));
        }
        dialog.set_filters(Some(&filters));

        let mgr = self.clone();
        let entry_clone = entry.clone();

        dialog.open_multiple(window.as_ref(), None::<&gio::Cancellable>, move |result| {
            let _keep_until_done = guard;
            let Ok(list_model) = result else {
                // User cancelled the file dialog.
                return;
            };

            let mut files = Vec::with_capacity(list_model.n_items() as usize);
            for idx in 0..list_model.n_items() {
                if let Some(file) = list_model.item(idx).and_downcast::<gio::File>() {
                    files.push(file);
                }
            }

            if !files.is_empty() {
                mgr.ingest_files(files, &entry_clone, None, false, FileIngestSource::Picker);
            }
        });
    }

    pub fn handle_paste(&self, entry: &gtk::TextView) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let entry_clone = entry.clone();
        let clipboard_clone = clipboard.clone();
        let self_clone = self.clone();

        clipboard.read_text_async(None::<&gio::Cancellable>, move |res| {
            let mut handled = false;
            if let Ok(Some(text)) = res {
                let text_str = text.to_string();
                if !text_str.is_empty() {
                    handled = true;
                    if let Some(candidate_path) = parse_candidate_image_path(&text_str) {
                        let mgr = self_clone.clone();
                        let entry_fallback = entry_clone.clone();
                        glib::spawn_future_local(async move {
                            let label = file_display_label_from_path(&candidate_path, "Image");
                            match read_and_encode_local_image(candidate_path).await {
                                Ok(b64) => mgr.add_attachment(label, b64, true),
                                Err(Some(err_msg)) => mgr.notify(err_msg),
                                Err(None) => {
                                    // Path didn't exist on disk; treat as ordinary pasted text.
                                    insert_pasted_text(&mgr, &entry_fallback, text_str);
                                }
                            }
                        });
                    } else {
                        insert_pasted_text(&self_clone, &entry_clone, text_str);
                    }
                }
            }

            if !handled {
                let self_clone_img = self_clone.clone();
                clipboard_clone.read_texture_async(None::<&gio::Cancellable>, move |res| {
                    if let Ok(Some(texture)) = res {
                        self_clone_img.attach_texture_async(texture);
                    }
                });
            }
        });
    }

    fn attach_texture_async(&self, texture: gdk::Texture) {
        let mgr = self.clone();
        glib::spawn_future_local(async move {
            let encoded = boxxy_sys_utils::runtime()
                .spawn_blocking(move || {
                    let bytes = texture.save_to_png_bytes();
                    if bytes.len() > MAX_ATTACHMENT_FILE_BYTES {
                        return Err(format!(
                            "Pasted image exceeds the {} limit.",
                            format_limit_mb(MAX_ATTACHMENT_FILE_BYTES)
                        ));
                    }
                    Ok(glib::base64_encode(&bytes).to_string())
                })
                .await;

            match encoded {
                Ok(Ok(b64)) => mgr.add_attachment("Pasted Image".to_string(), b64, true),
                Ok(Err(msg)) => mgr.notify(msg),
                Err(e) => log::error!("Clipboard PNG encoding task panicked: {e}"),
            }
        });
    }

    #[must_use]
    pub fn handle_dropped_files(
        &self,
        files: Vec<gio::File>,
        entry: &gtk::TextView,
        working_dir: Option<&str>,
        is_incognito: bool,
    ) -> bool {
        if files.is_empty() {
            return false;
        }
        self.ingest_files(
            files,
            entry,
            working_dir,
            is_incognito,
            FileIngestSource::Drop,
        );
        true
    }

    fn ingest_files(
        &self,
        files: Vec<gio::File>,
        entry: &gtk::TextView,
        working_dir: Option<&str>,
        is_incognito: bool,
        source: FileIngestSource,
    ) {
        let mut text_tokens = Vec::new();
        let mut async_files = Vec::new();

        for file in files {
            if let Some(path) = file.path()
                && source == FileIngestSource::Drop
                && !is_supported_image_path(&path)
            {
                // For drag-and-drop inside a normal workspace session, keep `@rel/path`
                // tokens for folders or workspace paths without spaces so `file_refs.rs`
                // can list directories and register Turn-1 `replace_in_file` hashes.
                let is_dir = path.is_dir();
                let rel_opt = working_dir
                    .map(Path::new)
                    .and_then(|cwd| path.strip_prefix(cwd).ok())
                    .filter(|rel| !rel.as_os_str().is_empty());
                let chosen_path = rel_opt.unwrap_or(&path);
                let path_str = chosen_path.to_string_lossy();
                let has_spaces = path_str.chars().any(char::is_whitespace);

                if !is_incognito && !has_spaces && (is_dir || rel_opt.is_some()) {
                    text_tokens.push(format!("@{path_str}"));
                    continue;
                }
            }
            async_files.push(file);
        }

        entry.grab_focus();
        if !text_tokens.is_empty() {
            insert_tokens_at_cursor(entry, &text_tokens);
        }

        if !async_files.is_empty() {
            let mgr = self.clone();
            glib::spawn_future_local(async move {
                for file in async_files {
                    mgr.attach_single_gio_file(&file).await;
                }
            });
        }
    }

    // GIO async file stream futures run on the GTK thread-default main context (`!Send`).
    #[allow(clippy::future_not_send)]
    async fn attach_single_gio_file(&self, file: &gio::File) {
        let label = resolve_gio_file_display_name(file).await;
        let bytes = match read_gio_file_bounded(file, MAX_ATTACHMENT_FILE_BYTES).await {
            Ok(b) => b,
            Err(FileReadReject::Directory) => {
                self.notify(format!(
                    "'{label}' is a folder — use @folder in the message box to reference directories."
                ));
                return;
            }
            Err(FileReadReject::Empty) => {
                self.notify(format!("'{label}' is empty and cannot be attached."));
                return;
            }
            Err(FileReadReject::TooLarge { limit_bytes }) => {
                self.notify(format!(
                    "'{label}' exceeds the {} attachment size limit.",
                    format_limit_mb(limit_bytes)
                ));
                return;
            }
            Err(FileReadReject::IoError) => {
                self.notify(format!("Failed to read '{label}'."));
                return;
            }
        };

        if has_supported_image_magic(&bytes) {
            let encoded = boxxy_sys_utils::runtime()
                .spawn_blocking(move || glib::base64_encode(&bytes).to_string())
                .await;
            if let Ok(b64) = encoded {
                self.add_attachment(label, b64, true);
            }
            return;
        }

        // If the filename had an image extension (including unsupported ones like `.bmp`, `.svg`,
        // `.avif`, `.heic`, `.ico`, `.tiff`) or looks like binary data, reject cleanly.
        if has_unsupported_image_or_media_extension(&label) || is_probably_binary(&bytes) {
            self.notify(format!(
                "Unsupported file '{label}' — attach PNG, JPEG, WebP, or GIF images, or UTF-8 text and code files."
            ));
            return;
        }

        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if text.len() > MAX_TEXT_SNIPPET_BYTES {
            let cut = text.floor_char_boundary(MAX_TEXT_SNIPPET_BYTES);
            text.truncate(cut);
            text.push_str("\n… [truncated to 512 KB]");
            self.notify(format!(
                "Attached '{label}' (large text file truncated to 512 KB)."
            ));
        }
        self.add_attachment(label, text, false);
    }
}

fn build_attachment_file_filters() -> gio::ListStore {
    let supported = gtk::FileFilter::new();
    supported.set_name(Some("Photos, Text & Code (PNG, JPEG, WebP, GIF, Text)"));
    for mime in [
        "image/png",
        "image/jpeg",
        "image/webp",
        "image/gif",
        "text/*",
        "application/json",
        "application/ld+json",
        "application/toml",
        "application/yaml",
        "application/x-yaml",
        "application/xml",
        "application/javascript",
        "application/typescript",
        "application/x-sh",
        "application/x-shellscript",
        "application/sql",
    ] {
        supported.add_mime_type(mime);
    }
    for pat in [
        "*.png",
        "*.jpg",
        "*.jpeg",
        "*.webp",
        "*.gif",
        "*.rs",
        "*.py",
        "*.js",
        "*.mjs",
        "*.cjs",
        "*.ts",
        "*.tsx",
        "*.jsx",
        "*.go",
        "*.c",
        "*.h",
        "*.cpp",
        "*.hpp",
        "*.cc",
        "*.java",
        "*.kt",
        "*.swift",
        "*.zig",
        "*.nix",
        "*.lua",
        "*.rb",
        "*.php",
        "*.cs",
        "*.sh",
        "*.bash",
        "*.zsh",
        "*.fish",
        "*.nu",
        "*.toml",
        "*.yaml",
        "*.yml",
        "*.json",
        "*.jsonc",
        "*.jsonl",
        "*.md",
        "*.txt",
        "*.log",
        "*.csv",
        "*.tsv",
        "*.sql",
        "*.html",
        "*.css",
        "*.scss",
        "*.xml",
        "*.ui",
        "*.conf",
        "*.cfg",
        "*.ini",
        "*.env",
        "*.lock",
        "Makefile*",
        "Dockerfile*",
        "Containerfile*",
        "Justfile*",
    ] {
        supported.add_pattern(pat);
    }

    let images_only = gtk::FileFilter::new();
    images_only.set_name(Some("Images (PNG, JPEG, WebP, GIF)"));
    images_only.add_mime_type("image/png");
    images_only.add_mime_type("image/jpeg");
    images_only.add_mime_type("image/webp");
    images_only.add_mime_type("image/gif");

    let all_files = gtk::FileFilter::new();
    all_files.set_name(Some("All Files"));
    all_files.add_pattern("*");

    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&supported);
    store.append(&images_only);
    store.append(&all_files);
    store
}

fn insert_tokens_at_cursor(entry: &gtk::TextView, text_tokens: &[String]) {
    let buf = entry.buffer();
    buf.delete_selection(true, true);
    let cursor_offset = buf.cursor_position();
    let needs_leading_space = if cursor_offset > 0 {
        let prev = buf.iter_at_offset(cursor_offset - 1);
        let cur = buf.iter_at_offset(cursor_offset);
        buf.text(&prev, &cur, false)
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace())
    } else {
        false
    };
    let mut insertion = String::new();
    if needs_leading_space {
        insertion.push(' ');
    }
    insertion.push_str(&text_tokens.join(" "));
    insertion.push(' ');
    buf.insert_at_cursor(&insertion);
}

fn insert_pasted_text(mgr: &AttachmentManager, entry: &gtk::TextView, mut text_str: String) {
    if text_str.len() > 250 || text_str.contains('\n') {
        if text_str.len() > MAX_TEXT_SNIPPET_BYTES {
            let cut = text_str.floor_char_boundary(MAX_TEXT_SNIPPET_BYTES);
            text_str.truncate(cut);
            text_str.push_str("\n… [truncated]");
        }
        mgr.add_attachment("Text Snippet".to_string(), text_str, false);
    } else {
        let buf = entry.buffer();
        buf.delete_selection(true, true);
        buf.insert_at_cursor(&text_str);
    }
}

fn build_text_preview_tooltip(label: &str, content: &str) -> String {
    const MAX_PREVIEW_LINES: usize = 8;
    const MAX_PREVIEW_CHARS: usize = 320;

    let mut lines = content.lines();
    let mut preview: String = lines
        .by_ref()
        .take(MAX_PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if preview.chars().count() > MAX_PREVIEW_CHARS {
        preview = preview.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
        preview.push('…');
    } else if lines.next().is_some() {
        preview.push_str("\n…");
    }

    if label.is_empty() || label == "Text Snippet" {
        preview
    } else if preview.is_empty() {
        label.to_string()
    } else {
        format!("{label}\n\n{preview}")
    }
}

/// Computes a Markdown code fence strictly longer than any contiguous backtick run in `content`
/// (minimum 3 backticks) so embedded code blocks never close the attachment block prematurely.
fn fence_for_content(content: &str) -> String {
    let mut max_run = 0usize;
    let mut cur_run = 0usize;
    for b in content.bytes() {
        if b == b'`' {
            cur_run += 1;
            if cur_run > max_run {
                max_run = cur_run;
            }
        } else {
            cur_run = 0;
        }
    }
    "`".repeat(max_run.saturating_add(1).max(3))
}

fn is_supported_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("png")
                || ext.eq_ignore_ascii_case("jpg")
                || ext.eq_ignore_ascii_case("jpeg")
                || ext.eq_ignore_ascii_case("webp")
                || ext.eq_ignore_ascii_case("gif")
        })
}

fn has_unsupported_image_or_media_extension(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "bmp"
                    | "ico"
                    | "tif"
                    | "tiff"
                    | "avif"
                    | "heic"
                    | "heif"
                    | "jxl"
                    | "pdf"
                    | "doc"
                    | "docx"
                    | "xls"
                    | "xlsx"
                    | "ppt"
                    | "pptx"
                    | "zip"
                    | "tar"
                    | "gz"
                    | "xz"
                    | "zst"
                    | "7z"
                    | "rar"
                    | "mp3"
                    | "wav"
                    | "ogg"
                    | "flac"
                    | "mp4"
                    | "mkv"
                    | "webm"
                    | "mov"
            )
        })
}

fn has_supported_image_magic(bytes: &[u8]) -> bool {
    // PNG (`89 50 4E 47 0D 0A 1A 0A`), JPEG (`FF D8 FF`), RIFF/WebP (`RIFF....WEBP`),
    // or GIF (`GIF87a` / `GIF89a`).
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    let check_len = bytes.len().min(BINARY_PROBE_BYTES);
    bytes[..check_len].contains(&0)
}

const fn format_limit_mb(bytes: usize) -> &'static str {
    if bytes == 10 * 1024 * 1024 {
        "10 MB"
    } else {
        "size"
    }
}

fn file_display_label_from_path(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

#[allow(clippy::future_not_send)]
async fn resolve_gio_file_display_name(file: &gio::File) -> String {
    if let Some(path) = file.path() {
        return file_display_label_from_path(&path, "File");
    }
    if let Ok(info) = file
        .query_info_future(
            gio::FILE_ATTRIBUTE_STANDARD_DISPLAY_NAME,
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await
    {
        let name = info.display_name();
        if !name.trim().is_empty() {
            return name.to_string();
        }
    }
    file.basename()
        .as_deref()
        .map_or_else(|| "File".to_string(), |p| file_display_label_from_path(p, "File"))
}

fn read_local_file_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, FileReadReject> {
    let meta = std::fs::metadata(path).map_err(|_| FileReadReject::IoError)?;
    if meta.is_dir() {
        return Err(FileReadReject::Directory);
    }
    if !meta.is_file() {
        return Err(FileReadReject::IoError);
    }
    let reported_len = meta.len();
    if reported_len == 0 {
        return Err(FileReadReject::Empty);
    }
    if reported_len > max_bytes as u64 {
        return Err(FileReadReject::TooLarge {
            limit_bytes: max_bytes,
        });
    }

    let file = std::fs::File::open(path).map_err(|_| FileReadReject::IoError)?;
    let cap = usize::try_from(reported_len).unwrap_or(0).min(max_bytes);
    let mut buf = Vec::with_capacity(cap);
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| FileReadReject::IoError)?;

    if buf.is_empty() {
        return Err(FileReadReject::Empty);
    }
    if buf.len() > max_bytes {
        return Err(FileReadReject::TooLarge {
            limit_bytes: max_bytes,
        });
    }
    Ok(buf)
}

#[allow(clippy::future_not_send)]
async fn read_gio_file_bounded(
    file: &gio::File,
    max_bytes: usize,
) -> Result<Vec<u8>, FileReadReject> {
    if let Some(path) = file.path() {
        return boxxy_sys_utils::runtime()
            .spawn_blocking(move || read_local_file_bounded(&path, max_bytes))
            .await
            .map_err(|_| FileReadReject::IoError)?;
    }

    // Fallback for Flatpak portal / GVFS streams without a direct host filesystem path.
    let mut cap_hint = STREAM_CHUNK_BYTES;
    if let Ok(info) = file
        .query_info_future(
            "standard::type,standard::size",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await
    {
        if info.file_type() == gio::FileType::Directory {
            return Err(FileReadReject::Directory);
        }
        let size = info.size();
        if size > 0 {
            let size_u64 = u64::try_from(size).unwrap_or(0);
            if size_u64 > max_bytes as u64 {
                return Err(FileReadReject::TooLarge {
                    limit_bytes: max_bytes,
                });
            }
            cap_hint = usize::try_from(size_u64)
                .unwrap_or(STREAM_CHUNK_BYTES)
                .min(max_bytes);
        }
    }

    let stream = file
        .read_future(glib::Priority::DEFAULT)
        .await
        .map_err(|_| FileReadReject::IoError)?;

    let mut out = Vec::with_capacity(cap_hint);
    loop {
        let remaining = (max_bytes + 1).saturating_sub(out.len());
        let to_read = remaining.clamp(1, STREAM_CHUNK_BYTES);
        let chunk = stream
            .read_bytes_future(to_read, glib::Priority::DEFAULT)
            .await
            .map_err(|_| FileReadReject::IoError)?;
        if chunk.is_empty() {
            break;
        }
        out.extend_from_slice(&chunk);
        if out.len() > max_bytes {
            return Err(FileReadReject::TooLarge {
                limit_bytes: max_bytes,
            });
        }
    }

    if out.is_empty() {
        return Err(FileReadReject::Empty);
    }
    Ok(out)
}

/// Pure, zero-I/O check returning a candidate local `PathBuf` if `text` looks like a pasted
/// path or `file://` URI to a supported image file. Actual `stat` + `read` runs off the UI thread.
fn parse_candidate_image_path(text: &str) -> Option<PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    let path_str = trimmed.strip_prefix("file://").map_or_else(
        || trimmed.to_string(),
        |stripped| {
            glib::Uri::unescape_string(stripped, None)
                .map_or_else(|| stripped.to_string(), |g| g.to_string())
        },
    );

    let path = PathBuf::from(path_str);
    is_supported_image_path(&path).then_some(path)
}

/// Reads and base64-encodes a pasted local image path on the Tokio runtime.
/// Returns `Err(None)` when the path does not exist (so the caller falls back to pasting raw text),
/// or `Err(Some(reason))` when the file exists on disk but exceeds limits or fails magic validation.
async fn read_and_encode_local_image(path: PathBuf) -> Result<String, Option<String>> {
    let label = file_display_label_from_path(&path, "Image");
    boxxy_sys_utils::runtime()
        .spawn_blocking(move || {
            if !path.exists() {
                return Err(None);
            }
            let bytes = match read_local_file_bounded(&path, MAX_ATTACHMENT_FILE_BYTES) {
                Ok(b) => b,
                Err(FileReadReject::TooLarge { limit_bytes }) => {
                    return Err(Some(format!(
                        "'{label}' exceeds the {} attachment size limit.",
                        format_limit_mb(limit_bytes)
                    )));
                }
                Err(_) => return Err(None),
            };
            if !has_supported_image_magic(&bytes) {
                return Err(Some(format!(
                    "'{label}' is not a valid PNG, JPEG, WebP, or GIF image."
                )));
            }
            Ok(glib::base64_encode(&bytes).to_string())
        })
        .await
        .map_err(|_| None)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_png_jpeg_webp_and_gif_extensions_and_magic() {
        for ext in ["png", "PNG", "jpg", "jpeg", "webp", "gif", "GIF"] {
            let p = PathBuf::from(format!("sample.{ext}"));
            assert!(is_supported_image_path(&p), "expected {ext} to be supported");
        }
        assert!(!is_supported_image_path(Path::new("archive.zip")));
        assert!(!is_supported_image_path(Path::new("document.pdf")));

        assert!(has_supported_image_magic(b"\x89PNG\r\n\x1a\nrest"));
        assert!(has_supported_image_magic(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(has_supported_image_magic(b"RIFF\x24\x00\x00\x00WEBPVP8 "));
        assert!(has_supported_image_magic(b"GIF89a\x01\x00\x01\x00"));
        assert!(has_supported_image_magic(b"GIF87a\x01\x00\x01\x00"));
        assert!(!has_supported_image_magic(b"%PDF-1.7\n"));
        assert!(!has_supported_image_magic(b"fn main() {}\n"));
    }

    #[test]
    fn bounded_reader_enforces_size_empty_and_directory_guards() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            read_local_file_bounded(dir.path(), 1024),
            Err(FileReadReject::Directory)
        );

        let empty_path = dir.path().join("empty.txt");
        std::fs::write(&empty_path, b"").expect("write empty");
        assert_eq!(
            read_local_file_bounded(&empty_path, 1024),
            Err(FileReadReject::Empty)
        );

        let big_path = dir.path().join("oversized.txt");
        std::fs::write(&big_path, vec![b'x'; 2048]).expect("write big");
        assert_eq!(
            read_local_file_bounded(&big_path, 1024),
            Err(FileReadReject::TooLarge { limit_bytes: 1024 })
        );

        let ok_path = dir.path().join("module.rs");
        std::fs::write(&ok_path, b"pub fn run() {}\n").expect("write ok");
        assert_eq!(
            read_local_file_bounded(&ok_path, 1024).expect("ok"),
            b"pub fn run() {}\n"
        );
    }

    #[test]
    fn binary_detection_and_backtick_fences_behave_safely() {
        assert!(is_probably_binary(b"\x7fELF\x02\x01\x01\x00"));
        assert!(!is_probably_binary("fn hello() { /* 🦀 */ }\n".as_bytes()));

        assert_eq!(fence_for_content("plain code"), "```");
        assert_eq!(fence_for_content("```rust\nlet x = 1;\n```"), "````");
        assert_eq!(fence_for_content("````markdown\nnested\n````"), "`````");
    }
}
