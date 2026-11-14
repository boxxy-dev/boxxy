use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Maximum image attachment size accepted into vision payloads (20 MiB, matching `boxxy-claw`).
const MAX_IMAGE_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
/// Maximum pasted text snippet size retained in memory/history (512 KiB).
const MAX_TEXT_SNIPPET_BYTES: usize = 512 * 1024;

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
            .column_spacing(6)
            .row_spacing(4)
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
        }
    }

    #[must_use]
    pub const fn widget(&self) -> &gtk::FlowBox {
        &self.tags_box
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
            build_text_preview_tooltip(&content)
        };

        let att = Attachment {
            id: id.clone(),
            label: label.clone(),
            content,
            is_image,
        };

        self.attachments.borrow_mut().push(att);

        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chip.add_css_class("msgbar-tag");
        chip.set_tooltip_text(Some(&tooltip));

        let lbl = gtk::Label::new(Some(&label));
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
                    let _ = write!(text, "\n\n**{}**\n```text\n{}\n```", att.label, att.content);
                }
            }
        }

        (text, images)
    }

    pub fn handle_paste(&self, entry: &gtk::TextView) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let entry_clone = entry.clone();
        let clipboard_clone = clipboard.clone();
        let self_clone = self.clone();

        clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |res| {
            let mut handled = false;
            if let Ok(Some(text)) = res {
                let text_str = text.to_string();
                if !text_str.is_empty() {
                    handled = true;
                    if let Some(candidate_path) = parse_candidate_image_path(&text_str) {
                        let mgr = self_clone.clone();
                        let entry_fallback = entry_clone.clone();
                        glib::spawn_future_local(async move {
                            if let Some(b64) = read_and_encode_image_file(candidate_path).await {
                                mgr.add_attachment("Image".to_string(), b64, true);
                            } else {
                                insert_pasted_text(&mgr, &entry_fallback, text_str);
                            }
                        });
                    } else {
                        insert_pasted_text(&self_clone, &entry_clone, text_str);
                    }
                }
            }

            if !handled {
                let self_clone_img = self_clone.clone();
                clipboard_clone.read_texture_async(None::<&gtk::gio::Cancellable>, move |res| {
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
                    if (bytes.len() as u64) > MAX_IMAGE_ATTACHMENT_BYTES {
                        return None;
                    }
                    Some(glib::base64_encode(&bytes).to_string())
                })
                .await
                .ok()
                .flatten();
            if let Some(b64) = encoded {
                mgr.add_attachment("Image".to_string(), b64, true);
            }
        });
    }

    pub fn handle_dropped_files(
        &self,
        files: Vec<gtk::gio::File>,
        entry: &gtk::TextView,
        working_dir: Option<&str>,
    ) -> bool {
        if files.is_empty() {
            return false;
        }

        let mut text_tokens = Vec::new();
        for file in files {
            if let Some(path) = file.path() {
                if is_supported_image_path(&path) {
                    let mgr = self.clone();
                    glib::spawn_future_local(async move {
                        if let Some(b64) = read_and_encode_image_file(path).await {
                            mgr.add_attachment("Image".to_string(), b64, true);
                        }
                    });
                    continue;
                }

                let display_path = working_dir
                    .map(Path::new)
                    .and_then(|cwd| path.strip_prefix(cwd).ok())
                    .filter(|rel| !rel.as_os_str().is_empty())
                    .unwrap_or(&path);
                let path_str = display_path.to_string_lossy();
                if path_str.chars().any(char::is_whitespace) {
                    text_tokens.push(path_str.into_owned());
                } else {
                    text_tokens.push(format!("@{path_str}"));
                }
            } else {
                let uri = file.uri();
                if !uri.is_empty() {
                    text_tokens.push(uri.to_string());
                }
            }
        }

        entry.grab_focus();
        if !text_tokens.is_empty() {
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
        true
    }
}

fn insert_pasted_text(mgr: &AttachmentManager, entry: &gtk::TextView, mut text_str: String) {
    if text_str.len() > 250 || text_str.contains('\n') {
        if text_str.len() > MAX_TEXT_SNIPPET_BYTES {
            let mut cut = MAX_TEXT_SNIPPET_BYTES;
            while !text_str.is_char_boundary(cut) {
                cut -= 1;
            }
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

fn build_text_preview_tooltip(content: &str) -> String {
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
    preview
}

fn is_supported_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("png")
                || ext.eq_ignore_ascii_case("jpg")
                || ext.eq_ignore_ascii_case("jpeg")
                || ext.eq_ignore_ascii_case("webp")
        })
}

fn has_supported_image_magic(bytes: &[u8]) -> bool {
    // PNG (`89 50 4E 47 0D 0A 1A 0A`), JPEG (`FF D8 FF`), or RIFF/WebP (`RIFF....WEBP`).
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
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

/// Reads and base64-encodes an image file on the background Tokio runtime without re-encoding
/// compressed JPEGs/WebPs into huge uncompressed PNGs on the GTK main thread.
async fn read_and_encode_image_file(path: PathBuf) -> Option<String> {
    boxxy_sys_utils::runtime()
        .spawn(async move {
            let meta = tokio::fs::metadata(&path).await.ok()?;
            if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_IMAGE_ATTACHMENT_BYTES {
                return None;
            }
            let bytes = tokio::fs::read(&path).await.ok()?;
            if !has_supported_image_magic(&bytes) {
                return None;
            }
            tokio::task::spawn_blocking(move || glib::base64_encode(&bytes).to_string())
                .await
                .ok()
        })
        .await
        .ok()
        .flatten()
}
