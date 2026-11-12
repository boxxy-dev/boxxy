# boxxy-assets

Headless, synchronous asset processing crate. No GTK deps, no async — CPU-bound work belongs here. Callers wrap entry points in `tokio::task::spawn_blocking` before calling from async or GTK UI code.

## Module Layout

```
src/
  lib.rs          — pub mod error; pub mod image; pub mod user;
  error.rs        — AssetError (shared across all asset types)
  image/
    mod.rs        — re-exports public types and entry points
    decode.rs     — internal DecodePolicy + single-pass decode_bounded() + direct PngEncoder helpers
    ops.rs        — RegionCrop + zero-copy SubImage square crop-and-resize + two-stage downscale
    avatar.rs     — process_avatar(), load_avatar_source(), crop_avatar_source(), process_avatar_cropped() + AvatarSource / AvatarOutput
    thumbnail.rs  — generate_thumbnail(), generate_square_thumbnail_png(), encode_rgba_to_png(), decode_rgba_from_png() + ThumbnailSpec / ThumbnailOutput
  user.rs         — pure, zero-I/O get_user_dir() / get_user_avatar_path() path resolvers
```

Future asset types (audio, icons, etc.) add a new sibling module alongside `image/`, sharing `error.rs`.

## Core Design Principles

1. **Single-Pass Bounded Decoding (`image/decode.rs`):**
   Every image entry point defines a `const DecodePolicy` (`max_input_bytes`, `min_dimension`, `max_dimension`, `max_pixels`, `allowed_formats`). `decode_bounded()` verifies byte size, checks magic bytes against the explicit format allowlist (mapping both disallowed and unrecognized non-image formats to `AssetError::UnsupportedFormat`), parses header dimensions via `into_decoder()` *before* allocating pixel buffers, attaches `image::Limits`, decodes, and applies EXIF orientation (`Orientation`).
2. **Zero-Copy Views & $O(1)$ Shared Sources (`image/ops.rs`, `image/avatar.rs`):**
   - `AvatarSource` wraps a single `Arc<RgbaImage>` (exposing `width()`, `height()`, and `rgba()`), avoiding duplicate pixel buffers and making UI-thread `.clone()` $O(1)$.
   - Cropping + resizing operates directly on borrowed `imageops::crop_imm` `SubImage` views without materializing multi-megabyte intermediate cropped images.
   - `RegionCrop::clamp_square_to_bounds` guarantees distortion-free square crops even when UI floating-point rounding lands 1 px past an edge.
   - High-ratio downscales (`> 2x` on both axes, e.g. 4K/5K/8K sources) use a two-stage integer box pre-shrink (`2 * target`) followed by `FilterType::Lanczos3`, delivering crisp anti-aliased output without blocking worker threads.

## Public Entry Points

### Avatars (`image/avatar.rs`)
- `process_avatar(input: &[u8]) -> Result<AvatarOutput, AssetError>`: validates `<= 20 MB`, allows `PNG, JPEG, WebP`, checks header dimensions (`16..=4096`), applies EXIF orientation, center-crops to square, resizes to `256×256`, and encodes to 8-bit PNG bytes.
- `load_avatar_source(input: &[u8]) -> Result<AvatarSource, AssetError>` & `crop_avatar_source(&AvatarSource, RegionCrop) -> Result<AvatarOutput, AssetError>`: interactive UI cropper workflow (`AvatarCropDialog`).
- `process_avatar_cropped(input: &[u8], RegionCrop) -> Result<AvatarOutput, AssetError>`: one-shot custom square crop + resize.

| Constant | Value | Meaning |
|---|---|---|
| `MAX_INPUT_BYTES` | 20 MB | Hard reject before decode |
| `MAX_DIMENSION` | 4096 px | Per-axis header limit before decode |
| `AVATAR_SIZE` | 256 px | Output square size |
| `MIN_AVATAR_DIMENSION` | 16 px | Minimum per-axis header dimension |

### Thumbnails (`image/thumbnail.rs`)
- `generate_thumbnail(input: &[u8], spec: ThumbnailSpec) -> Result<ThumbnailOutput, AssetError>`: validates `<= 50 MB`, allows `PNG, JPEG, WebP, GIF, BMP`, enforces `<= 16,384 px` per axis and `<= 64 MP` total pixels at header time, applies EXIF orientation, downscales without upscaling smaller images, and returns compact RGBA8 bytes + oriented original dimensions.
- `generate_square_thumbnail_png(input: &[u8], size: u32) -> Result<Vec<u8>, AssetError>`: center-crops and resizes attachments to `size × size` PNG bytes.
- `encode_rgba_to_png` / `decode_rgba_from_png`: bounded PNG cache helpers for `boxxy-viewer`'s Tier-2 `.bxt` disk cache.

### User Paths (`user.rs`)
```rust
pub fn get_user_dir() -> PathBuf         // <config_dir>/user
pub fn get_user_avatar_path() -> PathBuf // <config_dir>/user/AVATAR.png
```
Pure, infallible path resolvers that perform **zero disk I/O** (safe to call on the GTK main thread). Callers writing `AVATAR.png` create `get_user_dir()` asynchronously via `tokio::fs::create_dir_all`.

## `AssetError` Variants (`error.rs`)
- `ImageDecode(image::ImageError)` — format stream decode failure
- `ImageEncode(String)` — PNG encode or buffer-size validation failure
- `UnsupportedFormat(String)` — format not in the policy allowlist or unrecognized magic bytes
- `FileTooLarge(actual, max)`
- `DimensionsTooSmall(w, h, min)`
- `DimensionsTooLarge(w, h, max)`
- `TooManyPixels { width, height, pixels, max_pixels }`

## Extending `boxxy-assets`
To add a new image workflow (e.g. `process_icon`): define a `const DecodePolicy` in a new module under `image/`, call `decode_bounded(input, &POLICY)?`, and compose operations from `ops.rs` + `decode.rs`.
