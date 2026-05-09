/// Pixels per grid square at default zoom
pub const GRID_PX: f32 = 20.0;

/// Convert grid position to world pixel position
pub fn grid_to_world(grid: i32) -> f32 {
    grid as f32 * GRID_PX
}

/// Convert world pixel position to grid position
pub fn world_to_grid(world: f32) -> i32 {
    (world / GRID_PX).round() as i32
}

/// View transform: maps between world coordinates and screen coordinates.
/// Optionally applies rotation (in radians) around the canvas center.
#[derive(Clone, Debug)]
pub struct ViewTransform {
    pub offset: egui::Vec2,
    pub zoom: f32,
    pub canvas_rect: egui::Rect,
    /// Rotation in radians, applied around the canvas center after zoom+offset.
    pub rotation: f32,
}

impl ViewTransform {
    pub fn new(offset: egui::Vec2, zoom: f32, canvas_rect: egui::Rect) -> Self {
        Self {
            offset,
            zoom,
            canvas_rect,
            rotation: 0.0,
        }
    }

    pub fn with_rotation(mut self, rotation: f32) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn world_to_screen(&self, world: egui::Pos2) -> egui::Pos2 {
        let sx = world.x * self.zoom + self.offset.x + self.canvas_rect.min.x;
        let sy = world.y * self.zoom + self.offset.y + self.canvas_rect.min.y;
        if self.rotation == 0.0 {
            return egui::pos2(sx, sy);
        }
        let cx = self.canvas_rect.center().x;
        let cy = self.canvas_rect.center().y;
        let dx = sx - cx;
        let dy = sy - cy;
        let (sin, cos) = self.rotation.sin_cos();
        egui::pos2(cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
    }

    pub fn screen_to_world(&self, screen: egui::Pos2) -> egui::Pos2 {
        let (sx, sy) = if self.rotation == 0.0 {
            (screen.x, screen.y)
        } else {
            let cx = self.canvas_rect.center().x;
            let cy = self.canvas_rect.center().y;
            let dx = screen.x - cx;
            let dy = screen.y - cy;
            let (sin, cos) = (-self.rotation).sin_cos();
            (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
        };
        egui::pos2(
            (sx - self.canvas_rect.min.x - self.offset.x) / self.zoom,
            (sy - self.canvas_rect.min.y - self.offset.y) / self.zoom,
        )
    }
}

/// Distance from point `p` to the closest point on line segment `a`-`b`.
pub fn point_to_segment_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.dot(ab);
    if len_sq < 0.001 {
        return p.distance(a);
    }
    let t = (ap.dot(ab) / len_sq).clamp(0.0, 1.0);
    let closest = a + ab * t;
    p.distance(closest)
}
