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
    Polygon { pts: Vec<(f32, f32)>, color: [u8; 4] },
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

    fn fill_polygon(&mut self, pts: &[(f32, f32)], color: [u8; 4]) {
        self.commands.push(RenderCommand::Polygon { pts: pts.to_vec(), color });
    }

    fn draw_text(&mut self, _text: &str, _x: f32, _y: f32, _size: f32, _color: [u8; 4]) {
        // Text is drawn as a live egui overlay, not cached.
    }
}

fn color(c: [u8; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Replay cached render commands through an egui painter, applying the view transform.
fn replay_commands(
    painter: &egui::Painter,
    transform: &ViewTransform,
    commands: &[RenderCommand],
) {
    let mut shapes = Vec::with_capacity(commands.len());
    for cmd in commands {
        shapes.push(command_shape(cmd, transform));
    }
    painter.extend(shapes);
}

/// Everything a tessellated replay depends on besides pan.
#[derive(Clone, Copy, PartialEq)]
struct ReplayKey {
    generation: u64,
    zoom: u32,
    rotation: u32,
    pixels_per_point: u32,
    feathering: bool,
}

/// Cached tessellation of one command list at a fixed zoom and rotation.
///
/// Re-tessellating every hatch line and wall each frame costs several milliseconds on
/// a large map. At a fixed zoom and rotation, panning only translates the result, so
/// the meshes are built once and shifted per frame. While the zoom is changing we draw
/// directly and build once it holds steady for a frame.
#[derive(Default)]
pub struct ReplayCache {
    built: Option<ReplayKey>,
    /// The key seen last frame, so a build waits for the zoom to settle.
    last_seen: Option<ReplayKey>,
    /// Screen position of the world origin when the meshes were built.
    anchor: egui::Pos2,
    /// Pan offset from `anchor` drawn last frame.
    last_delta: egui::Vec2,
    /// Meshes in paint order, split so off-screen parts can be skipped.
    chunks: Vec<(egui::Rect, std::sync::Arc<egui::Mesh>)>,
}

/// Vertex budget per chunk: small enough to cull usefully, large enough to keep the
/// chunk count low.
const CHUNK_VERTICES: usize = 4_096;

impl ReplayCache {
    /// Paint `commands` (identified by `generation`, which must change whenever they do).
    pub fn paint(
        &mut self,
        painter: &egui::Painter,
        transform: &ViewTransform,
        commands: &[RenderCommand],
        generation: u64,
    ) {
        let ctx = painter.ctx();
        let pixels_per_point = ctx.pixels_per_point();
        let options = ctx.tessellation_options(|o| *o);
        let key = ReplayKey {
            generation,
            zoom: transform.zoom.to_bits(),
            rotation: transform.rotation.to_bits(),
            pixels_per_point: pixels_per_point.to_bits(),
            feathering: options.feathering,
        };
        let settled = self.last_seen == Some(key);
        self.last_seen = Some(key);

        if self.built != Some(key) && !settled {
            self.built = None;
            self.chunks.clear();
            replay_commands(painter, transform, commands);
            return;
        }

        // Shift by whole physical pixels so pixel-snapped edges stay crisp.
        let delta = |anchor: egui::Pos2| {
            let d = transform.world_to_screen(egui::Pos2::ZERO) - anchor;
            (d * pixels_per_point).round() / pixels_per_point
        };
        // Once a pan comes to rest, re-anchor so later frames (e.g. dragging a token)
        // can hand the meshes to egui without copying them to shift.
        let panned_and_stopped = delta(self.anchor) != egui::Vec2::ZERO && delta(self.anchor) == self.last_delta;
        if self.built != Some(key) || panned_and_stopped {
            self.build(transform, commands, pixels_per_point, options);
            self.built = Some(key);
        }
        let delta = delta(self.anchor);
        self.last_delta = delta;
        let clip = painter.clip_rect();
        for (bounds, mesh) in &self.chunks {
            if !clip.intersects(bounds.translate(delta)) {
                continue;
            }
            if delta == egui::Vec2::ZERO {
                painter.add(egui::Shape::Mesh(mesh.clone()));
            } else {
                let mut moved = (**mesh).clone();
                moved.translate(delta);
                painter.add(egui::Shape::mesh(moved));
            }
        }
    }

    fn build(
        &mut self,
        transform: &ViewTransform,
        commands: &[RenderCommand],
        pixels_per_point: f32,
        options: egui::epaint::TessellationOptions,
    ) {
        // No prepared discs: circles become plain geometry, so the meshes never point
        // into the font atlas (which egui may rebuild at any time).
        let mut tess = egui::epaint::Tessellator::new(pixels_per_point, options, [1, 1], Vec::new());
        self.anchor = transform.world_to_screen(egui::Pos2::ZERO);
        self.chunks.clear();
        let mut mesh = egui::Mesh::default();
        for cmd in commands {
            tess.tessellate_shape(command_shape(cmd, transform), &mut mesh);
            if mesh.vertices.len() >= CHUNK_VERTICES {
                let full = std::mem::take(&mut mesh);
                self.chunks.push((full.calc_bounds(), std::sync::Arc::new(full)));
            }
        }
        if !mesh.is_empty() {
            self.chunks.push((mesh.calc_bounds(), std::sync::Arc::new(mesh)));
        }
    }
}

fn command_shape(cmd: &RenderCommand, transform: &ViewTransform) -> egui::Shape {
    match cmd {
        RenderCommand::FillRect { x, y, w, h, color: c } => {
            let (x, y, w, h, c) = (*x, *y, *w, *h, *c);
            let min = transform.world_to_screen(egui::pos2(x, y));
            let max = transform.world_to_screen(egui::pos2(x + w, y + h));
            egui::Shape::rect_filled(
                egui::Rect::from_min_max(min, max),
                0.0,
                color(c),
            )
        }
        RenderCommand::StrokeRect { x, y, w, h, width, color: c } => {
            let (x, y, w, h, width, c) = (*x, *y, *w, *h, *width, *c);
            let min = transform.world_to_screen(egui::pos2(x, y));
            let max = transform.world_to_screen(egui::pos2(x + w, y + h));
            egui::Shape::rect_stroke(
                egui::Rect::from_min_max(min, max),
                0.0,
                egui::Stroke::new(width * transform.zoom, color(c)),
                egui::StrokeKind::Middle,
            )
        }
        RenderCommand::FillCircle { cx, cy, r, color: c } => {
            let (cx, cy, r, c) = (*cx, *cy, *r, *c);
            let center = transform.world_to_screen(egui::pos2(cx, cy));
            egui::Shape::circle_filled(
                center,
                r * transform.zoom,
                color(c),
            )
        }
        RenderCommand::StrokeCircle { cx, cy, r, width, color: c } => {
            let (cx, cy, r, width, c) = (*cx, *cy, *r, *width, *c);
            let center = transform.world_to_screen(egui::pos2(cx, cy));
            egui::Shape::circle_stroke(
                center,
                r * transform.zoom,
                egui::Stroke::new(width * transform.zoom, color(c)),
            )
        }
        RenderCommand::Line { x1, y1, x2, y2, width, color: c } => {
            let (x1, y1, x2, y2, width, c) = (*x1, *y1, *x2, *y2, *width, *c);
            let from = transform.world_to_screen(egui::pos2(x1, y1));
            let to = transform.world_to_screen(egui::pos2(x2, y2));
            egui::Shape::line_segment(
                [from, to],
                egui::Stroke::new(width * transform.zoom, color(c)),
            )
        }
        RenderCommand::Polygon { pts, color: c } => {
            let c = *c;
            let screen: Vec<egui::Pos2> = pts
                .iter()
                .map(|&(x, y)| transform.world_to_screen(egui::pos2(x, y)))
                .collect();
            egui::Shape::convex_polygon(screen, color(c), egui::Stroke::NONE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tessellated vertex positions of one frame's painting.
    fn frame_vertices(ctx: &egui::Context, mut paint: impl FnMut(&egui::Painter)) -> Vec<egui::Pos2> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            paint(&painter);
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).into_iter()
            .flat_map(|p| match p.primitive {
                egui::epaint::Primitive::Mesh(m) => m.vertices.into_iter().map(|v| v.pos).collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn cached_replay_matches_direct_after_pan() {
        // Rects, lines and polygons only: circles differ by design (no atlas discs).
        let commands = vec![
            RenderCommand::FillRect { x: 10.0, y: 10.0, w: 64.0, h: 32.0, color: [200, 180, 150, 255] },
            RenderCommand::Line { x1: 0.0, y1: 0.0, x2: 90.0, y2: 40.0, width: 1.5, color: [0, 0, 0, 255] },
            RenderCommand::StrokeRect { x: 5.0, y: 50.0, w: 20.0, h: 20.0, width: 2.0, color: [0, 0, 0, 255] },
            RenderCommand::Polygon { pts: vec![(100.0, 100.0), (140.0, 100.0), (120.0, 130.0)], color: [50, 50, 50, 255] },
        ];
        let canvas = egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(700.0, 500.0));
        for rotation in [0.0, std::f32::consts::FRAC_PI_2] {
            let start = ViewTransform::new(egui::vec2(3.0, 4.0), 1.7, canvas).with_rotation(rotation);
            let panned = ViewTransform::new(egui::vec2(40.0, -25.0), 1.7, canvas).with_rotation(rotation);
            let ctx = egui::Context::default();
            let mut cache = ReplayCache::default();
            // Settle and build at `start`, then pan: first frame shifts, second re-anchors.
            for t in [&start, &start, &panned, &panned] {
                let shifted = frame_vertices(&ctx, |p| cache.paint(p, t, &commands, 1));
                let direct = frame_vertices(&ctx, |p| replay_commands(p, t, &commands));
                assert_eq!(shifted.len(), direct.len());
                for (a, b) in shifted.iter().zip(&direct) {
                    assert!(a.distance(*b) < 0.51, "rotation {rotation}: {a:?} vs {b:?}");
                }
            }
            assert!(cache.built.is_some(), "cache should be in use once settled");
        }
    }
}
