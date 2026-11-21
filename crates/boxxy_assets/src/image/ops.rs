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

    /// Computes the largest centered crop matching `aspect_w : aspect_h` inside `(img_w, img_h)`.
    #[must_use]
    pub fn center_aspect(img_w: u32, img_h: u32, aspect_w: u32, aspect_h: u32) -> Self {
        let w = u64::from(img_w.max(1));
        let h = u64::from(img_h.max(1));
        let aw = u64::from(aspect_w.max(1));
        let ah = u64::from(aspect_h.max(1));

        let (crop_w, crop_h) = if w * ah >= h * aw {
            // Source is wider than target aspect: pillarbox horizontally.
            let cw = ((h * aw) / ah).clamp(1, w);
            (cw, h)
        } else {
            // Source is taller than target aspect: letterbox vertically.
            let ch = ((w * ah) / aw).clamp(1, h);
            (w, ch)
        };

        let width_px = u32::try_from(crop_w).unwrap_or(1);
        let height_px = u32::try_from(crop_h).unwrap_or(1);
        let bound_w = img_w.max(1);
        let bound_h = img_h.max(1);
        Self {
            x: (bound_w - width_px) / 2,
            y: (bound_h - height_px) / 2,
            width: width_px,
            height: height_px,
        }
    }

    /// Computes a distortion-free crop matching `aspect_w : aspect_h` inside `(img_w, img_h)`,
    /// zoomed by `zoom` (`1.0..=4.0`, where `1.0` = full largest fit) and positioned around
    /// normalized focal coordinates `(focus_x, focus_y)` in `0.0..=1.0` (clamped inside image bounds).
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn focused_aspect(
        img_w: u32,
        img_h: u32,
        aspect_w: u32,
        aspect_h: u32,
        focus_x: f32,
        focus_y: f32,
        zoom: f32,
    ) -> Self {
        let fx = if focus_x.is_finite() {
            f64::from(focus_x.clamp(0.0, 1.0))
        } else {
            0.5
        };
        let fy = if focus_y.is_finite() {
            f64::from(focus_y.clamp(0.0, 1.0))
        } else {
            0.5
        };
        let z = if zoom.is_finite() {
            f64::from(zoom.clamp(1.0, 4.0))
        } else {
            1.0
        };

        let base = Self::center_aspect(img_w, img_h, aspect_w, aspect_h);
        let bound_w = img_w.max(1);
        let bound_h = img_h.max(1);
        let aw = aspect_w.max(1);
        let ah = aspect_h.max(1);

        let (crop_w, crop_h) = if (z - 1.0).abs() <= 1e-6 {
            (base.width, base.height)
        } else if aw == ah {
            let side = (f64::from(base.width) / z)
                .round()
                .clamp(1.0, f64::from(base.width)) as u32;
            (side, side)
        } else {
            let scaled_h = (f64::from(base.height) / z)
                .round()
                .clamp(1.0, f64::from(base.height)) as u32;
            let scaled_w_u64 = (u64::from(scaled_h) * u64::from(aw) + u64::from(ah) / 2)
                / u64::from(ah);
            let scaled_w = u32::try_from(scaled_w_u64.clamp(1, u64::from(base.width))).unwrap_or(1);
            (scaled_w, scaled_h)
        };

        let max_x = bound_w.saturating_sub(crop_w);
        let max_y = bound_h.saturating_sub(crop_h);
        let x = f64::mul_add(f64::from(crop_w), -0.5, fx * f64::from(bound_w))
            .round()
            .clamp(0.0, f64::from(max_x)) as u32;
        let y = f64::mul_add(f64::from(crop_h), -0.5, fy * f64::from(bound_h))
            .round()
            .clamp(0.0, f64::from(max_y)) as u32;

        Self {
            x,
            y,
            width: crop_w,
            height: crop_h,
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

/// Crops `image` to `crop` (clamped to bounds) and resizes to `target_w × target_h`
/// while preserving whether the source has an alpha channel.
#[must_use]
pub(crate) fn crop_and_resize_dynamic_rect(
    image: DynamicImage,
    crop: RegionCrop,
    target_w: u32,
    target_h: u32,
) -> DynamicImage {
    let tw = target_w.max(1);
    let th = target_h.max(1);
    let rect = crop.clamp_to_bounds(image.width(), image.height());

    if rect.x == 0
        && rect.y == 0
        && rect.width == image.width()
        && rect.height == image.height()
        && image.width() == tw
        && image.height() == th
    {
        return image;
    }

    if image.color().has_alpha() {
        let rgba = image.into_rgba8();
        let view = imageops::crop_imm(&rgba, rect.x, rect.y, rect.width, rect.height);
        DynamicImage::ImageRgba8(resize_view_two_stage(&*view, tw, th))
    } else {
        let rgb = image.into_rgb8();
        let view = imageops::crop_imm(&rgb, rect.x, rect.y, rect.width, rect.height);
        DynamicImage::ImageRgb8(resize_view_two_stage(&*view, tw, th))
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

    #[test]
    fn center_aspect_computes_16_by_9_letterbox_and_pillarbox() {
        assert_eq!(
            RegionCrop::center_aspect(1920, 1200, 16, 9),
            RegionCrop::new(0, 60, 1920, 1080)
        );
        assert_eq!(
            RegionCrop::center_aspect(2400, 900, 16, 9),
            RegionCrop::new(400, 0, 1600, 900)
        );
    }

    #[test]
    fn focused_aspect_zooms_and_clamps_near_edges_and_nan() {
        // Default 0.5, 0.5, 1.0 matches center_aspect
        assert_eq!(
            RegionCrop::focused_aspect(1536, 1024, 16, 9, 0.5, 0.5, 1.0),
            RegionCrop::center_aspect(1536, 1024, 16, 9)
        );

        // Headshot square crop zoomed 1.6x near upper center (focus_y = 0.25) on 1344x768
        let face_crop = RegionCrop::focused_aspect(1344, 768, 1, 1, 0.5, 0.25, 1.6);
        assert_eq!(face_crop.width, 480);
        assert_eq!(face_crop.height, 480);
        assert_eq!(face_crop.x, (1344 - 480) / 2);
        assert_eq!(face_crop.y, 0); // 0.25 * 768 - 240 = -48 -> clamped cleanly to 0

        // Non-finite inputs fall back safely to centered unzoomed crop
        assert_eq!(
            RegionCrop::focused_aspect(800, 600, 1, 1, f32::NAN, f32::INFINITY, f32::NAN),
            RegionCrop::center_square(800, 600)
        );
    }
}
