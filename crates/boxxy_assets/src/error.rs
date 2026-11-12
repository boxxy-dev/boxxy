use thiserror::Error;

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("image decode failed: {0}")]
    ImageDecode(#[from] image::ImageError),
    #[error("image encode failed: {0}")]
    ImageEncode(String),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("input too large: {0} bytes (max {1})")]
    FileTooLarge(usize, usize),
    #[error("dimensions too small: {0}x{1} (min {2}x{2})")]
    DimensionsTooSmall(u32, u32, u32),
    #[error("dimensions too large: {0}x{1} (max {2}x{2})")]
    DimensionsTooLarge(u32, u32, u32),
    #[error("pixel count too large: {width}x{height} ({pixels} px, max {max_pixels} px)")]
    TooManyPixels {
        width: u32,
        height: u32,
        pixels: u64,
        max_pixels: u64,
    },
}
