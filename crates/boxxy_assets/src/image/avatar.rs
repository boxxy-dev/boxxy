use super::pipeline::Pipeline;
use super::transform::{Resize, SquareCrop};
use crate::AssetError;
use image::ImageFormat;
use std::io::Cursor;

pub const MAX_INPUT_BYTES: usize = 20 * 1024 * 1024; // 20 MB
pub const MAX_DIMENSION: u32 = 4096;
pub const AVATAR_SIZE: u32 = 256;
const MIN_DIMENSION: u32 = 16;

const ALLOWED_FORMATS: &[ImageFormat] = &[ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP];

pub struct AvatarOutput {
    /// PNG-encoded bytes, ready to write as `AVATAR.png`.
    pub png_bytes: Vec<u8>,
}

/// Full avatar processing pipeline:
/// 1. Reject if input exceeds MAX_INPUT_BYTES.
/// 2. Detect format from magic bytes; reject if not PNG/JPEG/WebP.
/// 3. Decode.
/// 4. Reject if decoded dimensions < MIN_DIMENSION or > MAX_DIMENSION.
/// 5. Center-crop to square.
/// 6. Resize to 256×256 (Lanczos3).
/// 7. Encode to PNG bytes.
pub fn process_avatar(input: &[u8]) -> Result<AvatarOutput, AssetError> {
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

    if w < MIN_DIMENSION || h < MIN_DIMENSION {
        return Err(AssetError::DimensionsTooSmall(w, h, MIN_DIMENSION));
    }

    if w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(AssetError::DimensionsTooLarge(w, h, MAX_DIMENSION));
    }

    let pipeline = Pipeline::new().add(SquareCrop).add(Resize {
        width: AVATAR_SIZE,
        height: AVATAR_SIZE,
    });

    let processed = pipeline.run(decoded)?;

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
}
