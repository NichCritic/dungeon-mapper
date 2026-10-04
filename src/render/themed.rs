use crate::util::CellSet;
use crate::render::overlap;
use crate::render::rotate::RotatedRenderer;
use crate::model::geometry::{self, CorridorShape};
use crate::model::*;
use crate::render::hatching::{draw_exterior_shading, ShadingParams};
use crate::render::decor::{draw_decor, DecorPalette, MapRendererSink};
use crate::render::traits::MapRenderer;
use crate::util::{DECOR_HALF_SIZE, GRID_PX};

/// Options controlling which elements to render.
pub struct RenderOptions {
    pub show_grid: bool,
    pub show_labels: bool,
    pub show_notes: bool,
    pub show_secrets: bool,
    /// Whether to render decor items. Set to false when decor is drawn as a live overlay.
    pub show_decor: bool,
    /// Whether to bake the radial lighting wash into the map. Set to false where the
    /// line-of-sight light map is drawn as a live overlay instead.
    pub show_lighting: bool,
}

pub fn render_themed(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    options: &RenderOptions,
) {
    let floor = build_floor_set(layout, graph);
    // Freeform geometry: angled corridors and rotated rooms (none on an ordinary map)
    let shapes: Vec<Option<CorridorShape>> = layout.corridors.iter()
        .map(|c| geometry::corridor_shape(c, layout, graph))
        .collect();
    let freeform = shapes.iter().any(Option::is_some) || layout.rooms.iter().any(|rl| rl.is_rotated());

    render_background(renderer, layout, theme);

    // Rooms in z-order so containers render before children
    let room_order = layout.render_order(graph);

    // Sort corridors by floor (max floor of connected rooms)
    let mut corridor_order: Vec<usize> = (0..layout.corridors.len()).collect();
    corridor_order.sort_by_key(|&i| {
        let conn_id = &layout.corridors[i].connection_id;
        graph.connections.iter()
            .find(|e| e.connection.id == *conn_id)
            .map(|e| {
                let sf = graph.room_by_id(&e.source_room_id)
                    .map(|r| *r.floor.floors().iter().max().unwrap_or(&0))
                    .unwrap_or(0);
                let tf = graph.room_by_id(&e.target_room_id)
                    .map(|r| *r.floor.floors().iter().max().unwrap_or(&0))
                    .unwrap_or(0);
                sf.max(tf)
            })
            .unwrap_or(0)
    });

    // Collect baked marching squares contour segments from cave rooms (used by shading)
    let mut contour_segments: Vec<(f32, f32, f32, f32)> = graph.rooms.iter()
        .filter_map(|r| r.cave_data.as_ref())
        .flat_map(|c| c.contour_segments.iter().copied())
        .collect();
    if freeform {
        // Hatch out from the exact angled walls, over cells the floor fully covers, so
        // the hatching meets a diagonal wall without stair-step gaps
        contour_segments.extend(freeform_wall_segments_px(&shapes, graph, layout));
        let covered = rasterize_floor(layout, graph, Coverage::Full);
        render_exterior_shading(renderer, layout, &covered, theme, &contour_segments);
    } else {
        render_exterior_shading(renderer, layout, &floor, theme, &contour_segments);
    }

    for &ri in &room_order {
        let rl = &layout.rooms[ri];
        if rl.is_rotated() {
            render_room_floor(&mut RotatedRenderer::for_room(renderer, rl), rl, graph, theme);
        } else {
            render_room_floor(renderer, rl, graph, theme);
        }
    }
    for &ci in &corridor_order {
        match &shapes[ci] {
            Some(shape) => render_freeform_corridor_floor(renderer, shape, theme.floor_color),
            None => render_corridor_floor(renderer, &layout.corridors[ci], theme),
        }
    }
    if theme.corridor_chamfer != ChamferStyle::Sharp {
        for &ci in &corridor_order {
            if shapes[ci].is_none() {
                render_corridor_chamfers(renderer, &layout.corridors[ci], theme);
            }
        }
    }
    if options.show_grid {
        render_map_grid(renderer, layout, graph, &floor, |_| true, |_| true);
    }
    // Render room decor and elevation sections (after floors/grid, before walls)
    for &ri in &room_order {
        let rl = &layout.rooms[ri];
        let mut turned;
        let r: &mut dyn MapRenderer = if rl.is_rotated() {
            turned = RotatedRenderer::for_room(renderer, rl);
            &mut turned
        } else {
            renderer
        };
        if options.show_decor {
            render_decor(r, rl, graph, theme);
        }
        render_elevation_sections(r, rl, graph, theme);
    }
    for &ri in &room_order {
        let rl = &layout.rooms[ri];
        // Cave rooms use baked marching squares contour segments
        let room = graph.room_by_id(&rl.room_id);
        if let Some(cave) = room.and_then(|r| {
            if r.shape == RoomShape::Cave { r.cave_data.as_ref() } else { None }
        }) {
            if !cave.contour_segments.is_empty() {
                render_cave_contours(renderer, &rl.room_id, &cave.contour_segments, graph, layout, theme.wall_color);
                continue;
            }
        }
        render_room_walls(renderer, rl, graph, layout, theme);
    }
    // Redraw corridor floors at circular room junctions to punch through
    // the circle wall stroke that covers the corridor opening.
    repair_circle_junctions(renderer, graph, layout, theme);
    // Build set of cells inside cave rooms (so corridor walls don't double-draw there)
    let cave_cells = build_cave_cell_set(layout, graph);
    for &ci in &corridor_order {
        if shapes[ci].is_some() {
            for ((ax, ay), (bx, by)) in overlap::freeform_corridor_walls(ci, &shapes, graph, layout) {
                renderer.draw_line(ax * GRID_PX, ay * GRID_PX, bx * GRID_PX, by * GRID_PX, 2.0, theme.wall_color);
            }
        } else {
            render_corridor_walls(renderer, &layout.corridors[ci], &floor, theme, &cave_cells);
        }
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
    floor: &CellSet,
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

/// Compute the floor color for a room based on its environment type.
pub fn room_floor_color(room: Option<&Room>, theme: &Theme) -> [u8; 4] {
    match room.map(|r| r.environment).unwrap_or_default() {
        RoomEnvironment::Indoor => theme.floor_color,
        RoomEnvironment::Outdoor => {
            // Tint toward a warm sandy/earthy tone
            let [r, g, b, a] = theme.floor_color;
            [
                (r as u16 * 230 / 255).min(255) as u8,
                (g as u16 * 225 / 255).min(255) as u8,
                (b as u16 * 200 / 255).min(255) as u8,
                a,
            ]
        }
        RoomEnvironment::Covered => {
            // Slightly darker than indoor
            let [r, g, b, a] = theme.floor_color;
            [
                (r as u16 * 240 / 255).min(255) as u8,
                (g as u16 * 240 / 255).min(255) as u8,
                (b as u16 * 235 / 255).min(255) as u8,
                a,
            ]
        }
    }
}

/// Render one room's floor.
pub fn render_room_floor(
    renderer: &mut dyn MapRenderer,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    theme: &Theme,
) {
    let room = graph.room_by_id(&rl.room_id);
    let mut color = room_floor_color(room, theme);

    // Container rooms: tint floor with the containment group's color
    if let Some(group) = graph.containment_group(&rl.room_id) {
        let [gr, gg, gb, _ga] = group.color;
        let [fr, fg, fb, fa] = color;
        // Blend group color into floor at ~20% opacity
        let blend = |f: u8, g: u8| -> u8 {
            ((f as u16 * 80 + g as u16 * 20) / 100).min(255) as u8
        };
        color = [blend(fr, gr), blend(fg, gg), blend(fb, gb), fa];
    }

    render_room_floor_with_color(renderer, rl, graph, color);
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
/// Rotated rectangles and caves (a circle looks the same turned).
fn is_turned_room(rl: &RoomLayout, graph: &DungeonGraph) -> bool {
    rl.is_rotated() && graph.room_by_id(&rl.room_id).is_none_or(|r| r.shape != RoomShape::Circle)
}

/// The map grid over `floor` (the cells of the shown rooms and corridors). A rotated
/// room gets its own grid turned with it, so the world grid leaves its cells out.
pub fn render_map_grid(
    renderer: &mut dyn MapRenderer,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    floor: &CellSet,
    room_shown: impl Fn(&RoomLayout) -> bool,
    corridor_shown: impl Fn(&CorridorSegment) -> bool,
) {
    if !layout.rooms.iter().any(|rl| is_turned_room(rl, graph)) {
        render_grid(renderer, floor);
        return;
    }
    let turned: Vec<&RoomLayout> = layout.rooms.iter().filter(|rl| is_turned_room(rl, graph)).collect();
    let mut world = rasterize_floor_filtered(
        layout, graph, Coverage::Center,
        |rl| room_shown(rl) && !is_turned_room(rl, graph),
        &corridor_shown,
    );
    // Corridor ends reach into rooms; inside a turned room only its own grid shows
    world.retain(|&(x, y)| !turned.iter().any(|rl| rl.contains_point(x as f32 + 0.5, y as f32 + 0.5)));
    render_grid(renderer, &world);
    for rl in layout.rooms.iter().filter(|rl| is_turned_room(rl, graph) && room_shown(rl)) {
        // The room's cells in its unrotated frame, drawn through the turn
        let (w, h) = (rl.width as i32, rl.height as i32);
        let cells = graph.room_by_id(&rl.room_id).and_then(|r| r.cave_data.as_ref())
            .filter(|c| c.cells.len() == (w * h) as usize);
        let mut local = CellSet::default();
        for ly in 0..h {
            for lx in 0..w {
                if cells.is_none_or(|c| c.cells[(ly * w + lx) as usize]) {
                    local.insert((rl.x + lx, rl.y + ly));
                }
            }
        }
        render_grid(&mut RotatedRenderer::for_room(renderer, rl), &local);
    }
}

pub fn render_grid(
    renderer: &mut dyn MapRenderer,
    floor: &CellSet,
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
/// Draw a wall line (pixels), leaving out the parts hidden by overlapping rooms
/// (see [`crate::render::overlap`]).
fn draw_wall_clipped(
    renderer: &mut dyn MapRenderer,
    hidden_by: &[overlap::Interior<'_>],
    (x1, y1, x2, y2): (f32, f32, f32, f32),
    width: f32,
    color: [u8; 4],
) {
    if hidden_by.is_empty() {
        renderer.draw_line(x1, y1, x2, y2, width, color);
        return;
    }
    let g = GRID_PX;
    for ((ax, ay), (bx, by)) in overlap::visible_parts((x1 / g, y1 / g), (x2 / g, y2 / g), hidden_by) {
        renderer.draw_line(ax * g, ay * g, bx * g, by * g, width, color);
    }
}

/// Draw a cave's baked contour segments, minus the parts hidden by overlapping rooms.
pub fn render_cave_contours(
    renderer: &mut dyn MapRenderer,
    room_id: &str,
    segments: &[(f32, f32, f32, f32)],
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    color: [u8; 4],
) {
    let hidden_by = overlap::wall_suppressors(room_id, graph, layout);
    for &seg in segments {
        draw_wall_clipped(renderer, &hidden_by, seg, 2.0, color);
    }
}

/// Fill a freeform corridor's floor.
pub fn render_freeform_corridor_floor(renderer: &mut dyn MapRenderer, shape: &CorridorShape, color: [u8; 4]) {
    for piece in shape.floor_pieces() {
        let px: Vec<(f32, f32)> = piece.iter().map(|&(x, y)| (x * GRID_PX, y * GRID_PX)).collect();
        renderer.fill_polygon(&px, color);
    }
}

/// The exact walls of rotated rooms and freeform corridors, in pixels (for hatching).
fn freeform_wall_segments_px(shapes: &[Option<CorridorShape>], graph: &DungeonGraph, layout: &SpatialLayout) -> Vec<(f32, f32, f32, f32)> {
    freeform_wall_segments_px_filtered(shapes, graph, layout, |_| true, |_| true)
}

/// [`freeform_wall_segments_px`] for the rooms and corridors the filters accept.
pub fn freeform_wall_segments_px_filtered(
    shapes: &[Option<CorridorShape>],
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    room_ok: impl Fn(&RoomLayout) -> bool,
    corridor_ok: impl Fn(&CorridorSegment) -> bool,
) -> Vec<(f32, f32, f32, f32)> {
    let g = GRID_PX;
    let mut out = Vec::new();
    for rl in layout.rooms.iter().filter(|rl| rl.is_rotated() && room_ok(rl)) {
        let c = rl.corners();
        for i in 0..4 {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            out.push((a.0 * g, a.1 * g, b.0 * g, b.1 * g));
        }
    }
    for ci in 0..shapes.len() {
        if !corridor_ok(&layout.corridors[ci]) {
            continue;
        }
        for ((ax, ay), (bx, by)) in overlap::freeform_corridor_walls(ci, shapes, graph, layout) {
            out.push((ax * g, ay * g, bx * g, by * g));
        }
    }
    out
}

/// Walls of a rotated room: its four turned edges (or the circle rim), minus open walls
/// and the parts hidden by overlapping rooms or opened by attached corridors.
fn render_rotated_room_walls(
    renderer: &mut dyn MapRenderer,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
) {
    let room = graph.room_by_id(&rl.room_id);
    let mut hidden_by = overlap::wall_suppressors(&rl.room_id, graph, layout);
    hidden_by.extend(overlap::attached_corridor_interiors(&rl.room_id, graph, layout, overlap::is_open_passage));
    let g = GRID_PX;
    let wall = |renderer: &mut dyn MapRenderer, a: (f32, f32), b: (f32, f32)| {
        draw_wall_clipped(renderer, &hidden_by, (a.0 * g, a.1 * g, b.0 * g, b.1 * g), 2.0, theme.wall_color);
    };
    match room.map(|r| r.shape).unwrap_or_default() {
        RoomShape::Circle => {
            let (cx, cy) = rl.center();
            for (a, b) in overlap::circle_rim(cx, cy, rl.width.min(rl.height) as f32 / 2.0) {
                wall(renderer, a, b);
            }
        }
        // Generated caves draw their (already turned) contours; this is the outline
        // before generation
        RoomShape::Cave | RoomShape::Rectangle => {
            let open = room.map(|r| r.open_walls).unwrap_or_default();
            let c = rl.corners();
            // Edges in local order: north (top), east, south, west
            for (i, is_open) in [open.north, open.east, open.south, open.west].into_iter().enumerate() {
                if !is_open {
                    wall(renderer, c[i], c[(i + 1) % 4]);
                }
            }
        }
    }
}

pub fn render_room_walls(
    renderer: &mut dyn MapRenderer,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
) {
    if rl.is_rotated() {
        render_rotated_room_walls(renderer, rl, graph, layout, theme);
        return;
    }
    let wall_w = 2.0;
    let rx = rl.x as f32 * GRID_PX;
    let ry = rl.y as f32 * GRID_PX;
    let rw = rl.width as f32 * GRID_PX;
    let rh = rl.height as f32 * GRID_PX;
    let room = graph.room_by_id(&rl.room_id);
    let shape = room.map(|r| r.shape).unwrap_or_default();
    let open = room.map(|r| r.open_walls).unwrap_or_default();

    // Compute which walls are suppressed by flush connections
    let flush = flush_walls_with_layout(&rl.room_id, rl, graph, layout);
    // ...and which parts are hidden inside overlapping connected rooms, or opened by
    // freeform corridors ending here
    let mut hidden_by = overlap::wall_suppressors(&rl.room_id, graph, layout);
    hidden_by.extend(overlap::attached_corridor_interiors(&rl.room_id, graph, layout, overlap::is_open_passage));
    let wall = |renderer: &mut dyn MapRenderer, x1: f32, y1: f32, x2: f32, y2: f32| {
        draw_wall_clipped(renderer, &hidden_by, (x1, y1, x2, y2), wall_w, theme.wall_color);
    };

    match shape {
        RoomShape::Circle => {
            let cx = rx + rw / 2.0;
            let cy = ry + rh / 2.0;
            let r = rw.min(rh) / 2.0;
            if hidden_by.is_empty() {
                renderer.stroke_circle(cx, cy, r, wall_w, theme.wall_color);
            } else {
                let g = GRID_PX;
                for ((ax, ay), (bx, by)) in overlap::circle_rim(cx / g, cy / g, r / g) {
                    wall(renderer, ax * g, ay * g, bx * g, by * g);
                }
            }
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
                                wall(renderer, px, py, px + GRID_PX, py);
                            }
                            // Bottom edge
                            if !is_floor(lx, ly + 1) {
                                wall(renderer, px, py + GRID_PX, px + GRID_PX, py + GRID_PX);
                            }
                            // Left edge
                            if !is_floor(lx - 1, ly) {
                                wall(renderer, px, py, px, py + GRID_PX);
                            }
                            // Right edge
                            if !is_floor(lx + 1, ly) {
                                wall(renderer, px + GRID_PX, py, px + GRID_PX, py + GRID_PX);
                            }
                        }
                    }
                    return;
                }
            }
            // No cells yet — draw as rectangle
            wall(renderer, rx, ry, rx + rw, ry);
            wall(renderer, rx, ry + rh, rx + rw, ry + rh);
            wall(renderer, rx, ry, rx, ry + rh);
            wall(renderer, rx + rw, ry, rx + rw, ry + rh);
        }
        RoomShape::Rectangle => {
            // Draw each wall individually, skipping open walls, flush edges, and wall openings
            // Compute wall opening gaps (corridor widths at boundary crossings)
            let openings = &rl.wall_openings;

            // Helper: draw a wall line with gaps cut for wall openings
            let draw_wall_with_gaps = |renderer: &mut dyn MapRenderer, x1: f32, y1: f32, x2: f32, y2: f32, is_horizontal: bool| {
                if openings.is_empty() {
                    wall(renderer, x1, y1, x2, y2);
                    return;
                }

                // Collect gap ranges along this wall
                let mut gaps: Vec<(f32, f32)> = Vec::new();
                for wp in openings {
                    let wpx = wp.x as f32 * GRID_PX;
                    let wpy = wp.y as f32 * GRID_PX;
                    // Find corridor width from the connection that created this opening
                    let cw = layout.corridors.iter()
                        .find(|c| c.waypoints.iter().any(|p| *p == *wp))
                        .map(|c| c.width as f32 * GRID_PX)
                        .unwrap_or(GRID_PX * 2.0);
                    let half_cw = cw / 2.0;

                    if is_horizontal {
                        // Wall is horizontal; check if opening is on this wall (y matches)
                        let wall_y = y1;
                        if (wpy - wall_y).abs() < GRID_PX {
                            gaps.push((wpx - half_cw, wpx + half_cw));
                        }
                    } else {
                        // Wall is vertical; check if opening is on this wall (x matches)
                        let wall_x = x1;
                        if (wpx - wall_x).abs() < GRID_PX {
                            gaps.push((wpy - half_cw, wpy + half_cw));
                        }
                    }
                }

                if gaps.is_empty() {
                    wall(renderer, x1, y1, x2, y2);
                    return;
                }

                // Sort gaps and draw wall segments between them
                gaps.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                if is_horizontal {
                    let mut cur_x = x1;
                    for (gap_start, gap_end) in &gaps {
                        if *gap_start > cur_x {
                            wall(renderer, cur_x, y1, *gap_start, y2);
                        }
                        cur_x = *gap_end;
                    }
                    if cur_x < x2 {
                        wall(renderer, cur_x, y1, x2, y2);
                    }
                } else {
                    let mut cur_y = y1;
                    for (gap_start, gap_end) in &gaps {
                        if *gap_start > cur_y {
                            wall(renderer, x1, cur_y, x2, *gap_start);
                        }
                        cur_y = *gap_end;
                    }
                    if cur_y < y2 {
                        wall(renderer, x1, cur_y, x2, y2);
                    }
                }
            };

            // North wall
            if !open.north && !flush.north {
                draw_wall_with_gaps(renderer, rx, ry, rx + rw, ry, true);
            }
            // South wall
            if !open.south && !flush.south {
                draw_wall_with_gaps(renderer, rx, ry + rh, rx + rw, ry + rh, true);
            }
            // West wall
            if !open.west && !flush.west {
                draw_wall_with_gaps(renderer, rx, ry, rx, ry + rh, false);
            }
            // East wall
            if !open.east && !flush.east {
                draw_wall_with_gaps(renderer, rx + rw, ry, rx + rw, ry + rh, false);
            }
        }
    }
}

/// Determine which walls of a room are suppressed by flush or merge connections.
/// A wall is suppressed when this room shares that edge with a flush/merge-connected neighbor.
pub fn flush_walls_with_layout(
    room_id: &str,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
) -> OpenWalls {
    let mut walls = OpenWalls::default();
    for edge in &graph.connections {
        let is_flush = edge.connection.connection_type == ConnectionType::Flush;
        let is_merge = edge.connection.connection_type == ConnectionType::Merge;
        if (!is_flush && !is_merge) || edge.connection.keep_walls {
            continue;
        }
        let neighbor_id = if edge.source_room_id == room_id {
            &edge.target_room_id
        } else if edge.target_room_id == room_id {
            &edge.source_room_id
        } else {
            continue;
        };
        let Some(nrl) = layout.room_by_id(neighbor_id) else { continue };

        // Check which edge is shared (rooms must be adjacent)
        let r_right = rl.x + rl.width as i32;
        let r_bottom = rl.y + rl.height as i32;
        let n_right = nrl.x + nrl.width as i32;
        let n_bottom = nrl.y + nrl.height as i32;

        // Rooms share an edge if they are flush on one axis and overlap on the other
        let y_overlap = rl.y < n_bottom && nrl.y < r_bottom;
        let x_overlap = rl.x < n_right && nrl.x < r_right;

        if y_overlap && r_right == nrl.x { walls.east = true; }
        if y_overlap && rl.x == n_right { walls.west = true; }
        if x_overlap && r_bottom == nrl.y { walls.south = true; }
        if x_overlap && rl.y == n_bottom { walls.north = true; }
    }
    walls
}

/// Build the set of all cells inside cave room bounding boxes.
/// Used to prevent corridor walls from double-drawing at cave boundaries.
pub fn build_cave_cell_set(layout: &SpatialLayout, graph: &DungeonGraph) -> CellSet {
    let mut cells = CellSet::default();
    for rl in &layout.rooms {
        let is_cave = graph.room_by_id(&rl.room_id)
            .is_some_and(|r| r.shape == RoomShape::Cave && r.cave_data.as_ref().is_some_and(|c| !c.cells.is_empty()));
        if !is_cave { continue; }
        if rl.is_rotated() {
            let (x0, y0, x1, y1) = rl.cell_bounds();
            for y in y0..y1 {
                for x in x0..x1 {
                    if rl.contains_point(x as f32 + 0.5, y as f32 + 0.5) {
                        cells.insert((x, y));
                    }
                }
            }
            continue;
        }
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
    floor: &CellSet,
    theme: &Theme,
    cave_cells: &CellSet,
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
        // Freeform corridors open the rim by clipping it instead
        if geometry::is_freeform(corridor, layout, graph) {
            continue;
        }
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
    let palette = DecorPalette::from_ink(theme.wall_color);
    let mut sink = MapRendererSink { renderer };

    for decor in &room.decor {
        let cx = room_px_x + decor.x * GRID_PX;
        let cy = room_px_y + decor.y * GRID_PX;
        draw_decor(
            &mut sink, decor.decor_type, cx, cy, DECOR_HALF_SIZE,
            decor.scale_x, decor.scale_y, decor.rotation, &palette,
        );
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

        if section.opaque {
            renderer.fill_rect(sx, sy, sw, sh, theme.floor_color);
        }

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
/// A door on a rotated room, drawn along its turned wall where the corridor attaches.
pub fn render_rotated_door(
    renderer: &mut dyn MapRenderer,
    corridor: &CorridorSegment,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    kind: ConnectionType,
    dw: f32,
    door_depth: f32,
) {
    let Some(shape) = geometry::corridor_shape(corridor, layout, graph) else { return };
    let Some(attach) = shape.ends.iter().flatten().find(|a| a.room_id == rl.room_id) else { return };
    let quad = geometry::door_quad(attach, dw, door_depth);
    let px: Vec<(f32, f32)> = quad.iter().map(|&(x, y)| (x * GRID_PX, y * GRID_PX)).collect();
    let (cx, cy) = (attach.point.0 * GRID_PX, attach.point.1 * GRID_PX);
    let outline = |renderer: &mut dyn MapRenderer| {
        for i in 0..4 {
            let (a, b) = (px[i], px[(i + 1) % 4]);
            renderer.draw_line(a.0, a.1, b.0, b.1, 1.0, theme.wall_color);
        }
    };
    match kind {
        ConnectionType::Open | ConnectionType::Flush | ConnectionType::Merge => {}
        ConnectionType::Door | ConnectionType::OneWay => {
            renderer.fill_polygon(&px, [255, 255, 255, 255]);
            outline(renderer);
        }
        ConnectionType::Locked => {
            renderer.fill_polygon(&px, [255, 255, 255, 255]);
            outline(renderer);
            let r = dw.min(door_depth) * GRID_PX * 0.15;
            renderer.fill_rect(cx - r, cy - r, r * 2.0, r * 2.0, theme.wall_color);
        }
        ConnectionType::Secret => {
            renderer.draw_text("S", cx, cy, 6.0, theme.wall_color);
        }
    }
}

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
        if matches!(edge.connection.connection_type,
            ConnectionType::Open | ConnectionType::Flush | ConnectionType::Merge
        ) {
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

        // For child-to-parent connections, only draw door on the child side
        let src_is_child_of_tgt = graph.parent_of(&edge.source_room_id)
            .map(|p| p == edge.target_room_id).unwrap_or(false);
        let tgt_is_child_of_src = graph.parent_of(&edge.target_room_id)
            .map(|p| p == edge.source_room_id).unwrap_or(false);

        for (i, ((room_id, wp), exit)) in room_ids.iter().zip(wp_ends.iter()).zip(exits.iter()).enumerate() {
            // Skip door on the parent side of child-to-parent connections
            if i == 1 && src_is_child_of_tgt { continue; }
            if i == 0 && tgt_is_child_of_src { continue; }
            // Skip drawing door on cave room walls — caves have irregular boundaries
            let is_cave = graph.room_by_id(room_id)
                .is_some_and(|r| r.shape == RoomShape::Cave);
            if is_cave { continue; }
            let Some(rl) = layout.room_by_id(room_id) else { continue };

            if rl.is_rotated() {
                render_rotated_door(renderer, corridor, rl, graph, layout, theme, edge.connection.connection_type, dw, door_depth);
                continue;
            }
            let (dx1, dy1, dx2, dy2) = door_rect(rl, wp, *exit, dw, door_depth);

            let px = dx1 * GRID_PX;
            let py = dy1 * GRID_PX;
            let pw = (dx2 - dx1) * GRID_PX;
            let ph = (dy2 - dy1) * GRID_PX;

            match edge.connection.connection_type {
                ConnectionType::Open | ConnectionType::Flush | ConnectionType::Merge => {}
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

            if options.show_notes && !room.note_excerpt.is_empty() {
                renderer.draw_text(&room.note_excerpt, cx, cy + 14.0, 7.0, [120, 120, 120, 255]);
            }
        }
    }
}

/// Build the set of all floor cells from room and corridor geometry.
/// How a rotated room or freeform corridor, which doesn't sit on the grid, maps to cells.
/// Ordinary rooms and corridors give the same cells in every mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// The cell's center is inside (the map grid, lighting, fog, tokens).
    Center,
    /// Any part of the cell is inside (line of sight: only untouched cells are rock).
    Touched,
    /// The whole cell is inside (hatching, which then hugs the exact walls).
    Full,
}

pub fn build_floor_set(layout: &SpatialLayout, graph: &DungeonGraph) -> CellSet {
    rasterize_floor(layout, graph, Coverage::Center)
}

pub fn rasterize_floor(layout: &SpatialLayout, graph: &DungeonGraph, coverage: Coverage) -> CellSet {
    rasterize_floor_filtered(layout, graph, coverage, |_| true, |_| true)
}

/// Floor cells of the rooms and corridors the filters accept (the player view shows
/// only what the party has seen).
pub fn rasterize_floor_filtered(
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    coverage: Coverage,
    room_ok: impl Fn(&RoomLayout) -> bool,
    corridor_ok: impl Fn(&CorridorSegment) -> bool,
) -> CellSet {
    let mut floor: CellSet = CellSet::default();
    for rl in &layout.rooms {
        if !room_ok(rl) {
            continue;
        }
        let room = graph.room_by_id(&rl.room_id);
        let shape = room.map(|r| r.shape).unwrap_or_default();
        if rl.is_rotated() && shape != RoomShape::Circle {
            rasterize_rotated_room(&mut floor, rl, room, coverage);
            continue;
        }
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
        if !corridor_ok(corridor) {
            continue;
        }
        if let Some(shape) = geometry::corridor_shape(corridor, layout, graph) {
            match coverage {
                Coverage::Center => floor.extend(geometry::polygon_cells(&shape.polygon)),
                Coverage::Touched => floor.extend(geometry::polygon_touched_cells(&shape.polygon)),
                Coverage::Full => {
                    let (x0, y0, x1, y1) = geometry::bounds(&shape.polygon);
                    for gy in (y0.floor() as i32)..(y1.ceil() as i32) {
                        for gx in (x0.floor() as i32)..(x1.ceil() as i32) {
                            if shape.covers_cell(gx, gy) {
                                floor.insert((gx, gy));
                            }
                        }
                    }
                }
            }
            continue;
        }
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

/// Cells of a rotated rectangle or cave under `coverage`, sampling the cell's center
/// and corners in the room's local frame.
fn rasterize_rotated_room(floor: &mut CellSet, rl: &RoomLayout, room: Option<&Room>, coverage: Coverage) {
    let (w, h) = (rl.width as i32, rl.height as i32);
    let cells = room.and_then(|r| r.cave_data.as_ref())
        .filter(|c| room.is_some_and(|r| r.shape == RoomShape::Cave) && c.cells.len() == (w * h) as usize)
        .map(|c| c.cells.as_slice());
    let is_floor = |x: f32, y: f32| {
        let (lx, ly) = rl.to_local(x, y);
        if lx < 0.0 || ly < 0.0 || lx >= w as f32 || ly >= h as f32 {
            return false;
        }
        cells.is_none_or(|c| c[(ly as i32 * w + lx as i32) as usize])
    };
    let (x0, y0, x1, y1) = rl.cell_bounds();
    for gy in y0..y1 {
        for gx in x0..x1 {
            let (x, y) = (gx as f32, gy as f32);
            let center = is_floor(x + 0.5, y + 0.5);
            let corners = [(x + 0.01, y + 0.01), (x + 0.99, y + 0.01), (x + 0.99, y + 0.99), (x + 0.01, y + 0.99)];
            let take = match coverage {
                Coverage::Center => center,
                Coverage::Touched => center || corners.iter().any(|&(px, py)| is_floor(px, py))
                    || geometry::polygons_overlap(&[(x, y), (x + 1.0, y), (x + 1.0, y + 1.0), (x, y + 1.0)], &rl.corners()) && cells.is_none(),
                Coverage::Full => center && corners.iter().all(|&(px, py)| is_floor(px, py)),
            };
            if take {
                floor.insert((gx, gy));
            }
        }
    }
}
