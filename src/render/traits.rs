/// Abstraction over rendering targets (egui painter vs image buffer).
pub trait MapRenderer {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]);
    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: [u8; 4]);
    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, color: [u8; 4]);
    fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, width: f32, color: [u8; 4]);
    fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [u8; 4]);
    fn draw_text(&mut self, text: &str, x: f32, y: f32, size: f32, color: [u8; 4]);
}
