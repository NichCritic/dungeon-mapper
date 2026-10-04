//! A renderer that turns everything drawn through it about a pivot. Rotated rooms draw
//! their floor, cave cells, decor and elevation sections with the ordinary routines, as
//! if unrotated, through this wrapper.

use crate::model::RoomLayout;
use crate::render::traits::MapRenderer;
use crate::util::GRID_PX;

pub struct RotatedRenderer<'a> {
    inner: &'a mut dyn MapRenderer,
    pivot: (f32, f32),
    sin: f32,
    cos: f32,
}

impl<'a> RotatedRenderer<'a> {
    /// Turn drawing by the room's rotation about its center (pixel coordinates).
    pub fn for_room(inner: &'a mut dyn MapRenderer, rl: &RoomLayout) -> Self {
        let (cx, cy) = rl.center();
        let (sin, cos) = rl.rotation.to_radians().sin_cos();
        Self { inner, pivot: (cx * GRID_PX, cy * GRID_PX), sin, cos }
    }

    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        let (dx, dy) = (x - self.pivot.0, y - self.pivot.1);
        (self.pivot.0 + dx * self.cos - dy * self.sin, self.pivot.1 + dx * self.sin + dy * self.cos)
    }
}

impl MapRenderer for RotatedRenderer<'_> {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let pts = [self.p(x, y), self.p(x + w, y), self.p(x + w, y + h), self.p(x, y + h)];
        self.inner.fill_polygon(&pts, color);
    }

    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: [u8; 4]) {
        let c = [self.p(x, y), self.p(x + w, y), self.p(x + w, y + h), self.p(x, y + h)];
        for i in 0..4 {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            self.inner.draw_line(a.0, a.1, b.0, b.1, width, color);
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
        let (x, y) = self.p(cx, cy);
        self.inner.fill_circle(x, y, r, color);
    }

    fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, width: f32, color: [u8; 4]) {
        let (x, y) = self.p(cx, cy);
        self.inner.stroke_circle(x, y, r, width, color);
    }

    fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [u8; 4]) {
        let (a, b) = (self.p(x1, y1), self.p(x2, y2));
        self.inner.draw_line(a.0, a.1, b.0, b.1, width, color);
    }

    fn fill_polygon(&mut self, pts: &[(f32, f32)], color: [u8; 4]) {
        let turned: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.inner.fill_polygon(&turned, color);
    }

    fn draw_text(&mut self, text: &str, x: f32, y: f32, size: f32, color: [u8; 4]) {
        // Text stays upright; only its anchor moves with the room
        let (px, py) = self.p(x, y);
        self.inner.draw_text(text, px, py, size, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::recording::{RecordingRenderer, RenderCommand};

    #[test]
    fn rotated_rect_becomes_a_turned_polygon() {
        let rl = RoomLayout { room_id: "r".into(), x: 0, y: 0, width: 2, height: 2, violations: Vec::new(), wall_openings: Vec::new(), rotation: 90.0 };
        let mut rec = RecordingRenderer::new();
        RotatedRenderer::for_room(&mut rec, &rl).fill_rect(0.0, 0.0, GRID_PX, GRID_PX, [0; 4]);
        let RenderCommand::Polygon { pts, .. } = &rec.commands[0] else { panic!("expected polygon") };
        // The top-left cell of a 2x2 room turned 90° clockwise lands in the top-right
        let (x0, y0) = pts.iter().fold((f32::MAX, f32::MAX), |m, p| (m.0.min(p.0), m.1.min(p.1)));
        assert!((x0 - GRID_PX).abs() < 1e-3 && y0.abs() < 1e-3, "{pts:?}");
    }
}
