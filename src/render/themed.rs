use std::collections::HashSet;

use crate::model::*;
use crate::render::hatching::draw_exterior_shading;
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
    let (ext_min_x, ext_min_y, ext_max_x, ext_max_y) = layout.extents();
    let margin = 2;
    let x0 = (ext_min_x - margin) as f32 * GRID_PX;
    let y0 = (ext_min_y - margin) as f32 * GRID_PX;
    let w = (ext_max_x - ext_min_x + margin * 2) as f32 * GRID_PX;
    let h = (ext_max_y - ext_min_y + margin * 2) as f32 * GRID_PX;

    // Build floor cell set for grid clipping and hatching exterior detection
    let mut floor: HashSet<(i32, i32)> = HashSet::new();
    for rl in &layout.rooms {
        for y in rl.y..(rl.y + rl.height as i32) {
            for x in rl.x..(rl.x + rl.width as i32) {
                floor.insert((x, y));
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

    // 1. Background
    renderer.fill_rect(x0, y0, w, h, theme.bg_color);

    // 2. Exterior shading (drawn on background, BEFORE floors)
    if theme.exterior_shading {
        draw_exterior_shading(
            renderer, graph, layout, &floor,
            theme.shading_radius,
            theme.shading_style,
            theme.hatching_density,
            theme.wall_color,
        );
    }

    // 3. Room floors (covers hatching that went inside)
    for rl in &layout.rooms {
        let rx = rl.x as f32 * GRID_PX;
        let ry = rl.y as f32 * GRID_PX;
        let rw = rl.width as f32 * GRID_PX;
        let rh = rl.height as f32 * GRID_PX;
        let is_circle = graph.room_by_id(&rl.room_id)
            .map_or(false, |r| r.shape == RoomShape::Circle);

        if is_circle {
            let cx = rx + rw / 2.0;
            let cy = ry + rh / 2.0;
            let r = rw.min(rh) / 2.0;
            renderer.fill_circle(cx, cy, r, theme.floor_color);
        } else {
            renderer.fill_rect(rx, ry, rw, rh, theme.floor_color);
        }
    }

    // 4. Corridor floors
    for corridor in &layout.corridors {
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
            renderer.fill_rect(px, py, pw, ph, theme.floor_color);
        }
    }

    // 5. Grid lines (only inside floor cells)
    if show_grid {
        let grid_color = [140, 140, 140, 100];
        // Draw grid lines that pass through floor cells
        let mut drawn_x: HashSet<i32> = HashSet::new();
        let mut drawn_y: HashSet<i32> = HashSet::new();
        for &(fx, fy) in &floor {
            drawn_x.insert(fx);
            drawn_x.insert(fx + 1);
            drawn_y.insert(fy);
            drawn_y.insert(fy + 1);
        }
        // Find floor extents for line endpoints per grid line
        for &gx in &drawn_x {
            let min_fy = floor.iter().filter(|(x, _)| *x == gx || *x == gx - 1).map(|(_, y)| *y).min();
            let max_fy = floor.iter().filter(|(x, _)| *x == gx || *x == gx - 1).map(|(_, y)| *y + 1).max();
            if let (Some(min_y), Some(max_y)) = (min_fy, max_fy) {
                let wx = gx as f32 * GRID_PX;
                renderer.draw_line(wx, min_y as f32 * GRID_PX, wx, max_y as f32 * GRID_PX, 0.5, grid_color);
            }
        }
        for &gy in &drawn_y {
            let min_fx = floor.iter().filter(|(_, y)| *y == gy || *y == gy - 1).map(|(x, _)| *x).min();
            let max_fx = floor.iter().filter(|(_, y)| *y == gy || *y == gy - 1).map(|(x, _)| *x + 1).max();
            if let (Some(min_x), Some(max_x)) = (min_fx, max_fx) {
                let wy = gy as f32 * GRID_PX;
                renderer.draw_line(min_x as f32 * GRID_PX, wy, max_x as f32 * GRID_PX, wy, 0.5, grid_color);
            }
        }
    }

    // 6. Room walls
    let wall_w = 2.0;
    for rl in &layout.rooms {
        let rx = rl.x as f32 * GRID_PX;
        let ry = rl.y as f32 * GRID_PX;
        let rw = rl.width as f32 * GRID_PX;
        let rh = rl.height as f32 * GRID_PX;
        let is_circle = graph.room_by_id(&rl.room_id)
            .map_or(false, |r| r.shape == RoomShape::Circle);

        if is_circle {
            let cx = rx + rw / 2.0;
            let cy = ry + rh / 2.0;
            let r = rw.min(rh) / 2.0;
            renderer.stroke_circle(cx, cy, r, wall_w, theme.wall_color);
        } else {
            renderer.stroke_rect(rx, ry, rw, rh, wall_w, theme.wall_color);
        }
    }

    // 7. Corridor walls — draw individual wall segments, skipping edges
    //    where ALL adjacent cells on the exterior side are floor.
    //    This prevents walls at elbow interiors where corridor segments meet.
    for corridor in &layout.corridors {
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

            // For each wall, draw only the portions where the exterior is non-floor.
            // Split the wall into per-cell segments and skip cells backed by floor.

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

    // 8. Door symbols
    for edge in &graph.connections {
        if !show_secrets && edge.connection.connection_type == ConnectionType::Secret {
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

    // 9. Room labels
    if show_labels {
        for rl in &layout.rooms {
            if let Some(room) = graph.room_by_id(&rl.room_id) {
                let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                renderer.draw_text(&room.label, cx, cy, 10.0, [60, 60, 60, 255]);

                if show_notes && !room.notes.is_empty() {
                    renderer.draw_text(&room.notes, cx, cy + 14.0, 7.0, [120, 120, 120, 255]);
                }
            }
        }
    }
}
