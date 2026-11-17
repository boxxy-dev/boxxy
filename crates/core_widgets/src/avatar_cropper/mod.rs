//! Reusable circular avatar editor widget and interactive square crop/resize dialog.

mod dialog;
mod editable;
mod geometry;
mod render;

pub use self::dialog::AvatarCropDialog;
pub use self::editable::{EditableAvatar, prompt_and_crop_avatar};

use boxxy_assets::image::MAX_INPUT_BYTES;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::io::Read as _;
use std::path::Path;

const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Reads a regular local file up to `max_bytes`, rejecting directories, FIFOs/devices, and
/// files that exceed `max_bytes` (even if modified concurrently while reading).
fn read_local_file_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| {
        log::error!("Failed to open selected avatar image: {e}");
        "Failed to read image file.".to_string()
    })?;
    let meta = file.metadata().map_err(|e| {
        log::error!("Failed to stat selected avatar image: {e}");
        "Failed to read image file.".to_string()
    })?;
    if !meta.is_file() {
        return Err("Selected path is not a regular image file.".to_string());
    }
    let reported_len = meta.len();
    if reported_len > max_bytes as u64 {
        return Err(format!(
            "input too large: {reported_len} bytes (max {max_bytes})"
        ));
    }

    let cap = usize::try_from(reported_len).unwrap_or(0).min(max_bytes);
    let mut buf = Vec::with_capacity(cap);
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| {
            log::error!("Failed to read selected avatar image: {e}");
            "Failed to read image file.".to_string()
        })?;

    if buf.len() > max_bytes {
        return Err(format!(
            "input too large: exceeds maximum of {max_bytes} bytes"
        ));
    }
    Ok(buf)
}

/// Reads a selected `gio::File` while enforcing `MAX_INPUT_BYTES` *before* allocating
/// multi-hundred-MB buffers, supporting both local paths and Flatpak/GVFS portal URIs.
// GIO async stream futures are bound to the GTK main-context thread (`!Send`).
#[allow(clippy::future_not_send)]
pub(super) async fn read_selected_file_bytes(file: &gio::File) -> Result<Vec<u8>, String> {
    if let Some(path) = file.path() {
        return boxxy_sys_utils::runtime()
            .spawn_blocking(move || read_local_file_bounded(&path, MAX_INPUT_BYTES))
            .await
            .map_err(|join_err| {
                log::error!("Avatar file reader task panicked: {join_err}");
                "Failed to read image file.".to_string()
            })?;
    }

    // Fallback for portal / GVFS virtual files without a host filesystem path
    let mut cap_hint = STREAM_CHUNK_BYTES;
    if let Ok(info) = file
        .query_info_future(
            gio::FILE_ATTRIBUTE_STANDARD_SIZE,
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await
    {
        let size = info.size();
        if size > 0 {
            let size_u64 = u64::try_from(size).unwrap_or(0);
            if size_u64 > MAX_INPUT_BYTES as u64 {
                return Err(format!(
                    "input too large: {size_u64} bytes (max {MAX_INPUT_BYTES})"
                ));
            }
            cap_hint = usize::try_from(size_u64)
                .unwrap_or(STREAM_CHUNK_BYTES)
                .min(MAX_INPUT_BYTES);
        }
    }

    let stream = file
        .read_future(glib::Priority::DEFAULT)
        .await
        .map_err(|e| {
            log::error!("Failed to open portal avatar stream: {e}");
            "Failed to read image file.".to_string()
        })?;

    let mut out = Vec::with_capacity(cap_hint);
    loop {
        let remaining = (MAX_INPUT_BYTES + 1).saturating_sub(out.len());
        let to_read = remaining.clamp(1, STREAM_CHUNK_BYTES);
        let chunk = stream
            .read_bytes_future(to_read, glib::Priority::DEFAULT)
            .await
            .map_err(|e| {
                log::error!("Failed to read portal avatar stream: {e}");
                "Failed to read image file.".to_string()
            })?;
        if chunk.is_empty() {
            break;
        }
        out.extend_from_slice(&chunk);
        if out.len() > MAX_INPUT_BYTES {
            return Err(format!(
                "input too large: exceeds maximum of {MAX_INPUT_BYTES} bytes"
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_local_file_bounded_enforces_size_and_regular_file_checks() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Directory rejected cleanly
        let dir_err = read_local_file_bounded(dir.path(), 1024).unwrap_err();
        assert!(dir_err.contains("regular"));

        // Exact-bound file succeeds
        let ok_path = dir.path().join("valid.bin");
        std::fs::write(&ok_path, [1_u8, 2, 3, 4]).expect("write sample");
        assert_eq!(
            read_local_file_bounded(&ok_path, 4).expect("within limit"),
            vec![1, 2, 3, 4]
        );

        // Oversized file rejected before/during read
        let over_err = read_local_file_bounded(&ok_path, 3).unwrap_err();
        assert!(over_err.contains("input too large"));
    }
}
