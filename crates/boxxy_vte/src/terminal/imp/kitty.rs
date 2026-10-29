use crate::engine::term::RenderState;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

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

        // Prune textures that are no longer in the engine's image set.
        // This ensures that when the engine reaps images (e.g. during mpv playback),
        // we also release the corresponding GPU-backed GdkTextures.
        texture_cache.retain(|id, _| state.kitty_images.contains_key(id));

        for placement in &state.kitty_placements {
            if (placement.z_index < 0) != is_background {
                continue;
            }
            let texture = if let Some(tex) = texture_cache.get(&placement.image_id) {
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
                        gtk4::gdk::MemoryTexture::new(
                            w,
                            h,
                            gtk4::gdk::MemoryFormat::R8g8b8,
                            &glib::Bytes::from_owned(data.clone()),
                            (w * 3) as usize,
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
                texture_cache.insert(placement.image_id, tex.clone());
                Some(tex)
            } else {
                None
            };
            if let Some(tex) = texture {
                let (row, col) = (
                    placement.point.line.0 + display_offset,
                    placement.point.column.0,
                );
                let scale = self.fractional_scale(&self.obj()) as f32;
                let approx_height_cells = placement
                    .height
                    .unwrap_or_else(|| {
                        if char_height > 0.0 {
                            ((placement.visible_height as f32 / scale) / char_height).ceil() as u32
                        } else {
                            1
                        }
                    })
                    .max(1);
                if row >= -(approx_height_cells as i32) && row < state.screen_lines as i32 {
                    let (x, y) = (
                        col as f32 * char_width + padding,
                        row as f32 * char_height + padding,
                    );
                    let target_width = if let Some(c) = placement.width {
                        c as f32 * char_width
                    } else {
                        placement.visible_width as f32 / scale
                    };
                    let target_height = if let Some(r) = placement.height {
                        r as f32 * char_height
                    } else {
                        placement.visible_height as f32 / scale
                    };
                    snapshot.append_texture(
                        &tex,
                        &gtk4::graphene::Rect::new(x, y, target_width, target_height),
                    );
                }
            }
        }
    }
}
