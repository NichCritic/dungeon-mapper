use image::{ImageBuffer, Rgba, RgbaImage};

use crate::render::traits::MapRenderer;

/// Renders to an image buffer for PNG export.
pub struct ImageRenderer {
    pub image: RgbaImage,
    pub scale: f32,
}

impl ImageRenderer {
    pub fn new(width: u32, height: u32, scale: f32) -> Self {
        Self {
            image: ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 0])),
            scale,
        }
    }
}

impl MapRenderer for ImageRenderer {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let sx = (x * self.scale) as i32;
        let sy = (y * self.scale) as i32;
        let sw = (w * self.scale) as i32;
        let sh = (h * self.scale) as i32;
        let pixel = Rgba(color);

        for py in sy.max(0)..(sy + sh).min(self.image.height() as i32) {
            for px in sx.max(0)..(sx + sw).min(self.image.width() as i32) {
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

    fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [u8; 4]) {
        // Bresenham with thickness
        let sx1 = (x1 * self.scale) as i32;
        let sy1 = (y1 * self.scale) as i32;
        let sx2 = (x2 * self.scale) as i32;
        let sy2 = (y2 * self.scale) as i32;
        let sw = ((width * self.scale) / 2.0).max(1.0) as i32;
        let pixel = Rgba(color);

        let dx = (sx2 - sx1).abs();
        let dy = (sy2 - sy1).abs();
        let step_x = if sx1 < sx2 { 1 } else { -1 };
        let step_y = if sy1 < sy2 { 1 } else { -1 };
        let mut err = dx - dy;
        let mut cx = sx1;
        let mut cy = sy1;

        loop {
            for oy in -sw..=sw {
                for ox in -sw..=sw {
                    let px = cx + ox;
                    let py = cy + oy;
                    if px >= 0
                        && py >= 0
                        && px < self.image.width() as i32
                        && py < self.image.height() as i32
                    {
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
        // Could use imageproc or rusttype in the future
    }
}
