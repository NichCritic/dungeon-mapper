use image::{ImageBuffer, Rgba, RgbaImage};

use crate::render::traits::MapRenderer;

/// Renders to an image buffer for PNG export.
pub struct ImageRenderer {
    pub image: RgbaImage,
    pub scale: f32,
    /// World-space offset: subtracted from coordinates before scaling to pixels.
    /// Set this so that the top-left of the rendered area maps to pixel (0, 0).
    pub offset_x: f32,
    pub offset_y: f32,
}

impl ImageRenderer {
    pub fn new(width: u32, height: u32, scale: f32) -> Self {
        Self {
            image: ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 0])),
            scale,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }

    /// Convert world x to pixel x.
    fn px(&self, x: f32) -> i32 {
        ((x - self.offset_x) * self.scale) as i32
    }

    /// Convert world y to pixel y.
    fn py(&self, y: f32) -> i32 {
        ((y - self.offset_y) * self.scale) as i32
    }
}

impl MapRenderer for ImageRenderer {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let sx = self.px(x) as i64;
        let sy = self.py(y) as i64;
        let sw = (w * self.scale) as i64;
        let sh = (h * self.scale) as i64;
        let pixel = Rgba(color);
        let img_w = self.image.width() as i64;
        let img_h = self.image.height() as i64;

        for py in sy.max(0)..(sy + sh).min(img_h) {
            for px in sx.max(0)..(sx + sw).min(img_w) {
                self.image.put_pixel(px as u32, py as u32, pixel);
            }
        }
    }

    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: [u8; 4]) {
        let lw = (width * self.scale).max(1.0);
        // Top
        self.fill_rect(x, y, w, lw / self.scale, color);
        // Bottom
        self.fill_rect(x, y + h - lw / self.scale, w, lw / self.scale, color);
        // Left
        self.fill_rect(x, y, lw / self.scale, h, color);
        // Right
        self.fill_rect(x + w - lw / self.scale, y, lw / self.scale, h, color);
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
        let scx = self.px(cx) as i64;
        let scy = self.py(cy) as i64;
        let sr = (r * self.scale) as i64;
        let pixel = image::Rgba(color);
        let w = self.image.width() as i64;
        let h = self.image.height() as i64;
        for py in (scy - sr).max(0)..(scy + sr).min(h) {
            for px in (scx - sr).max(0)..(scx + sr).min(w) {
                let dx = px - scx;
                let dy = py - scy;
                if dx * dx + dy * dy <= sr * sr {
                    self.image.put_pixel(px as u32, py as u32, pixel);
                }
            }
        }
    }

    fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, width: f32, color: [u8; 4]) {
        let scx = self.px(cx) as i64;
        let scy = self.py(cy) as i64;
        let sr = (r * self.scale) as i64;
        let sw = ((width * self.scale) / 2.0).max(1.0) as i64;
        let pixel = image::Rgba(color);
        let r_inner = (sr - sw).max(0);
        let r_outer = sr + sw;
        let w = self.image.width() as i64;
        let h = self.image.height() as i64;
        for py in (scy - r_outer).max(0)..(scy + r_outer).min(h) {
            for px in (scx - r_outer).max(0)..(scx + r_outer).min(w) {
                let dx = px - scx;
                let dy = py - scy;
                let d2 = dx * dx + dy * dy;
                if d2 >= r_inner * r_inner && d2 <= r_outer * r_outer {
                    self.image.put_pixel(px as u32, py as u32, pixel);
                }
            }
        }
    }

    fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [u8; 4]) {
        // Bresenham with thickness
        let sx1 = self.px(x1) as i64;
        let sy1 = self.py(y1) as i64;
        let sx2 = self.px(x2) as i64;
        let sy2 = self.py(y2) as i64;
        let sw = ((width * self.scale) / 2.0).max(1.0) as i64;
        let pixel = Rgba(color);
        let img_w = self.image.width() as i64;
        let img_h = self.image.height() as i64;

        let dx = (sx2 - sx1).abs();
        let dy = (sy2 - sy1).abs();
        let step_x: i64 = if sx1 < sx2 { 1 } else { -1 };
        let step_y: i64 = if sy1 < sy2 { 1 } else { -1 };
        let mut err = dx - dy;
        let mut cx = sx1;
        let mut cy = sy1;

        loop {
            for oy in -sw..=sw {
                for ox in -sw..=sw {
                    let px = cx + ox;
                    let py = cy + oy;
                    if px >= 0 && py >= 0 && px < img_w && py < img_h {
                        self.image.put_pixel(px as u32, py as u32, pixel);
                    }
                }
            }

            if cx == sx2 && cy == sy2 {
                break;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                cx += step_x;
            }
            if e2 < dx {
                err += dx;
                cy += step_y;
            }
        }
    }

    fn draw_text(&mut self, _text: &str, _x: f32, _y: f32, _size: f32, _color: [u8; 4]) {
        // Text rendering in image is complex - skip for MVP
        // Labels are drawn as an egui overlay in the styled view.
    }
}
