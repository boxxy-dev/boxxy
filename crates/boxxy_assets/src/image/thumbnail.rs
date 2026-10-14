use crate::AssetError;
use image::{ImageBuffer, ImageFormat, Limits, Rgba, imageops};
use std::io::Cursor;

/// Maximum input file size accepted for thumbnail generation (50 MB).
/// Covers high-resolution PNG/JPEG/WebP wallpapers and uncompressed screenshots.
pub const MAX_THUMBNAIL_INPUT_BYTES: usize = 50 * 1024 * 1024;

/// Maximum per-axis dimension for source images (16,384 px).
/// Covers 5K (5120×2880), 8K UHD (7680×4320), and ultrawide wallpapers.
pub const MAX_SOURCE_DIMENSION: u32 = 16_384;

/// Maximum total pixel count (64 megapixels, ~256 MB transient RGBA decode ceiling).
pub const MAX_SOURCE_PIXELS: u64 = 64_000_000;

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
    /// Original source dimensions before downscaling.
    pub original_width: u32,
    pub original_height: u32,
}

/// Validates, decodes, and downscales an image on a worker thread into a bounded RGBA8 thumbnail.
///
/// - Enforces header-level dimension and pixel limits *before* allocating full pixel buffers.
/// - Preserves aspect ratio and never upscales images smaller than `spec`.
/// - Uses `FilterType::Lanczos3` for crisp, high-clarity downsampling of 4K/8K images.
pub fn generate_thumbnail(input: &[u8], spec: ThumbnailSpec) -> Result<ThumbnailOutput, AssetError> {
    if input.len() > MAX_THUMBNAIL_INPUT_BYTES {
        return Err(AssetError::FileTooLarge(
            input.len(),
            MAX_THUMBNAIL_INPUT_BYTES,
        ));
    }

    let format = image::guess_format(input)?;

    let mut dim_reader = image::ImageReader::new(Cursor::new(input));
    dim_reader.set_format(format);
    let (orig_w, orig_h) = dim_reader.into_dimensions()?;

    if orig_w == 0 || orig_h == 0 {
        return Err(AssetError::DimensionsTooSmall(orig_w, orig_h, 1));
    }

    if orig_w > MAX_SOURCE_DIMENSION || orig_h > MAX_SOURCE_DIMENSION {
        return Err(AssetError::DimensionsTooLarge(
            orig_w,
            orig_h,
            MAX_SOURCE_DIMENSION,
        ));
    }

    let total_pixels = u64::from(orig_w) * u64::from(orig_h);
    if total_pixels > MAX_SOURCE_PIXELS {
        return Err(AssetError::DimensionsTooLarge(
            orig_w,
            orig_h,
            MAX_SOURCE_DIMENSION,
        ));
    }

    let mut decode_reader = image::ImageReader::new(Cursor::new(input));
    decode_reader.set_format(format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_SOURCE_PIXELS * 4);
    decode_reader.limits(limits);

    let decoded = decode_reader.decode()?;

    let max_w = spec.max_width.max(1);
    let max_h = spec.max_height.max(1);

    let (thumb_img, thumb_w, thumb_h) = if orig_w <= max_w && orig_h <= max_h {
        (decoded, orig_w, orig_h)
    } else {
        let scale = (f64::from(max_w) / f64::from(orig_w))
            .min(f64::from(max_h) / f64::from(orig_h));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tw = ((f64::from(orig_w) * scale).round() as u32).max(1);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let th = ((f64::from(orig_h) * scale).round() as u32).max(1);
        let resized = decoded.resize_exact(tw, th, imageops::FilterType::Lanczos3);
        (resized, tw, th)
    };

    let rgba = thumb_img.to_rgba8();

    Ok(ThumbnailOutput {
        rgba_bytes: rgba.into_raw(),
        thumb_width: thumb_w,
        thumb_height: thumb_h,
        original_width: orig_w,
        original_height: orig_h,
    })
}

/// Encodes an RGBA8 buffer to PNG bytes for disk thumbnail caching.
pub fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, AssetError> {
    let img: ImageBuffer<Rgba<u8>, &[u8]> = ImageBuffer::from_raw(width, height, rgba)
        .ok_or_else(|| AssetError::ImageEncode("invalid RGBA buffer length".to_string()))?;
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| AssetError::ImageEncode(e.to_string()))?;
    Ok(out)
}

/// Decodes a cached thumbnail PNG back into `(width, height, rgba_bytes)`.
pub fn decode_rgba_from_png(png_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), AssetError> {
    let mut reader = image::ImageReader::new(Cursor::new(png_bytes));
    reader.set_format(ImageFormat::Png);
    let decoded = reader.decode()?;
    let w = decoded.width();
    let h = decoded.height();
    Ok((w, h, decoded.to_rgba8().into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn make_png(width: u32, height: u32) -> Vec<u8> {
        let mut img = RgbImage::new(width, height);
        for p in img.pixels_mut() {
            *p = image::Rgb([40, 120, 200]);
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn downscales_5k_wallpaper_and_preserves_original_dimensions() {
        // 5120x2880 (5K wallpaper that previously failed the 4096px limit)
        let png = make_png(5120, 2880);
        let out = generate_thumbnail(&png, ThumbnailSpec::new(360, 220)).unwrap();
        assert_eq!(out.original_width, 5120);
        assert_eq!(out.original_height, 2880);
        assert!(out.thumb_width <= 360);
        assert!(out.thumb_height <= 220);
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
    fn rejects_oversized_dimensions_before_full_decode() {
        // 16385x10 exceeds MAX_SOURCE_DIMENSION
        let png = make_png(16_385, 10);
        let err = generate_thumbnail(&png, ThumbnailSpec::new(360, 220)).unwrap_err();
        assert!(matches!(err, AssetError::DimensionsTooLarge(16_385, 10, _)));
    }

    #[test]
    fn roundtrips_cached_png() {
        let rgba = vec![255u8; 4 * 4 * 4];
        let encoded = encode_rgba_to_png(&rgba, 4, 4).unwrap();
        let (w, h, decoded) = decode_rgba_from_png(&encoded).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(decoded, rgba);
    }
}
