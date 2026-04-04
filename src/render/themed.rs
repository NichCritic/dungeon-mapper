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
    /// Whether to render decor items. Set to false when decor is drawn as a live overlay.
    pub show_decor: bool,
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

    // Collect baked marching squares contour segments from cave rooms (used by shading)
    let contour_segments: Vec<(f32, f32, f32, f32)> = graph.rooms.iter()
        .filter_map(|r| r.cave_data.as_ref())
        .flat_map(|c| c.contour_segments.iter().copied())
        .collect();
    render_exterior_shading(renderer, layout, &floor, theme, &contour_segments);

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
    // Render room decor and elevation sections (after floors/grid, before walls)
    for rl in &layout.rooms {
        if options.show_decor {
            render_decor(renderer, rl, graph, theme);
        }
        render_elevation_sections(renderer, rl, graph, theme);
    }
    for rl in &layout.rooms {
        // Cave rooms use baked marching squares contour segments
        let room = graph.room_by_id(&rl.room_id);
        if let Some(cave) = room.and_then(|r| {
            if r.shape == RoomShape::Cave { r.cave_data.as_ref() } else { None }
        }) {
            if !cave.contour_segments.is_empty() {
                for &(x1, y1, x2, y2) in &cave.contour_segments {
                    renderer.draw_line(x1, y1, x2, y2, 2.0, theme.wall_color);
                }
                continue;
            }
        }
        render_room_walls(renderer, rl, graph, theme);
    }
    // Redraw corridor floors at circular room junctions to punch through
    // the circle wall stroke that covers the corridor opening.
    repair_circle_junctions(renderer, graph, layout, theme);
    // Build set of cells inside cave rooms (so corridor walls don't double-draw there)
    let cave_cells = build_cave_cell_set(layout, graph);
    for corridor in &layout.corridors {
        render_corridor_walls(renderer, corridor, &floor, theme, &cave_cells);
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
    contour_segments: &[(f32, f32, f32, f32)],
) {
    if theme.exterior_shading {
        let params = ShadingParams {
            radius: theme.shading_radius,
            style: theme.shading_style,
            density: theme.hatching_density,
            color: theme.wall_color,
        };
        draw_exterior_shading(renderer, layout, floor, &params, contour_segments);
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
    let room = graph.room_by_id(&rl.room_id);
    let shape = room.map(|r| r.shape).unwrap_or_default();

    match shape {
        RoomShape::Circle => {
            let cx = rx + rw / 2.0;
            let cy = ry + rh / 2.0;
            let r = rw.min(rh) / 2.0;
            renderer.fill_circle(cx, cy, r, color);
        }
        RoomShape::Cave => {
            if let Some(cave) = room.and_then(|r| r.cave_data.as_ref()) {
                if !cave.cells.is_empty() {
                    let w = rl.width as usize;
                    for ly in 0..rl.height as usize {
                        for lx in 0..w {
                            if cave.cells.get(ly * w + lx).copied().unwrap_or(false) {
                                let px = (rl.x as usize + lx) as f32 * GRID_PX;
                                let py = (rl.y as usize + ly) as f32 * GRID_PX;
                                renderer.fill_rect(px, py, GRID_PX, GRID_PX, color);
                            }
                        }
                    }
                    return;
                }
            }
            // No cells yet — draw as full rectangle
            renderer.fill_rect(rx, ry, rw, rh, color);
        }
        RoomShape::Rectangle => {
            renderer.fill_rect(rx, ry, rw, rh, color);
        }
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
    let room = graph.room_by_id(&rl.room_id);
    let shape = room.map(|r| r.shape).unwrap_or_default();

    match shape {
        RoomShape::Circle => {
            let cx = rx + rw / 2.0;
            let cy = ry + rh / 2.0;
            let r = rw.min(rh) / 2.0;
            renderer.stroke_circle(cx, cy, r, wall_w, theme.wall_color);
        }
        RoomShape::Cave => {
            // Cave walls are drawn per-cell-edge, similar to corridor walls.
            // For each floor cell, draw wall lines on edges where neighbor is not floor.
            if let Some(cave) = room.and_then(|r| r.cave_data.as_ref()) {
                if !cave.cells.is_empty() {
                    let w = rl.width as i32;
                    let h = rl.height as i32;
                    let is_floor = |lx: i32, ly: i32| -> bool {
                        if lx < 0 || ly < 0 || lx >= w || ly >= h { return false; }
                        cave.cells.get((ly * w + lx) as usize).copied().unwrap_or(false)
                    };
                    for ly in 0..h {
                        for lx in 0..w {
                            if !is_floor(lx, ly) { continue; }
                            let px = (rl.x + lx) as f32 * GRID_PX;
                            let py = (rl.y + ly) as f32 * GRID_PX;
                            // Top edge
                            if !is_floor(lx, ly - 1) {
                                renderer.draw_line(px, py, px + GRID_PX, py, wall_w, theme.wall_color);
                            }
                            // Bottom edge
                            if !is_floor(lx, ly + 1) {
                                renderer.draw_line(px, py + GRID_PX, px + GRID_PX, py + GRID_PX, wall_w, theme.wall_color);
                            }
                            // Left edge
                            if !is_floor(lx - 1, ly) {
                                renderer.draw_line(px, py, px, py + GRID_PX, wall_w, theme.wall_color);
                            }
                            // Right edge
                            if !is_floor(lx + 1, ly) {
                                renderer.draw_line(px + GRID_PX, py, px + GRID_PX, py + GRID_PX, wall_w, theme.wall_color);
                            }
                        }
                    }
                    return;
                }
            }
            // No cells yet — draw as rectangle
            renderer.stroke_rect(rx, ry, rw, rh, wall_w, theme.wall_color);
        }
        RoomShape::Rectangle => {
            renderer.stroke_rect(rx, ry, rw, rh, wall_w, theme.wall_color);
        }
    }
}

/// Build the set of all cells inside cave room bounding boxes.
/// Used to prevent corridor walls from double-drawing at cave boundaries.
pub fn build_cave_cell_set(layout: &SpatialLayout, graph: &DungeonGraph) -> HashSet<(i32, i32)> {
    let mut cells = HashSet::new();
    for rl in &layout.rooms {
        let is_cave = graph.room_by_id(&rl.room_id)
            .is_some_and(|r| r.shape == RoomShape::Cave && r.cave_data.as_ref().is_some_and(|c| !c.cells.is_empty()));
        if !is_cave { continue; }
        for y in rl.y..(rl.y + rl.height as i32) {
            for x in rl.x..(rl.x + rl.width as i32) {
                cells.insert((x, y));
            }
        }
    }
    cells
}

/// Render one corridor's walls, skipping edges where adjacent cells are floor
/// or inside a cave room (cave rooms handle their own wall rendering).
pub fn render_corridor_walls(
    renderer: &mut dyn MapRenderer,
    corridor: &CorridorSegment,
    floor: &HashSet<(i32, i32)>,
    theme: &Theme,
    cave_cells: &HashSet<(i32, i32)>,
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

        // Skip wall if neighbor is floor OR inside a cave room
        let skip = |gx: i32, gy: i32| -> bool {
            floor.contains(&(gx, gy)) || cave_cells.contains(&(gx, gy))
        };

        // Top wall
        for x in min_gx..max_gx {
            if !skip(x, min_gy - 1) {
                let lx = x as f32 * GRID_PX;
                renderer.draw_line(lx, py1, lx + GRID_PX, py1, wall_w, theme.wall_color);
            }
        }
        // Bottom wall
        for x in min_gx..max_gx {
            if !skip(x, max_gy) {
                let lx = x as f32 * GRID_PX;
                renderer.draw_line(lx, py2, lx + GRID_PX, py2, wall_w, theme.wall_color);
            }
        }
        // Left wall
        for y in min_gy..max_gy {
            if !skip(min_gx - 1, y) {
                let ly = y as f32 * GRID_PX;
                renderer.draw_line(px1, ly, px1, ly + GRID_PX, wall_w, theme.wall_color);
            }
        }
        // Right wall
        for y in min_gy..max_gy {
            if !skip(max_gx, y) {
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

/// Rotate a point (px, py) around center (cx, cy) by angle in degrees.
fn rot(px: f32, py: f32, cx: f32, cy: f32, deg: f32) -> (f32, f32) {
    let rad = deg.to_radians();
    let cos = rad.cos();
    let sin = rad.sin();
    let dx = px - cx;
    let dy = py - cy;
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

/// Draw a rotated line via the rotate helper.
fn rot_line(renderer: &mut dyn MapRenderer, x1: f32, y1: f32, x2: f32, y2: f32,
            cx: f32, cy: f32, deg: f32, width: f32, color: [u8; 4]) {
    let (rx1, ry1) = rot(x1, y1, cx, cy, deg);
    let (rx2, ry2) = rot(x2, y2, cx, cy, deg);
    renderer.draw_line(rx1, ry1, rx2, ry2, width, color);
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
    let light = [color[0].saturating_add(40), color[1].saturating_add(40), color[2].saturating_add(40), color[3]];

    for decor in &room.decor {
        let cx = room_px_x + decor.x * GRID_PX;
        let cy = room_px_y + decor.y * GRID_PX;
        let s = GRID_PX * 0.4 * decor.scale;
        let deg = decor.rotation;

        match decor.decor_type {
            DecorType::Table => {
                // Table with legs
                let hw = s;
                let hh = s * 0.6;
                // Tabletop outline
                rot_line(renderer, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + hw, cy - hh, cx + hw, cy + hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + hw, cy + hh, cx - hw, cy + hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - hw, cy + hh, cx - hw, cy - hh, cx, cy, deg, 1.5, color);
                // Fill
                let fill = [color[0], color[1], color[2], 40];
                renderer.fill_rect(cx - hw, cy - hh, hw * 2.0, hh * 2.0, fill);
                // Legs (circles at corners)
                let leg_r = s * 0.1;
                for &(lx, ly) in &[(-hw + leg_r, -hh + leg_r), (hw - leg_r, -hh + leg_r),
                                     (-hw + leg_r, hh - leg_r), (hw - leg_r, hh - leg_r)] {
                    let (rx, ry) = rot(cx + lx, cy + ly, cx, cy, deg);
                    renderer.fill_circle(rx, ry, leg_r, color);
                }
            }
            DecorType::Chest => {
                // Chest with lid line and clasp
                let cs = s * 0.6;
                // Body outline
                rot_line(renderer, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + cs, cy + cs, cx - cs, cy + cs, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - cs, cy + cs, cx - cs, cy - cs, cx, cy, deg, 1.5, color);
                // Lid line
                rot_line(renderer, cx - cs, cy - cs * 0.2, cx + cs, cy - cs * 0.2, cx, cy, deg, 1.0, color);
                // Clasp
                let (clx, cly) = rot(cx, cy + cs, cx, cy, deg);
                renderer.fill_circle(clx, cly, s * 0.1, color);
            }
            DecorType::Pillar => {
                // Pillar with ring detail
                renderer.fill_circle(cx, cy, s * 0.5, color);
                renderer.stroke_circle(cx, cy, s * 0.5, 1.0, light);
                renderer.stroke_circle(cx, cy, s * 0.35, 0.5, light);
            }
            DecorType::StairsUp => {
                // Stair treads with arrow up
                let steps = 5;
                rot_line(renderer, cx - s, cy - s, cx + s, cy - s, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s, cy - s, cx + s, cy + s, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s, cy + s, cx - s, cy + s, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - s, cy + s, cx - s, cy - s, cx, cy, deg, 1.5, color);
                for i in 1..steps {
                    let y = cy - s + (i as f32 / steps as f32) * s * 2.0;
                    rot_line(renderer, cx - s, y, cx + s, y, cx, cy, deg, 0.8, color);
                }
                // Arrow up
                rot_line(renderer, cx, cy - s * 0.9, cx - s * 0.3, cy - s * 0.5, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx, cy - s * 0.9, cx + s * 0.3, cy - s * 0.5, cx, cy, deg, 1.5, color);
            }
            DecorType::StairsDown => {
                // Same treads, arrow down
                let steps = 5;
                rot_line(renderer, cx - s, cy - s, cx + s, cy - s, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s, cy - s, cx + s, cy + s, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s, cy + s, cx - s, cy + s, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - s, cy + s, cx - s, cy - s, cx, cy, deg, 1.5, color);
                for i in 1..steps {
                    let y = cy - s + (i as f32 / steps as f32) * s * 2.0;
                    rot_line(renderer, cx - s, y, cx + s, y, cx, cy, deg, 0.8, color);
                }
                rot_line(renderer, cx, cy + s * 0.9, cx - s * 0.3, cy + s * 0.5, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx, cy + s * 0.9, cx + s * 0.3, cy + s * 0.5, cx, cy, deg, 1.5, color);
            }
            DecorType::Altar => {
                // Cross on a platform base
                let t = s * 0.2;
                // Platform base
                rot_line(renderer, cx - s * 0.8, cy + s * 0.6, cx + s * 0.8, cy + s * 0.6, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - s * 0.8, cy + s * 0.8, cx + s * 0.8, cy + s * 0.8, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - s * 0.8, cy + s * 0.6, cx - s * 0.8, cy + s * 0.8, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s * 0.8, cy + s * 0.6, cx + s * 0.8, cy + s * 0.8, cx, cy, deg, 1.5, color);
                // Cross vertical
                rot_line(renderer, cx - t, cy - s, cx + t, cy - s, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + t, cy - s, cx + t, cy + s * 0.5, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + t, cy + s * 0.5, cx - t, cy + s * 0.5, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx - t, cy + s * 0.5, cx - t, cy - s, cx, cy, deg, 1.0, color);
                // Cross horizontal
                rot_line(renderer, cx - s * 0.6, cy - t * 1.5, cx + s * 0.6, cy - t * 1.5, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx - s * 0.6, cy + t * 0.5, cx + s * 0.6, cy + t * 0.5, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx - s * 0.6, cy - t * 1.5, cx - s * 0.6, cy + t * 0.5, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + s * 0.6, cy - t * 1.5, cx + s * 0.6, cy + t * 0.5, cx, cy, deg, 1.0, color);
            }
            DecorType::Fountain => {
                // Three concentric circles with water lines
                renderer.stroke_circle(cx, cy, s * 0.85, 1.5, color);
                renderer.stroke_circle(cx, cy, s * 0.55, 1.0, color);
                renderer.stroke_circle(cx, cy, s * 0.25, 1.0, color);
                renderer.fill_circle(cx, cy, s * 0.1, color);
                // Water ripple arcs (small lines)
                for angle_deg in [45.0f32, 135.0, 225.0, 315.0] {
                    let rad = angle_deg.to_radians();
                    let r = s * 0.7;
                    let len = s * 0.15;
                    let px = cx + r * rad.cos();
                    let py = cy + r * rad.sin();
                    let perp_x = -rad.sin() * len;
                    let perp_y = rad.cos() * len;
                    renderer.draw_line(px - perp_x, py - perp_y, px + perp_x, py + perp_y, 0.5, color);
                }
            }
            DecorType::Trap => {
                // Triangle warning with X
                rot_line(renderer, cx, cy - s * 0.7, cx + s * 0.7, cy + s * 0.5, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s * 0.7, cy + s * 0.5, cx - s * 0.7, cy + s * 0.5, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - s * 0.7, cy + s * 0.5, cx, cy - s * 0.7, cx, cy, deg, 1.5, color);
                // X in center
                rot_line(renderer, cx - s * 0.25, cy - s * 0.1, cx + s * 0.25, cy + s * 0.35, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + s * 0.25, cy - s * 0.1, cx - s * 0.25, cy + s * 0.35, cx, cy, deg, 1.0, color);
            }
            DecorType::Rubble => {
                let rocks: [(f32, f32, f32); 7] = [
                    (0.0, 0.0, 0.18), (-0.45, -0.35, 0.14), (0.35, -0.25, 0.16),
                    (-0.25, 0.4, 0.12), (0.4, 0.35, 0.13), (-0.15, -0.45, 0.1),
                    (0.2, 0.15, 0.11),
                ];
                for &(dx, dy, r) in &rocks {
                    let (rx, ry) = rot(cx + dx * s, cy + dy * s, cx, cy, deg);
                    renderer.fill_circle(rx, ry, r * s, color);
                    renderer.stroke_circle(rx, ry, r * s, 0.5, light);
                }
            }
            DecorType::Chair => {
                // Small square seat with back
                let cs = s * 0.4;
                rot_line(renderer, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + cs, cy + cs, cx - cs, cy + cs, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx - cs, cy + cs, cx - cs, cy - cs, cx, cy, deg, 1.0, color);
                // Back rest (thicker line on one side)
                rot_line(renderer, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, 2.5, color);
            }
            DecorType::Bench => {
                // Long narrow rectangle
                let hw = s * 0.9;
                let hh = s * 0.25;
                rot_line(renderer, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + hw, cy - hh, cx + hw, cy + hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + hw, cy + hh, cx - hw, cy + hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - hw, cy + hh, cx - hw, cy - hh, cx, cy, deg, 1.5, color);
            }
            DecorType::Barrel => {
                // Circle with horizontal bands
                let (rx, ry) = rot(cx, cy, cx, cy, deg);
                renderer.stroke_circle(rx, ry, s * 0.55, 1.5, color);
                rot_line(renderer, cx - s * 0.5, cy - s * 0.15, cx + s * 0.5, cy - s * 0.15, cx, cy, deg, 0.8, color);
                rot_line(renderer, cx - s * 0.5, cy + s * 0.15, cx + s * 0.5, cy + s * 0.15, cx, cy, deg, 0.8, color);
            }
            DecorType::Crate => {
                // Square with X
                let cs = s * 0.55;
                rot_line(renderer, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + cs, cy + cs, cx - cs, cy + cs, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - cs, cy + cs, cx - cs, cy - cs, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, 0.8, color);
                rot_line(renderer, cx + cs, cy - cs, cx - cs, cy + cs, cx, cy, deg, 0.8, color);
            }
            DecorType::Ladder => {
                // Two vertical rails with rungs
                let hw = s * 0.3;
                rot_line(renderer, cx - hw, cy - s, cx - hw, cy + s, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + hw, cy - s, cx + hw, cy + s, cx, cy, deg, 1.5, color);
                for i in 0..5 {
                    let y = cy - s + (i as f32 + 0.5) * s * 2.0 / 5.0;
                    rot_line(renderer, cx - hw, y, cx + hw, y, cx, cy, deg, 1.0, color);
                }
            }
            DecorType::Well => {
                // Circle with cross inside
                renderer.stroke_circle(cx, cy, s * 0.7, 1.5, color);
                renderer.stroke_circle(cx, cy, s * 0.5, 1.0, color);
                rot_line(renderer, cx - s * 0.7, cy, cx + s * 0.7, cy, cx, cy, deg, 0.8, color);
                rot_line(renderer, cx, cy - s * 0.7, cx, cy + s * 0.7, cx, cy, deg, 0.8, color);
            }
            DecorType::Brazier => {
                // Bowl on stand with flames
                renderer.stroke_circle(cx, cy, s * 0.4, 1.5, color);
                renderer.fill_circle(cx, cy, s * 0.25, color);
                // Stand legs
                rot_line(renderer, cx - s * 0.3, cy + s * 0.3, cx - s * 0.5, cy + s * 0.7, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + s * 0.3, cy + s * 0.3, cx + s * 0.5, cy + s * 0.7, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx, cy + s * 0.4, cx, cy + s * 0.7, cx, cy, deg, 1.0, color);
            }
            DecorType::Fireplace => {
                // U-shape opening
                rot_line(renderer, cx - s * 0.7, cy - s * 0.6, cx - s * 0.7, cy + s * 0.6, cx, cy, deg, 2.0, color);
                rot_line(renderer, cx + s * 0.7, cy - s * 0.6, cx + s * 0.7, cy + s * 0.6, cx, cy, deg, 2.0, color);
                rot_line(renderer, cx - s * 0.7, cy + s * 0.6, cx + s * 0.7, cy + s * 0.6, cx, cy, deg, 2.0, color);
                // Flame
                rot_line(renderer, cx, cy + s * 0.3, cx - s * 0.15, cy - s * 0.2, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx, cy + s * 0.3, cx + s * 0.15, cy - s * 0.2, cx, cy, deg, 1.0, color);
            }
            DecorType::Statue => {
                // Circle on a square base
                let (rx, ry) = rot(cx, cy - s * 0.15, cx, cy, deg);
                renderer.fill_circle(rx, ry, s * 0.4, color);
                let bs = s * 0.5;
                rot_line(renderer, cx - bs, cy + s * 0.3, cx + bs, cy + s * 0.3, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + bs, cy + s * 0.3, cx + bs, cy + s * 0.7, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + bs, cy + s * 0.7, cx - bs, cy + s * 0.7, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - bs, cy + s * 0.7, cx - bs, cy + s * 0.3, cx, cy, deg, 1.5, color);
            }
            DecorType::Throne => {
                // Chair with high back and armrests
                let cs = s * 0.5;
                rot_line(renderer, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + cs, cy + cs, cx - cs, cy + cs, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx - cs, cy + cs, cx - cs, cy - cs, cx, cy, deg, 1.0, color);
                // High back
                rot_line(renderer, cx - cs, cy - cs, cx - cs, cy - s * 0.9, cx, cy, deg, 2.0, color);
                rot_line(renderer, cx + cs, cy - cs, cx + cs, cy - s * 0.9, cx, cy, deg, 2.0, color);
                rot_line(renderer, cx - cs, cy - s * 0.9, cx + cs, cy - s * 0.9, cx, cy, deg, 2.0, color);
                // Armrests
                rot_line(renderer, cx - cs, cy, cx - s * 0.8, cy, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + cs, cy, cx + s * 0.8, cy, cx, cy, deg, 1.5, color);
            }
            DecorType::Bed => {
                // Rectangle with headboard
                let hw = s * 0.6;
                let hh = s * 0.9;
                rot_line(renderer, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + hw, cy - hh, cx + hw, cy + hh, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + hw, cy + hh, cx - hw, cy + hh, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx - hw, cy + hh, cx - hw, cy - hh, cx, cy, deg, 1.0, color);
                // Headboard
                rot_line(renderer, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, 3.0, color);
                // Pillow
                rot_line(renderer, cx - hw * 0.6, cy - hh + s * 0.3, cx + hw * 0.6, cy - hh + s * 0.3, cx, cy, deg, 0.8, color);
            }
            DecorType::Bookshelf => {
                // Rectangle with horizontal shelf lines
                let hw = s * 0.8;
                let hh = s * 0.4;
                rot_line(renderer, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + hw, cy - hh, cx + hw, cy + hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + hw, cy + hh, cx - hw, cy + hh, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx - hw, cy + hh, cx - hw, cy - hh, cx, cy, deg, 1.5, color);
                // Shelves
                for i in 1..3 {
                    let y = cy - hh + (i as f32 / 3.0) * hh * 2.0;
                    rot_line(renderer, cx - hw, y, cx + hw, y, cx, cy, deg, 0.8, color);
                }
                // Vertical dividers
                for i in 1..4 {
                    let x = cx - hw + (i as f32 / 4.0) * hw * 2.0;
                    rot_line(renderer, x, cy - hh, x, cy + hh, cx, cy, deg, 0.5, color);
                }
            }
            DecorType::Bones => {
                // Crossed bones
                rot_line(renderer, cx - s * 0.6, cy - s * 0.4, cx + s * 0.6, cy + s * 0.4, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s * 0.6, cy - s * 0.4, cx - s * 0.6, cy + s * 0.4, cx, cy, deg, 1.5, color);
                // Bone ends
                for &(bx, by) in &[(-0.6, -0.4), (0.6, 0.4), (0.6, -0.4), (-0.6, 0.4)] {
                    let (rx, ry) = rot(cx + bx * s, cy + by * s, cx, cy, deg);
                    renderer.fill_circle(rx, ry, s * 0.1, color);
                }
                // Skull
                let (sx, sy) = rot(cx, cy - s * 0.1, cx, cy, deg);
                renderer.stroke_circle(sx, sy, s * 0.2, 1.0, color);
            }
            DecorType::Web => {
                // Radial lines from center
                for i in 0..6 {
                    let angle = (i as f32 / 6.0) * std::f32::consts::TAU;
                    let ex = cx + s * 0.8 * angle.cos();
                    let ey = cy + s * 0.8 * angle.sin();
                    rot_line(renderer, cx, cy, ex, ey, cx, cy, deg, 0.8, color);
                }
                // Concentric rings (approximated with lines)
                for ring in 1..3 {
                    let r = s * 0.8 * ring as f32 / 3.0;
                    for i in 0..6 {
                        let a1 = (i as f32 / 6.0) * std::f32::consts::TAU;
                        let a2 = ((i + 1) as f32 / 6.0) * std::f32::consts::TAU;
                        let (x1, y1) = (cx + r * a1.cos(), cy + r * a1.sin());
                        let (x2, y2) = (cx + r * a2.cos(), cy + r * a2.sin());
                        rot_line(renderer, x1, y1, x2, y2, cx, cy, deg, 0.5, color);
                    }
                }
            }
            DecorType::Door => {
                // Arc showing door swing
                let hw = s * 0.05;
                rot_line(renderer, cx - hw, cy - s * 0.7, cx - hw, cy + s * 0.7, cx, cy, deg, 2.0, color);
                // Door panel
                rot_line(renderer, cx - hw, cy - s * 0.7, cx + s * 0.7, cy - s * 0.7, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s * 0.7, cy - s * 0.7, cx + s * 0.7, cy + s * 0.7, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s * 0.7, cy + s * 0.7, cx - hw, cy + s * 0.7, cx, cy, deg, 1.5, color);
            }
            DecorType::Gate => {
                // Portcullis: vertical bars with cross
                for i in 0..5 {
                    let x = cx - s * 0.6 + (i as f32 / 4.0) * s * 1.2;
                    rot_line(renderer, x, cy - s * 0.7, x, cy + s * 0.7, cx, cy, deg, 1.0, color);
                }
                rot_line(renderer, cx - s * 0.6, cy - s * 0.2, cx + s * 0.6, cy - s * 0.2, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx - s * 0.6, cy + s * 0.2, cx + s * 0.6, cy + s * 0.2, cx, cy, deg, 1.0, color);
                // Frame
                rot_line(renderer, cx - s * 0.7, cy - s * 0.8, cx + s * 0.7, cy - s * 0.8, cx, cy, deg, 2.0, color);
                rot_line(renderer, cx - s * 0.7, cy - s * 0.8, cx - s * 0.7, cy + s * 0.7, cx, cy, deg, 2.0, color);
                rot_line(renderer, cx + s * 0.7, cy - s * 0.8, cx + s * 0.7, cy + s * 0.7, cx, cy, deg, 2.0, color);
            }
            DecorType::Vines => {
                // Curving tendrils radiating from center with small leaf marks
                let tendrils: [(f32, f32, f32, f32, f32, f32); 5] = [
                    // (start_x, start_y, ctrl_x, ctrl_y, end_x, end_y) relative to s
                    (0.0, 0.0, -0.4, -0.6, -0.7, -0.8),
                    (0.0, 0.0,  0.5, -0.4,  0.8, -0.7),
                    (0.0, 0.0, -0.6,  0.3, -0.8,  0.6),
                    (0.0, 0.0,  0.3,  0.5,  0.6,  0.8),
                    (0.0, 0.0,  0.1, -0.3, -0.2, -0.9),
                ];
                for &(sx, sy, cx1, cy1, ex, ey) in &tendrils {
                    // Approximate bezier with line segments
                    let steps = 6;
                    let mut px = cx + sx * s;
                    let mut py = cy + sy * s;
                    for i in 1..=steps {
                        let t = i as f32 / steps as f32;
                        let it = 1.0 - t;
                        let nx = cx + (it * it * sx + 2.0 * it * t * cx1 + t * t * ex) * s;
                        let ny = cy + (it * it * sy + 2.0 * it * t * cy1 + t * t * ey) * s;
                        rot_line(renderer, px, py, nx, ny, cx, cy, deg, 1.0, color);
                        px = nx;
                        py = ny;
                    }
                    // Small leaf near the end
                    let leaf_t = 0.7;
                    let it = 1.0 - leaf_t;
                    let lx = cx + (it * it * sx + 2.0 * it * leaf_t * cx1 + leaf_t * leaf_t * ex) * s;
                    let ly = cy + (it * it * sy + 2.0 * it * leaf_t * cy1 + leaf_t * leaf_t * ey) * s;
                    let leaf_s = s * 0.12;
                    rot_line(renderer, lx - leaf_s, ly, lx, ly - leaf_s, cx, cy, deg, 0.8, color);
                    rot_line(renderer, lx, ly - leaf_s, lx + leaf_s, ly, cx, cy, deg, 0.8, color);
                    rot_line(renderer, lx - leaf_s, ly, lx + leaf_s, ly, cx, cy, deg, 0.5, color);
                }
            }
            DecorType::OfferingMouth => {
                // Face outline (circle)
                renderer.stroke_circle(cx, cy, s * 0.85, 1.5, color);
                // Eyes (small filled circles)
                let (lex, ley) = rot(cx - s * 0.3, cy - s * 0.25, cx, cy, deg);
                let (rex, rey) = rot(cx + s * 0.3, cy - s * 0.25, cx, cy, deg);
                renderer.fill_circle(lex, ley, s * 0.12, color);
                renderer.fill_circle(rex, rey, s * 0.12, color);
                // Open mouth (oval) — drawn as a wider ellipse approximated with lines
                let mouth_cx = cx;
                let mouth_cy = cy + s * 0.3;
                let mouth_rx = s * 0.35;
                let mouth_ry = s * 0.25;
                let segments = 16;
                let dark = [color[0] / 2, color[1] / 2, color[2] / 2, color[3]];
                for i in 0..segments {
                    let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
                    let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
                    let x0 = mouth_cx + mouth_rx * a0.cos();
                    let y0 = mouth_cy + mouth_ry * a0.sin();
                    let x1 = mouth_cx + mouth_rx * a1.cos();
                    let y1 = mouth_cy + mouth_ry * a1.sin();
                    rot_line(renderer, x0, y0, x1, y1, cx, cy, deg, 1.5, color);
                }
                // Dark fill inside mouth
                let (mcx, mcy) = rot(mouth_cx, mouth_cy, cx, cy, deg);
                renderer.fill_circle(mcx, mcy, mouth_ry * 0.6, dark);
                // Brow ridges
                rot_line(renderer, cx - s * 0.5, cy - s * 0.45, cx - s * 0.1, cy - s * 0.5, cx, cy, deg, 1.5, color);
                rot_line(renderer, cx + s * 0.5, cy - s * 0.45, cx + s * 0.1, cy - s * 0.5, cx, cy, deg, 1.5, color);
                // Nose hint
                rot_line(renderer, cx, cy - s * 0.1, cx - s * 0.08, cy + s * 0.05, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx, cy - s * 0.1, cx + s * 0.08, cy + s * 0.05, cx, cy, deg, 1.0, color);
            }
            DecorType::Scales => {
                // Central stand (vertical post)
                rot_line(renderer, cx, cy - s * 0.9, cx, cy + s * 0.8, cx, cy, deg, 1.5, color);
                // Base
                rot_line(renderer, cx - s * 0.4, cy + s * 0.8, cx + s * 0.4, cy + s * 0.8, cx, cy, deg, 2.0, color);
                // Crossbeam (tilted slightly for visual interest)
                rot_line(renderer, cx - s * 0.8, cy - s * 0.5, cx + s * 0.8, cy - s * 0.7, cx, cy, deg, 1.5, color);
                // Pivot triangle at top
                rot_line(renderer, cx - s * 0.1, cy - s * 0.9, cx + s * 0.1, cy - s * 0.9, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx - s * 0.1, cy - s * 0.9, cx, cy - s * 0.7, cx, cy, deg, 1.0, color);
                rot_line(renderer, cx + s * 0.1, cy - s * 0.9, cx, cy - s * 0.7, cx, cy, deg, 1.0, color);
                // Left chain and pan
                rot_line(renderer, cx - s * 0.8, cy - s * 0.5, cx - s * 0.8, cy + s * 0.1, cx, cy, deg, 0.8, color);
                rot_line(renderer, cx - s * 1.05, cy + s * 0.1, cx - s * 0.55, cy + s * 0.1, cx, cy, deg, 1.0, color);
                // Left pan curve (arc via segments)
                for i in 0..6 {
                    let a0 = std::f32::consts::PI * (i as f32 / 6.0);
                    let a1 = std::f32::consts::PI * ((i + 1) as f32 / 6.0);
                    let pr = s * 0.25;
                    let pcx = cx - s * 0.8;
                    let pcy = cy + s * 0.1;
                    rot_line(renderer,
                        pcx + pr * a0.cos(), pcy + pr * a0.sin(),
                        pcx + pr * a1.cos(), pcy + pr * a1.sin(),
                        cx, cy, deg, 1.0, color);
                }
                // Right chain and pan
                rot_line(renderer, cx + s * 0.8, cy - s * 0.7, cx + s * 0.8, cy - s * 0.1, cx, cy, deg, 0.8, color);
                rot_line(renderer, cx + s * 0.55, cy - s * 0.1, cx + s * 1.05, cy - s * 0.1, cx, cy, deg, 1.0, color);
                // Right pan curve
                for i in 0..6 {
                    let a0 = std::f32::consts::PI * (i as f32 / 6.0);
                    let a1 = std::f32::consts::PI * ((i + 1) as f32 / 6.0);
                    let pr = s * 0.25;
                    let pcx = cx + s * 0.8;
                    let pcy = cy - s * 0.1;
                    rot_line(renderer,
                        pcx + pr * a0.cos(), pcy + pr * a0.sin(),
                        pcx + pr * a1.cos(), pcy + pr * a1.sin(),
                        cx, cy, deg, 1.0, color);
                }
            }
        }
    }
}

/// Render elevation sections (raised/lowered areas) inside rooms.
pub fn render_elevation_sections(
    renderer: &mut dyn MapRenderer,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    theme: &Theme,
) {
    let Some(room) = graph.room_by_id(&rl.room_id) else { return };
    if room.sections.is_empty() {
        return;
    }

    let room_px_x = rl.x as f32 * GRID_PX;
    let room_px_y = rl.y as f32 * GRID_PX;
    let wall_color = theme.wall_color;

    for section in &room.sections {
        let sx = room_px_x + section.x * GRID_PX;
        let sy = room_px_y + section.y * GRID_PX;
        let sw = section.width * GRID_PX;
        let sh = section.length * GRID_PX;

        match section.elevation {
            ElevationType::Raised => {
                // Light shading fill + solid border with tick marks pointing outward
                let fill = [wall_color[0], wall_color[1], wall_color[2], 25];
                renderer.fill_rect(sx, sy, sw, sh, fill);
                renderer.stroke_rect(sx, sy, sw, sh, 1.5, wall_color);

                // Tick marks along edges (pointing outward = raised)
                let tick = GRID_PX * 0.15;
                let spacing = GRID_PX * 0.5;
                // Top edge: ticks pointing up
                let mut tx = sx + spacing;
                while tx < sx + sw - spacing * 0.5 {
                    renderer.draw_line(tx, sy, tx, sy - tick, 1.0, wall_color);
                    tx += spacing;
                }
                // Bottom edge: ticks pointing down
                tx = sx + spacing;
                while tx < sx + sw - spacing * 0.5 {
                    renderer.draw_line(tx, sy + sh, tx, sy + sh + tick, 1.0, wall_color);
                    tx += spacing;
                }
                // Left edge: ticks pointing left
                let mut ty = sy + spacing;
                while ty < sy + sh - spacing * 0.5 {
                    renderer.draw_line(sx, ty, sx - tick, ty, 1.0, wall_color);
                    ty += spacing;
                }
                // Right edge: ticks pointing right
                ty = sy + spacing;
                while ty < sy + sh - spacing * 0.5 {
                    renderer.draw_line(sx + sw, ty, sx + sw + tick, ty, 1.0, wall_color);
                    ty += spacing;
                }
            }
            ElevationType::Lowered => {
                // Darker shading fill + dashed border with tick marks pointing inward
                let fill = [wall_color[0], wall_color[1], wall_color[2], 40];
                renderer.fill_rect(sx, sy, sw, sh, fill);
                renderer.stroke_rect(sx, sy, sw, sh, 1.5, wall_color);

                // Tick marks along edges (pointing inward = lowered)
                let tick = GRID_PX * 0.15;
                let spacing = GRID_PX * 0.5;
                // Top edge: ticks pointing down (inward)
                let mut tx = sx + spacing;
                while tx < sx + sw - spacing * 0.5 {
                    renderer.draw_line(tx, sy, tx, sy + tick, 1.0, wall_color);
                    tx += spacing;
                }
                // Bottom edge: ticks pointing up (inward)
                tx = sx + spacing;
                while tx < sx + sw - spacing * 0.5 {
                    renderer.draw_line(tx, sy + sh, tx, sy + sh - tick, 1.0, wall_color);
                    tx += spacing;
                }
                // Left edge: ticks pointing right (inward)
                let mut ty = sy + spacing;
                while ty < sy + sh - spacing * 0.5 {
                    renderer.draw_line(sx, ty, sx + tick, ty, 1.0, wall_color);
                    ty += spacing;
                }
                // Right edge: ticks pointing left (inward)
                ty = sy + spacing;
                while ty < sy + sh - spacing * 0.5 {
                    renderer.draw_line(sx + sw, ty, sx + sw - tick, ty, 1.0, wall_color);
                    ty += spacing;
                }
            }
            ElevationType::Steps => {
                // Parallel lines across the shorter dimension
                let fill = [wall_color[0], wall_color[1], wall_color[2], 15];
                renderer.fill_rect(sx, sy, sw, sh, fill);
                renderer.stroke_rect(sx, sy, sw, sh, 1.0, wall_color);

                let step_count = 4;
                if sw >= sh {
                    // Horizontal steps (lines vertical)
                    for i in 1..step_count {
                        let lx = sx + (i as f32 / step_count as f32) * sw;
                        renderer.draw_line(lx, sy, lx, sy + sh, 1.0, wall_color);
                    }
                } else {
                    // Vertical steps (lines horizontal)
                    for i in 1..step_count {
                        let ly = sy + (i as f32 / step_count as f32) * sh;
                        renderer.draw_line(sx, ly, sx + sw, ly, 1.0, wall_color);
                    }
                }
            }
            ElevationType::Slope => {
                // Gradient: strips of increasing opacity along the longer axis
                // High end is light, low end is dark — direction is inherent
                renderer.stroke_rect(sx, sy, sw, sh, 1.0, wall_color);

                let strips = 8;
                if sw >= sh {
                    let strip_w = sw / strips as f32;
                    for i in 0..strips {
                        let alpha = ((i as f32 + 1.0) / strips as f32 * 50.0) as u8;
                        let fill = [wall_color[0], wall_color[1], wall_color[2], alpha];
                        renderer.fill_rect(sx + i as f32 * strip_w, sy, strip_w, sh, fill);
                    }
                } else {
                    let strip_h = sh / strips as f32;
                    for i in 0..strips {
                        let alpha = ((i as f32 + 1.0) / strips as f32 * 50.0) as u8;
                        let fill = [wall_color[0], wall_color[1], wall_color[2], alpha];
                        renderer.fill_rect(sx, sy + i as f32 * strip_h, sw, strip_h, fill);
                    }
                }
            }
            ElevationType::BottomlessPit => {
                // Solid dark fill with heavy border — void
                let fill = [wall_color[0], wall_color[1], wall_color[2], 180];
                renderer.fill_rect(sx, sy, sw, sh, fill);
                renderer.stroke_rect(sx, sy, sw, sh, 2.0, wall_color);

                // Inset border for depth effect
                let inset = GRID_PX * 0.12;
                renderer.stroke_rect(sx + inset, sy + inset, sw - inset * 2.0, sh - inset * 2.0, 1.0, wall_color);
            }
            ElevationType::Hole => {
                // Dark fill (lighter than bottomless) with border and diagonal cross
                let fill = [wall_color[0], wall_color[1], wall_color[2], 100];
                renderer.fill_rect(sx, sy, sw, sh, fill);
                renderer.stroke_rect(sx, sy, sw, sh, 1.5, wall_color);

                // Diagonal cross indicating passage through floor
                renderer.draw_line(sx, sy, sx + sw, sy + sh, 1.0, wall_color);
                renderer.draw_line(sx + sw, sy, sx, sy + sh, 1.0, wall_color);
            }
            ElevationType::Water => {
                // Blue-tinted fill with wavy lines
                let fill = [80, 130, 200, 60];
                renderer.fill_rect(sx, sy, sw, sh, fill);
                renderer.stroke_rect(sx, sy, sw, sh, 1.0, [60, 100, 170, 200]);

                // Wavy lines across the section
                let wave_color = [60, 100, 170, 140];
                let wave_count = ((sh / GRID_PX) * 2.0).max(2.0) as i32;
                for i in 1..wave_count {
                    let y = sy + (i as f32 / wave_count as f32) * sh;
                    let segments = ((sw / GRID_PX) * 4.0).max(8.0) as i32;
                    for j in 0..segments {
                        let t0 = j as f32 / segments as f32;
                        let t1 = (j + 1) as f32 / segments as f32;
                        let x0 = sx + t0 * sw;
                        let x1 = sx + t1 * sw;
                        let amp = GRID_PX * 0.1;
                        let y0 = y + amp * (t0 * std::f32::consts::TAU * 2.0).sin();
                        let y1 = y + amp * (t1 * std::f32::consts::TAU * 2.0).sin();
                        renderer.draw_line(x0, y0, x1, y1, 0.8, wave_color);
                    }
                }
            }
        }
    }
}

/// Compute door rectangle in grid coordinates given an exit position or waypoint fallback.
/// Returns (x1, y1, x2, y2) in grid coords for the door rectangle.
pub fn door_rect(
    rl: &RoomLayout,
    wp: &GridPos,
    exit: Option<&ExitPos>,
    dw: f32,
    door_depth: f32,
) -> (f32, f32, f32, f32) {
    let dw_half = dw / 2.0;

    if let Some(exit) = exit {
        // Use stored exit to determine face and position
        let rw = rl.width as f32;
        let rh = rl.height as f32;
        let rx = rl.x as f32;
        let ry = rl.y as f32;
        let ex = exit.x;
        let ey = exit.y;
        let eps = 0.01;
        if (ex - (rx + rw)).abs() < eps {
            // Right wall
            let wall_x = rx + rw;
            (wall_x - door_depth / 2.0, ey - dw_half, wall_x + door_depth / 2.0, ey + dw_half)
        } else if (ex - rx).abs() < eps {
            // Left wall
            (rx - door_depth / 2.0, ey - dw_half, rx + door_depth / 2.0, ey + dw_half)
        } else if (ey - (ry + rh)).abs() < eps {
            // Bottom wall
            let wall_y = ry + rh;
            (ex - dw_half, wall_y - door_depth / 2.0, ex + dw_half, wall_y + door_depth / 2.0)
        } else {
            // Top wall
            (ex - dw_half, ry - door_depth / 2.0, ex + dw_half, ry + door_depth / 2.0)
        }
    } else {
        // Fallback: nearest-wall heuristic from waypoint
        let wp_cx = wp.x as f32;
        let wp_cy = wp.y as f32;
        let dist_right = (wp_cx - (rl.x + rl.width as i32) as f32).abs();
        let dist_left = (wp_cx - rl.x as f32).abs();
        let dist_bottom = (wp_cy - (rl.y + rl.height as i32) as f32).abs();
        let dist_top = (wp_cy - rl.y as f32).abs();
        let min_dist = dist_right.min(dist_left).min(dist_bottom).min(dist_top);

        if min_dist == dist_right {
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

        let exits = [edge.source_exit.as_ref(), edge.target_exit.as_ref()];

        for ((room_id, wp), exit) in room_ids.iter().zip(wp_ends.iter()).zip(exits.iter()) {
            // Skip drawing door on cave room walls — caves have irregular boundaries
            let is_cave = graph.room_by_id(room_id)
                .is_some_and(|r| r.shape == RoomShape::Cave);
            if is_cave { continue; }
            let Some(rl) = layout.room_by_id(room_id) else { continue };

            let (dx1, dy1, dx2, dy2) = door_rect(rl, wp, *exit, dw, door_depth);

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
        let room = graph.room_by_id(&rl.room_id);
        let shape = room.map(|r| r.shape).unwrap_or_default();
        match shape {
            RoomShape::Circle => {
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
            }
            RoomShape::Cave => {
                if let Some(cave) = room.and_then(|r| r.cave_data.as_ref()) {
                    if !cave.cells.is_empty() {
                        let w = rl.width as usize;
                        for ly in 0..rl.height as usize {
                            for lx in 0..w {
                                if cave.cells.get(ly * w + lx).copied().unwrap_or(false) {
                                    floor.insert((rl.x + lx as i32, rl.y + ly as i32));
                                }
                            }
                        }
                    } else {
                        // No cells yet — treat as full rectangle
                        for y in rl.y..(rl.y + rl.height as i32) {
                            for x in rl.x..(rl.x + rl.width as i32) {
                                floor.insert((x, y));
                            }
                        }
                    }
                }
            }
            RoomShape::Rectangle => {
                for y in rl.y..(rl.y + rl.height as i32) {
                    for x in rl.x..(rl.x + rl.width as i32) {
                        floor.insert((x, y));
                    }
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
