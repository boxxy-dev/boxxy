use super::transform::Transformation;
use crate::AssetError;
use image::DynamicImage;

pub struct Pipeline {
    transforms: Vec<Box<dyn Transformation>>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            transforms: Vec::new(),
        }
    }

    /// Append a transformation step (builder pattern).
    pub fn add(mut self, t: impl Transformation + 'static) -> Self {
        self.transforms.push(Box::new(t));
        self
    }

    /// Run all transformations in order. Synchronous; wrap in spawn_blocking if calling from async.
    pub fn run(&self, mut image: DynamicImage) -> Result<DynamicImage, AssetError> {
        for transform in &self.transforms {
            image = transform.apply(image)?;
        }
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::transform::{Resize, SquareCrop};
    use image::RgbaImage;

    #[test]
    fn test_pipeline() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(100, 200));
        let pipeline = Pipeline::new().add(SquareCrop).add(Resize {
            width: 50,
            height: 50,
        });

        let result = pipeline.run(img).unwrap();
        assert_eq!(result.width(), 50);
        assert_eq!(result.height(), 50);
    }
}
