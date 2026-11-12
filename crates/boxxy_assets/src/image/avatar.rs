use super::decode::{DecodePolicy, decode_bounded, encode_dynamic_png, encode_rgba_slice_png};
use super::ops::{RegionCrop, crop_and_resize_dynamic_square, crop_and_resize_rgba_square};
use crate::AssetError;
use image::{ImageFormat, RgbaImage};
use std::sync::Arc;

pub const MAX_INPUT_BYTES: usize = 20 * 1024 * 1024; // 20 MB
pub const MAX_DIMENSION: u32 = 4096;
pub const AVATAR_SIZE: u32 = 256;
pub const MIN_AVATAR_DIMENSION: u32 = 16;

const AVATAR_DECODE_POLICY: DecodePolicy = DecodePolicy {
    max_input_bytes: MAX_INPUT_BYTES,
    min_dimension: MIN_AVATAR_DIMENSION,
    max_dimension: MAX_DIMENSION,
    max_pixels: (MAX_DIMENSION as u64) * (MAX_DIMENSION as u64),
    allowed_formats: &[ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP],
    accepted_label: "JPEG, PNG, WebP",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarOutput {
    /// PNG-encoded bytes, ready to write as `AVATAR.png`.
    pub png_bytes: Vec<u8>,
}

/// Validated, EXIF-oriented RGBA8 source image shared via `Arc` for $O(1)$ UI cloning
/// and zero-copy interactive cropping.
#[derive(Clone)]
pub struct AvatarSource {
    image: Arc<RgbaImage>,
}

impl AvatarSource {
    /// Source width in pixels (after EXIF orientation).
    #[must_use]
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    /// Source height in pixels (after EXIF orientation).
    #[must_use]
    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// Tightly-packed RGBA8 pixel buffer (`width() * height() * 4` bytes).
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        self.image.as_raw()
    }
}

impl std::fmt::Debug for AvatarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AvatarSource")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("rgba_len", &self.rgba().len())
            .finish_non_exhaustive()
    }
}

/// Validates and decodes an input image buffer into an `AvatarSource` for interactive cropping.
///
/// # Errors
/// Returns [`AssetError`] if `input` exceeds [`MAX_INPUT_BYTES`], is not PNG/JPEG/WebP,
/// fails header dimension checks (`16..=4096`), or fails image decompression.
pub fn load_avatar_source(input: &[u8]) -> Result<AvatarSource, AssetError> {
    let decoded = decode_bounded(input, &AVATAR_DECODE_POLICY)?;
    Ok(AvatarSource {
        image: Arc::new(decoded.into_rgba8()),
    })
}

/// Crops an already-decoded [`AvatarSource`] to a distortion-free square [`RegionCrop`],
/// resizes to 256×256, and encodes to PNG bytes without cloning the source buffer.
///
/// # Errors
/// Returns [`AssetError::ImageEncode`] if PNG encoding fails.
pub fn crop_avatar_source(
    source: &AvatarSource,
    crop: RegionCrop,
) -> Result<AvatarOutput, AssetError> {
    let processed = crop_and_resize_rgba_square(&source.image, crop, AVATAR_SIZE);
    let png_bytes = encode_rgba_slice_png(processed.as_raw(), AVATAR_SIZE, AVATAR_SIZE)?;
    Ok(AvatarOutput { png_bytes })
}

/// Validates input bytes, crops to the specified square [`RegionCrop`], resizes to 256×256,
/// and encodes to PNG bytes.
///
/// # Errors
/// Returns [`AssetError`] on validation, decode, or PNG encode failure.
pub fn process_avatar_cropped(input: &[u8], crop: RegionCrop) -> Result<AvatarOutput, AssetError> {
    let decoded = decode_bounded(input, &AVATAR_DECODE_POLICY)?;
    let processed = crop_and_resize_dynamic_square(decoded, crop, AVATAR_SIZE);
    let png_bytes = encode_dynamic_png(processed)?;
    Ok(AvatarOutput { png_bytes })
}

/// Full center-cropped avatar processing pipeline:
/// 1. Reject if input exceeds [`MAX_INPUT_BYTES`] (20 MB).
/// 2. Detect format from magic bytes; reject if not PNG/JPEG/WebP.
/// 3. Validate header dimensions (`16..=4096`) *before* full pixel decode.
/// 4. Decode with allocation limits and apply EXIF orientation.
/// 5. Center-crop to square and resize to 256×256 via zero-copy `SubImage` view.
/// 6. Encode to 8-bit PNG bytes.
///
/// # Errors
/// Returns [`AssetError`] on validation, decode, or PNG encode failure.
pub fn process_avatar(input: &[u8]) -> Result<AvatarOutput, AssetError> {
    let decoded = decode_bounded(input, &AVATAR_DECODE_POLICY)?;
    let crop = RegionCrop::center_square(decoded.width(), decoded.height());
    let processed = crop_and_resize_dynamic_square(decoded, crop, AVATAR_SIZE);
    let png_bytes = encode_dynamic_png(processed)?;
    Ok(AvatarOutput { png_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, RgbImage};
    use std::io::Cursor;

    /// Builds a minimal 49-byte PNG (`signature + IHDR + stub IDAT header`) with arbitrary `(w, h)`.
    /// Contains no valid compressed scanlines, proving oversized/undersized headers are rejected
    /// *before* attempting pixel buffer allocation or scanline decompression.
    fn make_png_header_only(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(49);
        out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        out.extend_from_slice(&13u32.to_be_bytes());
        let chunk_start = out.len();
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
        let crc = crc32_ieee(&out[chunk_start..]);
        out.extend_from_slice(&crc.to_be_bytes());

        // Stub IDAT marker so PNG header reader finishes without valid scanlines
        out.extend_from_slice(&4u32.to_be_bytes());
        let idat_start = out.len();
        out.extend_from_slice(b"IDAT\x00\x00\x00\x00");
        let idat_crc = crc32_ieee(&out[idat_start..]);
        out.extend_from_slice(&idat_crc.to_be_bytes());
        out
    }

    fn crc32_ieee(bytes: &[u8]) -> u32 {
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

    fn make_rgb_png(width: u32, height: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(
            width,
            height,
            image::Rgb([200, 50, 50]),
        ));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// Injects an EXIF APP1 segment with the given TIFF orientation tag (`1..=8`) right after SOI.
    fn inject_jpeg_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        assert_eq!(&jpeg[..2], b"\xFF\xD8");
        let mut app1_payload = Vec::new();
        app1_payload.extend_from_slice(b"Exif\0\0");
        // Little-endian TIFF header + 1 IFD entry (tag 0x0112 = Orientation, type SHORT = 3, count = 1)
        app1_payload.extend_from_slice(b"II\x2A\x00\x08\x00\x00\x00");
        app1_payload.extend_from_slice(&1u16.to_le_bytes());
        app1_payload.extend_from_slice(&0x0112u16.to_le_bytes());
        app1_payload.extend_from_slice(&3u16.to_le_bytes());
        app1_payload.extend_from_slice(&1u32.to_le_bytes());
        app1_payload.extend_from_slice(&orientation.to_le_bytes());
        app1_payload.extend_from_slice(&0u16.to_le_bytes());
        app1_payload.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0

        let seg_len = u16::try_from(app1_payload.len() + 2).unwrap();
        let mut out = Vec::with_capacity(jpeg.len() + app1_payload.len() + 4);
        out.extend_from_slice(&jpeg[..2]);
        out.extend_from_slice(b"\xFF\xE1");
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(&app1_payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn test_process_avatar_success() {
        let bytes = make_rgb_png(300, 200);
        let output = process_avatar(&bytes).unwrap();
        let decoded_out = image::load_from_memory(&output.png_bytes).unwrap();
        assert_eq!(decoded_out.width(), 256);
        assert_eq!(decoded_out.height(), 256);
    }

    #[test]
    fn test_file_too_large() {
        let fake_large_input = vec![0u8; MAX_INPUT_BYTES + 1];
        let result = process_avatar(&fake_large_input);
        assert!(matches!(result, Err(AssetError::FileTooLarge(_, _))));
    }

    #[test]
    fn test_unsupported_known_and_unknown_formats() {
        let fake_gif = b"GIF89a\x10\x00\x10\x00\x00\x00\x00;";
        assert!(matches!(
            process_avatar(fake_gif),
            Err(AssetError::UnsupportedFormat(_))
        ));

        // Non-image / video container header (ISO BMFF / MP4) must also report UnsupportedFormat
        let fake_mp4 = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom";
        assert!(matches!(
            process_avatar(fake_mp4),
            Err(AssetError::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn test_dimensions_rejected_at_header_before_decode() {
        let tiny_header = make_png_header_only(10, 10);
        assert!(matches!(
            process_avatar(&tiny_header),
            Err(AssetError::DimensionsTooSmall(10, 10, MIN_AVATAR_DIMENSION))
        ));

        let bomb_header = make_png_header_only(4097, 4096);
        assert!(matches!(
            process_avatar(&bomb_header),
            Err(AssetError::DimensionsTooLarge(4097, 4096, MAX_DIMENSION))
        ));
    }

    #[test]
    fn test_process_avatar_cropped_selects_exact_region() {
        let mut img = RgbImage::new(200, 100);
        for (x, _y, pixel) in img.enumerate_pixels_mut() {
            *pixel = if x < 100 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            };
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let source = load_avatar_source(&bytes).unwrap();
        assert_eq!(source.width(), 200);
        assert_eq!(source.height(), 100);
        assert_eq!(source.rgba().len(), 200 * 100 * 4);

        let output = crop_avatar_source(&source, RegionCrop::new(110, 10, 80, 80)).unwrap();
        let decoded_out = image::load_from_memory(&output.png_bytes)
            .unwrap()
            .into_rgb8();
        assert_eq!(decoded_out.dimensions(), (256, 256));
        assert_eq!(*decoded_out.get_pixel(128, 128), image::Rgb([0, 0, 255]));
    }

    #[test]
    fn test_exif_orientation_applied_on_avatar_source() {
        // Encode a 60x30 landscape JPEG (top half green, bottom half red),
        // then tag it with EXIF Orientation = 6 (Rotate 90 CW -> 30x60 portrait).
        let mut img = RgbImage::new(60, 30);
        for (_x, y, p) in img.enumerate_pixels_mut() {
            *p = if y < 15 {
                image::Rgb([0, 220, 0])
            } else {
                image::Rgb([220, 0, 0])
            };
        }
        let mut jpeg = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();
        let oriented_jpeg = inject_jpeg_exif_orientation(&jpeg, 6);

        let source = load_avatar_source(&oriented_jpeg).unwrap();
        assert_eq!((source.width(), source.height()), (30, 60));
    }

    #[test]
    fn test_process_avatar_preserves_alpha_and_handles_exact_256_fast_path() {
        let mut rgba = RgbaImage::new(AVATAR_SIZE, AVATAR_SIZE);
        for (x, y, p) in rgba.enumerate_pixels_mut() {
            let alpha = if (x + y) % 2 == 0 { 0 } else { 255 };
            *p = image::Rgba([30, 140, 240, alpha]);
        }
        let mut input_png = Vec::new();
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut Cursor::new(&mut input_png), ImageFormat::Png)
            .unwrap();

        let output = process_avatar(&input_png).unwrap();
        let decoded = image::load_from_memory(&output.png_bytes).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (AVATAR_SIZE, AVATAR_SIZE)
        );
        assert!(decoded.color().has_alpha());
        assert_eq!(decoded.into_rgba8().get_pixel(0, 0)[3], 0);
    }
}
