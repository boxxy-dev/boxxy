use super::avatar::MAX_INPUT_BYTES;
use super::decode::{DecodePolicy, decode_bounded, encode_dynamic_png, encode_dynamic_webp};
use super::ops::{RegionCrop, crop_and_resize_dynamic_rect};
use crate::AssetError;
use image::ImageFormat;

/// Canonical Characters v2 landscape portrait width (`16:9`, 3× of `256 × 144` UI banner).
pub const PORTRAIT_WIDTH: u32 = 768;
/// Canonical Characters v2 landscape portrait height (`16:9`, 3× of `256 × 144` UI banner).
pub const PORTRAIT_HEIGHT: u32 = 432;
pub const PORTRAIT_ASPECT_W: u32 = 16;
pub const PORTRAIT_ASPECT_H: u32 = 9;
pub const MAX_PORTRAIT_DIMENSION: u32 = 8192;
pub const MIN_PORTRAIT_WIDTH: u32 = 64;
pub const MIN_PORTRAIT_HEIGHT: u32 = 36;

const PORTRAIT_DECODE_POLICY: DecodePolicy = DecodePolicy {
    max_input_bytes: MAX_INPUT_BYTES,
    min_dimension: MIN_PORTRAIT_HEIGHT,
    max_dimension: MAX_PORTRAIT_DIMENSION,
    // 36 MP ceiling comfortably accommodates 4K (8.3 MP), 5K (14.7 MP), and 8K-wide panoramas.
    max_pixels: 36_000_000,
    allowed_formats: &[ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP],
    accepted_label: "JPEG, PNG, WebP",
};

const CHARACTER_ASSET_DECODE_POLICY: DecodePolicy = DecodePolicy {
    max_input_bytes: MAX_INPUT_BYTES,
    min_dimension: 16,
    max_dimension: MAX_PORTRAIT_DIMENSION,
    max_pixels: 36_000_000,
    allowed_formats: &[ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP],
    accepted_label: "JPEG, PNG, WebP",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortraitOutput {
    /// Lossless WebP-encoded bytes (`768 × 432`), ready to write as `portrait.webp`.
    pub webp_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedRgbaAsset {
    pub width: u32,
    pub height: u32,
    /// Tightly-packed RGBA8 pixel buffer (`width * height * 4` bytes).
    pub rgba: Vec<u8>,
}

/// Validates and center-crops an input image to `16:9` (`768 × 432`), encoding to lossless WebP.
///
/// # Errors
/// Returns [`AssetError`] if `input` exceeds [`MAX_INPUT_BYTES`], is not PNG/JPEG/WebP,
/// fails header dimension checks (`>= 64×36`, `<= 8192`), or fails decode/encode.
pub fn process_portrait(input: &[u8]) -> Result<PortraitOutput, AssetError> {
    let decoded = decode_bounded(input, &PORTRAIT_DECODE_POLICY)?;
    if decoded.width() < MIN_PORTRAIT_WIDTH || decoded.height() < MIN_PORTRAIT_HEIGHT {
        return Err(AssetError::DimensionsTooSmall(
            decoded.width(),
            decoded.height(),
            MIN_PORTRAIT_WIDTH,
        ));
    }
    let crop = RegionCrop::center_aspect(
        decoded.width(),
        decoded.height(),
        PORTRAIT_ASPECT_W,
        PORTRAIT_ASPECT_H,
    );
    let processed = crop_and_resize_dynamic_rect(decoded, crop, PORTRAIT_WIDTH, PORTRAIT_HEIGHT);
    let webp_bytes = encode_dynamic_webp(processed)?;
    Ok(PortraitOutput { webp_bytes })
}

/// Validates input bytes, crops to `crop`, resizes to `768 × 432`, and encodes as PNG bytes.
///
/// # Errors
/// Returns [`AssetError`] on validation, decode, or PNG encode failure.
pub fn process_portrait_cropped_png(input: &[u8], crop: RegionCrop) -> Result<Vec<u8>, AssetError> {
    let decoded = decode_bounded(input, &PORTRAIT_DECODE_POLICY)?;
    if decoded.width() < MIN_PORTRAIT_WIDTH || decoded.height() < MIN_PORTRAIT_HEIGHT {
        return Err(AssetError::DimensionsTooSmall(
            decoded.width(),
            decoded.height(),
            MIN_PORTRAIT_WIDTH,
        ));
    }
    let processed = crop_and_resize_dynamic_rect(decoded, crop, PORTRAIT_WIDTH, PORTRAIT_HEIGHT);
    encode_dynamic_png(processed)
}

/// Bounded pure-Rust decoder for character `avatar` / `portrait` assets (`PNG`, `JPEG`, `WebP`).
///
/// Enables GTK UI surfaces to display `.webp` character packages reliably even on Linux
/// distributions or Nix closures that do not ship `webp-pixbuf-loader`.
///
/// # Errors
/// Returns [`AssetError`] if `input` violates size, format, or header dimension limits.
pub fn decode_character_asset_rgba(input: &[u8]) -> Result<DecodedRgbaAsset, AssetError> {
    let decoded = decode_bounded(input, &CHARACTER_ASSET_DECODE_POLICY)?;
    let rgba = decoded.into_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    Ok(DecodedRgbaAsset {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::io::Cursor;

    fn synthetic_png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_fn(w, h, |x, y| {
            #[allow(clippy::cast_possible_truncation)]
            Rgb([(x % 255) as u8, (y % 255) as u8, 128])
        });
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn process_portrait_outputs_768x432_webp_and_roundtrips_rgba() {
        let src = synthetic_png(1920, 1200);
        let out = process_portrait(&src).unwrap();
        assert_ne!(out.webp_bytes.len(), 0);

        let decoded = decode_character_asset_rgba(&out.webp_bytes).unwrap();
        assert_eq!(decoded.width, PORTRAIT_WIDTH);
        assert_eq!(decoded.height, PORTRAIT_HEIGHT);
        assert_eq!(
            decoded.rgba.len(),
            (PORTRAIT_WIDTH * PORTRAIT_HEIGHT * 4) as usize
        );
    }

    #[test]
    fn process_portrait_rejects_tiny_dimensions() {
        let tiny = synthetic_png(48, 48);
        assert!(matches!(
            process_portrait(&tiny),
            Err(AssetError::DimensionsTooSmall(48, 48, MIN_PORTRAIT_WIDTH))
        ));
    }
}
