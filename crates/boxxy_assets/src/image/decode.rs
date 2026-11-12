use crate::AssetError;
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, Limits};
use std::io::Cursor;

/// Validation constraints enforced before allocating full pixel buffers.
pub(super) struct DecodePolicy {
    pub max_input_bytes: usize,
    pub min_dimension: u32,
    pub max_dimension: u32,
    pub max_pixels: u64,
    pub allowed_formats: &'static [ImageFormat],
    pub accepted_label: &'static str,
}

/// Validates input size, magic-byte format, and header dimensions in a single pass
/// before decoding pixel data and applying EXIF orientation.
pub(super) fn decode_bounded(
    input: &[u8],
    policy: &DecodePolicy,
) -> Result<DynamicImage, AssetError> {
    if input.len() > policy.max_input_bytes {
        return Err(AssetError::FileTooLarge(
            input.len(),
            policy.max_input_bytes,
        ));
    }

    let format = match image::guess_format(input) {
        Ok(fmt) => fmt,
        Err(image::ImageError::Unsupported(_)) => {
            return Err(AssetError::UnsupportedFormat(format!(
                "unrecognized format (accepted: {})",
                policy.accepted_label
            )));
        }
        Err(err) => return Err(AssetError::ImageDecode(err)),
    };

    if !policy.allowed_formats.contains(&format) {
        return Err(AssetError::UnsupportedFormat(format!(
            "{format:?} (accepted: {})",
            policy.accepted_label
        )));
    }

    let mut reader = image::ImageReader::new(Cursor::new(input));
    reader.set_format(format);

    // Parse only header metadata first so dimension errors surface cleanly
    // before any scanline decompression or full-frame allocation occurs.
    let mut decoder = reader.into_decoder()?;
    let (raw_w, raw_h) = decoder.dimensions();

    if raw_w < policy.min_dimension || raw_h < policy.min_dimension {
        return Err(AssetError::DimensionsTooSmall(
            raw_w,
            raw_h,
            policy.min_dimension,
        ));
    }

    if raw_w > policy.max_dimension || raw_h > policy.max_dimension {
        return Err(AssetError::DimensionsTooLarge(
            raw_w,
            raw_h,
            policy.max_dimension,
        ));
    }

    let pixels = u64::from(raw_w) * u64::from(raw_h);
    if pixels > policy.max_pixels {
        return Err(AssetError::TooManyPixels {
            width: raw_w,
            height: raw_h,
            pixels,
            max_pixels: policy.max_pixels,
        });
    }

    // Attach defense-in-depth allocation limits during pixel decompression
    // (8 bytes/px ceiling accommodates transient 16-bit source channels).
    let mut limits = Limits::default();
    limits.max_image_width = Some(policy.max_dimension);
    limits.max_image_height = Some(policy.max_dimension);
    limits.max_alloc = Some(policy.max_pixels.saturating_mul(8));
    decoder.set_limits(limits)?;

    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    if orientation != Orientation::NoTransforms {
        img.apply_orientation(orientation);
    }

    Ok(img)
}

/// Encodes a tightly-packed RGBA8 slice directly to PNG bytes without intermediate wrappers.
pub(super) fn encode_rgba_slice_png(
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, AssetError> {
    if width == 0 || height == 0 {
        return Err(AssetError::ImageEncode(
            "cannot encode zero-dimension image".to_string(),
        ));
    }

    let expected_len = usize::try_from(u64::from(width) * u64::from(height) * 4)
        .map_err(|_| AssetError::ImageEncode("RGBA buffer dimensions overflow".to_string()))?;

    if rgba.len() != expected_len {
        return Err(AssetError::ImageEncode(format!(
            "invalid RGBA buffer length: got {}, expected {expected_len}",
            rgba.len()
        )));
    }

    let mut out = Vec::with_capacity((expected_len / 4).clamp(256, 256 * 1024));
    PngEncoder::new(&mut out)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|e| AssetError::ImageEncode(e.to_string()))?;
    Ok(out)
}

/// Encodes an 8-bit RGB slice directly to PNG bytes.
pub(super) fn encode_rgb_slice_png(
    rgb: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, AssetError> {
    if width == 0 || height == 0 {
        return Err(AssetError::ImageEncode(
            "cannot encode zero-dimension image".to_string(),
        ));
    }

    let expected_len = usize::try_from(u64::from(width) * u64::from(height) * 3)
        .map_err(|_| AssetError::ImageEncode("RGB buffer dimensions overflow".to_string()))?;

    if rgb.len() != expected_len {
        return Err(AssetError::ImageEncode(format!(
            "invalid RGB buffer length: got {}, expected {expected_len}",
            rgb.len()
        )));
    }

    let mut out = Vec::with_capacity((expected_len / 4).clamp(256, 256 * 1024));
    PngEncoder::new(&mut out)
        .write_image(rgb, width, height, ExtendedColorType::Rgb8)
        .map_err(|e| AssetError::ImageEncode(e.to_string()))?;
    Ok(out)
}

/// Encodes a `DynamicImage` as a compact 8-bit PNG (preserving alpha only when present).
pub(super) fn encode_dynamic_png(image: DynamicImage) -> Result<Vec<u8>, AssetError> {
    let w = image.width();
    let h = image.height();
    if image.color().has_alpha() {
        let rgba = image.into_rgba8();
        encode_rgba_slice_png(rgba.as_raw(), w, h)
    } else {
        let rgb = image.into_rgb8();
        encode_rgb_slice_png(rgb.as_raw(), w, h)
    }
}
