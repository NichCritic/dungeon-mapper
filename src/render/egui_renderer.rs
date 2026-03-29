use crate::render::traits::MapRenderer;
use crate::util::ViewTransform;

/// Renders through egui::Painter, applying a view transform.
pub struct EguiRenderer<'a> {
    painter: &'a egui::Painter,
    transform: &'a ViewTransform,
}

impl<'a> EguiRenderer<'a> {
    pub fn new(painter: &'a egui::Painter, transform: &'a ViewTransform) -> Self {
        Self { painter, transform }
    }

    fn to_screen(&self, x: f32, y: f32) -> egui::Pos2 {
        self.transform.world_to_screen(egui::pos2(x, y))
    }

    fn color(c: [u8; 4]) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
    }
}

impl<'a> MapRenderer for EguiRenderer<'a> {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let min = self.to_screen(x, y);
        let max = self.to_screen(x + w, y + h);
        let rect = egui::Rect::from_min_max(min, max);
        self.painter.rect_filled(rect, 0.0, Self::color(color));
    }

    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: [u8; 4]) {
        let min = self.to_screen(x, y);
        let max = self.to_screen(x + w, y + h);
        let rect = egui::Rect::from_min_max(min, max);
        self.painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(width * self.transform.zoom, Self::color(color)),
            egui::StrokeKind::Middle,
        );
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
        let center = self.to_screen(cx, cy);
        let screen_r = r * self.transform.zoom;
        self.painter.circle_filled(center, screen_r, Self::color(color));
    }

    fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, width: f32, color: [u8; 4]) {
        let center = self.to_screen(cx, cy);
        let screen_r = r * self.transform.zoom;
        self.painter.circle_stroke(
            center,
            screen_r,
            egui::Stroke::new(width * self.transform.zoom, Self::color(color)),
        );
    }

    fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [u8; 4]) {
        let from = self.to_screen(x1, y1);
        let to = self.to_screen(x2, y2);
        self.painter.line_segment(
            [from, to],
            egui::Stroke::new(width * self.transform.zoom, Self::color(color)),
        );
    }

    fn draw_text(&mut self, text: &str, x: f32, y: f32, size: f32, color: [u8; 4]) {
        let pos = self.to_screen(x, y);
        self.painter.text(
            pos,
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::monospace(size * self.transform.zoom),
            Self::color(color),
        );
    }
}
