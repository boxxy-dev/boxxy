pub mod analyze;
pub mod avatar;
pub mod pipeline;
pub mod thumbnail;
pub mod transform;

pub use analyze::Analyzer;
pub use avatar::{
    AVATAR_SIZE, AvatarOutput, AvatarSource, MAX_DIMENSION, MAX_INPUT_BYTES, MIN_AVATAR_DIMENSION,
    crop_avatar_source, load_avatar_source, process_avatar, process_avatar_cropped,
};
pub use pipeline::Pipeline;
pub use thumbnail::{
    MAX_SOURCE_DIMENSION, MAX_SOURCE_PIXELS, MAX_THUMBNAIL_INPUT_BYTES, ThumbnailOutput,
    ThumbnailSpec, decode_rgba_from_png, encode_rgba_to_png, generate_square_thumbnail_png,
    generate_thumbnail,
};
pub use transform::{RegionCrop, Resize, SquareCrop, Transformation};
