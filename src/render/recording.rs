use crate::render::traits::MapRenderer;
use crate::util::ViewTransform;

/// A world-space drawing command captured from render_themed.
/// Text commands are excluded — they're drawn as a live egui overlay.
#[derive(Clone)]
pub enum RenderCommand {
    FillRect { x: f32, y: f32, w: f32, h: f32, color: [u8; 4] },
    StrokeRect { x: f32, y: f32, w: f32, h: f32, width: f32, color: [u8; 4] },
    FillCircle { cx: f32, cy: f32, r: f32, color: [u8; 4] },
    StrokeCircle { cx: f32, cy: f32, r: f32, width: f32, color: [u8; 4] },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [u8; 4] },
}

/// A MapRenderer that records drawing commands instead of rendering them.
pub struct RecordingRenderer {
    pub commands: Vec<RenderCommand>,
}

impl RecordingRenderer {
    pub fn new() -> Self {
        Self { commands: Vec::new() }
    }
}

impl MapRenderer for RecordingRenderer {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        self.commands.push(RenderCommand::FillRect { x, y, w, h, color });
    }

    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: [u8; 4]) {
        self.commands.push(RenderCommand::StrokeRect { x, y, w, h, width, color });
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
        self.commands.push(RenderCommand::FillCircle { cx, cy, r, color });
    }

    fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, width: f32, color: [u8; 4]) {
        self.commands.push(RenderCommand::StrokeCircle { cx, cy, r, width, color });
    }

    fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [u8; 4]) {
        self.commands.push(RenderCommand::Line { x1, y1, x2, y2, width, color });
    }

    fn draw_text(&mut self, _text: &str, _x: f32, _y: f32, _size: f32, _color: [u8; 4]) {
        // Text is drawn as a live egui overlay, not cached.
    }
}

fn color(c: [u8; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Replay cached render commands through an egui painter, applying the view transform.
pub fn replay_commands(
    painter: &egui::Painter,
    transform: &ViewTransform,
    commands: &[RenderCommand],
) {
    let mut shapes = Vec::with_capacity(commands.len());

    for cmd in commands {
        match *cmd {
            RenderCommand::FillRect { x, y, w, h, color: c } => {
                let min = transform.world_to_screen(egui::pos2(x, y));
                let max = transform.world_to_screen(egui::pos2(x + w, y + h));
                shapes.push(egui::Shape::rect_filled(
                    egui::Rect::from_min_max(min, max),
                    0.0,
                    color(c),
                ));
            }
            RenderCommand::StrokeRect { x, y, w, h, width, color: c } => {
                let min = transform.world_to_screen(egui::pos2(x, y));
                let max = transform.world_to_screen(egui::pos2(x + w, y + h));
                shapes.push(egui::Shape::rect_stroke(
                    egui::Rect::from_min_max(min, max),
                    0.0,
                    egui::Stroke::new(width * transform.zoom, color(c)),
                    egui::StrokeKind::Middle,
                ));
            }
            RenderCommand::FillCircle { cx, cy, r, color: c } => {
                let center = transform.world_to_screen(egui::pos2(cx, cy));
                shapes.push(egui::Shape::circle_filled(
                    center,
                    r * transform.zoom,
                    color(c),
                ));
            }
            RenderCommand::StrokeCircle { cx, cy, r, width, color: c } => {
                let center = transform.world_to_screen(egui::pos2(cx, cy));
                shapes.push(egui::Shape::circle_stroke(
                    center,
                    r * transform.zoom,
                    egui::Stroke::new(width * transform.zoom, color(c)),
                ));
            }
            RenderCommand::Line { x1, y1, x2, y2, width, color: c } => {
                let from = transform.world_to_screen(egui::pos2(x1, y1));
                let to = transform.world_to_screen(egui::pos2(x2, y2));
                shapes.push(egui::Shape::line_segment(
                    [from, to],
                    egui::Stroke::new(width * transform.zoom, color(c)),
                ));
            }
        }
    }

    painter.extend(shapes);
}
