//! Pure square-crop geometry, viewport fitting, and pointer hit-testing.

use boxxy_assets::image::{MIN_AVATAR_DIMENSION, RegionCrop};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DragMode {
    Move,
    ResizeNW,
    ResizeNE,
    ResizeSW,
    ResizeSE,
    ResizeN,
    ResizeS,
    ResizeE,
    ResizeW,
}

#[derive(Debug, Clone)]
pub(super) struct CropState {
    pub(super) img_w: f64,
    pub(super) img_h: f64,
    pub(super) min_size: f64,
    pub(super) max_size: f64,
    pub(super) crop_x: f64,
    pub(super) crop_y: f64,
    pub(super) crop_size: f64,
    pub(super) drag_mode: Option<DragMode>,
    pub(super) drag_start_crop_x: f64,
    pub(super) drag_start_crop_y: f64,
    pub(super) drag_start_crop_size: f64,
    pub(super) drag_origin_canvas_x: f64,
    pub(super) drag_origin_canvas_y: f64,
}

#[allow(clippy::suboptimal_flops)]
impl CropState {
    #[must_use]
    pub(super) fn new(width: u32, height: u32) -> Self {
        let img_w = f64::from(width.max(1));
        let img_h = f64::from(height.max(1));
        let max_size = img_w.min(img_h);
        let min_size = f64::from(MIN_AVATAR_DIMENSION).max(24.0).min(max_size);

        let mut state = Self {
            img_w,
            img_h,
            min_size,
            max_size,
            crop_x: 0.0,
            crop_y: 0.0,
            crop_size: max_size,
            drag_mode: None,
            drag_start_crop_x: 0.0,
            drag_start_crop_y: 0.0,
            drag_start_crop_size: max_size,
            drag_origin_canvas_x: 0.0,
            drag_origin_canvas_y: 0.0,
        };
        let _ = state.reset_centered();
        state
    }

    pub(super) fn reset_centered(&mut self) -> bool {
        let next_size = self.max_size;
        let next_x = (self.img_w - next_size) * 0.5;
        let next_y = (self.img_h - next_size) * 0.5;
        self.set_geometry(next_x, next_y, next_size)
    }

    pub(super) fn set_size_around_center(&mut self, new_size: f64) -> bool {
        let clamped_size = new_size.clamp(self.min_size, self.max_size);
        let center_x = self.crop_x + self.crop_size * 0.5;
        let center_y = self.crop_y + self.crop_size * 0.5;

        let next_x = (center_x - clamped_size * 0.5).clamp(0.0, self.img_w - clamped_size);
        let next_y = (center_y - clamped_size * 0.5).clamp(0.0, self.img_h - clamped_size);
        self.set_geometry(next_x, next_y, clamped_size)
    }

    pub(super) fn nudge(&mut self, dx: f64, dy: f64) -> bool {
        let next_x = (self.crop_x + dx).clamp(0.0, self.img_w - self.crop_size);
        let next_y = (self.crop_y + dy).clamp(0.0, self.img_h - self.crop_size);
        self.set_geometry(next_x, next_y, self.crop_size)
    }

    pub(super) fn recenter_at(&mut self, img_x: f64, img_y: f64) {
        let half = self.crop_size * 0.5;
        self.crop_x = (img_x - half).clamp(0.0, self.img_w - self.crop_size);
        self.crop_y = (img_y - half).clamp(0.0, self.img_h - self.crop_size);
    }

    pub(super) const fn begin_drag(
        &mut self,
        mode: Option<DragMode>,
        origin_x: f64,
        origin_y: f64,
    ) {
        self.drag_mode = mode;
        self.drag_start_crop_x = self.crop_x;
        self.drag_start_crop_y = self.crop_y;
        self.drag_start_crop_size = self.crop_size;
        self.drag_origin_canvas_x = origin_x;
        self.drag_origin_canvas_y = origin_y;
    }

    pub(super) fn apply_drag(
        &mut self,
        mode: DragMode,
        delta_img_x: f64,
        delta_img_y: f64,
    ) -> bool {
        let sx = self.drag_start_crop_x;
        let sy = self.drag_start_crop_y;
        let ss = self.drag_start_crop_size;

        let (next_x, next_y, next_size) = match mode {
            DragMode::Move => (
                (sx + delta_img_x).clamp(0.0, self.img_w - ss),
                (sy + delta_img_y).clamp(0.0, self.img_h - ss),
                ss,
            ),
            // Project corner drag vectors onto the 45° outward diagonal so dragging inward
            // along either axis shrinks smoothly instead of fighting single-axis motion.
            DragMode::ResizeSE => {
                let anchor_x = sx;
                let anchor_y = sy;
                let raw = ss + f64::midpoint(delta_img_x, delta_img_y);
                let max_s = (self.img_w - anchor_x).min(self.img_h - anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                (anchor_x, anchor_y, new_size)
            }
            DragMode::ResizeNW => {
                let anchor_x = sx + ss;
                let anchor_y = sy + ss;
                let raw = ss - f64::midpoint(delta_img_x, delta_img_y);
                let max_s = anchor_x.min(anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                (anchor_x - new_size, anchor_y - new_size, new_size)
            }
            DragMode::ResizeNE => {
                let anchor_x = sx;
                let anchor_y = sy + ss;
                let raw = ss + f64::midpoint(delta_img_x, -delta_img_y);
                let max_s = (self.img_w - anchor_x).min(anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                (anchor_x, anchor_y - new_size, new_size)
            }
            DragMode::ResizeSW => {
                let anchor_x = sx + ss;
                let anchor_y = sy;
                let raw = ss + f64::midpoint(-delta_img_x, delta_img_y);
                let max_s = anchor_x.min(self.img_h - anchor_y);
                let new_size = raw.clamp(self.min_size, max_s);
                (anchor_x - new_size, anchor_y, new_size)
            }
            DragMode::ResizeE => {
                let anchor_x = sx;
                let center_y = sy + ss * 0.5;
                let raw = ss + delta_img_x;
                let max_s = (self.img_w - anchor_x).min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                let y = (center_y - new_size * 0.5).clamp(0.0, self.img_h - new_size);
                (anchor_x, y, new_size)
            }
            DragMode::ResizeW => {
                let anchor_x = sx + ss;
                let center_y = sy + ss * 0.5;
                let raw = ss - delta_img_x;
                let max_s = anchor_x.min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                let y = (center_y - new_size * 0.5).clamp(0.0, self.img_h - new_size);
                (anchor_x - new_size, y, new_size)
            }
            DragMode::ResizeS => {
                let anchor_y = sy;
                let center_x = sx + ss * 0.5;
                let raw = ss + delta_img_y;
                let max_s = (self.img_h - anchor_y).min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                let x = (center_x - new_size * 0.5).clamp(0.0, self.img_w - new_size);
                (x, anchor_y, new_size)
            }
            DragMode::ResizeN => {
                let anchor_y = sy + ss;
                let center_x = sx + ss * 0.5;
                let raw = ss - delta_img_y;
                let max_s = anchor_y.min(self.max_size);
                let new_size = raw.clamp(self.min_size, max_s);
                let x = (center_x - new_size * 0.5).clamp(0.0, self.img_w - new_size);
                (x, anchor_y - new_size, new_size)
            }
        };

        self.set_geometry(next_x, next_y, next_size)
    }

    fn set_geometry(&mut self, next_x: f64, next_y: f64, next_size: f64) -> bool {
        let changed = (self.crop_x - next_x).abs() > 1e-4
            || (self.crop_y - next_y).abs() > 1e-4
            || (self.crop_size - next_size).abs() > 1e-4;
        self.crop_x = next_x;
        self.crop_y = next_y;
        self.crop_size = next_size;
        changed
    }

    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(super) fn to_region_crop(&self) -> RegionCrop {
        let img_w = self.img_w.max(1.0) as u32;
        let img_h = self.img_h.max(1.0) as u32;
        let size = (self.crop_size.round().max(1.0) as u32).max(1);
        let x = self.crop_x.round().max(0.0) as u32;
        let y = self.crop_y.round().max(0.0) as u32;
        RegionCrop::new(x, y, size, size).clamp_square_to_bounds(img_w, img_h)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Viewport {
    pub(super) offset_x: f64,
    pub(super) offset_y: f64,
    pub(super) draw_w: f64,
    pub(super) draw_h: f64,
    pub(super) scale: f64,
}

#[allow(clippy::suboptimal_flops)]
impl Viewport {
    #[must_use]
    pub(super) fn compute(canvas_w: f64, canvas_h: f64, img_w: f64, img_h: f64) -> Self {
        let pad = 22.0;
        let safe_img_w = img_w.max(1.0);
        let safe_img_h = img_h.max(1.0);
        let avail_w = (canvas_w - pad * 2.0).max(32.0);
        let avail_h = (canvas_h - pad * 2.0).max(32.0);
        let scale = (avail_w / safe_img_w).min(avail_h / safe_img_h);
        let draw_w = safe_img_w * scale;
        let draw_h = safe_img_h * scale;
        let offset_x = (canvas_w - draw_w) * 0.5;
        let offset_y = (canvas_h - draw_h) * 0.5;
        Self {
            offset_x,
            offset_y,
            draw_w,
            draw_h,
            scale,
        }
    }
}

#[must_use]
#[allow(clippy::suboptimal_flops)]
pub(super) fn hit_test(st: &CropState, vp: &Viewport, cx: f64, cy: f64) -> Option<DragMode> {
    let sx = vp.offset_x + st.crop_x * vp.scale;
    let sy = vp.offset_y + st.crop_y * vp.scale;
    let sw = st.crop_size * vp.scale;

    let corner_r_sq = 14.0 * 14.0;
    let dist_sq = |x1: f64, y1: f64, x2: f64, y2: f64| (x1 - x2).powi(2) + (y1 - y2).powi(2);

    if dist_sq(cx, cy, sx, sy) <= corner_r_sq {
        return Some(DragMode::ResizeNW);
    }
    if dist_sq(cx, cy, sx + sw, sy) <= corner_r_sq {
        return Some(DragMode::ResizeNE);
    }
    if dist_sq(cx, cy, sx, sy + sw) <= corner_r_sq {
        return Some(DragMode::ResizeSW);
    }
    if dist_sq(cx, cy, sx + sw, sy + sw) <= corner_r_sq {
        return Some(DragMode::ResizeSE);
    }

    let edge_r = 8.0;
    if (cy - sy).abs() <= edge_r && cx >= sx && cx <= sx + sw {
        return Some(DragMode::ResizeN);
    }
    if (cy - (sy + sw)).abs() <= edge_r && cx >= sx && cx <= sx + sw {
        return Some(DragMode::ResizeS);
    }
    if (cx - sx).abs() <= edge_r && cy >= sy && cy <= sy + sw {
        return Some(DragMode::ResizeW);
    }
    if (cx - (sx + sw)).abs() <= edge_r && cy >= sy && cy <= sy + sw {
        return Some(DragMode::ResizeE);
    }

    if cx >= sx && cx <= sx + sw && cy >= sy && cy <= sy + sw {
        return Some(DragMode::Move);
    }

    None
}

#[must_use]
pub(super) const fn cursor_for_mode(mode: Option<DragMode>, dragging: bool) -> &'static str {
    match mode {
        Some(DragMode::ResizeNW | DragMode::ResizeSE) => "nwse-resize",
        Some(DragMode::ResizeNE | DragMode::ResizeSW) => "nesw-resize",
        Some(DragMode::ResizeN | DragMode::ResizeS) => "ns-resize",
        Some(DragMode::ResizeE | DragMode::ResizeW) => "ew-resize",
        Some(DragMode::Move) => {
            if dragging {
                "grabbing"
            } else {
                "grab"
            }
        }
        None => "crosshair",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crop_state_initial_and_move_clamping() {
        let mut st = CropState::new(400, 200);
        assert!((st.crop_size - 200.0).abs() < 1e-6);
        assert!((st.crop_x - 100.0).abs() < 1e-6);
        assert!(st.crop_y.abs() < 1e-6);

        st.begin_drag(Some(DragMode::Move), 0.0, 0.0);

        // Drag far right — clamps to x = 200 and reports changed = true
        assert!(st.apply_drag(DragMode::Move, 500.0, 50.0));
        assert!((st.crop_x - 200.0).abs() < 1e-6);
        assert!(st.crop_y.abs() < 1e-6);

        // Dragging further right against the same wall reports changed = false
        assert!(!st.apply_drag(DragMode::Move, 600.0, 50.0));
    }

    #[test]
    fn test_crop_state_corner_and_edge_resize() {
        let mut st = CropState::new(300, 300);
        assert!(st.set_size_around_center(120.0));
        assert!((st.crop_x - 90.0).abs() < 1e-6);
        assert!((st.crop_y - 90.0).abs() < 1e-6);
        assert!((st.crop_size - 120.0).abs() < 1e-6);

        // Resize NW corner outward by (-40, -40) -> size grows to 160 while SE anchor stays at (210, 210)
        st.begin_drag(Some(DragMode::ResizeNW), 0.0, 0.0);
        assert!(st.apply_drag(DragMode::ResizeNW, -40.0, -40.0));
        assert!((st.crop_size - 160.0).abs() < 1e-6);
        assert!((st.crop_x - 50.0).abs() < 1e-6);
        assert!((st.crop_y - 50.0).abs() < 1e-6);

        // Dragging SE corner inward purely horizontally (-60, 0) smoothly shrinks by 30px
        st.begin_drag(Some(DragMode::ResizeSE), 0.0, 0.0);
        assert!(st.apply_drag(DragMode::ResizeSE, -60.0, 0.0));
        assert!((st.crop_size - 130.0).abs() < 1e-6);
        assert!((st.crop_x - 50.0).abs() < 1e-6);
        assert!((st.crop_y - 50.0).abs() < 1e-6);

        // East edge resize keeps vertical center anchored
        st.begin_drag(Some(DragMode::ResizeE), 0.0, 0.0);
        assert!(st.apply_drag(DragMode::ResizeE, 20.0, 0.0));
        assert!((st.crop_size - 150.0).abs() < 1e-6);
        assert!((st.crop_x - 50.0).abs() < 1e-6);
        assert!((st.crop_y - 40.0).abs() < 1e-6);
    }

    #[test]
    fn test_hit_test_priority_and_viewport_safety() {
        let st = CropState::new(200, 200);
        let vp = Viewport::compute(244.0, 244.0, st.img_w, st.img_h);
        // avail = 200x200 -> scale = 1.0, offset = (22, 22)
        assert!((vp.scale - 1.0).abs() < 1e-6);
        assert_eq!(hit_test(&st, &vp, 22.0, 22.0), Some(DragMode::ResizeNW));
        assert_eq!(hit_test(&st, &vp, 122.0, 22.0), Some(DragMode::ResizeN));
        assert_eq!(hit_test(&st, &vp, 122.0, 122.0), Some(DragMode::Move));
        assert_eq!(hit_test(&st, &vp, 2.0, 2.0), None);

        // Degenerate zero dimensions never produce NaN or zero scale
        let degen = Viewport::compute(0.0, 0.0, 0.0, 0.0);
        assert!(degen.scale.is_finite() && degen.scale > 0.0);
    }
}
