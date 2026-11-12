use super::decode::{DecodePolicy, decode_bounded, encode_dynamic_png, encode_rgba_slice_png};
use super::ops::{RegionCrop, crop_and_resize_dynamic_square, downscale_dynamic};
use crate::AssetError;
use image::ImageFormat;

/// Maximum input file size accepted for thumbnail generation (50 MB).
/// Covers high-resolution PNG/JPEG/WebP wallpapers and uncompressed screenshots.
pub const MAX_THUMBNAIL_INPUT_BYTES: usize = 50 * 1024 * 1024;

/// Maximum per-axis dimension for source images (16,384 px).
/// Covers 5K (5120×2880), 8K UHD (7680×4320), and ultrawide wallpapers.
pub const MAX_SOURCE_DIMENSION: u32 = 16_384;

/// Maximum total pixel count (64 megapixels, ~256 MB transient RGBA decode ceiling).
pub const MAX_SOURCE_PIXELS: u64 = 64_000_000;

const THUMBNAIL_DECODE_POLICY: DecodePolicy = DecodePolicy {
    max_input_bytes: MAX_THUMBNAIL_INPUT_BYTES,
    min_dimension: 1,
    max_dimension: MAX_SOURCE_DIMENSION,
    max_pixels: MAX_SOURCE_PIXELS,
    allowed_formats: &[
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::WebP,
        ImageFormat::Gif,
        ImageFormat::Bmp,
    ],
    accepted_label: "PNG, JPEG, WebP, GIF, BMP",
};

const CACHED_PNG_DECODE_POLICY: DecodePolicy = DecodePolicy {
    max_input_bytes: MAX_THUMBNAIL_INPUT_BYTES,
    min_dimension: 1,
    max_dimension: MAX_SOURCE_DIMENSION,
    max_pixels: MAX_SOURCE_PIXELS,
    allowed_formats: &[ImageFormat::Png],
    accepted_label: "PNG",
};

/// Target bounding box for a generated thumbnail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThumbnailSpec {
    pub max_width: u32,
    pub max_height: u32,
}

impl ThumbnailSpec {
    #[must_use]
    pub const fn new(max_width: u32, max_height: u32) -> Self {
        Self {
            max_width,
            max_height,
        }
    }
}

/// Result of decoding and downscaling an image into a display-ready thumbnail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailOutput {
    /// Downscaled RGBA8 pixel buffer (`thumb_width * thumb_height * 4` bytes).
    pub rgba_bytes: Vec<u8>,
    pub thumb_width: u32,
    pub thumb_height: u32,
    /// Original source dimensions (after EXIF orientation) before downscaling.
    pub original_width: u32,
    pub original_height: u32,
}

/// Validates, decodes, applies EXIF orientation, and downscales an image into a bounded
/// RGBA8 thumbnail buffer.
///
/// - Enforces header-level dimension and pixel limits *before* allocating full pixel buffers.
/// - Preserves aspect ratio and never upscales images smaller than `spec`.
/// - Uses fast two-stage box pre-shrink + `Lanczos3` resampling for 4K/8K sources.
///
/// # Errors
/// Returns [`AssetError`] if size, format, dimension, pixel-count, or decode checks fail.
pub fn generate_thumbnail(
    input: &[u8],
    spec: ThumbnailSpec,
) -> Result<ThumbnailOutput, AssetError> {
    let decoded = decode_bounded(input, &THUMBNAIL_DECODE_POLICY)?;
    let orig_w = decoded.width();
    let orig_h = decoded.height();

    let max_w = spec.max_width.max(1);
    let max_h = spec.max_height.max(1);

    let (thumb_img, thumb_w, thumb_h) = if orig_w <= max_w && orig_h <= max_h {
        (decoded, orig_w, orig_h)
    } else {
        let scale =
            (f64::from(max_w) / f64::from(orig_w)).min(f64::from(max_h) / f64::from(orig_h));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tw = ((f64::from(orig_w) * scale).round() as u32).clamp(1, max_w);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let th = ((f64::from(orig_h) * scale).round() as u32).clamp(1, max_h);
        let resized = downscale_dynamic(decoded, tw, th);
        (resized, tw, th)
    };

    // Consume `thumb_img` by value so RGBA8 sources avoid an extra buffer clone.
    let rgba = thumb_img.into_rgba8();

    Ok(ThumbnailOutput {
        rgba_bytes: rgba.into_raw(),
        thumb_width: thumb_w,
        thumb_height: thumb_h,
        original_width: orig_w,
        original_height: orig_h,
    })
}

/// Validates, decodes, center-crops to a square via zero-copy `SubImage`, and resizes
/// to `size × size`, returning 8-bit PNG bytes.
///
/// # Errors
/// Returns [`AssetError`] on validation, decode, or PNG encode failure.
pub fn generate_square_thumbnail_png(input: &[u8], size: u32) -> Result<Vec<u8>, AssetError> {
    let decoded = decode_bounded(input, &THUMBNAIL_DECODE_POLICY)?;
    let target = size.max(1);
    let crop = RegionCrop::center_square(decoded.width(), decoded.height());
    let processed = crop_and_resize_dynamic_square(decoded, crop, target);
    encode_dynamic_png(processed)
}

/// Encodes an RGBA8 buffer to PNG bytes for disk thumbnail caching.
///
/// # Errors
/// Returns [`AssetError::ImageEncode`] if `rgba.len() != width * height * 4` or PNG encoding fails.
pub fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, AssetError> {
    encode_rgba_slice_png(rgba, width, height)
}

/// Decodes a cached thumbnail PNG back into `(width, height, rgba_bytes)` with bounded limits.
///
/// # Errors
/// Returns [`AssetError`] if `png_bytes` fails PNG validation/limits or decompression.
pub fn decode_rgba_from_png(png_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), AssetError> {
    let decoded = decode_bounded(png_bytes, &CACHED_PNG_DECODE_POLICY)?;
    let w = decoded.width();
    let h = decoded.height();
    Ok((w, h, decoded.into_rgba8().into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};
    use std::io::Cursor;

    fn make_png(width: u32, height: u32) -> Vec<u8> {
        let img = RgbImage::from_pixel(width, height, image::Rgb([40, 120, 200]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn make_png_header_only(width: u32, height: u32) -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for &b in bytes {
                crc ^= u32::from(b);
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
                }
            }
            !crc
        }

        let mut out = Vec::with_capacity(49);
        out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        out.extend_from_slice(&13u32.to_be_bytes());
        let chunk_start = out.len();
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&[8, 2, 0, 0, 0]);
        out.extend_from_slice(&crc32(&out[chunk_start..]).to_be_bytes());

        // Stub IDAT marker so PNG header reader finishes without valid scanlines
        out.extend_from_slice(&4u32.to_be_bytes());
        let idat_start = out.len();
        out.extend_from_slice(b"IDAT\x00\x00\x00\x00");
        out.extend_from_slice(&crc32(&out[idat_start..]).to_be_bytes());
        out
    }

    #[test]
    fn downscales_high_res_widescreen_and_preserves_original_dimensions() {
        // 1920x1080 (16:9 widescreen > 2x target on both axes, exercising two-stage downscale)
        let png = make_png(1920, 1080);
        let out = generate_thumbnail(&png, ThumbnailSpec::new(360, 220)).unwrap();
        assert_eq!(out.original_width, 1920);
        assert_eq!(out.original_height, 1080);
        assert_eq!(out.thumb_width, 360);
        assert_eq!(out.thumb_height, 203);
        assert_eq!(
            out.rgba_bytes.len(),
            (out.thumb_width * out.thumb_height * 4) as usize
        );
    }

    #[test]
    fn small_image_is_not_upscaled() {
        let png = make_png(120, 80);
        let out = generate_thumbnail(&png, ThumbnailSpec::new(480, 320)).unwrap();
        assert_eq!(out.original_width, 120);
        assert_eq!(out.original_height, 80);
        assert_eq!(out.thumb_width, 120);
        assert_eq!(out.thumb_height, 80);
    }

    #[test]
    fn rejects_oversized_dimensions_and_pixel_counts_before_decode() {
        // 16385x10 exceeds MAX_SOURCE_DIMENSION
        let wide_hdr = make_png_header_only(16_385, 10);
        let err = generate_thumbnail(&wide_hdr, ThumbnailSpec::new(360, 220)).unwrap_err();
        assert!(matches!(
            err,
            AssetError::DimensionsTooLarge(16_385, 10, MAX_SOURCE_DIMENSION)
        ));

        // 10000x8000 = 80 MP > 64 MP MAX_SOURCE_PIXELS while both axes <= 16384
        let mp_hdr = make_png_header_only(10_000, 8_000);
        let mp_err = generate_thumbnail(&mp_hdr, ThumbnailSpec::new(360, 220)).unwrap_err();
        assert!(matches!(
            mp_err,
            AssetError::TooManyPixels {
                width: 10_000,
                height: 8_000,
                pixels: 80_000_000,
                max_pixels: MAX_SOURCE_PIXELS
            }
        ));
    }

    #[test]
    fn roundtrips_cached_png() {
        let rgba = vec![255u8; 4 * 4 * 4];
        let encoded = encode_rgba_to_png(&rgba, 4, 4).unwrap();
        let (w, h, decoded) = decode_rgba_from_png(&encoded).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn generates_square_thumbnail_png() {
        let png = make_png(300, 180);
        let out_png = generate_square_thumbnail_png(&png, 64).unwrap();
        let (w, h, _) = decode_rgba_from_png(&out_png).unwrap();
        assert_eq!((w, h), (64, 64));
    }

    #[test]
    fn supports_gif_and_bmp_thumbnails_and_rejects_disallowed_tiff() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(32, 24, image::Rgb([10, 80, 160])));

        let mut bmp = Vec::new();
        img.write_to(&mut Cursor::new(&mut bmp), ImageFormat::Bmp)
            .unwrap();
        let bmp_out = generate_thumbnail(&bmp, ThumbnailSpec::new(64, 64)).unwrap();
        assert_eq!((bmp_out.original_width, bmp_out.original_height), (32, 24));

        // Little-endian TIFF magic bytes (`II*\0`) should be rejected cleanly as UnsupportedFormat
        let fake_tiff = b"II\x2A\x00\x08\x00\x00\x00\x00\x00";
        let err = generate_thumbnail(fake_tiff, ThumbnailSpec::new(64, 64)).unwrap_err();
        assert!(matches!(err, AssetError::UnsupportedFormat(_)));
    }

    #[test]
    fn cached_png_helpers_validate_buffer_and_header_bounds() {
        // Mismatched RGBA slice length
        let bad_encode = encode_rgba_to_png(&[0u8; 10], 4, 4).unwrap_err();
        assert!(matches!(bad_encode, AssetError::ImageEncode(_)));

        // Corrupted/tampered cached PNG header exceeding MAX_SOURCE_DIMENSION
        let oversized_cached = make_png_header_only(MAX_SOURCE_DIMENSION + 1, 16);
        let bad_decode = decode_rgba_from_png(&oversized_cached).unwrap_err();
        assert!(matches!(
            bad_decode,
            AssetError::DimensionsTooLarge(_, _, _)
        ));
    }
}
