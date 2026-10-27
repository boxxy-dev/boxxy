use crate::AssetError;
use image::{DynamicImage, imageops};

pub trait Transformation: Send + Sync {
    fn apply(&self, image: DynamicImage) -> Result<DynamicImage, AssetError>;
}

/// Resize to exact pixel dimensions using Lanczos3.
pub struct Resize {
    pub width: u32,
    pub height: u32,
}

impl Transformation for Resize {
    fn apply(&self, image: DynamicImage) -> Result<DynamicImage, AssetError> {
        let resized = image.resize_exact(self.width, self.height, imageops::FilterType::Lanczos3);
        Ok(resized)
    }
}

/// Center-crop to a square (takes min(width, height), no resize).
pub struct SquareCrop;

impl Transformation for SquareCrop {
    fn apply(&self, mut image: DynamicImage) -> Result<DynamicImage, AssetError> {
        let w = image.width();
        let h = image.height();
        let size = std::cmp::min(w, h);
        let x = (w - size) / 2;
        let y = (h - size) / 2;

        let cropped = image.crop(x, y, size, size);
        Ok(cropped)
    }
}

/// Crop to a specific rectangular (or square) region in pixel coordinates.
/// Coordinates and dimensions are automatically clamped to the image bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Transformation for RegionCrop {
    fn apply(&self, image: DynamicImage) -> Result<DynamicImage, AssetError> {
        let img_w = image.width().max(1);
        let img_h = image.height().max(1);

        let x = self.x.min(img_w - 1);
        let y = self.y.min(img_h - 1);
        let w = self.width.max(1).min(img_w - x);
        let h = self.height.max(1).min(img_h - y);

        Ok(image.crop_imm(x, y, w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    #[test]
    fn test_resize() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(100, 200));
        let resize = Resize {
            width: 50,
            height: 50,
        };
        let result = resize.apply(img).unwrap();
        assert_eq!(result.width(), 50);
        assert_eq!(result.height(), 50);
    }

    #[test]
    fn test_square_crop() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(100, 200));
        let crop = SquareCrop;
        let result = crop.apply(img).unwrap();
        assert_eq!(result.width(), 100);
        assert_eq!(result.height(), 100);

        let img2 = DynamicImage::ImageRgba8(RgbaImage::new(300, 50));
        let result2 = crop.apply(img2).unwrap();
        assert_eq!(result2.width(), 50);
        assert_eq!(result2.height(), 50);
    }

    #[test]
    fn test_region_crop_and_clamping() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(200, 150));
        let crop = RegionCrop {
            x: 20,
            y: 30,
            width: 80,
            height: 80,
        };
        let result = crop.apply(img.clone()).unwrap();
        assert_eq!(result.width(), 80);
        assert_eq!(result.height(), 80);

        // Out-of-bounds crop should clamp safely without panicking
        let clamped_crop = RegionCrop {
            x: 150,
            y: 120,
            width: 500,
            height: 500,
        };
        let result2 = clamped_crop.apply(img).unwrap();
        assert_eq!(result2.width(), 50);
        assert_eq!(result2.height(), 30);
    }
}
