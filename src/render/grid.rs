use crate::model::SpatialLayout;
use crate::render::traits::MapRenderer;
use crate::util::GRID_PX;

pub fn draw_grid(renderer: &mut dyn MapRenderer, layout: &SpatialLayout, theme_grid_color: [u8; 4]) {
    let (min_x, min_y, max_x, max_y) = layout.extents();

    let mut light = theme_grid_color;
    light[3] = 60;

    let mut heavy = theme_grid_color;
    heavy[3] = 120;

    for x in min_x..=max_x {
        let color = if x % 5 == 0 { heavy } else { light };
        let wx = x as f32 * GRID_PX;
        renderer.draw_line(
            wx,
            min_y as f32 * GRID_PX,
            wx,
            max_y as f32 * GRID_PX,
            0.5,
            color,
        );
    }
    for y in min_y..=max_y {
        let color = if y % 5 == 0 { heavy } else { light };
        let wy = y as f32 * GRID_PX;
        renderer.draw_line(
            min_x as f32 * GRID_PX,
            wy,
            max_x as f32 * GRID_PX,
            wy,
            0.5,
            color,
        );
    }
}
