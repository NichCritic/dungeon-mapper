use crate::util::CellSet;
use crate::render::overlap;
use crate::render::rotate::RotatedRenderer;
use crate::model::geometry::{self, CorridorShape};
use crate::model::*;
use crate::presentation::Visibility;
use crate::render::hatching::{draw_exterior_shading, ShadingParams};
use crate::render::decor::{draw_decor, DecorPalette, MapRendererSink};
use crate::render::traits::MapRenderer;
use crate::util::{DECOR_HALF_SIZE, GRID_PX};

/// Options controlling which elements to render.
#[derive(Clone, Copy)]
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
    render_map(renderer, graph, layout, theme, options, &Audience::Dm);
}

/// Who a render is for. The DM sees the whole map. Players see only what the party
/// has: hidden rooms and corridors are left out, explored ones dimmed, details and
/// labels only where they can see now, secrets never, and lighting baked in.
pub enum Audience<'a> {
    Dm,
    Players {
        visibility: &'a dyn crate::presentation::VisibilityProvider,
        lights: &'a [LightSource],
        ambient: f32,
        /// When play is on one floor: the whole map's layout and that floor, so the
        /// rooms players have seen on the floors below can show faintly underneath.
        ghosts: Option<(&'a SpatialLayout, i32)>,
    },
}

impl Audience<'_> {
    fn room(&self, room_id: &str) -> Visibility {
        match self {
            Audience::Dm => Visibility::Visible,
            Audience::Players { visibility, .. } => visibility.room_visibility(room_id).clone(),
        }
    }

    fn corridor(&self, connection_id: &str, graph: &DungeonGraph) -> Visibility {
        match self {
            Audience::Dm => Visibility::Visible,
            Audience::Players { visibility, .. } => crate::presentation::fog::corridor_visibility(connection_id, *visibility, graph),
        }
    }
}

/// Render the styled map for `audience` (the cached DM views, export, and the
/// player window and web image).
pub fn render_map(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    options: &RenderOptions,
    audience: &Audience,
) {
    let room_shown = |rl: &RoomLayout| audience.room(&rl.room_id) != Visibility::Hidden;
    let corridor_shown = |c: &CorridorSegment| audience.corridor(&c.connection_id, graph) != Visibility::Hidden;
    let floor = rasterize_floor_filtered(layout, graph, Coverage::Center, room_shown, corridor_shown);
    // Freeform geometry: angled corridors and rotated rooms (none on an ordinary map)
    let shapes: Vec<Option<CorridorShape>> = layout.corridors.iter()
        .map(|c| geometry::corridor_shape(c, layout, graph))
        .collect();
    let freeform = shapes.iter().any(Option::is_some) || layout.rooms.iter().any(|rl| rl.is_rotated());
    // Explored but not in sight: drawn faded toward the background
    let dimmed_floor = blend_toward(theme.floor_color, theme.background(), 0.4);
    let dimmed_theme = Theme { wall_color: blend_toward(theme.wall_color, theme.background(), 0.4), ..theme.clone() };

    // With ghosts the background reaches under the lower floors too
    let ghosts = match *audience { Audience::Players { ghosts, .. } => ghosts, Audience::Dm => None };
    render_background(renderer, ghosts.map_or(layout, |(full, _)| full), theme);
    // Outdoors the ground is part of the battlefield: grid it (rooms draw their own over it)
    if theme.ground.is_outdoor() && options.show_grid {
        render_ground_grid(renderer, ghosts.map_or(layout, |(full, _)| full));
    }
    if let Some((full_layout, floor)) = ghosts {
        render_floor_ghosts(renderer, graph, full_layout, floor, |room_id| audience.room(room_id) != Visibility::Hidden);
    }

    // Rooms in z-order so containers render before children
    let room_order = layout.render_order(graph);

    // Sort corridors by floor (max floor of connected rooms)
    let mut corridor_order: Vec<usize> = (0..layout.corridors.len()).collect();
    corridor_order.sort_by_key(|&i| {
        graph.connection_by_id(&layout.corridors[i].connection_id)
            .map(|e| {
                let sf = graph.room_by_id(&e.source_room_id).map(|r| r.floor.max_floor()).unwrap_or(0);
                let tf = graph.room_by_id(&e.target_room_id).map(|r| r.floor.max_floor()).unwrap_or(0);
                sf.max(tf)
            })
            .unwrap_or(0)
    });

    // Collect baked marching squares contour segments from cave rooms (used by shading)
    let mut contour_segments: Vec<(f32, f32, f32, f32)> = graph.rooms.iter()
        .filter(|r| audience.room(&r.id) != Visibility::Hidden)
        .filter_map(|r| r.cave_data.as_ref())
        .flat_map(|c| c.contour_segments.iter().copied())
        .collect();
    if freeform {
        // Hatch out from the exact angled walls, over cells the floor fully covers, so
        // the hatching meets a diagonal wall without stair-step gaps
        contour_segments.extend(freeform_wall_segments_px_filtered(&shapes, graph, layout, room_shown, corridor_shown));
        let covered = rasterize_floor_filtered(layout, graph, Coverage::Full, room_shown, corridor_shown);
        render_exterior_shading(renderer, layout, &covered, theme, &contour_segments);
    } else {
        render_exterior_shading(renderer, layout, &floor, theme, &contour_segments);
    }

    for &ri in &room_order {
        let rl = &layout.rooms[ri];
        let mut turned;
        let r: &mut dyn MapRenderer = if rl.is_rotated() {
            turned = RotatedRenderer::for_room(renderer, rl);
            &mut turned
        } else {
            renderer
        };
        match audience.room(&rl.room_id) {
            Visibility::Hidden => {}
            Visibility::Explored => render_room_floor_with_color(r, rl, graph, dimmed_floor),
            Visibility::Visible => render_room_floor(r, rl, graph, theme),
        }
    }
    for &ci in &corridor_order {
        let path = is_path(&layout.corridors[ci], graph);
        // Outdoors a corridor is just open ground: only tracks are drawn
        if theme.ground.is_outdoor() && !path {
            continue;
        }
        let surface = if path { PATH_COLOR } else { theme.floor_color };
        let color = match audience.corridor(&layout.corridors[ci].connection_id, graph) {
            Visibility::Hidden => continue,
            Visibility::Explored => blend_toward(surface, theme.background(), 0.4),
            Visibility::Visible => surface,
        };
        match &shapes[ci] {
            Some(shape) => render_freeform_corridor_floor(renderer, shape, color),
            None => render_corridor_floor_with_color(renderer, &layout.corridors[ci], color),
        }
    }
    if theme.corridor_chamfer != ChamferStyle::Sharp {
        for &ci in &corridor_order {
            if shapes[ci].is_none() && corridor_shown(&layout.corridors[ci]) {
                render_corridor_chamfers(renderer, &layout.corridors[ci], theme);
            }
        }
    }
    if options.show_grid && theme.ground.is_outdoor() {
        // The ground grid shows through open-ground areas; grid what covered it:
        // buildings' floors and tracks
        let covered = rasterize_floor_filtered(
            layout, graph, Coverage::Center,
            |rl| room_shown(rl) && !is_open_ground(rl, graph, theme),
            |c| corridor_shown(c) && is_path(c, graph),
        );
        render_map_grid(renderer, layout, graph, &covered, |rl| room_shown(rl) && !is_open_ground(rl, graph, theme), |c| corridor_shown(c) && is_path(c, graph));
    } else if options.show_grid {
        render_map_grid(renderer, layout, graph, &floor, room_shown, corridor_shown);
    }
    // Render room decor and elevation sections (after floors/grid, before walls)
    for &ri in &room_order {
        let rl = &layout.rooms[ri];
        if audience.room(&rl.room_id) != Visibility::Visible {
            continue;
        }
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
        let wall_theme = match audience.room(&rl.room_id) {
            Visibility::Hidden => continue,
            Visibility::Explored => &dimmed_theme,
            Visibility::Visible => theme,
        };
        // Cave rooms use baked marching squares contour segments
        let room = graph.room_by_id(&rl.room_id);
        if let Some(cave) = room.and_then(|r| {
            if r.shape == RoomShape::Cave { r.cave_data.as_ref() } else { None }
        }) {
            if !cave.contour_segments.is_empty() {
                render_cave_contours(renderer, &rl.room_id, &cave.contour_segments, graph, layout, wall_theme.wall_color);
                continue;
            }
        }
        render_room_walls(renderer, rl, graph, layout, wall_theme);
    }
    // Redraw corridor floors at circular room junctions to punch through
    // the circle wall stroke that covers the corridor opening.
    repair_circle_junctions(renderer, graph, layout, |c| match audience.corridor(&c.connection_id, graph) {
        Visibility::Hidden => None,
        Visibility::Explored => Some(dimmed_floor),
        Visibility::Visible => Some(theme.floor_color),
    });
    // Build set of cells inside cave rooms (so corridor walls don't double-draw there)
    let cave_cells = build_cave_cell_set(layout, graph);
    for &ci in &corridor_order {
        let wall_theme = match audience.corridor(&layout.corridors[ci].connection_id, graph) {
            Visibility::Hidden => continue,
            Visibility::Explored => &dimmed_theme,
            Visibility::Visible => theme,
        };
        // A path is a track across open ground, and outdoors nothing has corridor walls
        if theme.ground.is_outdoor() || is_path(&layout.corridors[ci], graph) {
            continue;
        }
        if shapes[ci].is_some() {
            for ((ax, ay), (bx, by)) in overlap::freeform_corridor_walls(ci, &shapes, graph, layout) {
                renderer.draw_line(ax * GRID_PX, ay * GRID_PX, bx * GRID_PX, by * GRID_PX, 2.0, wall_theme.wall_color);
            }
        } else {
            render_corridor_walls(renderer, &layout.corridors[ci], &floor, wall_theme, &cave_cells);
        }
    }
    render_doors(renderer, graph, layout, theme, options, audience);
    if options.show_labels {
        render_labels(renderer, graph, layout, options, |rl| audience.room(&rl.room_id) == Visibility::Visible);
    }
    // Players see the light the party has (the DM views overlay it live instead)
    if let Audience::Players { lights, ambient, .. } = *audience {
        if options.show_lighting && (!lights.is_empty() || ambient < 1.0) {
            // Outdoors the darkness lies over the open ground too
            let mut lit_area = floor.clone();
            add_open_ground(&mut lit_area, layout, theme);
            crate::render::presentation::render_lighting_overlay(renderer, layout, lights, ambient, &lit_area);
        }
    }
}

/// An outdoor area on an outdoor map: drawn as the ground itself, not as a floor.
fn is_open_ground(rl: &RoomLayout, graph: &DungeonGraph, theme: &Theme) -> bool {
    theme.ground.is_outdoor()
        && graph.room_by_id(&rl.room_id).is_some_and(|r| r.environment == RoomEnvironment::Outdoor)
}

/// A track's colour, for Path connections: a light grey against the white ground.
const PATH_COLOR: [u8; 4] = [228, 226, 220, 255];

/// Whether a corridor belongs to a Path connection (drawn as a track, without walls).
fn is_path(corridor: &CorridorSegment, graph: &DungeonGraph) -> bool {
    graph.connection_by_id(&corridor.connection_id)
        .is_some_and(|e| e.connection.connection_type == ConnectionType::Path)
}

/// Faint silhouettes of the rooms on floors below `floor` that `show_room` accepts, and
/// the corridors between them, drawn under the floor itself.
fn render_floor_ghosts(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    floor: i32,
    show_room: impl Fn(&str) -> bool,
) {
    const GHOST: [u8; 4] = [50, 50, 60, 45];
    let g = GRID_PX;
    let below = |id: &str| graph.room_by_id(id).is_some_and(|r| r.floor.relation(floor) == FloorRel::Below) && show_room(id);
    for rl in layout.rooms.iter().filter(|rl| below(&rl.room_id)) {
        let circle = graph.room_by_id(&rl.room_id).is_some_and(|r| r.shape == RoomShape::Circle);
        if circle {
            let (cx, cy) = rl.center();
            renderer.fill_circle(cx * g, cy * g, rl.width.min(rl.height) as f32 / 2.0 * g, GHOST);
        } else {
            let pts: Vec<(f32, f32)> = rl.corners().iter().map(|&(x, y)| (x * g, y * g)).collect();
            renderer.fill_polygon(&pts, GHOST);
        }
    }
    for c in &layout.corridors {
        let Some(edge) = graph.connection_by_id(&c.connection_id) else { continue };
        if graph.edge_floor_relation(edge, floor) != FloorRel::Below || !(below(&edge.source_room_id) || below(&edge.target_room_id)) {
            continue;
        }
        for (x0, y0, x1, y1) in c.run_boxes() {
            renderer.fill_rect(x0 as f32 * g, y0 as f32 * g, (x1 - x0) as f32 * g, (y1 - y0) as f32 * g, GHOST);
        }
    }
}

/// `color` moved `ratio` of the way toward `bg` (for areas only explored).
fn blend_toward(color: [u8; 4], bg: [u8; 4], ratio: f32) -> [u8; 4] {
    [
        (color[0] as f32 + (bg[0] as f32 - color[0] as f32) * ratio) as u8,
        (color[1] as f32 + (bg[1] as f32 - color[1] as f32) * ratio) as u8,
        (color[2] as f32 + (bg[2] as f32 - color[2] as f32) * ratio) as u8,
        color[3],
    ]
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

    renderer.fill_rect(x0, y0, w, h, theme.background());
    render_ground_texture(renderer, theme.ground, (ext_min_x - margin, ext_min_y - margin, ext_max_x + margin, ext_max_y + margin), theme.wall_color);
}

/// Sparse ink marks over an outdoor map's open ground, so it reads as grass, earth
/// or sand while staying black and white. Rooms drawn on top cover it.
fn render_ground_texture(renderer: &mut dyn MapRenderer, ground: Ground, (gx0, gy0, gx1, gy1): (i32, i32, i32, i32), ink: [u8; 4]) {
    if matches!(ground, Ground::Rock | Ground::Snow) {
        return;
    }
    let ink = [ink[0], ink[1], ink[2], 95];
    for gy in gy0..gy1 {
        for gx in gx0..gx1 {
            // A stable scatter: one mark in roughly one cell of four
            let h = (gx.wrapping_mul(73_856_093) ^ gy.wrapping_mul(19_349_663)) as u32;
            if h % 4 != 0 {
                continue;
            }
            let fx = (h >> 4) % 100;
            let fy = (h >> 12) % 100;
            let px = (gx as f32 + 0.15 + fx as f32 / 140.0) * GRID_PX;
            let py = (gy as f32 + 0.15 + fy as f32 / 140.0) * GRID_PX;
            match ground {
                Ground::Rock | Ground::Snow => {}
                Ground::Grass => {
                    renderer.draw_line(px, py, px - 1.8, py - 3.2, 0.6, ink);
                    renderer.draw_line(px, py, px + 0.3, py - 3.8, 0.6, ink);
                    renderer.draw_line(px, py, px + 2.0, py - 3.0, 0.6, ink);
                }
                Ground::Earth => {
                    renderer.fill_circle(px, py, 0.7, ink);
                    renderer.fill_circle(px + 3.0, py + 1.5, 0.5, ink);
                }
                Ground::Sand => {
                    renderer.fill_circle(px, py, 0.4, ink);
                    renderer.fill_circle(px + 2.5, py - 1.0, 0.4, ink);
                    renderer.fill_circle(px + 1.0, py + 2.2, 0.4, ink);
                }
            }
        }
    }
}

/// A grid over the open ground of an outdoor map, across the same area as the background.
fn render_ground_grid(renderer: &mut dyn MapRenderer, layout: &SpatialLayout) {
    let (ext_min_x, ext_min_y, ext_max_x, ext_max_y) = layout.extents();
    let margin = 2;
    let (gx0, gy0, gx1, gy1) = (ext_min_x - margin, ext_min_y - margin, ext_max_x + margin, ext_max_y + margin);
    let color = [60, 70, 50, 70];
    for gx in gx0..=gx1 {
        let x = gx as f32 * GRID_PX;
        renderer.draw_line(x, gy0 as f32 * GRID_PX, x, gy1 as f32 * GRID_PX, 0.5, color);
    }
    for gy in gy0..=gy1 {
        let y = gy as f32 * GRID_PX;
        renderer.draw_line(gx0 as f32 * GRID_PX, y, gx1 as f32 * GRID_PX, y, 0.5, color);
    }
}

pub fn render_exterior_shading(
    renderer: &mut dyn MapRenderer,
    layout: &SpatialLayout,
    floor: &CellSet,
    theme: &Theme,
    contour_segments: &[(f32, f32, f32, f32)],
) {
    // Open ground has no rock to shade
    if theme.exterior_shading && !theme.ground.is_outdoor() {
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
        // On open ground an outdoor area is that ground (its edges show as fences etc.)
        RoomEnvironment::Outdoor if theme.ground.is_outdoor() => theme.floor_color,
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

    // Container rooms: tint floor with the containment group's color (not an area of
    // open ground, which stays the plain paper of the ground around it)
    if is_open_ground(rl, graph, theme) {
        // The ground (and its texture) is already drawn beneath: an area of it adds nothing
        return;
    }
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
                    for (ly, x0, x1) in cave.floor_runs(rl.width as usize, 0..rl.height as usize) {
                        let px = (rl.x + x0 as i32) as f32 * GRID_PX;
                        let py = (rl.y + ly as i32) as f32 * GRID_PX;
                        renderer.fill_rect(px, py, (x1 - x0) as f32 * GRID_PX, GRID_PX, color);
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

/// Render one corridor's floor with a specific color.
pub fn render_corridor_floor_with_color(
    renderer: &mut dyn MapRenderer,
    corridor: &CorridorSegment,
    color: [u8; 4],
) {
    for (min_gx, min_gy, max_gx, max_gy) in corridor.run_boxes() {

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
                renderer.fill_circle(ocx, ocy, chamfer_r, theme.background());
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
                    renderer.draw_line(x1, y1, x2, y2, 1.0, theme.background());
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

/// Draw one side of a room in its boundary's style (in pixels), minus the parts hidden
/// by overlapping rooms. Full walls are the plain wall line.
fn draw_boundary_clipped(
    renderer: &mut dyn MapRenderer,
    hidden_by: &[overlap::Interior<'_>],
    (x1, y1, x2, y2): (f32, f32, f32, f32),
    kind: WallKind,
    theme: &Theme,
) {
    // Outdoors an open side still marks where the area ends, faintly
    if kind == WallKind::Open && !theme.ground.is_outdoor() {
        return;
    }
    if hidden_by.is_empty() {
        draw_boundary(renderer, (x1, y1, x2, y2), kind, theme);
        return;
    }
    let g = GRID_PX;
    for ((ax, ay), (bx, by)) in overlap::visible_parts((x1 / g, y1 / g), (x2 / g, y2 / g), hidden_by) {
        draw_boundary(renderer, (ax * g, ay * g, bx * g, by * g), kind, theme);
    }
}

/// Points every `step` pixels along a segment, starting half a step in.
fn points_along((x1, y1, x2, y2): (f32, f32, f32, f32), step: f32) -> impl Iterator<Item = (f32, f32)> {
    let len = (x2 - x1).hypot(y2 - y1);
    let n = (len / step).floor().max(1.0) as usize;
    let spacing = len / n as f32;
    (0..n).map(move |i| {
        let t = (i as f32 + 0.5) * spacing / len.max(f32::EPSILON);
        (x1 + (x2 - x1) * t, y1 + (y2 - y1) * t)
    })
}

/// One stretch of boundary in its kind's style (pixels).
pub fn draw_boundary(renderer: &mut dyn MapRenderer, seg: (f32, f32, f32, f32), kind: WallKind, theme: &Theme) {
    let (x1, y1, x2, y2) = seg;
    match kind {
        WallKind::Open => {
            // Dotted: the edge of an open area outdoors
            let ink = [theme.wall_color[0], theme.wall_color[1], theme.wall_color[2], 110];
            for (px, py) in points_along(seg, GRID_PX / 4.0) {
                renderer.fill_circle(px, py, 0.8, ink);
            }
        }
        WallKind::Wall => renderer.draw_line(x1, y1, x2, y2, 2.0, theme.wall_color),
        WallKind::Fence => {
            // A rail with a post every square
            renderer.draw_line(x1, y1, x2, y2, 1.0, theme.wall_color);
            for (px, py) in points_along(seg, GRID_PX) {
                renderer.fill_rect(px - 1.8, py - 1.8, 3.6, 3.6, theme.wall_color);
            }
        }
        WallKind::StoneWall => {
            // Two faces with stones laid between them
            let len = (x2 - x1).hypot(y2 - y1).max(f32::EPSILON);
            let (nx, ny) = (-(y2 - y1) / len * 2.4, (x2 - x1) / len * 2.4);
            renderer.draw_line(x1 + nx, y1 + ny, x2 + nx, y2 + ny, 0.9, theme.wall_color);
            renderer.draw_line(x1 - nx, y1 - ny, x2 - nx, y2 - ny, 0.9, theme.wall_color);
            for (i, (px, py)) in points_along(seg, GRID_PX / 3.0).enumerate() {
                let r = if i % 2 == 0 { 1.6 } else { 1.2 };
                renderer.stroke_circle(px, py, r, 0.6, theme.wall_color);
            }
        }
        WallKind::Hedge => {
            // Scalloped foliage: a pale green band, its two edges inked as rows of
            // little arcs so it reads as one bushy mass
            let len = (x2 - x1).hypot(y2 - y1).max(f32::EPSILON);
            let (tx, ty) = ((x2 - x1) / len, (y2 - y1) / len);
            let (nx, ny) = (-ty, tx);
            let r = 4.5;
            renderer.draw_line(x1, y1, x2, y2, r * 2.4, FOLIAGE_WASH);
            for (px, py) in points_along(seg, r * 1.7) {
                for side in [1.0f32, -1.0] {
                    // A half circle bulging outward from the band's edge
                    let (cx, cy) = (px + nx * side * r * 0.5, py + ny * side * r * 0.5);
                    let pts: Vec<(f32, f32)> = (0..=6).map(|k| {
                        let a = std::f32::consts::PI * k as f32 / 6.0;
                        let (along, out) = (-a.cos() * r * 0.8, a.sin() * r * 0.8 * side);
                        (cx + tx * along + nx * out, cy + ty * along + ny * out)
                    }).collect();
                    for w in pts.windows(2) {
                        renderer.draw_line(w[0].0, w[0].1, w[1].0, w[1].1, 0.8, theme.wall_color);
                    }
                }
            }
        }
    }
}

/// The one colour boundaries and ground cover use: a pale wash for foliage, matching
/// the green of the tree and bush decor.
const FOLIAGE_WASH: [u8; 4] = [120, 175, 110, 120];

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
            let sides = room.map(|r| r.walls).unwrap_or_default();
            let c = rl.corners();
            // Edges in local order: north (top), east, south, west
            for (i, kind) in [sides.north, sides.east, sides.south, sides.west].into_iter().enumerate() {
                let (a, b) = (c[i], c[(i + 1) % 4]);
                draw_boundary_clipped(renderer, &hidden_by, (a.0 * g, a.1 * g, b.0 * g, b.1 * g), kind, theme);
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
    let sides = room.map(|r| r.walls).unwrap_or_default();

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

            // Helper: draw one side in its boundary style, with gaps cut for wall openings
            let draw_wall_with_gaps = |renderer: &mut dyn MapRenderer, x1: f32, y1: f32, x2: f32, y2: f32, is_horizontal: bool, kind: WallKind| {
                let wall = |renderer: &mut dyn MapRenderer, x1: f32, y1: f32, x2: f32, y2: f32| {
                    draw_boundary_clipped(renderer, &hidden_by, (x1, y1, x2, y2), kind, theme);
                };
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

            if !flush.north {
                draw_wall_with_gaps(renderer, rx, ry, rx + rw, ry, true, sides.north);
            }
            if !flush.south {
                draw_wall_with_gaps(renderer, rx, ry + rh, rx + rw, ry + rh, true, sides.south);
            }
            if !flush.west {
                draw_wall_with_gaps(renderer, rx, ry, rx, ry + rh, false, sides.west);
            }
            if !flush.east {
                draw_wall_with_gaps(renderer, rx + rw, ry, rx + rw, ry + rh, false, sides.east);
            }
        }
    }
}

/// Sides of a room left without a wall because a flush or merge neighbour shares them.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlushSides {
    pub north: bool,
    pub south: bool,
    pub east: bool,
    pub west: bool,
}

/// Determine which walls of a room are suppressed by flush or merge connections.
/// A wall is suppressed when this room shares that edge with a flush/merge-connected neighbor.
pub fn flush_walls_with_layout(
    room_id: &str,
    rl: &RoomLayout,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
) -> FlushSides {
    let mut walls = FlushSides::default();
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
    for (min_gx, min_gy, max_gx, max_gy) in corridor.run_boxes() {

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
/// `floor_color` gives each corridor's floor colour for the audience, or `None` for a
/// corridor they can't see (its opening stays closed).
pub fn repair_circle_junctions(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    floor_color: impl Fn(&CorridorSegment) -> Option<[u8; 4]>,
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
        // A corridor the audience can't see leaves the rim whole
        let Some(color) = floor_color(corridor) else { continue };
        for (min_gx, min_gy, max_gx, max_gy) in corridor.run_boxes() {

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
                    renderer.fill_rect(px, py, pw, ph, color);
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

        // Ground cover and natural outlines are drawn as areas, not boxes with ticks
        if section.organic || section.elevation.is_terrain() {
            let pts: Vec<(f32, f32)> = section.outline().iter()
                .map(|&(x, y)| (room_px_x + x * GRID_PX, room_px_y + y * GRID_PX))
                .collect();
            draw_section_area(renderer, &pts, section, theme);
            continue;
        }

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
            // Drawn as areas above
            ElevationType::Rough | ElevationType::TallGrass | ElevationType::Thicket => {}
        }
    }
}

/// A section drawn as an area within its outline (pixels): organic shapes of any kind,
/// and the ground-cover kinds, which get a scattered texture.
fn draw_section_area(renderer: &mut dyn MapRenderer, pts: &[(f32, f32)], section: &ElevationSection, theme: &Theme) {
    let wall = theme.wall_color;
    let tint = |a: u8| [wall[0], wall[1], wall[2], a];
    // Plain paper first, so the ground's texture doesn't show through a translucent fill
    renderer.fill_polygon(pts, theme.floor_color);
    let (fill, edge) = match section.elevation {
        ElevationType::Raised => (tint(25), wall),
        ElevationType::Lowered => (tint(40), wall),
        ElevationType::Steps | ElevationType::Slope => (tint(20), wall),
        ElevationType::BottomlessPit => ([20, 20, 20, 210], wall),
        ElevationType::Hole => (tint(90), wall),
        ElevationType::Water => ([80, 140, 210, 110], [50, 100, 170, 220]),
        ElevationType::Rough => ([0, 0, 0, 0], tint(110)),
        ElevationType::TallGrass => ([0, 0, 0, 0], tint(110)),
        ElevationType::Thicket => (FOLIAGE_WASH, tint(200)),
    };
    renderer.fill_polygon(pts, fill);
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        renderer.draw_line(a.0, a.1, b.0, b.1, 1.2, edge);
    }
    if !section.elevation.is_terrain() {
        return;
    }
    // Scatter marks on a jittered grid, kept inside the outline
    let (min_x, max_x) = pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
    let (min_y, max_y) = pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    let step = GRID_PX * if section.elevation == ElevationType::TallGrass { 0.6 } else { 0.45 };
    let jitter = |i: i32, j: i32, salt: i32| {
        let h = (i.wrapping_mul(73_856_093) ^ j.wrapping_mul(19_349_663) ^ salt.wrapping_mul(83_492_791)) as u32;
        ((h % 1000) as f32 / 1000.0 - 0.5) * step * 0.8
    };
    let (cols, rows) = (((max_x - min_x) / step).ceil() as i32, ((max_y - min_y) / step).ceil() as i32);
    for j in 0..rows {
        for i in 0..cols {
            let px = min_x + (i as f32 + 0.5) * step + jitter(i, j, 1);
            let py = min_y + (j as f32 + 0.5) * step + jitter(i, j, 2);
            if !geometry::point_in_polygon((px, py), pts) {
                continue;
            }
            match section.elevation {
                ElevationType::Rough => {
                    if (i + j) % 3 == 0 {
                        renderer.draw_line(px - 2.0, py, px + 2.0, py - 0.6, 0.8, tint(180));
                    } else {
                        renderer.fill_circle(px, py, 1.1, tint(180));
                    }
                }
                ElevationType::TallGrass => {
                    renderer.draw_line(px, py, px - 2.5, py - 4.0, 0.7, tint(150));
                    renderer.draw_line(px, py, px, py - 5.0, 0.7, tint(150));
                    renderer.draw_line(px, py, px + 2.5, py - 4.0, 0.7, tint(150));
                }
                _ => renderer.stroke_circle(px, py, 3.2, 0.8, tint(200)),
            }
        }
    }
}

/// Render door symbols on corridors.
/// Draw a door of `kind` with outline `shape` (grid units).
fn draw_door(renderer: &mut dyn MapRenderer, shape: &geometry::DoorShape, kind: ConnectionType, theme: &Theme) {
    let g = GRID_PX;
    let (cx, cy) = { let c = shape.center(); (c.0 * g, c.1 * g) };
    let white = [255, 255, 255, 255];
    match *shape {
        geometry::DoorShape::Rect(x0, y0, x1, y1) => {
            let (px, py, pw, ph) = (x0 * g, y0 * g, (x1 - x0) * g, (y1 - y0) * g);
            match kind {
                ConnectionType::Open | ConnectionType::Flush | ConnectionType::Merge | ConnectionType::Path => {}
                ConnectionType::Door | ConnectionType::OneWay | ConnectionType::Locked => {
                    renderer.fill_rect(px, py, pw, ph, white);
                    renderer.stroke_rect(px, py, pw, ph, 1.0, theme.wall_color);
                    if kind == ConnectionType::Locked {
                        let r = pw.min(ph) * 0.15;
                        renderer.fill_rect(cx - r, cy - r, r * 2.0, r * 2.0, theme.wall_color);
                    }
                }
                ConnectionType::Secret => renderer.draw_text("S", cx, cy, 6.0, theme.wall_color),
            }
        }
        geometry::DoorShape::Quad(quad, _) => {
            let px: Vec<(f32, f32)> = quad.iter().map(|&(x, y)| (x * g, y * g)).collect();
            match kind {
                ConnectionType::Open | ConnectionType::Flush | ConnectionType::Merge | ConnectionType::Path => {}
                ConnectionType::Door | ConnectionType::OneWay | ConnectionType::Locked => {
                    renderer.fill_polygon(&px, white);
                    for i in 0..4 {
                        let (a, b) = (px[i], px[(i + 1) % 4]);
                        renderer.draw_line(a.0, a.1, b.0, b.1, 1.0, theme.wall_color);
                    }
                    if kind == ConnectionType::Locked {
                        // The quad's short side is the door's depth across the wall
                        let side = |i: usize| ((px[i].0 - px[i + 1].0).powi(2) + (px[i].1 - px[i + 1].1).powi(2)).sqrt();
                        let r = side(0).min(side(1)) * 0.15;
                        renderer.fill_rect(cx - r, cy - r, r * 2.0, r * 2.0, theme.wall_color);
                    }
                }
                ConnectionType::Secret => renderer.draw_text("S", cx, cy, 6.0, theme.wall_color),
            }
        }
    }
}

pub fn render_doors(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    options: &RenderOptions,
    audience: &Audience,
) {
    let players = matches!(audience, Audience::Players { .. });
    for end in geometry::door_ends(graph, layout) {
        let kind = end.edge.connection.connection_type;
        // Players never see secret doors; passages have no door
        if kind.is_passage() || ((players || !options.show_secrets) && kind == ConnectionType::Secret) {
            continue;
        }
        // Skip drawing door on cave room walls — caves have irregular boundaries
        if graph.room_by_id(&end.rl.room_id).is_some_and(|r| r.shape == RoomShape::Cave) {
            continue;
        }
        // Players see a door from either side they know: its room or its corridor
        if audience.room(&end.rl.room_id) == Visibility::Hidden
            && audience.corridor(&end.edge.connection.id, graph) == Visibility::Hidden
        {
            continue;
        }
        let width = end.edge.connection.door_width() as f32;
        if let Some(shape) = end.shape(graph, layout, width, 0.3) {
            draw_door(renderer, &shape, kind, theme);
        }
    }
}

/// Render room labels and notes.
pub fn render_labels(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    options: &RenderOptions,
    labelled: impl Fn(&RoomLayout) -> bool,
) {
    for rl in layout.rooms.iter().filter(|rl| labelled(rl)) {
        if let Some(room) = graph.room_by_id(&rl.room_id) {
            let (cx, cy) = crate::util::room_center_px(rl);
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

/// Cells creatures stand on and light and sight cross: the floor, and on an outdoor
/// map the open ground around it too (out to the background's margin).
pub fn open_cells(layout: &SpatialLayout, graph: &DungeonGraph, theme: &Theme) -> CellSet {
    let mut cells = build_floor_set(layout, graph);
    add_open_ground(&mut cells, layout, theme);
    cells
}

/// On an outdoor map, add the open ground around the rooms to `cells`.
fn add_open_ground(cells: &mut CellSet, layout: &SpatialLayout, theme: &Theme) {
    if !theme.ground.is_outdoor() || layout.rooms.is_empty() {
        return;
    }
    let (x0, y0, x1, y1) = layout.extents();
    for y in (y0 - 2)..(y1 + 2) {
        for x in (x0 - 2)..(x1 + 2) {
            cells.insert((x, y));
        }
    }
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
                let (cx, cy) = rl.center();
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
        for (min_x, min_y, max_x, max_y) in corridor.run_boxes() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::recording::{RecordingRenderer, RenderCommand};

    #[test]
    fn a_cave_left_of_the_origin_renders_its_floor_in_place() {
        // A 3x2 cave at x=-13: its floor once went through `x as usize`, which panics
        // (overflow) in debug builds and so stalled the background renders
        let mut graph = DungeonGraph::new();
        let mut r = Room::new("Cave".into());
        r.shape = RoomShape::Cave;
        r.cave_data = Some(CaveData {
            cells: vec![true, true, false, false, true, true],
            seed: 0,
            algorithm: CaveAlgorithm::CellularAutomata,
            density: 0.45,
            smoothing_iterations: 4,
            generation: 0,
            contour_segments: Vec::new(),
        });
        let layout = SpatialLayout {
            rooms: vec![RoomLayout {
                room_id: r.id.clone(), x: -13, y: 5, width: 3, height: 2,
                violations: Vec::new(), wall_openings: Vec::new(), rotation: 0.0,
            }],
            ..SpatialLayout::new()
        };
        graph.add_room(r);
        let mut rec = RecordingRenderer::new();
        let options = RenderOptions { show_grid: false, show_labels: false, show_notes: false, show_secrets: false, show_decor: false, show_lighting: false };
        render_themed(&mut rec, &graph, &layout, &Theme::default(), &options);
        let g = GRID_PX;
        let floor = |x: f32, y: f32, w: f32| rec.commands.iter().any(|c| matches!(c,
            RenderCommand::FillRect { x: cx, y: cy, w: cw, h, .. } if *cx == x * g && *cy == y * g && *cw == w * g && *h == g));
        assert!(floor(-13.0, 5.0, 2.0), "first row's run");
        assert!(floor(-12.0, 6.0, 2.0), "second row's run");
    }
}
