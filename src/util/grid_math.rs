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

/// View transform: maps between world coordinates and screen coordinates
#[derive(Clone, Debug)]
pub struct ViewTransform {
    pub offset: egui::Vec2,
    pub zoom: f32,
    pub canvas_rect: egui::Rect,
}

impl ViewTransform {
    pub fn new(offset: egui::Vec2, zoom: f32, canvas_rect: egui::Rect) -> Self {
        Self {
            offset,
            zoom,
            canvas_rect,
        }
    }

    /// World position -> screen position
    pub fn world_to_screen(&self, world: egui::Pos2) -> egui::Pos2 {
        egui::pos2(
            world.x * self.zoom + self.offset.x + self.canvas_rect.min.x,
            world.y * self.zoom + self.offset.y + self.canvas_rect.min.y,
        )
    }

    /// Screen position -> world position
    pub fn screen_to_world(&self, screen: egui::Pos2) -> egui::Pos2 {
        egui::pos2(
            (screen.x - self.canvas_rect.min.x - self.offset.x) / self.zoom,
            (screen.y - self.canvas_rect.min.y - self.offset.y) / self.zoom,
        )
    }

    /// Scale a world distance to screen distance
    #[allow(dead_code)]
    pub fn scale(&self, world_dist: f32) -> f32 {
        world_dist * self.zoom
    }
}
