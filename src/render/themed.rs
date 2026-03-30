use std::collections::HashSet;

use crate::model::*;
use crate::render::hatching::{draw_exterior_shading, ShadingParams};
use crate::render::traits::MapRenderer;
use crate::util::GRID_PX;

/// Options controlling which elements to render.
pub struct RenderOptions {
    pub show_grid: bool,
    pub show_labels: bool,
    pub show_notes: bool,
    pub show_secrets: bool,
}

pub fn render_themed(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    options: &RenderOptions,
) {
    let floor = build_floor_set(layout, graph);

    render_background(renderer, layout, theme);
    render_exterior_shading(renderer, layout, &floor, theme);

    for rl in &layout.rooms {
        render_room_floor(renderer, rl, graph, theme);
    }
    for corridor in &layout.corridors {
        render_corridor_floor(renderer, corridor, theme);
    }
    if theme.corridor_chamfer != ChamferStyle::Sharp {
        for corridor in &layout.corridors {
            render_corridor_chamfers(renderer, corridor, theme);
        }
    }
    if options.show_grid {
        render_grid(renderer, &floor);
    }
    // Render room decor (after floors/grid, before walls)
    for rl in &layout.rooms {
        render_decor(renderer, rl, graph, theme);
    }
    for rl in &layout.rooms {
        render_room_walls(renderer, rl, graph, theme);
    }
    // Redraw corridor floors at circular room junctions to punch through
    // the circle wall stroke that covers the corridor opening.
    repair_circle_junctions(renderer, graph, layout, theme);
    for corridor in &layout.corridors {
        render_corridor_walls(renderer, corridor, &floor, theme);
    }
    render_doors(renderer, graph, layout, theme, options);
    if options.show_labels {
        render_labels(renderer, graph, layout, options);
    }
}

/// Step 1+2: Background fill and exterior shading.
pub fn render_background(
    renderer: &mut dyn MapRenderer,
    layout: &SpatialLayout,
    theme: &Theme,
) {
    let (ext_min_x, ext_min_y, ext_max_x, ext_max_y) = layout.extents();
    let margin = 2;
    let x0 = (ext_min_x - margin) as f32 * GRID_PX;
    let y0 = (ext_min_y - margin) as f32 * GRID_PX;
    let w = (ext_max_x - ext_min_x + margin * 2) as f32 * GRID_PX;
    let h = (ext_max_y - ext_min_y + margin * 2) as f32 * GRID_PX;

    renderer.fill_rect(x0, y0, w, h, theme.bg_color);
}

pub fn render_exterior_shading(
    renderer: &mut dyn MapRenderer,
    layout: &SpatialLayout,
    floor: &HashSet<(i32, i32)>,
    theme: &Theme,
) {
    if theme.exterior_shading {
        let params = ShadingParams {
            radius: theme.shading_radius,
            style: theme.shading_style,
            density: theme.hatching_density,
            color: theme.wall_color,
        };
        draw_exterior_shading(renderer, layout, floor, &params);
    }
}

/// Render one room's floor.
pub fn render_room_floor(
    renderer: &mut dyn MapRenderer,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    theme: &Theme,
) {
    render_room_floor_with_color(renderer, rl, graph, theme.floor_color);
}

/// Render one room's floor with a specific color.
pub fn render_room_floor_with_color(
    renderer: &mut dyn MapRenderer,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    color: [u8; 4],
) {
    let rx = rl.x as f32 * GRID_PX;
    let ry = rl.y as f32 * GRID_PX;
    let rw = rl.width as f32 * GRID_PX;
    let rh = rl.height as f32 * GRID_PX;
    let is_circle = graph.room_by_id(&rl.room_id)
        .is_some_and(|r| r.shape == RoomShape::Circle);

    if is_circle {
        let cx = rx + rw / 2.0;
        let cy = ry + rh / 2.0;
        let r = rw.min(rh) / 2.0;
        renderer.fill_circle(cx, cy, r, color);
    } else {
        renderer.fill_rect(rx, ry, rw, rh, color);
    }
}

/// Render one corridor's floor.
pub fn render_corridor_floor(
    renderer: &mut dyn MapRenderer,
    corridor: &CorridorSegment,
    theme: &Theme,
) {
    render_corridor_floor_with_color(renderer, corridor, theme.floor_color);
}

/// Render one corridor's floor with a specific color.
pub fn render_corridor_floor_with_color(
    renderer: &mut dyn MapRenderer,
    corridor: &CorridorSegment,
    color: [u8; 4],
) {
    let cw = corridor.width as i32;
    let half = cw / 2;
    for pair in corridor.waypoints.windows(2) {
        let min_gx = pair[0].x.min(pair[1].x) - half;
        let min_gy = pair[0].y.min(pair[1].y) - half;
        let max_gx = pair[0].x.max(pair[1].x) - half + cw;
        let max_gy = pair[0].y.max(pair[1].y) - half + cw;

        let px = min_gx as f32 * GRID_PX;
        let py = min_gy as f32 * GRID_PX;
        let pw = (max_gx - min_gx) as f32 * GRID_PX;
        let ph = (max_gy - min_gy) as f32 * GRID_PX;
        renderer.fill_rect(px, py, pw, ph, color);
    }
}

/// Render chamfered corners on corridor turns by drawing over the sharp outside corners.
pub fn render_corridor_chamfers(
    renderer: &mut dyn MapRenderer,
    corridor: &CorridorSegment,
    theme: &Theme,
) {
    if corridor.waypoints.len() < 3 {
        return;
    }
    let cw = corridor.width as f32;
    let half = cw / 2.0;
    let chamfer_r = half * GRID_PX;

    // Iterate over interior waypoints (each is a potential corner)
    for triple in corridor.waypoints.windows(3) {
        let prev = &triple[0];
        let curr = &triple[1];
        let next = &triple[2];

        // Determine direction of incoming and outgoing segments
        let dx_in = (curr.x - prev.x).signum();
        let dy_in = (curr.y - prev.y).signum();
        let dx_out = (next.x - curr.x).signum();
        let dy_out = (next.y - curr.y).signum();

        // Only chamfer when direction changes (a real corner)
        if (dx_in, dy_in) == (dx_out, dy_out) {
            continue;
        }

        // The corridor center at this waypoint
        let cx = curr.x as f32 * GRID_PX;
        let cy = curr.y as f32 * GRID_PX;

        // The outside corner is opposite to the turn direction.
        // For a horizontal-to-vertical turn, the outside corner depends on the signs.
        // Compute the outside corner position relative to the waypoint center.
        // The outside corner is at (cx + dx_in * half * GRID_PX, cy + dy_out * half * GRID_PX)
        // when incoming is horizontal and outgoing is vertical, or vice versa.

        let (ocx, ocy) = if dx_in != 0 && dy_out != 0 {
            // Incoming horizontal, outgoing vertical
            (cx + dx_in as f32 * half * GRID_PX, cy + dy_out as f32 * half * GRID_PX)
        } else if dy_in != 0 && dx_out != 0 {
            // Incoming vertical, outgoing horizontal
            (cx + dx_out as f32 * half * GRID_PX, cy + dy_in as f32 * half * GRID_PX)
        } else {
            continue;
        };

        match theme.corridor_chamfer {
            ChamferStyle::Sharp => {}
            ChamferStyle::Rounded => {
                // Draw a filled circle of background color at the outside corner,
                // then redraw a quarter-circle of floor color for the inner curve.
                // Simpler: fill bg circle to "bite" the corner, the inner edge creates the round.
                renderer.fill_circle(ocx, ocy, chamfer_r, theme.bg_color);
            }
            ChamferStyle::Angled => {
                // Draw a triangle of background color to cut the corner at 45°.
                // The triangle vertices are:
                //   ocx, ocy (the corner itself)
                //   ocx - dx * chamfer_r, ocy (along one edge)
                //   ocx, ocy - dy * chamfer_r (along the other edge)
                // Since MapRenderer doesn't have a triangle primitive, approximate
                // with multiple thin lines from the corner inward.
                let steps = (chamfer_r / 0.5).ceil() as i32;
                let edge1_x;
                let edge1_y;
                let edge2_x;
                let edge2_y;

                if dx_in != 0 && dy_out != 0 {
                    edge1_x = ocx - dx_in as f32 * chamfer_r;
                    edge1_y = ocy;
                    edge2_x = ocx;
                    edge2_y = ocy - dy_out as f32 * chamfer_r;
                } else {
                    edge1_x = ocx;
                    edge1_y = ocy - dy_in as f32 * chamfer_r;
                    edge2_x = ocx - dx_out as f32 * chamfer_r;
                    edge2_y = ocy;
                }

                // Fill the triangle by drawing scanlines
                for i in 0..=steps {
                    let t = i as f32 / steps as f32;
                    let x1 = ocx + (edge1_x - ocx) * t;
                    let y1 = ocy + (edge1_y - ocy) * t;
                    let x2 = ocx + (edge2_x - ocx) * t;
                    let y2 = ocy + (edge2_y - ocy) * t;
                    renderer.draw_line(x1, y1, x2, y2, 1.0, theme.bg_color);
                }
            }
        }
    }
}

/// Render grid lines only inside floor cells.
///
/// Each interior grid edge shared by two adjacent floor cells is drawn once.
/// Edges on the boundary of the floor set are skipped (walls handle those).
pub fn render_grid(
    renderer: &mut dyn MapRenderer,
    floor: &HashSet<(i32, i32)>,
) {
    let grid_color = [80, 80, 80, 180];

    for &(fx, fy) in floor {
        let px = fx as f32 * GRID_PX;
        let py = fy as f32 * GRID_PX;

        // Draw the right edge if the neighbor to the right is also floor
        if floor.contains(&(fx + 1, fy)) {
            let rx = (fx + 1) as f32 * GRID_PX;
            renderer.draw_line(rx, py, rx, py + GRID_PX, 0.5, grid_color);
        }
        // Draw the bottom edge if the neighbor below is also floor
        if floor.contains(&(fx, fy + 1)) {
            let by = (fy + 1) as f32 * GRID_PX;
            renderer.draw_line(px, by, px + GRID_PX, by, 0.5, grid_color);
        }
    }
}

/// Render one room's walls.
pub fn render_room_walls(
    renderer: &mut dyn MapRenderer,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    theme: &Theme,
) {
    let wall_w = 2.0;
    let rx = rl.x as f32 * GRID_PX;
    let ry = rl.y as f32 * GRID_PX;
    let rw = rl.width as f32 * GRID_PX;
    let rh = rl.height as f32 * GRID_PX;
    let is_circle = graph.room_by_id(&rl.room_id)
        .is_some_and(|r| r.shape == RoomShape::Circle);

    if is_circle {
        let cx = rx + rw / 2.0;
        let cy = ry + rh / 2.0;
        let r = rw.min(rh) / 2.0;
        renderer.stroke_circle(cx, cy, r, wall_w, theme.wall_color);
    } else {
        renderer.stroke_rect(rx, ry, rw, rh, wall_w, theme.wall_color);
    }
}

/// Render one corridor's walls, skipping edges where adjacent cells are floor.
pub fn render_corridor_walls(
    renderer: &mut dyn MapRenderer,
    corridor: &CorridorSegment,
    floor: &HashSet<(i32, i32)>,
    theme: &Theme,
) {
    let wall_w = 2.0;
    let cw = corridor.width as i32;
    let half = cw / 2;
    for pair in corridor.waypoints.windows(2) {
        let min_gx = pair[0].x.min(pair[1].x) - half;
        let min_gy = pair[0].y.min(pair[1].y) - half;
        let max_gx = pair[0].x.max(pair[1].x) - half + cw;
        let max_gy = pair[0].y.max(pair[1].y) - half + cw;

        let px1 = min_gx as f32 * GRID_PX;
        let py1 = min_gy as f32 * GRID_PX;
        let px2 = max_gx as f32 * GRID_PX;
        let py2 = max_gy as f32 * GRID_PX;

        // Top wall
        for x in min_gx..max_gx {
            if !floor.contains(&(x, min_gy - 1)) {
                let lx = x as f32 * GRID_PX;
                renderer.draw_line(lx, py1, lx + GRID_PX, py1, wall_w, theme.wall_color);
            }
        }
        // Bottom wall
        for x in min_gx..max_gx {
            if !floor.contains(&(x, max_gy)) {
                let lx = x as f32 * GRID_PX;
                renderer.draw_line(lx, py2, lx + GRID_PX, py2, wall_w, theme.wall_color);
            }
        }
        // Left wall
        for y in min_gy..max_gy {
            if !floor.contains(&(min_gx - 1, y)) {
                let ly = y as f32 * GRID_PX;
                renderer.draw_line(px1, ly, px1, ly + GRID_PX, wall_w, theme.wall_color);
            }
        }
        // Right wall
        for y in min_gy..max_gy {
            if !floor.contains(&(max_gx, y)) {
                let ly = y as f32 * GRID_PX;
                renderer.draw_line(px2, ly, px2, ly + GRID_PX, wall_w, theme.wall_color);
            }
        }
    }
}

/// Redraw corridor floor segments that overlap with circular room bounding boxes.
/// This repairs the circle wall stroke that would otherwise cover the corridor opening.
pub fn repair_circle_junctions(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
) {
    // Collect circular room bounding rects
    let circle_rooms: Vec<&RoomLayout> = layout.rooms.iter().filter(|rl| {
        graph.room_by_id(&rl.room_id)
            .is_some_and(|r| r.shape == RoomShape::Circle)
    }).collect();

    if circle_rooms.is_empty() {
        return;
    }

    for corridor in &layout.corridors {
        let cw = corridor.width as i32;
        let half = cw / 2;
        for pair in corridor.waypoints.windows(2) {
            let min_gx = pair[0].x.min(pair[1].x) - half;
            let min_gy = pair[0].y.min(pair[1].y) - half;
            let max_gx = pair[0].x.max(pair[1].x) - half + cw;
            let max_gy = pair[0].y.max(pair[1].y) - half + cw;

            for rl in &circle_rooms {
                let room_max_x = rl.x + rl.width as i32;
                let room_max_y = rl.y + rl.height as i32;
                // Check if this corridor segment overlaps with the room bounds
                let overlap_min_x = min_gx.max(rl.x);
                let overlap_min_y = min_gy.max(rl.y);
                let overlap_max_x = max_gx.min(room_max_x);
                let overlap_max_y = max_gy.min(room_max_y);
                if overlap_min_x < overlap_max_x && overlap_min_y < overlap_max_y {
                    // Redraw the corridor floor in this overlap region
                    let px = overlap_min_x as f32 * GRID_PX;
                    let py = overlap_min_y as f32 * GRID_PX;
                    let pw = (overlap_max_x - overlap_min_x) as f32 * GRID_PX;
                    let ph = (overlap_max_y - overlap_min_y) as f32 * GRID_PX;
                    renderer.fill_rect(px, py, pw, ph, theme.floor_color);
                }
            }
        }
    }
}

/// Render decorative elements inside rooms.
pub fn render_decor(
    renderer: &mut dyn MapRenderer,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    theme: &Theme,
) {
    let Some(room) = graph.room_by_id(&rl.room_id) else { return };
    if room.decor.is_empty() {
        return;
    }

    let room_px_x = rl.x as f32 * GRID_PX;
    let room_px_y = rl.y as f32 * GRID_PX;
    let color = theme.wall_color;

    for decor in &room.decor {
        let cx = room_px_x + decor.x * GRID_PX;
        let cy = room_px_y + decor.y * GRID_PX;
        let s = GRID_PX * 0.4; // symbol half-size

        match decor.decor_type {
            DecorType::Table => {
                // Rounded rectangle
                renderer.fill_rect(cx - s, cy - s * 0.6, s * 2.0, s * 1.2, color);
            }
            DecorType::Chest => {
                // Small filled square with border
                let cs = s * 0.7;
                renderer.fill_rect(cx - cs, cy - cs, cs * 2.0, cs * 2.0, color);
            }
            DecorType::Pillar => {
                // Filled circle
                renderer.fill_circle(cx, cy, s * 0.5, color);
            }
            DecorType::StairsUp => {
                // Stair lines going up (parallel horizontal lines in a box)
                renderer.stroke_rect(cx - s, cy - s, s * 2.0, s * 2.0, 1.0, color);
                let steps = 4;
                for i in 1..steps {
                    let y = cy - s + (i as f32 / steps as f32) * s * 2.0;
                    renderer.draw_line(cx - s, y, cx + s, y, 1.0, color);
                }
                // Arrow pointing up
                renderer.draw_line(cx, cy - s, cx - s * 0.3, cy - s * 0.5, 1.0, color);
                renderer.draw_line(cx, cy - s, cx + s * 0.3, cy - s * 0.5, 1.0, color);
            }
            DecorType::StairsDown => {
                // Same as up but arrow points down
                renderer.stroke_rect(cx - s, cy - s, s * 2.0, s * 2.0, 1.0, color);
                let steps = 4;
                for i in 1..steps {
                    let y = cy - s + (i as f32 / steps as f32) * s * 2.0;
                    renderer.draw_line(cx - s, y, cx + s, y, 1.0, color);
                }
                renderer.draw_line(cx, cy + s, cx - s * 0.3, cy + s * 0.5, 1.0, color);
                renderer.draw_line(cx, cy + s, cx + s * 0.3, cy + s * 0.5, 1.0, color);
            }
            DecorType::Altar => {
                // Cross shape
                let t = s * 0.25;
                renderer.fill_rect(cx - t, cy - s, t * 2.0, s * 2.0, color);
                renderer.fill_rect(cx - s * 0.7, cy - t, s * 1.4, t * 2.0, color);
            }
            DecorType::Fountain => {
                // Concentric circles
                renderer.stroke_circle(cx, cy, s * 0.8, 1.0, color);
                renderer.stroke_circle(cx, cy, s * 0.4, 1.0, color);
                renderer.fill_circle(cx, cy, s * 0.15, color);
            }
            DecorType::Trap => {
                // X mark
                renderer.draw_line(cx - s * 0.6, cy - s * 0.6, cx + s * 0.6, cy + s * 0.6, 1.5, color);
                renderer.draw_line(cx + s * 0.6, cy - s * 0.6, cx - s * 0.6, cy + s * 0.6, 1.5, color);
            }
            DecorType::Rubble => {
                // Scattered dots
                for &(dx, dy) in &[(0.0, 0.0), (-0.4, -0.3), (0.3, -0.2), (-0.2, 0.4), (0.4, 0.3)] {
                    renderer.fill_circle(cx + dx * s, cy + dy * s, s * 0.15, color);
                }
            }
        }
    }
}

/// Render door symbols on corridors.
pub fn render_doors(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    options: &RenderOptions,
) {
    for edge in &graph.connections {
        if !options.show_secrets && edge.connection.connection_type == ConnectionType::Secret {
            continue;
        }
        if edge.connection.connection_type == ConnectionType::Open {
            continue;
        }

        let corridor = layout.corridors.iter().find(|c| c.connection_id == edge.connection.id);
        let Some(corridor) = corridor else { continue };
        if corridor.waypoints.len() < 2 { continue; }

        let dw = edge.connection.door_width() as f32;
        let door_depth = 0.3;

        let room_ids = [&edge.source_room_id, &edge.target_room_id];
        let wp_ends = [&corridor.waypoints[0], corridor.waypoints.last().unwrap()];

        for (room_id, wp) in room_ids.iter().zip(wp_ends.iter()) {
            let Some(rl) = layout.room_by_id(room_id) else { continue };

            let wp_cx = wp.x as f32;
            let wp_cy = wp.y as f32;
            let dist_right = (wp_cx - (rl.x + rl.width as i32) as f32).abs();
            let dist_left = (wp_cx - rl.x as f32).abs();
            let dist_bottom = (wp_cy - (rl.y + rl.height as i32) as f32).abs();
            let dist_top = (wp_cy - rl.y as f32).abs();
            let min_dist = dist_right.min(dist_left).min(dist_bottom).min(dist_top);
            let dw_half = dw / 2.0;

            let (dx1, dy1, dx2, dy2) = if min_dist == dist_right {
                let wall_x = (rl.x + rl.width as i32) as f32;
                (wall_x - door_depth / 2.0, wp_cy - dw_half, wall_x + door_depth / 2.0, wp_cy + dw_half)
            } else if min_dist == dist_left {
                let wall_x = rl.x as f32;
                (wall_x - door_depth / 2.0, wp_cy - dw_half, wall_x + door_depth / 2.0, wp_cy + dw_half)
            } else if min_dist == dist_bottom {
                let wall_y = (rl.y + rl.height as i32) as f32;
                (wp_cx - dw_half, wall_y - door_depth / 2.0, wp_cx + dw_half, wall_y + door_depth / 2.0)
            } else {
                let wall_y = rl.y as f32;
                (wp_cx - dw_half, wall_y - door_depth / 2.0, wp_cx + dw_half, wall_y + door_depth / 2.0)
            };

            let px = dx1 * GRID_PX;
            let py = dy1 * GRID_PX;
            let pw = (dx2 - dx1) * GRID_PX;
            let ph = (dy2 - dy1) * GRID_PX;

            match edge.connection.connection_type {
                ConnectionType::Open => {}
                ConnectionType::Door | ConnectionType::OneWay => {
                    renderer.fill_rect(px, py, pw, ph, [255, 255, 255, 255]);
                    renderer.stroke_rect(px, py, pw, ph, 1.0, theme.wall_color);
                }
                ConnectionType::Locked => {
                    renderer.fill_rect(px, py, pw, ph, [255, 255, 255, 255]);
                    renderer.stroke_rect(px, py, pw, ph, 1.0, theme.wall_color);
                    let cx = px + pw / 2.0;
                    let cy = py + ph / 2.0;
                    let r = pw.min(ph) * 0.15;
                    renderer.fill_rect(cx - r, cy - r, r * 2.0, r * 2.0, theme.wall_color);
                }
                ConnectionType::Secret => {
                    let cx = px + pw / 2.0;
                    let cy = py + ph / 2.0;
                    renderer.draw_text("S", cx, cy, 6.0, theme.wall_color);
                }
            }
        }
    }
}

/// Render room labels and notes.
pub fn render_labels(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    options: &RenderOptions,
) {
    for rl in &layout.rooms {
        if let Some(room) = graph.room_by_id(&rl.room_id) {
            let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
            let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
            renderer.draw_text(&room.label, cx, cy, 10.0, [60, 60, 60, 255]);

            if options.show_notes && !room.notes.is_empty() {
                renderer.draw_text(&room.notes, cx, cy + 14.0, 7.0, [120, 120, 120, 255]);
            }
        }
    }
}

/// Build the set of all floor cells from room and corridor geometry.
pub fn build_floor_set(layout: &SpatialLayout, graph: &DungeonGraph) -> HashSet<(i32, i32)> {
    let mut floor: HashSet<(i32, i32)> = HashSet::new();
    for rl in &layout.rooms {
        let is_circle = graph.room_by_id(&rl.room_id)
            .is_some_and(|r| r.shape == RoomShape::Circle);
        if is_circle {
            let cx = rl.x as f32 + rl.width as f32 / 2.0;
            let cy = rl.y as f32 + rl.height as f32 / 2.0;
            let r = (rl.width.min(rl.height) as f32) / 2.0;
            for y in rl.y..(rl.y + rl.height as i32) {
                for x in rl.x..(rl.x + rl.width as i32) {
                    let cell_cx = x as f32 + 0.5;
                    let cell_cy = y as f32 + 0.5;
                    let dx = cell_cx - cx;
                    let dy = cell_cy - cy;
                    if dx * dx + dy * dy <= r * r {
                        floor.insert((x, y));
                    }
                }
            }
        } else {
            for y in rl.y..(rl.y + rl.height as i32) {
                for x in rl.x..(rl.x + rl.width as i32) {
                    floor.insert((x, y));
                }
            }
        }
    }
    for corridor in &layout.corridors {
        let cw = corridor.width as i32;
        let half = cw / 2;
        for pair in corridor.waypoints.windows(2) {
            let min_x = pair[0].x.min(pair[1].x) - half;
            let min_y = pair[0].y.min(pair[1].y) - half;
            let max_x = pair[0].x.max(pair[1].x) - half + cw;
            let max_y = pair[0].y.max(pair[1].y) - half + cw;
            for y in min_y..max_y {
                for x in min_x..max_x {
                    floor.insert((x, y));
                }
            }
        }
    }
    floor
}
