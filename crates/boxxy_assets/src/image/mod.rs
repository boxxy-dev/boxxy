pub mod avatar;
mod decode;
pub mod ops;
pub mod thumbnail;

pub use avatar::{
    AVATAR_SIZE, AvatarOutput, AvatarSource, MAX_DIMENSION, MAX_INPUT_BYTES, MIN_AVATAR_DIMENSION,
    crop_avatar_source, load_avatar_source, process_avatar, process_avatar_cropped,
};
pub use ops::RegionCrop;
pub use thumbnail::{
    MAX_SOURCE_DIMENSION, MAX_SOURCE_PIXELS, MAX_THUMBNAIL_INPUT_BYTES, ThumbnailOutput,
    ThumbnailSpec, decode_rgba_from_png, encode_rgba_to_png, generate_square_thumbnail_png,
    generate_thumbnail,
};
