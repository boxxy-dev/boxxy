use super::tokens::{MAX_ATTACHMENT_FILE_BYTES, MAX_ATTACHMENTS_PER_MESSAGE};
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Header window inspected for NUL bytes to reject binary files (PDFs, archives, media).
const BINARY_PROBE_BYTES: usize = 8 * 1024;
/// Chunk size used when streaming portal / GVFS files without a local host path.
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FileReadReject {
    Directory,
    Empty,
    TooLarge { limit_bytes: usize },
    IoError,
}

pub(super) fn build_attachment_file_filters() -> gio::ListStore {
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

pub(super) fn is_supported_image_path(path: &Path) -> bool {
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

pub(super) fn has_unsupported_image_or_media_extension(filename: &str) -> bool {
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

pub(super) fn has_supported_image_magic(bytes: &[u8]) -> bool {
    // PNG (`89 50 4E 47 0D 0A 1A 0A`), JPEG (`FF D8 FF`), RIFF/WebP (`RIFF....WEBP`),
    // or GIF (`GIF87a` / `GIF89a`).
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
}

pub(super) fn is_probably_binary(bytes: &[u8]) -> bool {
    let check_len = bytes.len().min(BINARY_PROBE_BYTES);
    bytes[..check_len].contains(&0)
}

pub(super) const fn format_limit_mb(bytes: usize) -> &'static str {
    if bytes == 10 * 1024 * 1024 {
        "10 MB"
    } else if bytes == 20 * 1024 * 1024 {
        "20 MB"
    } else {
        "size"
    }
}

pub(super) fn file_display_label_from_path(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

#[allow(clippy::future_not_send)]
pub(super) async fn resolve_gio_file_display_name(file: &gio::File) -> String {
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
    file.basename().as_deref().map_or_else(
        || "File".to_string(),
        |p| file_display_label_from_path(p, "File"),
    )
}

pub(super) fn read_local_file_bounded(
    path: &Path,
    max_bytes: usize,
) -> Result<Vec<u8>, FileReadReject> {
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
pub(super) async fn read_gio_file_bounded(
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

fn normalize_single_pasted_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim().trim_matches(['\'', '"']);
    if trimmed.is_empty()
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        return None;
    }

    let decoded = trimmed.strip_prefix("file://").map_or_else(
        || {
            if trimmed.contains('\\') {
                // Undo POSIX shell backslash escapes (`my\ photo.png` -> `my photo.png`).
                let mut unescaped = String::with_capacity(trimmed.len());
                let mut chars = trimmed.chars();
                while let Some(ch) = chars.next() {
                    if ch == '\\'
                        && let Some(next) = chars.next()
                    {
                        unescaped.push(next);
                    } else {
                        unescaped.push(ch);
                    }
                }
                unescaped
            } else {
                trimmed.to_string()
            }
        },
        |stripped| {
            let host_stripped = if stripped.starts_with("localhost/") {
                // Keep leading `/` for Unix absolute paths (`file://localhost/tmp/a.png` -> `/tmp/a.png`).
                &stripped["localhost".len()..]
            } else {
                stripped
            };
            glib::Uri::unescape_string(host_stripped, None)
                .map_or_else(|| host_stripped.to_string(), |g| g.to_string())
        },
    );

    let path = PathBuf::from(decoded);
    is_supported_image_path(&path).then_some(path)
}

/// Pure, zero-I/O check returning candidate local image `PathBuf`s when `text` is one or more
/// newline-separated file paths or `file://` URIs (ignoring `http(s)://` URLs and capping at
/// `MAX_ATTACHMENTS_PER_MESSAGE`). Actual `stat` + `read` runs off the UI thread.
pub(super) fn parse_candidate_image_paths(text: &str) -> Option<Vec<PathBuf>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    if !trimmed.contains('\n') {
        return normalize_single_pasted_path(trimmed).map(|p| vec![p]);
    }

    let lines: Vec<&str> = trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() || lines.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return None;
    }

    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        out.push(normalize_single_pasted_path(line)?);
    }
    Some(out)
}

/// Reads and base64-encodes a pasted local image path on the Tokio runtime.
/// Returns `Ok((b64, raw_len))` on success, `Err(None)` when the path does not exist
/// (so the caller falls back to pasting raw text), or `Err(Some(reason))` when the file
/// exists on disk but exceeds limits or fails magic validation.
pub(super) async fn read_and_encode_local_image(
    path: PathBuf,
) -> Result<(String, usize), Option<String>> {
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
            let raw_len = bytes.len();
            Ok((glib::base64_encode(&bytes).to_string(), raw_len))
        })
        .await
        .map_err(|_| None)?
}

#[cfg(test)]
mod tests {
    use super::super::tokens::fence_for_content;
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
    fn parses_file_uris_shell_escapes_and_multi_line_image_paths_while_rejecting_web_urls() {
        assert_eq!(
            parse_candidate_image_paths("https://example.com/banner.png"),
            None
        );
        assert_eq!(
            parse_candidate_image_paths("file:///tmp/diagram%20v2.png"),
            Some(vec![PathBuf::from("/tmp/diagram v2.png")])
        );
        assert_eq!(
            parse_candidate_image_paths("/tmp/my\\ screenshot.jpg"),
            Some(vec![PathBuf::from("/tmp/my screenshot.jpg")])
        );
        assert_eq!(
            parse_candidate_image_paths("/tmp/first.png\n/tmp/second.webp\n"),
            Some(vec![
                PathBuf::from("/tmp/first.png"),
                PathBuf::from("/tmp/second.webp")
            ])
        );
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
