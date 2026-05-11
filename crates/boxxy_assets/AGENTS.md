# boxxy-assets

Headless, synchronous asset processing pipeline. No GTK deps, no async — CPU-bound work belongs here. Callers wrap `process_avatar` (and any future entry points) in `tokio::spawn_blocking` before calling from async GTK code.

## Module Layout

```
src/
  lib.rs          — pub mod error; pub mod image; pub mod user;
  error.rs        — AssetError (shared across all asset types)
  image/
    mod.rs        — re-exports everything below
    transform.rs  — Transformation trait + Resize + SquareCrop
    analyze.rs    — Analyzer<O> trait (extensibility hook; no impl yet)
    pipeline.rs   — Pipeline builder (ordered chain of Transformations)
    avatar.rs     — process_avatar() entry point + AvatarOutput
  user.rs         — get_user_dir() / get_user_avatar_path() helpers
```

Future asset types (audio, icons, etc.) add a new sibling module alongside `image/`. They share only `error.rs` — there is no common trait between image and audio pipelines.

## Key Types

### `Transformation` trait (`image/transform.rs`)
```rust
pub trait Transformation: Send + Sync {
    fn apply(&self, image: DynamicImage) -> Result<DynamicImage, AssetError>;
}
```
Stateless, composable step. Concrete implementations: `Resize { width, height }` (Lanczos3 exact resize) and `SquareCrop` (center-crop to `min(w, h)` square).

### `Pipeline` (`image/pipeline.rs`)
Builder that chains `Transformation` steps and runs them in order via `pipeline.run(image)`. Synchronous.

### `Analyzer<O>` trait (`image/analyze.rs`)
Read-only counterpart to `Transformation` — returns data instead of a new image. No concrete implementations currently; reserved for future use (e.g. metadata extraction).

### `process_avatar(input: &[u8]) -> Result<AvatarOutput, AssetError>` (`image/avatar.rs`)
The primary public entry point. Full validation + processing pipeline:
1. Reject if `input.len() > MAX_INPUT_BYTES` (20 MB).
2. Detect format from magic bytes — only PNG, JPEG, WebP accepted. Videos, GIFs, and unrecognised formats return `AssetError::UnsupportedFormat`.
3. Decode.
4. Reject if any dimension `< MIN_DIMENSION` (16 px) or `> MAX_DIMENSION` (4096 px). The upper bound guards against decompression bombs.
5. `SquareCrop` → center-crop to square.
6. `Resize(256, 256)` → Lanczos3.
7. Encode to PNG bytes.

`AvatarOutput` carries only `png_bytes: Vec<u8>` — ready to write as `AVATAR.png`.

### `user.rs`
```rust
pub fn get_user_dir() -> Option<PathBuf>         // ~/.config/boxxy-terminal/user/
pub fn get_user_avatar_path() -> Option<PathBuf> // ~/.config/boxxy-terminal/user/AVATAR.png
```
Creates `user/` on first call if it doesn't exist. Used by `boxxy-preferences` to locate the user's own avatar asset.

## Constants (`image/avatar.rs`)
| Constant | Value | Meaning |
|---|---|---|
| `MAX_INPUT_BYTES` | 20 MB | Hard reject before decode |
| `MAX_DIMENSION` | 4096 px | Per-axis limit after decode (~64 MB peak RAM at RGBA) |
| `AVATAR_SIZE` | 256 px | Output square size |
| `MIN_DIMENSION` | 16 px | Minimum per-axis size after decode |

## AssetError variants
- `ImageDecode` — `image` crate decode failure (via `#[from]`)
- `ImageEncode` — PNG encode failure
- `UnsupportedFormat(String)` — format not in the PNG/JPEG/WebP allowlist
- `FileTooLarge(actual, max)`
- `DimensionsTooSmall(w, h, min)`
- `DimensionsTooLarge(w, h, max)`

## Extending the pipeline
To add a new transformation: implement `Transformation` in `transform.rs` (or a new file if the module grows past 700 lines). To add a new analyzer: implement `Analyzer<YourOutput>` in `analyze.rs`. To add a new high-level entry point (e.g. `process_icon`): add a new file under `image/` following the same validation pattern as `avatar.rs`.
