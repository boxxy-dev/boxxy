//! Premultiplied-alpha backdrop & preview samplers and Cairo crop-canvas rendering.

use super::geometry::{CropState, Viewport};
use boxxy_assets::image::{AvatarSource, RegionCrop};
use gtk4::cairo;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::WidgetExt;
use libadwaita as adw;

/// Maximum pixel dimension on the longest axis for the interactive Cairo canvas backdrop.
/// Keeps the software-composited `GtkDrawingArea` surface <= 4 MB (instead of 64 MB for 4K images)
/// for 120 Hz drag responsiveness while final export still crops from the full-res `AvatarSource`.
const MAX_BACKDROP_DIM: u32 = 1024;

/// Logical size of `preview_avatar` in `avatar_crop_dialog.ui` (multiplied by `scale_factor()`).
const PREVIEW_LOGICAL_SIZE: u32 = 44;

/// Pre-scaled premultiplied native-endian ARGB32 buffer prepared off the GTK main thread.
pub(super) struct BackdropBuffer {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) stride: i32,
    pub(super) data: Vec<u8>,
}

/// Builds a bounded (`<= 1024px`) premultiplied native-endian ARGB32 backdrop buffer.
/// Safe to execute inside `spawn_blocking` (contains no GTK widget references).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn build_backdrop_buffer(source: &AvatarSource) -> Option<BackdropBuffer> {
    let src_w = source.width();
    let src_h = source.height();
    if src_w == 0 || src_h == 0 {
        return None;
    }

    let (dst_w, dst_h) = if src_w <= MAX_BACKDROP_DIM && src_h <= MAX_BACKDROP_DIM {
        (src_w, src_h)
    } else {
        let scale = (f64::from(MAX_BACKDROP_DIM) / f64::from(src_w))
            .min(f64::from(MAX_BACKDROP_DIM) / f64::from(src_h));
        let dw = ((f64::from(src_w) * scale).round() as u32).clamp(1, MAX_BACKDROP_DIM);
        let dh = ((f64::from(src_h) * scale).round() as u32).clamp(1, MAX_BACKDROP_DIM);
        (dw, dh)
    };

    let w_i32 = i32::try_from(dst_w).ok()?;
    let h_i32 = i32::try_from(dst_h).ok()?;
    let stride = cairo::Format::ARgb32.stride_for_width(dst_w).ok()?;
    let stride_usize = usize::try_from(stride).ok()?;
    let mut data = vec![0_u8; stride_usize * (dst_h as usize)];
    let rgba = source.rgba();

    if dst_w == src_w && dst_h == src_h {
        let row_pixels = dst_w as usize;
        for y in 0..(dst_h as usize) {
            let src_row = y * row_pixels * 4;
            let dst_row = y * stride_usize;
            for x in 0..row_pixels {
                let si = src_row + x * 4;
                let di = dst_row + x * 4;
                let premul = premultiply_rgba(rgba[si], rgba[si + 1], rgba[si + 2], rgba[si + 3]);
                let word = pack_cairo_argb32(premul);
                data[di..di + 4].copy_from_slice(&word.to_ne_bytes());
            }
        }
    } else {
        // 4-tap premultiplied box sample when downscaling 2K/4K sources to <=1024px.
        // Averaging in premultiplied space prevents transparent [0, 0, 0, 0] pixels from
        // bleeding dark halos into adjacent opaque edges.
        let last_col = src_w - 1;
        let last_row = src_h - 1;
        for dy in 0..dst_h {
            let sy0 = ((dy * 4 + 1) * src_h / (dst_h * 4)).min(last_row);
            let sy1 = ((dy * 4 + 3) * src_h / (dst_h * 4)).min(last_row);
            let dst_row = (dy as usize) * stride_usize;
            for dx in 0..dst_w {
                let sx0 = ((dx * 4 + 1) * src_w / (dst_w * 4)).min(last_col);
                let sx1 = ((dx * 4 + 3) * src_w / (dst_w * 4)).min(last_col);
                let premul = average_4_premultiplied_rgba(rgba, src_w, sx0, sy0, sx1, sy1);
                let word = pack_cairo_argb32(premul);
                let di = dst_row + (dx as usize) * 4;
                data[di..di + 4].copy_from_slice(&word.to_ne_bytes());
            }
        }
    }

    Some(BackdropBuffer {
        width: w_i32,
        height: h_i32,
        stride,
        data,
    })
}

#[inline]
#[must_use]
#[allow(clippy::cast_possible_truncation)]
fn premultiply_rgba(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    if a == 0 {
        return [0, 0, 0, 0];
    }
    if a == 255 {
        return [r, g, b, 255];
    }
    let a16 = u16::from(a);
    let pr = ((u16::from(r) * a16 + 127) / 255) as u8;
    let pg = ((u16::from(g) * a16 + 127) / 255) as u8;
    let pb = ((u16::from(b) * a16 + 127) / 255) as u8;
    [pr, pg, pb, a]
}

#[inline]
#[must_use]
const fn pack_cairo_argb32([pr, pg, pb, a]: [u8; 4]) -> u32 {
    (a as u32) << 24 | (pr as u32) << 16 | (pg as u32) << 8 | (pb as u32)
}

#[inline]
#[must_use]
#[allow(clippy::cast_possible_truncation)]
fn average_4_premultiplied_rgba(
    rgba: &[u8],
    img_w: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> [u8; 4] {
    let sample = |x: u32, y: u32| -> [u8; 4] {
        let i = ((u64::from(y) * u64::from(img_w) + u64::from(x)) * 4) as usize;
        premultiply_rgba(rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3])
    };

    let p00 = sample(x0, y0);
    let p10 = sample(x1, y0);
    let p01 = sample(x0, y1);
    let p11 = sample(x1, y1);

    let avg = |c: usize| -> u8 {
        ((u16::from(p00[c]) + u16::from(p10[c]) + u16::from(p01[c]) + u16::from(p11[c]) + 2) >> 2)
            as u8
    };

    [avg(0), avg(1), avg(2), avg(3)]
}

/// Samples `crop` from `source` into a `preview_dim × preview_dim` premultiplied RGBA8 buffer.
#[must_use]
pub(super) fn sample_preview_premultiplied_rgba(
    source: &AvatarSource,
    crop: RegionCrop,
    preview_dim: u32,
) -> Vec<u8> {
    let dim = preview_dim.max(1);
    let mut preview_rgba = vec![0_u8; (dim * dim * 4) as usize];

    let img_w = source.width().max(1);
    let img_h = source.height().max(1);
    let max_x = img_w - 1;
    let max_y = img_h - 1;
    let rgba = source.rgba();
    let sq = crop.clamp_square_to_bounds(img_w, img_h);

    let denom = u64::from(dim) * 4;
    for py in 0..dim {
        let sy0 = (sq.y
            + u32::try_from((u64::from(py) * 4 + 1) * u64::from(sq.height) / denom).unwrap_or(0))
        .min(max_y);
        let sy1 = (sq.y
            + u32::try_from((u64::from(py) * 4 + 3) * u64::from(sq.height) / denom).unwrap_or(0))
        .min(max_y);
        for px in 0..dim {
            let sx0 = (sq.x
                + u32::try_from((u64::from(px) * 4 + 1) * u64::from(sq.width) / denom)
                    .unwrap_or(0))
            .min(max_x);
            let sx1 = (sq.x
                + u32::try_from((u64::from(px) * 4 + 3) * u64::from(sq.width) / denom)
                    .unwrap_or(0))
            .min(max_x);
            let sample = average_4_premultiplied_rgba(rgba, img_w, sx0, sy0, sx1, sy1);
            let dst_idx = ((py * dim + px) * 4) as usize;
            preview_rgba[dst_idx..dst_idx + 4].copy_from_slice(&sample);
        }
    }

    preview_rgba
}

/// Updates `avatar` with a scale-factor-aware premultiplied `MemoryTexture` sampled from `crop`.
pub(super) fn update_preview_avatar(avatar: &adw::Avatar, source: &AvatarSource, crop: RegionCrop) {
    let scale = u32::try_from(avatar.scale_factor().clamp(1, 4)).unwrap_or(2);
    let dim = PREVIEW_LOGICAL_SIZE * scale;
    let dim_i32 = i32::try_from(dim).unwrap_or(88);

    let preview_rgba = sample_preview_premultiplied_rgba(source, crop, dim);
    let bytes = glib::Bytes::from_owned(preview_rgba);
    let texture = gdk::MemoryTexture::new(
        dim_i32,
        dim_i32,
        gdk::MemoryFormat::R8g8b8a8Premultiplied,
        &bytes,
        (dim * 4) as usize,
    );
    avatar.set_custom_image(Some(&texture));
}

/// Builds a tiny 24×24 repeating 2×2 checkerboard tile once so redraws execute a single `fill()`.
#[must_use]
pub(super) fn create_checkerboard_pattern() -> Option<cairo::SurfacePattern> {
    const TILE: i32 = 12;
    let surface = cairo::ImageSurface::create(cairo::Format::Rgb24, TILE * 2, TILE * 2).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;

    cr.set_source_rgb(0.18, 0.18, 0.20);
    cr.rectangle(0.0, 0.0, f64::from(TILE * 2), f64::from(TILE * 2));
    let _ = cr.fill();

    cr.set_source_rgb(0.24, 0.24, 0.27);
    cr.rectangle(0.0, 0.0, f64::from(TILE), f64::from(TILE));
    cr.rectangle(
        f64::from(TILE),
        f64::from(TILE),
        f64::from(TILE),
        f64::from(TILE),
    );
    let _ = cr.fill();
    drop(cr);

    let pattern = cairo::SurfacePattern::create(&surface);
    pattern.set_extend(cairo::Extend::Repeat);
    pattern.set_filter(cairo::Filter::Nearest);
    Some(pattern)
}

#[allow(clippy::suboptimal_flops)]
pub(super) fn draw_crop_canvas(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    st: &CropState,
    checker_pattern: Option<&cairo::SurfacePattern>,
    backdrop_surface: Option<&(cairo::ImageSurface, f64, f64)>,
) {
    let cw = f64::from(width);
    let ch = f64::from(height);
    let vp = Viewport::compute(cw, ch, st.img_w, st.img_h);

    // 1. Tiled checkerboard background inside the image bounds
    draw_checkerboard(
        cr,
        checker_pattern,
        vp.offset_x,
        vp.offset_y,
        vp.draw_w,
        vp.draw_h,
    );

    // 2. Pre-scaled source backdrop
    if let Some((surface, surf_w, surf_h)) = backdrop_surface {
        let _ = cr.save();
        cr.rectangle(vp.offset_x, vp.offset_y, vp.draw_w, vp.draw_h);
        cr.clip();
        cr.translate(vp.offset_x, vp.offset_y);
        cr.scale(vp.draw_w / *surf_w, vp.draw_h / *surf_h);
        let _ = cr.set_source_surface(surface, 0.0, 0.0);
        let _ = cr.paint();
        let _ = cr.restore();
    }

    let sx = vp.offset_x + st.crop_x * vp.scale;
    let sy = vp.offset_y + st.crop_y * vp.scale;
    let sw = st.crop_size * vp.scale;

    // 3. Darkened scrim outside the selected square (within image bounds)
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.62);
    cr.rectangle(
        vp.offset_x,
        vp.offset_y,
        vp.draw_w,
        (sy - vp.offset_y).max(0.0),
    );
    let bottom_y = sy + sw;
    cr.rectangle(
        vp.offset_x,
        bottom_y,
        vp.draw_w,
        (vp.offset_y + vp.draw_h - bottom_y).max(0.0),
    );
    cr.rectangle(vp.offset_x, sy, (sx - vp.offset_x).max(0.0), sw);
    let right_x = sx + sw;
    cr.rectangle(
        right_x,
        sy,
        (vp.offset_x + vp.draw_w - right_x).max(0.0),
        sw,
    );
    let _ = cr.fill();

    // 4. Subtle circular avatar guide inscribed inside the square
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.32);
    cr.set_line_width(1.0);
    cr.arc(
        sx + sw * 0.5,
        sy + sw * 0.5,
        (sw * 0.5 - 1.0).max(1.0),
        0.0,
        std::f64::consts::TAU,
    );
    let _ = cr.stroke();

    // 5. Rule-of-thirds grid inside the crop square
    let grid_alpha = if st.drag_mode.is_some() { 0.38 } else { 0.18 };
    cr.set_source_rgba(1.0, 1.0, 1.0, grid_alpha);
    cr.set_line_width(1.0);
    for i in 1..=2 {
        let offset = sw * (f64::from(i) / 3.0);
        cr.move_to(sx + offset, sy);
        cr.line_to(sx + offset, sy + sw);
        cr.move_to(sx, sy + offset);
        cr.line_to(sx + sw, sy + offset);
    }
    let _ = cr.stroke();

    // 6. Outer shadow + crisp white border around the crop square
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
    cr.set_line_width(3.0);
    cr.rectangle(sx, sy, sw, sw);
    let _ = cr.stroke();

    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    cr.set_line_width(1.6);
    cr.rectangle(sx, sy, sw, sw);
    let _ = cr.stroke();

    // 7. Corner & edge resize handles
    let handles = [
        (sx, sy),
        (sx + sw, sy),
        (sx, sy + sw),
        (sx + sw, sy + sw),
        (sx + sw * 0.5, sy),
        (sx + sw * 0.5, sy + sw),
        (sx, sy + sw * 0.5),
        (sx + sw, sy + sw * 0.5),
    ];
    for (i, (hx, hy)) in handles.iter().enumerate() {
        let radius = if i < 4 { 5.5 } else { 4.2 };
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        cr.arc(*hx, *hy, radius + 1.2, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();

        cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
        cr.arc(*hx, *hy, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }
}

fn draw_checkerboard(
    cr: &cairo::Context,
    pattern: Option<&cairo::SurfacePattern>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) {
    let _ = cr.save();
    if let Some(pat) = pattern {
        let mut matrix = cairo::Matrix::identity();
        matrix.translate(-x, -y);
        pat.set_matrix(matrix);
        let _ = cr.set_source(pat);
    } else {
        cr.set_source_rgb(0.18, 0.18, 0.20);
    }
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();
    let _ = cr.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_premultiply_and_argb32_packing() {
        assert_eq!(premultiply_rgba(255, 128, 64, 0), [0, 0, 0, 0]);
        assert_eq!(premultiply_rgba(255, 128, 64, 255), [255, 128, 64, 255]);
        assert_eq!(
            pack_cairo_argb32([255, 128, 64, 255]),
            (255 << 24) | (255 << 16) | (128 << 8) | 64
        );
    }

    #[test]
    fn test_premultiplied_4tap_prevents_transparent_dark_halo() {
        // 2x2 image: top row opaque white [255, 255, 255, 255], bottom row transparent [0, 0, 0, 0]
        let rgba: [u8; 16] = [
            255, 255, 255, 255, 255, 255, 255, 255, // row 0
            0, 0, 0, 0, 0, 0, 0, 0, // row 1
        ];
        let avg = average_4_premultiplied_rgba(&rgba, 2, 0, 0, 1, 1);
        // Premultiplied 50%-transparent white is [128, 128, 128, 128] (straight RGB = 255, 255, 255),
        // NOT double-darkened [64, 64, 64, 128] gray.
        assert_eq!(avg, [128, 128, 128, 128]);
        assert!(avg[0] <= avg[3] && avg[1] <= avg[3] && avg[2] <= avg[3]);
    }

    #[test]
    fn test_backdrop_downscale_and_preview_sampler_with_synthetic_source() {
        use boxxy_assets::image::{encode_rgba_to_png, load_avatar_source};

        let width = 1200_u32;
        let height = 600_u32;
        let rgba: Vec<u8> = [200_u8, 150, 100, 255]
            .into_iter()
            .cycle()
            .take((width * height * 4) as usize)
            .collect();
        let png = encode_rgba_to_png(&rgba, width, height).expect("synthetic png");
        let source = load_avatar_source(&png).expect("decode synthetic source");

        let backdrop = build_backdrop_buffer(&source).expect("backdrop buffer");
        assert_eq!(backdrop.width, 1024);
        assert_eq!(backdrop.height, 512);
        assert_eq!(
            backdrop.data.len(),
            usize::try_from(backdrop.stride * backdrop.height).unwrap()
        );

        let preview =
            sample_preview_premultiplied_rgba(&source, RegionCrop::new(300, 0, 600, 600), 88);
        assert_eq!(preview.len(), 88 * 88 * 4);
        assert!(
            preview
                .as_chunks::<4>()
                .0
                .iter()
                .all(|&px| px == [200, 150, 100, 255])
        );
    }
}
