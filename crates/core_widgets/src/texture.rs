//! Bounded `gdk::Texture` loader supporting `PNG`, `JPEG`, and `WebP` (including on
//! Linux systems or Nix closures without `webp-pixbuf-loader`).

use boxxy_assets::image::{MAX_INPUT_BYTES, decode_character_asset_rgba};
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

fn is_webp_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// Decodes raw image bytes (`PNG`, `JPEG`, or `WebP`, `<= 20 MiB`) into a [`gdk::Texture`].
///
/// Uses `boxxy-assets`'s bounded header-first decoder (`<= 8192 px` per axis, EXIF-aware)
/// for `.webp` streams and as a fallback whenever GTK's built-in loader rejects an image.
#[must_use]
pub fn decode_texture_from_bytes(raw: &[u8]) -> Option<gdk::Texture> {
    if raw.is_empty() || raw.len() > MAX_INPUT_BYTES {
        return None;
    }

    if !is_webp_magic(raw) {
        let gbytes = glib::Bytes::from(raw);
        if let Ok(tex) = gdk::Texture::from_bytes(&gbytes)
            && tex.width() <= 8192
            && tex.height() <= 8192
        {
            return Some(tex);
        }
    }

    let decoded = decode_character_asset_rgba(raw).ok()?;
    let width = i32::try_from(decoded.width).ok()?;
    let height = i32::try_from(decoded.height).ok()?;
    let stride = usize::try_from(decoded.width.checked_mul(4)?).ok()?;
    let gbytes = glib::Bytes::from_owned(decoded.rgba);
    let mem_tex = gdk::MemoryTexture::new(
        width,
        height,
        gdk::MemoryFormat::R8g8b8a8,
        &gbytes,
        stride,
    );
    Some(mem_tex.upcast::<gdk::Texture>())
}

/// Reads up to `MAX_INPUT_BYTES` from a regular file at `path` and decodes a [`gdk::Texture`].
#[must_use]
pub fn decode_texture_from_file(path: &Path) -> Option<gdk::Texture> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_INPUT_BYTES as u64 {
        return None;
    }

    let cap = usize::try_from(meta.len()).unwrap_or(0);
    let file = File::open(path).ok()?;
    let mut buf = Vec::with_capacity(cap.min(256 * 1024));
    let limit = (MAX_INPUT_BYTES as u64) + 1;
    file.take(limit).read_to_end(&mut buf).ok()?;
    if buf.len() > MAX_INPUT_BYTES {
        return None;
    }
    decode_texture_from_bytes(&buf)
}
