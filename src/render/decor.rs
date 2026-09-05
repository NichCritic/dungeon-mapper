//! Top-down glyphs for room decor.
//!
//! Every decor type is drawn once here, in a local unit space where `1.0` is the
//! glyph's base half-size (`GRID_PX * 0.4` world pixels). The [`Glyph`] helper applies
//! the per-item scale and rotation and forwards primitives to a [`GlyphSink`], so the
//! same drawing code feeds the egui editor overlay, the cached world-space renderer and
//! PNG export.

use std::f32::consts::{PI, TAU};

use crate::model::DecorType;
use crate::render::traits::MapRenderer;

/// Stroke widths in local units (fractions of the glyph half-size).
const W_BOLD: f32 = 0.2;
const W_MAIN: f32 = 0.13;
const W_THIN: f32 = 0.085;
const W_HAIR: f32 = 0.05;

/// Where transformed glyph geometry ends up. Coordinates are in the sink's own space.
pub trait GlyphSink {
    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: [u8; 4]);
    /// Fill a convex polygon.
    fn fill_convex(&mut self, pts: &[(f32, f32)], color: [u8; 4]);
    fn fill_circle(&mut self, c: (f32, f32), r: f32, color: [u8; 4]);
    fn stroke_circle(&mut self, c: (f32, f32), r: f32, width: f32, color: [u8; 4]);
}

/// Sink that draws through the world-space [`MapRenderer`] trait.
pub struct MapRendererSink<'a> {
    pub renderer: &'a mut dyn MapRenderer,
}

impl GlyphSink for MapRendererSink<'_> {
    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: [u8; 4]) {
        self.renderer.draw_line(a.0, a.1, b.0, b.1, width, color);
    }
    fn fill_convex(&mut self, pts: &[(f32, f32)], color: [u8; 4]) {
        self.renderer.fill_polygon(pts, color);
    }
    fn fill_circle(&mut self, c: (f32, f32), r: f32, color: [u8; 4]) {
        self.renderer.fill_circle(c.0, c.1, r, color);
    }
    fn stroke_circle(&mut self, c: (f32, f32), r: f32, width: f32, color: [u8; 4]) {
        self.renderer.stroke_circle(c.0, c.1, r, width, color);
    }
}

/// Sink that draws directly to an egui painter in screen space.
pub struct PainterSink<'a> {
    pub painter: &'a egui::Painter,
}

impl PainterSink<'_> {
    /// Keep strokes visible when zoomed far out.
    const MIN_WIDTH: f32 = 0.75;
}

fn to_c32(c: [u8; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

impl GlyphSink for PainterSink<'_> {
    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: [u8; 4]) {
        let stroke = egui::Stroke::new(width.max(Self::MIN_WIDTH), to_c32(color));
        self.painter.line_segment([egui::pos2(a.0, a.1), egui::pos2(b.0, b.1)], stroke);
    }
    fn fill_convex(&mut self, pts: &[(f32, f32)], color: [u8; 4]) {
        let pts: Vec<egui::Pos2> = pts.iter().map(|p| egui::pos2(p.0, p.1)).collect();
        self.painter.add(egui::Shape::convex_polygon(pts, to_c32(color), egui::Stroke::NONE));
    }
    fn fill_circle(&mut self, c: (f32, f32), r: f32, color: [u8; 4]) {
        self.painter.circle_filled(egui::pos2(c.0, c.1), r, to_c32(color));
    }
    fn stroke_circle(&mut self, c: (f32, f32), r: f32, width: f32, color: [u8; 4]) {
        let stroke = egui::Stroke::new(width.max(Self::MIN_WIDTH), to_c32(color));
        self.painter.circle_stroke(egui::pos2(c.0, c.1), r, stroke);
    }
}

/// Colours used by the glyphs, derived from the theme's ink colour plus fixed accents.
#[derive(Clone, Copy, Debug)]
pub struct DecorPalette {
    /// Outline colour (theme wall colour).
    pub ink: [u8; 4],
    /// Light translucent ink for object tops.
    pub shade: [u8; 4],
    /// Denser translucent ink for recessed or heavy parts.
    pub dark: [u8; 4],
    pub water: [u8; 4],
    pub water_edge: [u8; 4],
    pub fire: [u8; 4],
    pub ember: [u8; 4],
    pub leaf: [u8; 4],
}

impl DecorPalette {
    /// Build a palette from an ink colour. The ink's alpha also fades the accents,
    /// so a translucent ink yields a uniformly ghosted glyph.
    pub fn from_ink(ink: [u8; 4]) -> Self {
        let a = ink[3] as f32 / 255.0;
        let fade = |c: [u8; 4]| [c[0], c[1], c[2], (c[3] as f32 * a) as u8];
        Self {
            ink,
            shade: fade([ink[0], ink[1], ink[2], 45]),
            dark: fade([ink[0], ink[1], ink[2], 110]),
            water: fade([80, 140, 210, 110]),
            water_edge: fade([50, 100, 170, 220]),
            fire: fade([235, 120, 30, 230]),
            ember: fade([250, 200, 60, 230]),
            leaf: fade([60, 130, 60, 230]),
        }
    }
}

/// Local-space drawing helper. `x`, `y` and radii are in units of the half-size `s`;
/// stroke widths are also in local units so the whole glyph scales together.
struct Glyph<'a> {
    sink: &'a mut dyn GlyphSink,
    cx: f32,
    cy: f32,
    s: f32,
    sclx: f32,
    scly: f32,
    cos: f32,
    sin: f32,
}

impl Glyph<'_> {
    fn savg(&self) -> f32 {
        (self.sclx.abs() + self.scly.abs()) / 2.0
    }

    fn width(&self, w: f32) -> f32 {
        w * self.s * self.savg()
    }

    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        let dx = x * self.s * self.sclx;
        let dy = y * self.s * self.scly;
        (self.cx + dx * self.cos - dy * self.sin, self.cy + dx * self.sin + dy * self.cos)
    }

    fn uniform(&self) -> bool {
        (self.sclx - self.scly).abs() < 1e-3
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, w: f32, color: [u8; 4]) {
        let a = self.map(x1, y1);
        let b = self.map(x2, y2);
        let width = self.width(w);
        self.sink.line(a, b, width, color);
    }

    fn polyline(&mut self, pts: &[(f32, f32)], w: f32, color: [u8; 4]) {
        for pair in pts.windows(2) {
            self.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1, w, color);
        }
    }

    fn poly_stroke(&mut self, pts: &[(f32, f32)], w: f32, color: [u8; 4]) {
        self.polyline(pts, w, color);
        if let (Some(f), Some(l)) = (pts.first(), pts.last()) {
            self.line(l.0, l.1, f.0, f.1, w, color);
        }
    }

    /// Fill a convex polygon given in local space.
    fn poly_fill(&mut self, pts: &[(f32, f32)], color: [u8; 4]) {
        let mapped: Vec<(f32, f32)> = pts.iter().map(|p| self.map(p.0, p.1)).collect();
        self.sink.fill_convex(&mapped, color);
    }

    fn rect_pts(x0: f32, y0: f32, x1: f32, y1: f32) -> [(f32, f32); 4] {
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }

    fn rect_fill(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) {
        self.poly_fill(&Self::rect_pts(x0, y0, x1, y1), color);
    }

    fn rect_stroke(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, color: [u8; 4]) {
        self.poly_stroke(&Self::rect_pts(x0, y0, x1, y1), w, color);
    }

    fn ellipse_pts(x: f32, y: f32, rx: f32, ry: f32, n: usize) -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * TAU;
                (x + rx * a.cos(), y + ry * a.sin())
            })
            .collect()
    }

    fn circle_fill(&mut self, x: f32, y: f32, r: f32, color: [u8; 4]) {
        if self.uniform() {
            let c = self.map(x, y);
            self.sink.fill_circle(c, r * self.s * self.sclx.abs(), color);
        } else {
            self.poly_fill(&Self::ellipse_pts(x, y, r, r, 24), color);
        }
    }

    fn circle_stroke(&mut self, x: f32, y: f32, r: f32, w: f32, color: [u8; 4]) {
        if self.uniform() {
            let c = self.map(x, y);
            let width = self.width(w);
            self.sink.stroke_circle(c, r * self.s * self.sclx.abs(), width, color);
        } else {
            self.poly_stroke(&Self::ellipse_pts(x, y, r, r, 24), w, color);
        }
    }

    fn ellipse_fill(&mut self, x: f32, y: f32, rx: f32, ry: f32, color: [u8; 4]) {
        self.poly_fill(&Self::ellipse_pts(x, y, rx, ry, 24), color);
    }

    fn ellipse_stroke(&mut self, x: f32, y: f32, rx: f32, ry: f32, w: f32, color: [u8; 4]) {
        self.poly_stroke(&Self::ellipse_pts(x, y, rx, ry, 24), w, color);
    }

    /// Stroke an arc from angle `a0` to `a1` (radians, clockwise on screen).
    fn arc(&mut self, x: f32, y: f32, r: f32, a0: f32, a1: f32, w: f32, color: [u8; 4]) {
        let n = (((a1 - a0).abs() / TAU) * 32.0).ceil().max(2.0) as usize;
        let pts: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / n as f32;
                (x + r * a.cos(), y + r * a.sin())
            })
            .collect();
        self.polyline(&pts, w, color);
    }

    /// Quadratic bezier as a polyline.
    fn bezier(&mut self, p0: (f32, f32), c: (f32, f32), p1: (f32, f32), w: f32, color: [u8; 4]) {
        let pts: Vec<(f32, f32)> = (0..=8).map(|i| bezier_at(p0, c, p1, i as f32 / 8.0)).collect();
        self.polyline(&pts, w, color);
    }

    /// Filled teardrop flame: base at (x, y), tip `h` above, half-width `hw`.
    fn flame(&mut self, x: f32, y: f32, hw: f32, h: f32, color: [u8; 4]) {
        let pts = [
            (x, y - h),
            (x + hw * 0.55, y - h * 0.62),
            (x + hw, y - h * 0.22),
            (x + hw * 0.7, y),
            (x - hw * 0.7, y),
            (x - hw, y - h * 0.22),
            (x - hw * 0.55, y - h * 0.62),
        ];
        self.poly_fill(&pts, color);
    }

    /// Filled leaf (pointed oval) centred at (x, y) with its axis along `angle`.
    fn leaf(&mut self, x: f32, y: f32, len: f32, angle: f32, color: [u8; 4]) {
        let (c, s) = (angle.cos(), angle.sin());
        let hl = len / 2.0;
        let hw = len * 0.22;
        let local = [(-hl, 0.0), (-hl * 0.3, -hw), (hl * 0.35, -hw * 0.8), (hl, 0.0), (hl * 0.35, hw * 0.8), (-hl * 0.3, hw)];
        let pts: Vec<(f32, f32)> = local.iter().map(|(lx, ly)| (x + lx * c - ly * s, y + lx * s + ly * c)).collect();
        self.poly_fill(&pts, color);
    }
}

fn bezier_at(p0: (f32, f32), c: (f32, f32), p1: (f32, f32), t: f32) -> (f32, f32) {
    let it = 1.0 - t;
    (
        it * it * p0.0 + 2.0 * it * t * c.0 + t * t * p1.0,
        it * it * p0.1 + 2.0 * it * t * c.1 + t * t * p1.1,
    )
}

/// Draw one decor glyph.
///
/// * `cx`, `cy` — centre in the sink's coordinate space.
/// * `s` — base half-size in the sink's units (before per-item scale).
/// * `sclx`, `scly` — per-item scale; `deg` — rotation in degrees.
#[allow(clippy::too_many_arguments)]
pub fn draw_decor(
    sink: &mut dyn GlyphSink,
    kind: DecorType,
    cx: f32,
    cy: f32,
    s: f32,
    sclx: f32,
    scly: f32,
    deg: f32,
    pal: &DecorPalette,
) {
    let rad = deg.to_radians();
    let mut g = Glyph { sink, cx, cy, s, sclx, scly, cos: rad.cos(), sin: rad.sin() };
    let ink = pal.ink;
    let shade = pal.shade;
    let dark = pal.dark;

    match kind {
        DecorType::Table => {
            g.rect_fill(-1.0, -0.6, 1.0, 0.6, shade);
            g.rect_stroke(-1.0, -0.6, 1.0, 0.6, W_MAIN, ink);
            for &(x, y) in &[(-0.9, -0.5), (0.9, -0.5), (-0.9, 0.5), (0.9, 0.5)] {
                g.circle_fill(x, y, 0.1, ink);
            }
        }
        DecorType::Chair => {
            g.rect_stroke(-0.4, -0.4, 0.4, 0.4, W_THIN, ink);
            g.line(-0.4, -0.4, 0.4, -0.4, W_BOLD, ink);
        }
        DecorType::Bench => {
            g.rect_stroke(-0.9, -0.25, 0.9, 0.25, W_MAIN, ink);
        }
        DecorType::Chest => {
            g.rect_fill(-0.65, -0.45, 0.65, 0.45, shade);
            // Lid
            g.rect_fill(-0.65, -0.45, 0.65, -0.05, dark);
            g.rect_stroke(-0.65, -0.45, 0.65, 0.45, W_MAIN, ink);
            g.line(-0.65, -0.05, 0.65, -0.05, W_MAIN, ink);
            // Iron bands
            g.line(-0.38, -0.45, -0.38, 0.45, W_HAIR, ink);
            g.line(0.38, -0.45, 0.38, 0.45, W_HAIR, ink);
            // Clasp
            g.rect_fill(-0.1, -0.16, 0.1, 0.1, ink);
        }
        DecorType::Barrel => {
            g.circle_stroke(0.0, 0.0, 0.55, W_MAIN, ink);
            g.line(-0.5, -0.15, 0.5, -0.15, W_THIN, ink);
            g.line(-0.5, 0.15, 0.5, 0.15, W_THIN, ink);
        }
        DecorType::Crate => {
            g.rect_stroke(-0.55, -0.55, 0.55, 0.55, W_MAIN, ink);
            g.line(-0.55, -0.55, 0.55, 0.55, W_THIN, ink);
            g.line(0.55, -0.55, -0.55, 0.55, W_THIN, ink);
        }
        DecorType::Pillar => {
            g.circle_fill(0.0, 0.0, 0.5, ink);
            g.circle_stroke(0.0, 0.0, 0.5, W_THIN, shade_on(ink));
            g.circle_stroke(0.0, 0.0, 0.35, W_HAIR, shade_on(ink));
        }
        DecorType::StairsUp | DecorType::StairsDown => {
            let steps = 6;
            // Treads darken toward +y (the lower end of the flight)
            for i in 0..steps {
                let y0 = -1.0 + 2.0 * i as f32 / steps as f32;
                let y1 = -1.0 + 2.0 * (i + 1) as f32 / steps as f32;
                let alpha = 12 + (i as f32 / (steps - 1) as f32 * 70.0) as u8;
                let tread = fade_to(ink, alpha, pal.ink[3]);
                g.rect_fill(-1.0, y0, 1.0, y1, tread);
                if i > 0 {
                    g.line(-1.0, y0, 1.0, y0, W_HAIR, ink);
                }
            }
            g.rect_stroke(-1.0, -1.0, 1.0, 1.0, W_MAIN, ink);
            // Direction arrow: shaft plus filled head
            let up = kind == DecorType::StairsUp;
            let dir = if up { -1.0 } else { 1.0 };
            g.line(0.0, -0.55 * dir, 0.0, 0.45 * dir, W_BOLD, ink);
            g.poly_fill(&[(0.0, 0.85 * dir), (-0.38, 0.3 * dir), (0.38, 0.3 * dir)], ink);
        }
        DecorType::Ladder => {
            g.line(-0.32, -1.0, -0.32, 1.0, W_MAIN, ink);
            g.line(0.32, -1.0, 0.32, 1.0, W_MAIN, ink);
            for i in 0..5 {
                let y = -0.8 + i as f32 * 0.4;
                g.line(-0.32, y, 0.32, y, W_THIN, ink);
            }
        }
        DecorType::Altar => {
            // Stepped base, slab with a runner, two candles
            g.rect_stroke(-0.98, -0.58, 0.98, 0.58, W_HAIR, ink);
            g.rect_stroke(-0.8, -0.42, 0.8, 0.42, W_MAIN, ink);
            g.line(-0.22, -0.42, -0.22, 0.42, W_THIN, ink);
            g.line(0.22, -0.42, 0.22, 0.42, W_THIN, ink);
            for x in [-0.55, 0.55] {
                g.circle_stroke(x, 0.0, 0.11, W_THIN, ink);
            }
        }
        DecorType::Fountain => {
            g.circle_fill(0.0, 0.0, 0.85, pal.water);
            g.circle_stroke(0.0, 0.0, 0.92, W_MAIN, ink);
            g.circle_stroke(0.0, 0.0, 0.8, W_HAIR, ink);
            // Ripples around the spout
            for i in 0..4 {
                let a = i as f32 / 4.0 * TAU + TAU / 8.0;
                g.arc(0.0, 0.0, 0.55, a - 0.35, a + 0.35, W_HAIR, pal.water_edge);
            }
            // Central pedestal and spout
            g.circle_fill(0.0, 0.0, 0.3, shade);
            g.circle_stroke(0.0, 0.0, 0.3, W_THIN, ink);
            g.circle_fill(0.0, 0.0, 0.12, ink);
        }
        DecorType::Well => {
            // Stone ring around dark water
            g.circle_fill(0.0, 0.0, 0.72, dark);
            g.circle_fill(0.0, 0.0, 0.46, pal.water_edge);
            g.circle_stroke(0.0, 0.0, 0.72, W_MAIN, ink);
            g.circle_stroke(0.0, 0.0, 0.46, W_THIN, ink);
            // Stone joints
            for i in 0..8 {
                let a = i as f32 / 8.0 * TAU;
                g.line(0.46 * a.cos(), 0.46 * a.sin(), 0.72 * a.cos(), 0.72 * a.sin(), W_HAIR, ink);
            }
            // Winch beam and posts
            g.line(-0.95, 0.0, 0.95, 0.0, W_MAIN, ink);
            g.rect_fill(-1.0, -0.13, -0.82, 0.13, ink);
            g.rect_fill(0.82, -0.13, 1.0, 0.13, ink);
        }
        DecorType::Brazier => {
            // Tripod legs
            for i in 0..3 {
                let a = PI / 2.0 + i as f32 / 3.0 * TAU;
                g.line(0.4 * a.cos(), 0.4 * a.sin(), 0.78 * a.cos(), 0.78 * a.sin(), W_MAIN, ink);
                g.circle_fill(0.78 * a.cos(), 0.78 * a.sin(), 0.09, ink);
            }
            g.circle_fill(0.0, 0.0, 0.5, dark);
            g.circle_stroke(0.0, 0.0, 0.5, W_MAIN, ink);
            g.flame(-0.18, 0.3, 0.14, 0.5, pal.fire);
            g.flame(0.18, 0.3, 0.14, 0.45, pal.fire);
            g.flame(0.0, 0.32, 0.18, 0.72, pal.fire);
            g.flame(0.0, 0.32, 0.09, 0.38, pal.ember);
        }
        DecorType::Fireplace => {
            // Hearth floor, back wall and jambs
            g.rect_fill(-0.8, -0.6, 0.8, 0.45, shade);
            g.line(-0.9, -0.6, 0.9, -0.6, W_BOLD, ink);
            g.line(-0.8, -0.6, -0.8, 0.45, W_BOLD, ink);
            g.line(0.8, -0.6, 0.8, 0.45, W_BOLD, ink);
            // Hearthstone in front
            g.rect_stroke(-0.95, 0.45, 0.95, 0.68, W_HAIR, ink);
            // Logs
            g.line(-0.42, 0.3, 0.42, 0.05, W_MAIN, ink);
            g.line(0.42, 0.3, -0.42, 0.05, W_MAIN, ink);
            // Fire
            g.flame(-0.2, 0.15, 0.15, 0.5, pal.fire);
            g.flame(0.22, 0.15, 0.15, 0.45, pal.fire);
            g.flame(0.0, 0.18, 0.2, 0.7, pal.fire);
            g.flame(0.0, 0.18, 0.1, 0.36, pal.ember);
        }
        DecorType::Statue => {
            // Stick figure on a plinth
            g.rect_stroke(-0.72, -0.72, 0.72, 0.72, W_THIN, ink);
            g.circle_stroke(0.0, -0.4, 0.17, W_MAIN, ink);
            g.line(0.0, -0.23, 0.0, 0.22, W_MAIN, ink);
            g.line(-0.32, -0.02, 0.32, -0.02, W_MAIN, ink);
            g.line(0.0, 0.22, -0.24, 0.58, W_MAIN, ink);
            g.line(0.0, 0.22, 0.24, 0.58, W_MAIN, ink);
        }
        DecorType::Throne => {
            // Chair with armrests and a crown-shaped high back
            g.rect_stroke(-0.45, -0.45, 0.45, 0.45, W_THIN, ink);
            g.line(-0.6, -0.45, 0.6, -0.45, W_BOLD, ink);
            g.polyline(
                &[(-0.6, -0.45), (-0.6, -0.8), (-0.32, -0.62), (0.0, -0.95), (0.32, -0.62), (0.6, -0.8), (0.6, -0.45)],
                W_MAIN, ink,
            );
            g.line(-0.6, -0.45, -0.6, 0.3, W_BOLD, ink);
            g.line(0.6, -0.45, 0.6, 0.3, W_BOLD, ink);
        }
        DecorType::Bed => {
            g.rect_fill(-0.6, -0.9, 0.6, 0.9, shade);
            g.rect_stroke(-0.6, -0.9, 0.6, 0.9, W_THIN, ink);
            // Headboard
            g.rect_fill(-0.66, -1.0, 0.66, -0.86, ink);
            // Pillow
            g.rect_fill(-0.42, -0.74, 0.42, -0.42, shade);
            g.rect_stroke(-0.42, -0.74, 0.42, -0.42, W_HAIR, ink);
            // Blanket with a turned-down edge
            g.rect_fill(-0.6, -0.25, 0.6, 0.9, shade);
            g.line(-0.6, -0.25, 0.6, -0.25, W_THIN, ink);
            g.line(-0.6, -0.1, 0.6, -0.1, W_HAIR, ink);
        }
        DecorType::Bookshelf => {
            g.rect_fill(-0.85, -0.4, 0.85, 0.4, shade);
            // Rows of book spines, varying width and height
            let widths = [0.14, 0.1, 0.18, 0.12, 0.16, 0.1, 0.14, 0.12, 0.16, 0.1];
            let heights = [0.3, 0.34, 0.26, 0.32, 0.3, 0.36, 0.28, 0.32, 0.34, 0.26];
            for (row, base) in [(0usize, 0.0f32), (1, 0.4)] {
                let mut x = -0.78;
                for i in 0..widths.len() {
                    let j = (i + row * 3) % widths.len();
                    let (w, h) = (widths[j], heights[j]);
                    if x + w > 0.8 { break; }
                    let fill = if (i + row) % 2 == 0 { dark } else { ink };
                    g.rect_fill(x, base - 0.02, x + w, base - h, fill);
                    x += w + 0.03;
                }
            }
            g.rect_stroke(-0.85, -0.4, 0.85, 0.4, W_MAIN, ink);
            g.line(-0.85, 0.0, 0.85, 0.0, W_THIN, ink);
        }
        DecorType::Trap => {
            // Dashed pressure plate
            let d = 0.6;
            let seg = d * 2.0 / 5.0;
            for i in 0..5 {
                if i % 2 == 1 { continue; }
                let t0 = -d + i as f32 * seg;
                let t1 = t0 + seg;
                g.line(t0, -d, t1, -d, W_THIN, ink);
                g.line(t0, d, t1, d, W_THIN, ink);
                g.line(-d, t0, -d, t1, W_THIN, ink);
                g.line(d, t0, d, t1, W_THIN, ink);
            }
            // Warning triangle with exclamation mark
            let tri = [(0.0, -0.5), (0.48, 0.38), (-0.48, 0.38)];
            g.poly_fill(&tri, shade);
            g.poly_stroke(&tri, W_MAIN, ink);
            g.line(0.0, -0.22, 0.0, 0.12, W_MAIN, ink);
            g.circle_fill(0.0, 0.26, 0.06, ink);
        }
        DecorType::Rubble => {
            // Irregular stones
            let rocks: [(f32, f32, f32, f32); 6] = [
                (0.0, 0.05, 0.3, 0.2),
                (-0.55, -0.35, 0.22, 0.6),
                (0.5, -0.3, 0.26, 1.1),
                (-0.4, 0.5, 0.18, 2.0),
                (0.45, 0.5, 0.2, 0.3),
                (-0.05, -0.6, 0.16, 1.5),
            ];
            let bumps = [1.0, 0.75, 0.95, 0.7, 1.0, 0.8, 0.9];
            for &(x, y, r, phase) in &rocks {
                let pts: Vec<(f32, f32)> = bumps
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let a = i as f32 / bumps.len() as f32 * TAU + phase;
                        (x + r * b * a.cos(), y + r * b * 0.85 * a.sin())
                    })
                    .collect();
                g.poly_fill(&pts, dark);
                g.poly_stroke(&pts, W_HAIR, ink);
            }
            for &(x, y) in &[(0.75, 0.05), (-0.8, 0.15), (0.2, 0.8), (-0.15, -0.9)] {
                g.circle_fill(x, y, 0.06, ink);
            }
        }
        DecorType::Bones => {
            // Two crossed long bones
            for &(x1, y1, x2, y2) in &[(-0.7, 0.55, 0.5, -0.3), (-0.5, -0.3, 0.7, 0.55)] {
                g.line(x1, y1, x2, y2, W_MAIN, ink);
                for &(bx, by) in &[(x1, y1), (x2, y2)] {
                    let dx: f32 = x2 - x1;
                    let dy: f32 = y2 - y1;
                    let len = (dx * dx + dy * dy).sqrt();
                    let (nx, ny) = (-dy / len * 0.09, dx / len * 0.09);
                    g.circle_fill(bx + nx, by + ny, 0.1, ink);
                    g.circle_fill(bx - nx, by - ny, 0.1, ink);
                }
            }
            // Skull
            let (sx, sy) = (0.35, -0.55);
            g.circle_fill(sx, sy, 0.32, shade);
            g.circle_stroke(sx, sy, 0.32, W_THIN, ink);
            g.rect_fill(sx - 0.18, sy + 0.22, sx + 0.18, sy + 0.4, shade);
            g.rect_stroke(sx - 0.18, sy + 0.22, sx + 0.18, sy + 0.4, W_HAIR, ink);
            g.circle_fill(sx - 0.12, sy - 0.05, 0.08, ink);
            g.circle_fill(sx + 0.12, sy - 0.05, 0.08, ink);
            for i in 0..3 {
                let x = sx - 0.1 + i as f32 * 0.1;
                g.line(x, sy + 0.24, x, sy + 0.38, W_HAIR, ink);
            }
        }
        DecorType::Web => {
            let spokes = 8;
            for i in 0..spokes {
                let a = i as f32 / spokes as f32 * TAU;
                g.line(0.0, 0.0, 0.95 * a.cos(), 0.95 * a.sin(), W_HAIR, ink);
            }
            // Sagging rings
            for r in [0.3, 0.55, 0.82] {
                let mut pts = Vec::with_capacity(spokes * 2 + 1);
                for i in 0..=spokes {
                    let a = i as f32 / spokes as f32 * TAU;
                    pts.push((r * a.cos(), r * a.sin()));
                    if i < spokes {
                        let am = a + TAU / spokes as f32 / 2.0;
                        let rm = r * 0.86;
                        pts.push((rm * am.cos(), rm * am.sin()));
                    }
                }
                g.polyline(&pts, W_HAIR, ink);
            }
        }
        DecorType::Door => {
            // Opening between two jambs, leaf swung open, swing arc
            g.rect_fill(-0.78, -0.1, -0.6, 0.1, ink);
            g.rect_fill(0.6, -0.1, 0.78, 0.1, ink);
            g.line(-0.6, 0.0, 0.6, 0.0, W_HAIR, ink);
            g.rect_fill(-0.6, -1.2, -0.46, 0.0, dark);
            g.rect_stroke(-0.6, -1.2, -0.46, 0.0, W_THIN, ink);
            g.arc(-0.6, 0.0, 1.2, -PI / 2.0, 0.0, W_HAIR, ink);
        }
        DecorType::Gate => {
            // Portcullis: frame, bars, cross-bands, spiked ends
            g.rect_fill(-0.85, -0.7, -0.68, 0.6, ink);
            g.rect_fill(0.68, -0.7, 0.85, 0.6, ink);
            g.line(-0.85, -0.7, 0.85, -0.7, W_BOLD, ink);
            for i in 0..5 {
                let x = -0.5 + i as f32 * 0.25;
                g.line(x, -0.7, x, 0.55, W_THIN, ink);
                g.poly_fill(&[(x - 0.07, 0.55), (x + 0.07, 0.55), (x, 0.78)], ink);
            }
            g.line(-0.68, -0.25, 0.68, -0.25, W_THIN, ink);
            g.line(-0.68, 0.2, 0.68, 0.2, W_THIN, ink);
        }
        DecorType::Vines => {
            let tendrils: [((f32, f32), (f32, f32), (f32, f32)); 5] = [
                ((0.0, 0.1), (-0.5, -0.5), (-0.75, -0.85)),
                ((0.0, 0.1), (0.55, -0.35), (0.85, -0.7)),
                ((0.0, 0.1), (-0.65, 0.3), (-0.85, 0.65)),
                ((0.0, 0.1), (0.3, 0.55), (0.6, 0.85)),
                ((0.0, 0.1), (0.05, -0.45), (-0.15, -0.95)),
            ];
            for &(p0, c, p1) in &tendrils {
                g.bezier(p0, c, p1, W_THIN, pal.leaf);
                for (t, side) in [(0.45, 1.0f32), (0.75, -1.0)] {
                    let p = bezier_at(p0, c, p1, t);
                    let q = bezier_at(p0, c, p1, t + 0.05);
                    let tangent = (q.1 - p.1).atan2(q.0 - p.0);
                    g.leaf(p.0, p.1, 0.34, tangent + side * 0.9, pal.leaf);
                }
            }
        }
        DecorType::OfferingMouth => {
            // Carved stone face with a gaping mouth
            g.circle_fill(0.0, 0.0, 0.88, shade);
            g.circle_stroke(0.0, 0.0, 0.88, W_MAIN, ink);
            // Brows
            g.bezier((-0.62, -0.32), (-0.35, -0.6), (-0.08, -0.42), W_MAIN, ink);
            g.bezier((0.62, -0.32), (0.35, -0.6), (0.08, -0.42), W_MAIN, ink);
            // Eyes
            g.poly_fill(&[(-0.55, -0.22), (-0.35, -0.36), (-0.15, -0.22), (-0.35, -0.1)], ink);
            g.poly_fill(&[(0.55, -0.22), (0.35, -0.36), (0.15, -0.22), (0.35, -0.1)], ink);
            // Nose
            g.line(0.0, -0.15, -0.1, 0.12, W_THIN, ink);
            g.line(-0.1, 0.12, 0.1, 0.12, W_THIN, ink);
            // Mouth
            g.ellipse_fill(0.0, 0.42, 0.42, 0.26, ink);
            g.ellipse_stroke(0.0, 0.42, 0.42, 0.26, W_THIN, ink);
        }
        DecorType::Scales => {
            // Base and post
            g.poly_fill(&[(-0.4, 0.95), (0.4, 0.95), (0.25, 0.8), (-0.25, 0.8)], ink);
            g.line(0.0, 0.8, 0.0, -0.75, W_MAIN, ink);
            g.poly_fill(&[(-0.12, -0.92), (0.12, -0.92), (0.0, -0.7)], ink);
            // Beam, tilted
            g.line(-0.85, -0.5, 0.85, -0.72, W_MAIN, ink);
            // Chains and pans
            for (px, py) in [(-0.85f32, -0.5f32), (0.85, -0.72)] {
                let pan_y = py + 0.6;
                g.line(px, py, px - 0.2, pan_y, W_HAIR, ink);
                g.line(px, py, px + 0.2, pan_y, W_HAIR, ink);
                let mut pts = vec![(px - 0.26, pan_y), (px + 0.26, pan_y)];
                for i in 1..6 {
                    let a = i as f32 / 6.0 * PI;
                    pts.push((px + 0.26 * a.cos(), pan_y + 0.16 * a.sin()));
                }
                g.poly_fill(&pts, dark);
                g.line(px - 0.26, pan_y, px + 0.26, pan_y, W_THIN, ink);
            }
        }
        DecorType::Crack => {
            let path = [(0.0, -1.0), (0.15, -0.55), (-0.2, -0.2), (0.12, 0.2), (-0.12, 0.55), (0.06, 0.95)];
            let widths = [W_THIN, W_MAIN, W_BOLD, W_MAIN, W_THIN];
            for (pair, w) in path.windows(2).zip(widths) {
                g.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1, w, ink);
            }
            g.polyline(&[(-0.2, -0.2), (-0.5, 0.05), (-0.62, 0.35)], W_HAIR, ink);
            g.polyline(&[(0.12, 0.2), (0.45, 0.32), (0.55, 0.6)], W_HAIR, ink);
        }
        DecorType::Stream => {
            // Filled meandering channel flowing along -y..+y
            let n = 12;
            let half = 0.28;
            let bank = |t: f32| (t * TAU * 1.2).sin() * 0.2;
            for i in 0..n {
                let t0 = i as f32 / n as f32;
                let t1 = (i + 1) as f32 / n as f32;
                let (y0, y1) = (-1.0 + 2.0 * t0, -1.0 + 2.0 * t1);
                let (b0, b1) = (bank(t0), bank(t1));
                g.poly_fill(&[(b0 - half, y0), (b0 + half, y0), (b1 + half, y1), (b1 - half, y1)], pal.water);
                g.line(b0 - half, y0, b1 - half, y1, W_THIN, pal.water_edge);
                g.line(b0 + half, y0, b1 + half, y1, W_THIN, pal.water_edge);
            }
            for i in 0..4 {
                let t = (i as f32 + 0.5) / 4.0;
                let y = -1.0 + 2.0 * t;
                let b = bank(t);
                g.line(b - 0.12, y - 0.04, b + 0.12, y + 0.04, W_HAIR, pal.water_edge);
            }
        }
        DecorType::Pool => {
            g.ellipse_fill(0.0, 0.0, 0.9, 0.7, pal.water);
            g.ellipse_stroke(0.0, 0.0, 0.9, 0.7, W_MAIN, pal.water_edge);
            g.ellipse_stroke(0.0, 0.0, 0.55, 0.4, W_HAIR, pal.water_edge);
            g.ellipse_stroke(0.0, 0.0, 0.25, 0.16, W_HAIR, pal.water_edge);
            // Stones around the rim
            for i in 0..9 {
                let a = i as f32 / 9.0 * TAU + 0.3;
                g.circle_fill(0.93 * a.cos(), 0.74 * a.sin(), 0.07, dark);
            }
        }
    }
}

/// A lighter version of the ink for highlights drawn on top of solid ink.
fn shade_on(ink: [u8; 4]) -> [u8; 4] {
    [
        ink[0].saturating_add(70),
        ink[1].saturating_add(70),
        ink[2].saturating_add(70),
        ink[3],
    ]
}

/// Ink with a specific alpha, faded by the ink's own alpha.
fn fade_to(ink: [u8; 4], alpha: u8, ink_alpha: u8) -> [u8; 4] {
    [ink[0], ink[1], ink[2], (alpha as f32 * ink_alpha as f32 / 255.0) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ImageRenderer;

    /// Renders every decor type to a contact sheet PNG for visual inspection.
    /// Run with `cargo test decor_contact_sheet -- --ignored`; set `DECOR_SHEET_OUT`
    /// to choose the output path.
    #[test]
    #[ignore]
    fn decor_contact_sheet() {
        let cell: u32 = std::env::var("DECOR_SHEET_CELL").ok().and_then(|v| v.parse().ok()).unwrap_or(96);
        let cols = 8u32;
        let rows = (DecorType::ALL.len() as u32).div_ceil(cols);
        let mut r = ImageRenderer::new(cell * cols, cell * rows, 1.0);
        r.fill_rect(0.0, 0.0, (cell * cols) as f32, (cell * rows) as f32, [255, 255, 255, 255]);
        let pal = DecorPalette::from_ink([0, 0, 0, 255]);
        for (i, kind) in DecorType::ALL.iter().enumerate() {
            let cx = ((i as u32 % cols) * cell + cell / 2) as f32;
            let cy = ((i as u32 / cols) * cell + cell / 2) as f32;
            r.stroke_rect(cx - cell as f32 / 2.0, cy - cell as f32 / 2.0, cell as f32, cell as f32, 1.0, [200, 200, 200, 255]);
            let mut sink = MapRendererSink { renderer: &mut r };
            draw_decor(&mut sink, *kind, cx, cy, cell as f32 * 0.3, 1.0, 1.0, 0.0, &pal);
        }
        let out = std::env::var("DECOR_SHEET_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("decor_contact_sheet.png"));
        r.image.save(&out).unwrap();
    }
}
