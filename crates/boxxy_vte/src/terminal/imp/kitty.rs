use crate::engine::ansi::Color;
use crate::engine::term::RenderState;
use crate::engine::term::cell::Cell;
use gtk4::glib;
use gtk4::prelude::*;

/// Unicode Private Use Area codepoint reserved by the Kitty Graphics Protocol
/// for virtual image placements (`U=1`).
pub(crate) const KITTY_UNICODE_PLACEHOLDER: char = '\u{10EEEE}';

#[inline]
fn cell_matches_kitty_image(cell: &Cell, image_id: u32) -> bool {
    if cell.c != KITTY_UNICODE_PLACEHOLDER {
        return false;
    }
    match cell.fg {
        Color::Spec(rgb) => {
            let fg_id = ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | (rgb.b as u32);
            (image_id & 0x00FF_FFFF) == fg_id
        }
        Color::Indexed(idx) => (image_id & 0xFF) == (idx as u32),
        Color::Named(named) if (named as usize) < 256 => (image_id & 0xFF) == (named as u32),
        _ => false,
    }
}

/// Find the viewport bounding box `(min_row, min_col, cols, rows)` of `\u{10EEEE}`
/// placeholder cells matching `image_id` in `state.cells`.
fn find_unicode_placeholder_bounds(
    state: &RenderState,
    image_id: u32,
) -> Option<(i32, usize, u32, u32)> {
    if state.columns == 0 || state.screen_lines == 0 {
        return None;
    }
    let mut min_row = usize::MAX;
    let mut max_row = 0usize;
    let mut min_col = usize::MAX;
    let mut max_col = 0usize;
    let mut found = false;

    for row in 0..state.screen_lines {
        let row_slice = &state.cells[row * state.columns..(row + 1) * state.columns];
        for (col, cell) in row_slice.iter().enumerate() {
            if cell_matches_kitty_image(cell, image_id) {
                found = true;
                min_row = min_row.min(row);
                max_row = max_row.max(row);
                min_col = min_col.min(col);
                max_col = max_col.max(col);
            }
        }
    }

    if found {
        let cols = (max_col - min_col + 1) as u32;
        let rows = (max_row - min_row + 1) as u32;
        Some((min_row as i32, min_col, cols, rows))
    } else {
        None
    }
}

impl super::TerminalWidget {
    pub(super) fn draw_kitty_images(
        &self,
        snapshot: &gtk4::Snapshot,
        state: &RenderState,
        is_background: bool,
        display_offset: i32,
        char_width: f32,
        char_height: f32,
        padding: f32,
    ) {
        let mut texture_cache = self.kitty_textures.borrow_mut();

        // Prune textures that are no longer in the engine's image set or whose
        // `unique_id` changed (e.g. when Yazi reuses the same `image_id` for a new file).
        texture_cache.retain(|id, (cached_uid, _)| {
            state
                .kitty_images
                .get(id)
                .is_some_and(|img| img.unique_id == *cached_uid)
        });

        for placement in &state.kitty_placements {
            if (placement.z_index < 0) != is_background {
                continue;
            }
            let texture = if let Some((_, tex)) = texture_cache.get(&placement.image_id) {
                Some(tex.clone())
            } else if let Some(img) = state.kitty_images.get(&placement.image_id) {
                let tex = match &img.data {
                    crate::engine::kitty::KittyImageData::Dynamic(dyn_img) => {
                        let rgba = dyn_img.to_rgba8();
                        let (w, h) = (rgba.width() as i32, rgba.height() as i32);
                        gtk4::gdk::MemoryTexture::new(
                            w,
                            h,
                            gtk4::gdk::MemoryFormat::R8g8b8a8,
                            &glib::Bytes::from(&rgba.into_raw()),
                            (w * 4) as usize,
                        )
                        .upcast::<gtk4::gdk::Texture>()
                    }
                    crate::engine::kitty::KittyImageData::RawRgb {
                        width,
                        height,
                        data,
                    } => {
                        let (w, h) = (*width as i32, *height as i32);
                        let pixel_count = (*width as usize) * (*height as usize);
                        let mut rgba = Vec::with_capacity(pixel_count * 4);
                        for rgb in data.chunks_exact(3).take(pixel_count) {
                            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                        }
                        gtk4::gdk::MemoryTexture::new(
                            w,
                            h,
                            gtk4::gdk::MemoryFormat::R8g8b8a8,
                            &glib::Bytes::from_owned(rgba),
                            (w * 4) as usize,
                        )
                        .upcast::<gtk4::gdk::Texture>()
                    }
                    crate::engine::kitty::KittyImageData::RawRgba {
                        width,
                        height,
                        data,
                    } => {
                        let (w, h) = (*width as i32, *height as i32);
                        gtk4::gdk::MemoryTexture::new(
                            w,
                            h,
                            gtk4::gdk::MemoryFormat::R8g8b8a8,
                            &glib::Bytes::from_owned(data.clone()),
                            (w * 4) as usize,
                        )
                        .upcast::<gtk4::gdk::Texture>()
                    }
                };
                texture_cache.insert(placement.image_id, (img.unique_id, tex.clone()));
                Some(tex)
            } else {
                None
            };
            if let Some(tex) = texture {
                let (row, col, placeholder_span) = if placement.unicode_placeholder {
                    // Virtual (`U=1`) placement: position and visibility are determined
                    // by the `\u{10EEEE}` placeholder cells currently in the viewport.
                    let Some((p_row, p_col, p_cols, p_rows)) =
                        find_unicode_placeholder_bounds(state, placement.image_id)
                    else {
                        continue;
                    };
                    (p_row, p_col, Some((p_cols, p_rows)))
                } else {
                    (
                        placement.point.line.0 + display_offset,
                        placement.point.column.0,
                        None,
                    )
                };

                let approx_height_cells = placement
                    .height
                    .or_else(|| placeholder_span.map(|(_, r)| r))
                    .unwrap_or_else(|| {
                        if char_height > 0.0 {
                            (placement.visible_height as f32 / char_height).ceil() as u32
                        } else {
                            1
                        }
                    })
                    .max(1);
                if row >= -(approx_height_cells as i32) && row < state.screen_lines as i32 {
                    // When `is_background` is true, `snapshot` is already translated by
                    // `(padding, padding)` inside `paint_snapshot`; when `is_background` is
                    // false, `snapshot.restore()` has popped that translation.
                    let pad = if is_background { 0.0 } else { padding };
                    let (x, y) = (
                        col as f32 * char_width + pad,
                        row as f32 * char_height + pad,
                    );
                    let mut target_width = if let Some(c) = placement.width {
                        c as f32 * char_width
                    } else {
                        placement.visible_width as f32
                    };
                    let mut target_height = if let Some(r) = placement.height {
                        r as f32 * char_height
                    } else {
                        placement.visible_height as f32
                    };

                    // If placed via Unicode placeholders (`U=1`) without explicit `c`/`r`,
                    // clamp proportionally to the placeholder cell bounding box if the source
                    // image exceeds it (e.g. HiDPI physical pixel buffers).
                    if placement.width.is_none()
                        && placement.height.is_none()
                        && let Some((p_cols, p_rows)) = placeholder_span
                    {
                        let max_w = p_cols as f32 * char_width;
                        let max_h = p_rows as f32 * char_height;
                        if target_width > max_w + 1.0 || target_height > max_h + 1.0 {
                            let scale = (max_w / target_width.max(1.0))
                                .min(max_h / target_height.max(1.0));
                            target_width *= scale;
                            target_height *= scale;
                        }
                    }

                    snapshot.append_texture(
                        &tex,
                        &gtk4::graphene::Rect::new(x, y, target_width, target_height),
                    );
                }
            }
        }
    }
}
