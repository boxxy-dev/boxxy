use crate::AssetError;
use image::DynamicImage;

pub trait Analyzer: Send + Sync {
    type Output;
    fn analyze(&self, image: &DynamicImage) -> Result<Self::Output, AssetError>;
}
