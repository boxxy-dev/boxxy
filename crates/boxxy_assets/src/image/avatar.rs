use super::pipeline::Pipeline;
use super::transform::{RegionCrop, Resize, SquareCrop};
use crate::AssetError;
use image::{DynamicImage, ImageFormat};
use std::io::Cursor;

pub const MAX_INPUT_BYTES: usize = 20 * 1024 * 1024; // 20 MB
pub const MAX_DIMENSION: u32 = 4096;
pub const AVATAR_SIZE: u32 = 256;
pub const MIN_AVATAR_DIMENSION: u32 = 16;

const ALLOWED_FORMATS: &[ImageFormat] = &[ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP];

pub struct AvatarOutput {
    /// PNG-encoded bytes, ready to write as `AVATAR.png`.
    pub png_bytes: Vec<u8>,
}

/// Validated, decoded source image ready for interactive cropping and previewing.
#[derive(Clone)]
pub struct AvatarSource {
    pub width: u32,
    pub height: u32,
    /// Tightly-packed RGBA8 pixel buffer (`width * height * 4` bytes) for zero-copy UI preview.
    pub rgba: Vec<u8>,
    image: DynamicImage,
}

impl std::fmt::Debug for AvatarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AvatarSource")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba_len", &self.rgba.len())
            .finish_non_exhaustive()
    }
}

/// Validates and decodes an input image buffer into an `AvatarSource` for interactive cropping.
pub fn load_avatar_source(input: &[u8]) -> Result<AvatarSource, AssetError> {
    let decoded = validate_and_decode_avatar(input)?;
    let width = decoded.width();
    let height = decoded.height();
    let rgba = decoded.to_rgba8().into_raw();
    Ok(AvatarSource {
        width,
        height,
        rgba,
        image: decoded,
    })
}

/// Crops an already-decoded `AvatarSource` to a square `RegionCrop`, resizes to 256×256
/// (Lanczos3), and encodes to PNG bytes.
pub fn crop_avatar_source(
    source: &AvatarSource,
    crop: RegionCrop,
) -> Result<AvatarOutput, AssetError> {
    let size = crop.width.min(crop.height).max(1);
    let square_crop = RegionCrop {
        x: crop.x,
        y: crop.y,
        width: size,
        height: size,
    };

    let pipeline = Pipeline::new().add(square_crop).add(Resize {
        width: AVATAR_SIZE,
        height: AVATAR_SIZE,
    });

    let processed = pipeline.run(source.image.clone())?;
    encode_avatar_png(&processed)
}

/// Validates input bytes, crops to the specified square `RegionCrop`, resizes to 256×256
/// (Lanczos3), and encodes to PNG bytes.
pub fn process_avatar_cropped(
    input: &[u8],
    crop: RegionCrop,
) -> Result<AvatarOutput, AssetError> {
    let source = load_avatar_source(input)?;
    crop_avatar_source(&source, crop)
}

/// Full avatar processing pipeline:
/// 1. Reject if input exceeds MAX_INPUT_BYTES.
/// 2. Detect format from magic bytes; reject if not PNG/JPEG/WebP.
/// 3. Decode.
/// 4. Reject if decoded dimensions < MIN_AVATAR_DIMENSION or > MAX_DIMENSION.
/// 5. Center-crop to square.
/// 6. Resize to 256×256 (Lanczos3).
/// 7. Encode to PNG bytes.
pub fn process_avatar(input: &[u8]) -> Result<AvatarOutput, AssetError> {
    let decoded = validate_and_decode_avatar(input)?;

    let pipeline = Pipeline::new().add(SquareCrop).add(Resize {
        width: AVATAR_SIZE,
        height: AVATAR_SIZE,
    });

    let processed = pipeline.run(decoded)?;
    encode_avatar_png(&processed)
}

fn validate_and_decode_avatar(input: &[u8]) -> Result<DynamicImage, AssetError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(AssetError::FileTooLarge(input.len(), MAX_INPUT_BYTES));
    }

    let format = image::guess_format(input)?;
    if !ALLOWED_FORMATS.contains(&format) {
        return Err(AssetError::UnsupportedFormat(format!("{format:?}")));
    }

    let mut reader = image::ImageReader::new(Cursor::new(input));
    reader.set_format(format);
    let decoded = reader.decode()?;

    let w = decoded.width();
    let h = decoded.height();

    if w < MIN_AVATAR_DIMENSION || h < MIN_AVATAR_DIMENSION {
        return Err(AssetError::DimensionsTooSmall(w, h, MIN_AVATAR_DIMENSION));
    }

    if w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(AssetError::DimensionsTooLarge(w, h, MAX_DIMENSION));
    }

    Ok(decoded)
}

fn encode_avatar_png(processed: &DynamicImage) -> Result<AvatarOutput, AssetError> {
    let mut png_bytes = Vec::new();
    processed
        .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
        .map_err(|e| AssetError::ImageEncode(e.to_string()))?;

    Ok(AvatarOutput { png_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn create_synthetic_png(width: u32, height: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::new(width, height));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn test_process_avatar_success() {
        let mut img = RgbImage::new(300, 200);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgb([200, 50, 50]);
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

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
    fn test_unsupported_format() {
        // Create a fake GIF header
        let fake_gif = b"GIF89a...";
        let result = process_avatar(fake_gif);
        assert!(matches!(result, Err(AssetError::UnsupportedFormat(_))));
    }

    #[test]
    fn test_dimensions_too_small() {
        let tiny_png = create_synthetic_png(10, 10);
        let result = process_avatar(&tiny_png);
        assert!(matches!(
            result,
            Err(AssetError::DimensionsTooSmall(10, 10, _))
        ));
    }

    #[test]
    fn test_dimensions_too_large() {
        let huge_png = create_synthetic_png(4097, 100);
        let result = process_avatar(&huge_png);
        assert!(matches!(
            result,
            Err(AssetError::DimensionsTooLarge(4097, 100, _))
        ));
    }

    #[test]
    fn test_process_avatar_cropped_selects_exact_region() {
        // Left half pure red, right half pure blue (200 x 100)
        let mut img = RgbImage::new(200, 100);
        for (x, _y, pixel) in img.enumerate_pixels_mut() {
            if x < 100 {
                *pixel = image::Rgb([255, 0, 0]);
            } else {
                *pixel = image::Rgb([0, 0, 255]);
            }
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let source = load_avatar_source(&bytes).unwrap();
        assert_eq!(source.width, 200);
        assert_eq!(source.height, 100);
        assert_eq!(source.rgba.len(), 200 * 100 * 4);

        // Crop exclusively from the right (blue) half
        let output = crop_avatar_source(
            &source,
            RegionCrop {
                x: 110,
                y: 10,
                width: 80,
                height: 80,
            },
        )
        .unwrap();

        let decoded_out = image::load_from_memory(&output.png_bytes)
            .unwrap()
            .to_rgb8();
        assert_eq!(decoded_out.width(), 256);
        assert_eq!(decoded_out.height(), 256);
        let center_pixel = decoded_out.get_pixel(128, 128);
        assert_eq!(*center_pixel, image::Rgb([0, 0, 255]));
    }
}
