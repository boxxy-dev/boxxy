use crate::AssetError;
use image::{imageops, DynamicImage};

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

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    #[test]
    fn test_resize() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(100, 200));
        let resize = Resize { width: 50, height: 50 };
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
}
