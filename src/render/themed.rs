use crate::model::*;
use crate::render::doors::draw_door_icon;
use crate::render::grid::draw_grid;
use crate::render::hatching::draw_hatching;
use crate::render::traits::MapRenderer;
use crate::util::GRID_PX;

pub fn render_themed(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    show_grid: bool,
    show_labels: bool,
    show_notes: bool,
    show_secrets: bool,
) {
    let (min_x, min_y, max_x, max_y) = layout.extents();
    let margin = 2;
    let x0 = (min_x - margin) as f32 * GRID_PX;
    let y0 = (min_y - margin) as f32 * GRID_PX;
    let w = (max_x - min_x + margin * 2) as f32 * GRID_PX;
    let h = (max_y - min_y + margin * 2) as f32 * GRID_PX;

    // 1. Background
    renderer.fill_rect(x0, y0, w, h, theme.bg_color);

    // 2. Room floors
    for rl in &layout.rooms {
        let rx = rl.x as f32 * GRID_PX;
        let ry = rl.y as f32 * GRID_PX;
        let rw = rl.width as f32 * GRID_PX;
        let rh = rl.height as f32 * GRID_PX;
        renderer.fill_rect(rx, ry, rw, rh, theme.floor_color);
    }

    // 3. Corridor floors
    for corridor in &layout.corridors {
        let cw = corridor.width as f32 * GRID_PX;
        for pair in corridor.waypoints.windows(2) {
            let x1 = pair[0].x as f32 * GRID_PX;
            let y1 = pair[0].y as f32 * GRID_PX;
            let x2 = pair[1].x as f32 * GRID_PX;
            let y2 = pair[1].y as f32 * GRID_PX;

            // Fill corridor segment as a rect
            let min_x = x1.min(x2) - cw / 2.0;
            let min_y = y1.min(y2) - cw / 2.0;
            let max_x = x1.max(x2) + cw / 2.0;
            let max_y = y1.max(y2) + cw / 2.0;
            renderer.fill_rect(min_x, min_y, max_x - min_x, max_y - min_y, theme.floor_color);
        }
    }

    // 4. Grid
    if show_grid {
        let grid_color = [180, 180, 180, 80];
        draw_grid(renderer, layout, grid_color);
    }

    // 5. Room walls
    for rl in &layout.rooms {
        let rx = rl.x as f32 * GRID_PX;
        let ry = rl.y as f32 * GRID_PX;
        let rw = rl.width as f32 * GRID_PX;
        let rh = rl.height as f32 * GRID_PX;
        renderer.stroke_rect(rx, ry, rw, rh, 2.0, theme.wall_color);

        // 7. Dyson hatching
        if theme.hatching {
            // Top wall - exterior is above
            draw_hatching(renderer, rx, ry, rx + rw, ry, -1.0, theme.wall_color);
            // Bottom wall - exterior is below
            draw_hatching(renderer, rx, ry + rh, rx + rw, ry + rh, 1.0, theme.wall_color);
            // Left wall - exterior is left
            draw_hatching(renderer, rx, ry, rx, ry + rh, -1.0, theme.wall_color);
            // Right wall - exterior is right
            draw_hatching(renderer, rx + rw, ry, rx + rw, ry + rh, 1.0, theme.wall_color);
        }
    }

    // 8. Door icons at corridor-room junctions
    for edge in &graph.connections {
        if !show_secrets && edge.connection.connection_type == ConnectionType::Secret {
            continue;
        }

        if let Some(corridor) = layout.corridors.iter().find(|c| c.connection_id == edge.connection.id) {
            if let Some(first) = corridor.waypoints.first() {
                let dx = first.x as f32 * GRID_PX;
                let dy = first.y as f32 * GRID_PX;
                draw_door_icon(renderer, dx, dy, edge.connection.connection_type, true, theme.wall_color);
            }
        }
    }

    // 9. Room labels
    if show_labels {
        for rl in &layout.rooms {
            if let Some(room) = graph.room_by_id(&rl.room_id) {
                let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                renderer.draw_text(&room.label, cx, cy, 10.0, [60, 60, 60, 255]);

                // 10. DM notes
                if show_notes && !room.notes.is_empty() {
                    renderer.draw_text(&room.notes, cx, cy + 14.0, 7.0, [120, 120, 120, 255]);
                }
            }
        }
    }
}
