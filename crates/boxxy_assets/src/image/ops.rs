use image::{DynamicImage, GenericImageView, ImageBuffer, Pixel, RgbaImage, imageops};

/// Rectangular or square crop region in source pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl RegionCrop {
    #[must_use]
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Computes a centered square crop (`min(img_w, img_h)`) within `(img_w, img_h)`.
    #[must_use]
    pub fn center_square(img_w: u32, img_h: u32) -> Self {
        let w = img_w.max(1);
        let h = img_h.max(1);
        let side = w.min(h);
        Self {
            x: (w - side) / 2,
            y: (h - side) / 2,
            width: side,
            height: side,
        }
    }

    /// Clamps a rectangular crop region to fit within `(img_w, img_h)`.
    #[must_use]
    pub fn clamp_to_bounds(self, img_w: u32, img_h: u32) -> Self {
        let w_bound = img_w.max(1);
        let h_bound = img_h.max(1);
        let x = self.x.min(w_bound - 1);
        let y = self.y.min(h_bound - 1);
        let width = self.width.max(1).min(w_bound - x);
        let height = self.height.max(1).min(h_bound - y);
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Clamps this region into a guaranteed distortion-free square inside `(img_w, img_h)`,
    /// shifting `(x, y)` inward if rounding pushed `x + side` or `y + side` 1 px past an edge.
    #[must_use]
    pub fn clamp_square_to_bounds(self, img_w: u32, img_h: u32) -> Self {
        let w_bound = img_w.max(1);
        let h_bound = img_h.max(1);
        let max_side = w_bound.min(h_bound);
        let side = self.width.min(self.height).clamp(1, max_side);
        let x = self.x.min(w_bound - side);
        let y = self.y.min(h_bound - side);
        Self {
            x,
            y,
            width: side,
            height: side,
        }
    }
}

/// Crops `source` to a guaranteed square `crop` and resizes to `target_size × target_size`
/// without cloning the source image or allocating a full-resolution cropped intermediate buffer.
#[must_use]
pub(crate) fn crop_and_resize_rgba_square(
    source: &RgbaImage,
    crop: RegionCrop,
    target_size: u32,
) -> RgbaImage {
    let target = target_size.max(1);
    let sq = crop.clamp_square_to_bounds(source.width(), source.height());
    let view = imageops::crop_imm(source, sq.x, sq.y, sq.width, sq.height);
    resize_view_two_stage(&*view, target, target)
}

/// Crops `image` to a guaranteed square `crop` and resizes to `target_size × target_size`
/// while preserving whether the source has an alpha channel.
#[must_use]
pub(crate) fn crop_and_resize_dynamic_square(
    image: DynamicImage,
    crop: RegionCrop,
    target_size: u32,
) -> DynamicImage {
    let target = target_size.max(1);
    let sq = crop.clamp_square_to_bounds(image.width(), image.height());

    // Zero-cost fast path when the image is already the exact target square.
    if sq.x == 0 && sq.y == 0 && image.width() == target && image.height() == target {
        return image;
    }

    if image.color().has_alpha() {
        let rgba = image.into_rgba8();
        let view = imageops::crop_imm(&rgba, sq.x, sq.y, sq.width, sq.height);
        DynamicImage::ImageRgba8(resize_view_two_stage(&*view, target, target))
    } else {
        let rgb = image.into_rgb8();
        let view = imageops::crop_imm(&rgb, sq.x, sq.y, sq.width, sq.height);
        DynamicImage::ImageRgb8(resize_view_two_stage(&*view, target, target))
    }
}

/// Downscales `image` to `(target_w, target_h)` using a fast two-stage pipeline:
/// - For large reductions (`> 2x` on both axes, e.g. 4K/8K -> preview), first box-averages to
///   `2 * target` in integer math (visiting each input pixel once), then finishes with `Lanczos3`.
/// - Otherwise applies `Lanczos3` directly for crisp resampling without integer-binning artifacts.
#[must_use]
pub(crate) fn downscale_dynamic(image: DynamicImage, target_w: u32, target_h: u32) -> DynamicImage {
    let tw = target_w.max(1);
    let th = target_h.max(1);
    if image.width() == tw && image.height() == th {
        return image;
    }

    let pre_w = tw.saturating_mul(2);
    let pre_h = th.saturating_mul(2);
    let base = if image.width() > pre_w && image.height() > pre_h {
        image.thumbnail_exact(pre_w, pre_h)
    } else {
        image
    };
    base.resize_exact(tw, th, imageops::FilterType::Lanczos3)
}

fn resize_view_two_stage<I>(
    view: &I,
    target_w: u32,
    target_h: u32,
) -> ImageBuffer<I::Pixel, Vec<u8>>
where
    I: GenericImageView,
    I::Pixel: Pixel<Subpixel = u8> + 'static,
{
    let pre_w = target_w.saturating_mul(2);
    let pre_h = target_h.saturating_mul(2);
    if view.width() > pre_w && view.height() > pre_h {
        let pre = imageops::thumbnail(view, pre_w, pre_h);
        imageops::resize(&pre, target_w, target_h, imageops::FilterType::Lanczos3)
    } else {
        imageops::resize(view, target_w, target_h, imageops::FilterType::Lanczos3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_square_computes_centered_region() {
        assert_eq!(
            RegionCrop::center_square(100, 200),
            RegionCrop::new(0, 50, 100, 100)
        );
        assert_eq!(
            RegionCrop::center_square(300, 50),
            RegionCrop::new(125, 0, 50, 50)
        );
    }

    #[test]
    fn clamp_square_preserves_square_aspect_at_right_and_bottom_edges() {
        // Rounding in UI cropper can produce x + size = 201 on a 200px-wide image;
        // clamp_square_to_bounds shifts x to 120 instead of squashing width to 79.
        let crop = RegionCrop::new(121, 75, 80, 80);
        let clamped = crop.clamp_square_to_bounds(200, 150);
        assert_eq!(clamped, RegionCrop::new(120, 70, 80, 80));
        assert_eq!(clamped.width, clamped.height);
    }

    #[test]
    fn clamp_to_bounds_handles_oversized_rect() {
        let crop = RegionCrop::new(150, 120, 500, 500);
        assert_eq!(
            crop.clamp_to_bounds(200, 150),
            RegionCrop::new(150, 120, 50, 30)
        );
    }
}
