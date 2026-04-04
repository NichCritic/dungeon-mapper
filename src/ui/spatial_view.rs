use crate::model::*;
use crate::ui::canvas_common::{
    handle_pan_zoom, draw_dashed_line,
    ViewState, COLOR_SPATIAL_BG, COLOR_SELECTION, COLOR_PLACEHOLDER_TEXT,
};
use crate::util::{grid_to_world, point_to_segment_dist, world_to_grid, ViewTransform, GRID_PX};

/// What's currently being dragged in the spatial view
#[derive(Clone, Debug, Default)]
enum DragTarget {
    #[default]
    None,
    Room(String),
    /// Dragging a corridor waypoint: (corridor index, waypoint index)
    Waypoint(usize, usize),
    /// Dragging a group constraint corner: (group index, corner: 0=TL 1=TR 2=BL 3=BR)
    GroupCorner(usize, u8),
    /// Dragging a whole group (group index)
    Group(usize),
    /// Dragging an elevation section: (room_id, section index)
    Section(String, usize),
    /// Dragging a corridor exit handle: (connection_id, is_source_exit)
    Exit(String, bool),
}

pub struct SpatialViewState {
    pub view: ViewState,
    pub selected_room: Option<String>,
    pub selected_corridor: Option<usize>,
    pub selected_waypoint: Option<usize>,
    pub selected_group: Option<usize>,
    /// Selected elevation section within a room (room_id, section index).
    pub selected_section: Option<(String, usize)>,
    drag_target: DragTarget,
    drag_accum: egui::Vec2,
    pub density_gap: u32,
    /// Set by sidebar "Recompute All" button, consumed by app.
    pub recompute_requested: bool,
    /// Set when cave cells are edited, consumed by app to recompute contours.
    pub cave_contours_dirty: bool,
    /// Currently viewed floor (None = show all floors)
    pub current_floor: Option<i32>,
}

impl Default for SpatialViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            selected_room: None,
            selected_corridor: None,
            selected_waypoint: None,
            selected_group: None,
            selected_section: None,
            drag_target: DragTarget::None,
            drag_accum: egui::Vec2::ZERO,
            density_gap: 0,
            recompute_requested: false,
            cave_contours_dirty: false,
            current_floor: None,
        }
    }
}

/// Fix diagonal segments by inserting corner waypoints.
/// Does NOT remove collinear points — user-placed waypoints are preserved.
fn resolve_diagonal_segments(waypoints: &mut Vec<GridPos>) {
    let mut i = 0;
    while i + 1 < waypoints.len() {
        let a = waypoints[i];
        let b = waypoints[i + 1];
        if a.x != b.x && a.y != b.y {
            let corner = GridPos { x: b.x, y: a.y };
            waypoints.insert(i + 1, corner);
            i += 2;
        } else {
            i += 1;
        }
    }
}

/// Clean up auto-inserted corners (collinear/duplicate points) then
/// re-insert corners for diagonals. Used during live drag to prevent
/// phantom waypoint accumulation.
fn resolve_diagonal_segments_clean(waypoints: &mut Vec<GridPos>) {
    // Remove collinear points and duplicates (auto-inserted corners from
    // previous frames that are no longer needed after the drag moved).
    let mut i = 1;
    while i + 1 < waypoints.len() {
        let prev = waypoints[i - 1];
        let curr = waypoints[i];
        let next = waypoints[i + 1];

        let collinear_x = prev.x == curr.x && curr.x == next.x;
        let collinear_y = prev.y == curr.y && curr.y == next.y;
        let duplicate = (curr.x == prev.x && curr.y == prev.y)
            || (curr.x == next.x && curr.y == next.y);

        if collinear_x || collinear_y || duplicate {
            waypoints.remove(i);
        } else {
            i += 1;
        }
    }

    resolve_diagonal_segments(waypoints);
}

/// Collect all distinct floor numbers used by rooms in the graph, sorted.
pub(crate) fn collect_floors(graph: &DungeonGraph) -> Vec<i32> {
    let mut floors: Vec<i32> = graph.rooms.iter()
        .flat_map(|r| r.floor.floors())
        .collect::<std::collections::BTreeSet<i32>>()
        .into_iter()
        .collect();
    if floors.is_empty() {
        floors.push(0);
    }
    floors
}

/// Darken a color by multiplying RGB and reducing alpha.
fn dim_color(c: egui::Color32, factor: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (c.r() as f32 * factor) as u8,
        (c.g() as f32 * factor) as u8,
        (c.b() as f32 * factor) as u8,
        (c.a() as f32 * factor.sqrt()) as u8,
    )
}

/// Check if a room is on a lower floor than the current floor.
fn is_lower_floor(room: &crate::model::Room, current_floor: i32) -> bool {
    room.floor.floors().iter().all(|f| *f < current_floor)
}

const LOWER_FLOOR_DIM: f32 = 0.35;

const HANDLE_RADIUS: f32 = 5.0;
/// Hit radius in screen pixels (fixed, does not scale with zoom).
const HANDLE_HIT_RADIUS: f32 = 12.0;
/// Size of exit handle diamond (multiplied by zoom at draw time).
const EXIT_HANDLE_SIZE: f32 = 4.0;

/// Test if a screen-space point is inside an exit handle diamond.
/// Returns Some((connection_id, is_source)) if hit, None otherwise.
fn hit_test_exit_handles(
    pos: egui::Pos2,
    selected_room_id: &str,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    transform: &ViewTransform,
    zoom: f32,
) -> Option<(String, bool)> {
    let room_rl = layout.room_by_id(selected_room_id)?;
    let diamond_size = EXIT_HANDLE_SIZE * zoom;

    for edge in &graph.connections {
        let (is_source, other_room_id) = if edge.source_room_id == selected_room_id {
            (true, &edge.target_room_id)
        } else if edge.target_room_id == selected_room_id {
            (false, &edge.source_room_id)
        } else {
            continue;
        };
        let exit_opt = if is_source { &edge.source_exit } else { &edge.target_exit };
        let other_rl = layout.room_by_id(other_room_id)?;
        let exit_pos = match exit_opt {
            Some(p) => *p,
            None => default_exit_pos(room_rl, other_rl, edge.connection.corridor_width),
        };
        let center = transform.world_to_screen(
            egui::pos2(exit_pos.x * GRID_PX, exit_pos.y * GRID_PX),
        );
        // Diamond hit = Manhattan distance <= size
        let dx = (pos.x - center.x).abs();
        let dy = (pos.y - center.y).abs();
        if dx + dy <= diamond_size {
            return Some((edge.connection.id.clone(), is_source));
        }
    }
    None
}

pub fn spatial_view(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut SpatialViewState) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;

    painter.rect_filled(rect, 0.0, COLOR_SPATIAL_BG);

    handle_pan_zoom(&response, &mut state.view);
    let transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);

    if let Some(layout) = &dungeon.layout {
        draw_infinite_grid(&painter, &transform, rect);
        draw_groups_spatial(&painter, &transform, layout, &dungeon.graph, state);
        draw_bounds(&painter, &transform, layout);
        draw_corridors(&painter, &transform, layout, &dungeon.graph, state);
        draw_rooms(&painter, &transform, layout, &dungeon.graph, state);
        draw_doors(&painter, &transform, layout, &dungeon.graph, state);
        draw_waypoint_handles(&painter, &transform, layout, state);
        draw_exit_handles(&painter, &transform, layout, &dungeon.graph, state);
    } else if !dungeon.graph.rooms.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Layout will be generated automatically.",
            egui::FontId::proportional(16.0),
            COLOR_PLACEHOLDER_TEXT,
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Add rooms in the Graph tab first.",
            egui::FontId::proportional(16.0),
            COLOR_PLACEHOLDER_TEXT,
        );
    }

    if dungeon.layout.is_some() {
        handle_spatial_interactions(ui, &response, &transform, dungeon, state);
    }
}

fn handle_spatial_interactions(
    ui: &egui::Ui,
    response: &egui::Response,
    transform: &ViewTransform,
    dungeon: &mut Dungeon,
    state: &mut SpatialViewState,
) {
    // === DRAG START ===
    if response.drag_started_by(egui::PointerButton::Primary) {
        if let Some(pos) = response.hover_pos() {
            let world = transform.screen_to_world(pos);

            // First check: waypoint handles (highest priority when a corridor is selected)
            if let Some(ci) = state.selected_corridor {
                if let Some(layout) = &dungeon.layout {
                    if ci < layout.corridors.len() {
                        let corridor = &layout.corridors[ci];
                        for (wi, wp) in corridor.waypoints.iter().enumerate() {
                            let wp_screen = transform.world_to_screen(
                                egui::pos2(grid_to_world(wp.x), grid_to_world(wp.y)),
                            );
                            if pos.distance(wp_screen) < HANDLE_HIT_RADIUS {
                                state.selected_waypoint = Some(wi);
                                state.drag_target = DragTarget::Waypoint(ci, wi);
                                state.drag_accum = egui::Vec2::ZERO;
                                return;
                            }
                        }
                    }
                }
            }

            // Check exit handles (when a room is selected)
            if let Some(ref selected_room_id) = state.selected_room {
                if let Some(layout) = &dungeon.layout {
                    if let Some((conn_id, is_source)) = hit_test_exit_handles(
                        pos, selected_room_id, layout, &dungeon.graph, &transform, state.view.zoom,
                    ) {
                        state.drag_target = DragTarget::Exit(conn_id, is_source);
                        state.drag_accum = egui::Vec2::ZERO;
                        return;
                    }
                }
            }

            // Check group corners (when a group with constraints is visible)
            if let Some(layout) = &dungeon.layout {
                for (gi, group) in dungeon.graph.groups.iter().enumerate() {
                    if group.max_width.is_none() && group.max_height.is_none() {
                        continue;
                    }
                    if let Some((gx, gy, gw, gh)) = group.spatial_bounds(layout) {
                        let corners = [
                            (gx, gy),                              // TL
                            (gx + gw as i32, gy),                  // TR
                            (gx, gy + gh as i32),                  // BL
                            (gx + gw as i32, gy + gh as i32),     // BR
                        ];
                        for (ci, &(cx, cy)) in corners.iter().enumerate() {
                            let screen_c = transform.world_to_screen(
                                egui::pos2(grid_to_world(cx), grid_to_world(cy)),
                            );
                            if pos.distance(screen_c) < HANDLE_HIT_RADIUS {
                                state.selected_group = Some(gi);
                                state.drag_target = DragTarget::GroupCorner(gi, ci as u8);
                                state.drag_accum = egui::Vec2::ZERO;
                                return;
                            }
                        }
                    }
                }
            }

            // Check elevation sections (if a room with sections is selected)
            if let Some((ref sec_room_id, sec_idx)) = state.selected_section {
                if let Some(layout) = &dungeon.layout {
                    if let Some(rl) = layout.room_by_id(sec_room_id) {
                        if let Some(room) = dungeon.graph.room_by_id(sec_room_id) {
                            if sec_idx < room.sections.len() {
                                let sec = &room.sections[sec_idx];
                                let room_px_x = rl.x as f32 * GRID_PX;
                                let room_px_y = rl.y as f32 * GRID_PX;
                                let sx = room_px_x + sec.x * GRID_PX;
                                let sy = room_px_y + sec.y * GRID_PX;
                                let sw = sec.width * GRID_PX;
                                let sh = sec.length * GRID_PX;
                                if world.x >= sx && world.x <= sx + sw
                                    && world.y >= sy && world.y <= sy + sh
                                {
                                    state.drag_target = DragTarget::Section(sec_room_id.clone(), sec_idx);
                                    state.drag_accum = egui::Vec2::ZERO;
                                    return;
                                }
                            }
                        }
                    }
                }
            }

            // Check rooms
            let gx = world_to_grid(world.x);
            let gy = world_to_grid(world.y);
            if let Some(layout) = &dungeon.layout {
                for rl in &layout.rooms {
                    // Floor filtering: skip rooms not on the current floor
                    if let Some(floor) = state.current_floor {
                        if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
                            if !room.floor.visible_on(floor) {
                                continue;
                            }
                        }
                    }

                    // Use world-coordinate hit test with a small margin for narrow rooms
                    let margin = GRID_PX * 0.4; // ~8px at 1x zoom
                    let room_x1 = rl.x as f32 * GRID_PX - margin;
                    let room_y1 = rl.y as f32 * GRID_PX - margin;
                    let room_x2 = (rl.x + rl.width as i32) as f32 * GRID_PX + margin;
                    let room_y2 = (rl.y + rl.height as i32) as f32 * GRID_PX + margin;
                    if world.x >= room_x1
                        && world.x <= room_x2
                        && world.y >= room_y1
                        && world.y <= room_y2
                    {
                        // If this is a cave room that's already selected, toggle the clicked cell
                        if state.selected_room.as_deref() == Some(&rl.room_id) {
                            if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
                                if room.shape == RoomShape::Cave {
                                    if let Some(cave) = &room.cave_data {
                                        if !cave.cells.is_empty() {
                                            let lx = (gx - rl.x) as usize;
                                            let ly = (gy - rl.y) as usize;
                                            let w = rl.width as usize;
                                            let idx = ly * w + lx;
                                            let new_val = !cave.cells.get(idx).copied().unwrap_or(false);
                                            if let Some(room) = dungeon.graph.room_by_id_mut(&rl.room_id) {
                                                if let Some(cave) = &mut room.cave_data {
                                                    if let Some(cell) = cave.cells.get_mut(idx) {
                                                        *cell = new_val;
                                                        cave.generation += 1;
                                                    }
                                                }
                                            }
                                            state.cave_contours_dirty = true;
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        state.selected_room = Some(rl.room_id.clone());
                        state.selected_corridor = None;
                        state.selected_waypoint = None;
                        state.drag_target = DragTarget::Room(rl.room_id.clone());
                        state.drag_accum = egui::Vec2::ZERO;
                        return;
                    }
                }

                // Check group body (after rooms, so rooms take priority)
                for (gi, group) in dungeon.graph.groups.iter().enumerate() {
                    if let Some((bx, by, bw, bh)) = group.spatial_bounds(layout) {
                        if gx >= bx && gx < bx + bw as i32
                            && gy >= by && gy < by + bh as i32
                        {
                            state.selected_group = Some(gi);
                            state.selected_room = None;
                            state.selected_corridor = None;
                            state.selected_waypoint = None;
                            state.drag_target = DragTarget::Group(gi);
                            state.drag_accum = egui::Vec2::ZERO;
                            return;
                        }
                    }
                }
            }
        }
    }

    // === DOUBLE-CLICK — insert waypoint on any corridor segment ===
    if response.double_clicked() {
        if let Some(pos) = response.hover_pos() {
            let world = transform.screen_to_world(pos);
            if let Some(layout) = &mut dungeon.layout {
                // Search all corridors for the best hit
                let mut best_hit: Option<(usize, usize, f32)> = None; // (corridor_idx, segment_idx, dist)
                for (ci, corridor) in layout.corridors.iter().enumerate() {
                    for (si, pair) in corridor.waypoints.windows(2).enumerate() {
                        let a = egui::pos2(grid_to_world(pair[0].x), grid_to_world(pair[0].y));
                        let b = egui::pos2(grid_to_world(pair[1].x), grid_to_world(pair[1].y));
                        let dist = point_to_segment_dist(world, a, b);
                        let threshold = corridor.width as f32 * GRID_PX / 2.0 + HANDLE_HIT_RADIUS / state.view.zoom;
                        if dist < threshold
                            && best_hit.is_none_or(|(_, _, bd)| dist < bd)
                        {
                            best_hit = Some((ci, si, dist));
                        }
                    }
                }
                if let Some((ci, si, _)) = best_hit {
                    let new_wp = GridPos {
                        x: world_to_grid(world.x),
                        y: world_to_grid(world.y),
                    };
                    layout.corridors[ci].waypoints.insert(si + 1, new_wp);
                    resolve_diagonal_segments(&mut layout.corridors[ci].waypoints);
                    layout.corridors[ci].pinned_waypoints =
                        layout.corridors[ci].waypoints.clone();
                    state.selected_corridor = Some(ci);
                    // Find the inserted waypoint (may have shifted due to diagonal resolution)
                    state.selected_waypoint = layout.corridors[ci].waypoints.iter()
                        .position(|wp| wp.x == new_wp.x && wp.y == new_wp.y)
                        .or(Some(si + 1));
                    state.selected_room = None;
                    state.selected_group = None;
                }
            }
        }
    }

    // === CLICK (no drag) — select corridors / waypoints ===
    if response.clicked() && !response.double_clicked() {
        if let Some(pos) = response.hover_pos() {
            let world = transform.screen_to_world(pos);

            // First: if corridor selected, check waypoint handle click
            if let Some(ci) = state.selected_corridor {
                if let Some(layout) = &dungeon.layout {
                    if ci < layout.corridors.len() {
                        let corridor = &layout.corridors[ci];
                        for (wi, wp) in corridor.waypoints.iter().enumerate() {
                            let wp_screen = transform.world_to_screen(
                                egui::pos2(grid_to_world(wp.x), grid_to_world(wp.y)),
                            );
                            if pos.distance(wp_screen) < HANDLE_HIT_RADIUS {
                                state.selected_waypoint = Some(wi);
                                return;
                            }
                        }
                    }
                }
            }

            // Check exit handles (when room is selected)
            if let Some(ref selected_room_id) = state.selected_room {
                if let Some(layout) = &dungeon.layout {
                    if hit_test_exit_handles(
                        pos, selected_room_id, layout, &dungeon.graph, &transform, state.view.zoom,
                    ).is_some() {
                        // Click on exit handle — keep room selected, don't change selection
                        return;
                    }
                }
            }

            // Check corridor segment hit
            if let Some(layout) = &dungeon.layout {
                let mut hit_corridor = None;
                for (ci, corridor) in layout.corridors.iter().enumerate() {
                    for pair in corridor.waypoints.windows(2) {
                        let a = egui::pos2(grid_to_world(pair[0].x), grid_to_world(pair[0].y));
                        let b = egui::pos2(grid_to_world(pair[1].x), grid_to_world(pair[1].y));
                        let dist = point_to_segment_dist(world, a, b);
                        let threshold = corridor.width as f32 * GRID_PX / 2.0 + HANDLE_HIT_RADIUS / state.view.zoom;
                        if dist < threshold {
                            hit_corridor = Some(ci);
                            break;
                        }
                    }
                    if hit_corridor.is_some() {
                        break;
                    }
                }

                if let Some(ci) = hit_corridor {
                    state.selected_corridor = Some(ci);
                    state.selected_waypoint = None;
                    state.selected_room = None;
                    state.selected_group = None;
                } else {
                    // Check room hit (with margin for narrow rooms)
                    let mut hit_room = false;
                    for rl in &layout.rooms {
                        let margin = GRID_PX * 0.4;
                        let room_x1 = rl.x as f32 * GRID_PX - margin;
                        let room_y1 = rl.y as f32 * GRID_PX - margin;
                        let room_x2 = (rl.x + rl.width as i32) as f32 * GRID_PX + margin;
                        let room_y2 = (rl.y + rl.height as i32) as f32 * GRID_PX + margin;
                        if world.x >= room_x1
                            && world.x <= room_x2
                            && world.y >= room_y1
                            && world.y <= room_y2
                        {
                            // Check if click is on an elevation section
                            let room_px_x = rl.x as f32 * GRID_PX;
                            let room_px_y = rl.y as f32 * GRID_PX;
                            let mut hit_section = None;
                            if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
                                for (si, sec) in room.sections.iter().enumerate() {
                                    let sx = room_px_x + sec.x * GRID_PX;
                                    let sy = room_px_y + sec.y * GRID_PX;
                                    let sw = sec.width * GRID_PX;
                                    let sh = sec.length * GRID_PX;
                                    if world.x >= sx && world.x <= sx + sw
                                        && world.y >= sy && world.y <= sy + sh
                                    {
                                        hit_section = Some((rl.room_id.clone(), si));
                                        break;
                                    }
                                }
                            }

                            state.selected_room = Some(rl.room_id.clone());
                            state.selected_corridor = None;
                            state.selected_waypoint = None;
                            state.selected_group = None;
                            state.selected_section = hit_section;
                            hit_room = true;
                            break;
                        }
                    }
                    if !hit_room {
                        // Check group hit
                        let gx = world_to_grid(world.x);
                        let gy = world_to_grid(world.y);
                        let mut hit_group = false;
                        for (gi, group) in dungeon.graph.groups.iter().enumerate() {
                            if group.max_width.is_none() && group.max_height.is_none() {
                                continue;
                            }
                            if let Some((bx, by, bw, bh)) = group.spatial_bounds(layout) {
                                if gx >= bx && gx < bx + bw as i32
                                    && gy >= by && gy < by + bh as i32
                                {
                                    state.selected_group = Some(gi);
                                    state.selected_corridor = None;
                                    state.selected_waypoint = None;
                                    state.selected_room = None;
                                    hit_group = true;
                                    break;
                                }
                            }
                        }
                        if !hit_group {
                            state.selected_room = None;
                            state.selected_corridor = None;
                            state.selected_waypoint = None;
                            state.selected_group = None;
                            state.selected_section = None;
                        }
                    }
                }
            }
        }
    }

    // === DELETE KEY — remove selected waypoint ===
    let canvas_id = response.id;
    let can_delete = response.has_focus()
        || (response.hovered() && !ui.ctx().memory(|m| {
            m.focused().is_some_and(|id| id != canvas_id)
        }));
    if can_delete {
        let delete_pressed = ui.input(|i| {
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
        });
        if delete_pressed {
            if let (Some(ci), Some(wi)) = (state.selected_corridor, state.selected_waypoint) {
                if let Some(layout) = &mut dungeon.layout {
                    if ci < layout.corridors.len() {
                        let len = layout.corridors[ci].waypoints.len();
                        // Don't delete if only 2 waypoints left (start + end)
                        // Don't delete start (0) or end (len-1)
                        if len > 2 && wi > 0 && wi < len - 1 {
                            layout.corridors[ci].waypoints.remove(wi);
                            layout.corridors[ci].pinned_waypoints =
                                layout.corridors[ci].waypoints.clone();
                            state.selected_waypoint = None;
                            layout.recheck_corridor_overlaps();
                        }
                    }
                }
            }
        }
    }


    // === DRAGGING ===
    if response.dragged_by(egui::PointerButton::Primary) {
        // Exit drag uses absolute cursor position — handle before grid-step accumulation
        if let DragTarget::Exit(ref conn_id, is_source) = state.drag_target {
            let conn_id = conn_id.clone();
            if let Some(ptr_pos) = response.interact_pointer_pos() {
                let world = transform.screen_to_world(ptr_pos);
                let room_id = dungeon.graph.connections.iter()
                    .find(|e| e.connection.id == conn_id)
                    .map(|e| if is_source { &e.source_room_id } else { &e.target_room_id })
                    .cloned();
                let cw = dungeon.graph.connections.iter()
                    .find(|e| e.connection.id == conn_id)
                    .map(|e| e.connection.corridor_width)
                    .unwrap_or(2);
                if let Some(room_id) = room_id {
                    if let Some(layout) = &dungeon.layout {
                        if let Some(room_rl) = layout.room_by_id(&room_id) {
                            let new_exit = snap_to_perimeter(world, room_rl, cw);
                            if let Some(edge) = dungeon.graph.connections.iter_mut()
                                .find(|e| e.connection.id == conn_id)
                            {
                                if is_source {
                                    edge.source_exit = Some(new_exit);
                                } else {
                                    edge.target_exit = Some(new_exit);
                                }
                            }
                        }
                    }
                }
            }
        }

        state.drag_accum += response.drag_delta() / state.view.zoom;

        let grid_steps_x = (state.drag_accum.x / GRID_PX).round() as i32;
        let grid_steps_y = (state.drag_accum.y / GRID_PX).round() as i32;

        if grid_steps_x != 0 || grid_steps_y != 0 {
            match &state.drag_target {
                DragTarget::Room(room_id) => {
                    let room_id = room_id.clone();

                    // Find which connections are attached to this room
                    let connected_ids: Vec<(String, bool, bool)> = dungeon.graph.connections
                        .iter()
                        .filter_map(|e| {
                            let is_src = e.source_room_id == room_id;
                            let is_tgt = e.target_room_id == room_id;
                            if is_src || is_tgt {
                                Some((e.connection.id.clone(), is_src, is_tgt))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if let Some(layout) = &mut dungeon.layout {
                        // Move the room
                        if let Some(rl) = layout.room_by_id_mut(&room_id) {
                            rl.x += grid_steps_x;
                            rl.y += grid_steps_y;
                        }

                        // Shift pinned waypoints for connected corridors
                        for corridor in &mut layout.corridors {
                            for (conn_id, is_src, is_tgt) in &connected_ids {
                                if corridor.connection_id != *conn_id {
                                    continue;
                                }
                                if !corridor.pinned_waypoints.is_empty() {
                                    // Shift the start waypoint if this room is the source
                                    if *is_src {
                                        corridor.pinned_waypoints.first_mut().unwrap().x += grid_steps_x;
                                        corridor.pinned_waypoints.first_mut().unwrap().y += grid_steps_y;
                                    }
                                    // Shift the end waypoint if this room is the target
                                    if *is_tgt {
                                        corridor.pinned_waypoints.last_mut().unwrap().x += grid_steps_x;
                                        corridor.pinned_waypoints.last_mut().unwrap().y += grid_steps_y;
                                    }
                                }
                            }
                        }
                    }

                    // Shift exit positions for connected edges
                    for (conn_id, is_src, is_tgt) in &connected_ids {
                        if let Some(edge) = dungeon.graph.connections.iter_mut()
                            .find(|e| e.connection.id == *conn_id)
                        {
                            if *is_src {
                                if let Some(ref mut exit) = edge.source_exit {
                                    exit.x += grid_steps_x as f32;
                                    exit.y += grid_steps_y as f32;
                                }
                            }
                            if *is_tgt {
                                if let Some(ref mut exit) = edge.target_exit {
                                    exit.x += grid_steps_x as f32;
                                    exit.y += grid_steps_y as f32;
                                }
                            }
                        }
                    }
                }
                DragTarget::Waypoint(ci, wi) => {
                    let ci = *ci;
                    let wi = *wi;
                    if let Some(layout) = &mut dungeon.layout {
                        let wps = &mut layout.corridors[ci].waypoints;
                        if wi < wps.len() {
                            // Check segment orientations BEFORE moving
                            let prev_horizontal = wi > 0 && wps[wi - 1].y == wps[wi].y;
                            let prev_vertical = wi > 0 && wps[wi - 1].x == wps[wi].x;
                            let next_horizontal = wi + 1 < wps.len() && wps[wi + 1].y == wps[wi].y;
                            let next_vertical = wi + 1 < wps.len() && wps[wi + 1].x == wps[wi].x;

                            // Remember the dragged point's identity

                            // Move the dragged waypoint
                            wps[wi].x += grid_steps_x;
                            wps[wi].y += grid_steps_y;

                            let dragged_pos_after = wps[wi];
                            let last = wps.len() - 1;

                            // Pull the previous neighbor along the shared axis,
                            // but never move the first endpoint (index 0)
                            if wi > 0 && wi - 1 != 0 {
                                if prev_horizontal {
                                    wps[wi - 1].y += grid_steps_y;
                                }
                                if prev_vertical {
                                    wps[wi - 1].x += grid_steps_x;
                                }
                            }

                            // Pull the next neighbor along the shared axis,
                            // but never move the last endpoint
                            if wi + 1 < wps.len() && wi + 1 != last {
                                if next_horizontal {
                                    wps[wi + 1].y += grid_steps_y;
                                }
                                if next_vertical {
                                    wps[wi + 1].x += grid_steps_x;
                                }
                            }

                            // Clean up stale auto-corners and resolve new diagonals
                            resolve_diagonal_segments_clean(wps);

                            // Update the drag target index to track the moved waypoint
                            if let Some(new_wi) = wps.iter().position(|wp| wp.x == dragged_pos_after.x && wp.y == dragged_pos_after.y) {
                                state.drag_target = DragTarget::Waypoint(ci, new_wi);
                            }
                        }
                    }
                }
                DragTarget::GroupCorner(gi, corner) => {
                    let gi = *gi;
                    let corner = *corner;
                    if gi < dungeon.graph.groups.len() {
                        if let Some(layout) = &dungeon.layout {
                            if let Some((gx, gy, gw, gh)) = dungeon.graph.groups[gi].spatial_bounds(layout) {
                                let group = &mut dungeon.graph.groups[gi];
                                match corner {
                                    0 => { // TL: move origin, shrink size
                                        let new_x = gx + grid_steps_x;
                                        let new_y = gy + grid_steps_y;
                                        let new_w = (gw as i32 - grid_steps_x).max(1) as u32;
                                        let new_h = (gh as i32 - grid_steps_y).max(1) as u32;
                                        group.spatial_x = Some(new_x);
                                        group.spatial_y = Some(new_y);
                                        group.max_width = Some(new_w);
                                        group.max_height = Some(new_h);
                                    }
                                    1 => { // TR: grow/shrink width
                                        let new_w = (gw as i32 + grid_steps_x).max(1) as u32;
                                        let new_h = (gh as i32 - grid_steps_y).max(1) as u32;
                                        let new_y = gy + grid_steps_y;
                                        group.spatial_y = Some(new_y);
                                        group.max_width = Some(new_w);
                                        group.max_height = Some(new_h);
                                    }
                                    2 => { // BL: grow/shrink height
                                        let new_x = gx + grid_steps_x;
                                        let new_w = (gw as i32 - grid_steps_x).max(1) as u32;
                                        let new_h = (gh as i32 + grid_steps_y).max(1) as u32;
                                        group.spatial_x = Some(new_x);
                                        group.max_width = Some(new_w);
                                        group.max_height = Some(new_h);
                                    }
                                    3 => { // BR: grow both
                                        let new_w = (gw as i32 + grid_steps_x).max(1) as u32;
                                        let new_h = (gh as i32 + grid_steps_y).max(1) as u32;
                                        group.max_width = Some(new_w);
                                        group.max_height = Some(new_h);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                DragTarget::Group(gi) => {
                    let gi = *gi;
                    if gi < dungeon.graph.groups.len() {
                        let group_room_ids = dungeon.graph.groups[gi].room_ids.clone();
                        let room_id_set: std::collections::HashSet<&String> = group_room_ids.iter().collect();

                        // Find connections internal to the group
                        let internal_conn_ids: Vec<String> = dungeon.graph.connections.iter()
                            .filter(|e| room_id_set.contains(&e.source_room_id) && room_id_set.contains(&e.target_room_id))
                            .map(|e| e.connection.id.clone())
                            .collect();

                        if let Some(layout) = &mut dungeon.layout {
                            // Move all rooms in the group
                            for rid in &group_room_ids {
                                if let Some(rl) = layout.room_by_id_mut(rid) {
                                    rl.x += grid_steps_x;
                                    rl.y += grid_steps_y;
                                }
                            }
                            // Move internal corridor waypoints
                            for corridor in &mut layout.corridors {
                                if internal_conn_ids.contains(&corridor.connection_id) {
                                    for wp in &mut corridor.waypoints {
                                        wp.x += grid_steps_x;
                                        wp.y += grid_steps_y;
                                    }
                                    for wp in &mut corridor.pinned_waypoints {
                                        wp.x += grid_steps_x;
                                        wp.y += grid_steps_y;
                                    }
                                }
                            }
                        }

                        // Shift exit positions for connections touching group rooms
                        for edge in &mut dungeon.graph.connections {
                            if room_id_set.contains(&edge.source_room_id) {
                                if let Some(ref mut exit) = edge.source_exit {
                                    exit.x += grid_steps_x as f32;
                                    exit.y += grid_steps_y as f32;
                                }
                            }
                            if room_id_set.contains(&edge.target_room_id) {
                                if let Some(ref mut exit) = edge.target_exit {
                                    exit.x += grid_steps_x as f32;
                                    exit.y += grid_steps_y as f32;
                                }
                            }
                        }
                    }
                }
                DragTarget::Section(room_id, sec_idx) => {
                    let room_id = room_id.clone();
                    let sec_idx = *sec_idx;
                    // Use layout dimensions (which reflect rotation) rather than model grid_size()
                    let layout_size = dungeon.layout.as_ref()
                        .and_then(|l| l.room_by_id(&room_id))
                        .map(|rl| (rl.width as f32, rl.height as f32));
                    if let Some(room) = dungeon.graph.room_by_id_mut(&room_id) {
                        if sec_idx < room.sections.len() {
                            let (rw, rh) = layout_size.unwrap_or_else(|| {
                                let (w, h) = room.grid_size();
                                (w as f32, h as f32)
                            });
                            let sec = &mut room.sections[sec_idx];
                            let max_x = (rw - sec.width).max(0.0);
                            let max_y = (rh - sec.length).max(0.0);
                            sec.x = (sec.x + grid_steps_x as f32).clamp(0.0, max_x);
                            sec.y = (sec.y + grid_steps_y as f32).clamp(0.0, max_y);
                        }
                    }
                }
                DragTarget::Exit(_, _) => {} // handled above, before grid-step check
                DragTarget::None => {}
            }
            state.drag_accum.x -= grid_steps_x as f32 * GRID_PX;
            state.drag_accum.y -= grid_steps_y as f32 * GRID_PX;
        }
    }

    // === DRAG STOP ===
    if response.drag_stopped_by(egui::PointerButton::Primary) {
        match &state.drag_target {
            DragTarget::Room(room_id) => {
                let room_id = room_id.clone();
                if let Some(layout) = &mut dungeon.layout {
                    let affected = std::collections::HashSet::from([room_id]);
                    layout.corridors =
                        crate::solver::corridor::route_corridors_for_rooms(
                            &dungeon.graph, layout, &affected,
                        );
                    layout.recheck_corridor_overlaps();
                }
            }
            DragTarget::Waypoint(ci, _) => {
                let ci = *ci;
                if let Some(layout) = &mut dungeon.layout {
                    if ci < layout.corridors.len() {
                        resolve_diagonal_segments(&mut layout.corridors[ci].waypoints);
                        layout.corridors[ci].pinned_waypoints =
                            layout.corridors[ci].waypoints.clone();
                    }
                    layout.recheck_corridor_overlaps();
                }
            }
            DragTarget::GroupCorner(_, _) => {
                // Group constraint changed — will trigger re-solve via hash check
            }
            DragTarget::Group(gi) => {
                let gi = *gi;
                // Re-route only corridors connected to rooms in the group
                if let Some(layout) = &mut dungeon.layout {
                    let affected: std::collections::HashSet<String> =
                        if gi < dungeon.graph.groups.len() {
                            dungeon.graph.groups[gi].room_ids.iter().cloned().collect()
                        } else {
                            std::collections::HashSet::new()
                        };
                    layout.corridors =
                        crate::solver::corridor::route_corridors_for_rooms(
                            &dungeon.graph, layout, &affected,
                        );
                    layout.recheck_corridor_overlaps();
                }
            }
            DragTarget::Section(_, _) => {} // position already updated during drag
            DragTarget::Exit(conn_id, _) => {
                // Re-route the corridor for this connection
                let conn_id = conn_id.clone();
                if let Some(edge) = dungeon.graph.connections.iter().find(|e| e.connection.id == conn_id) {
                    let affected = std::collections::HashSet::from([
                        edge.source_room_id.clone(),
                        edge.target_room_id.clone(),
                    ]);
                    if let Some(layout) = &mut dungeon.layout {
                        layout.corridors =
                            crate::solver::corridor::route_corridors_for_rooms(
                                &dungeon.graph, layout, &affected,
                            );
                        layout.recheck_corridor_overlaps();
                    }
                }
            }
            DragTarget::None => {}
        }
        state.drag_target = DragTarget::None;
    }
}

/// Draw an infinite grid based on the visible viewport.
fn draw_infinite_grid(painter: &egui::Painter, transform: &ViewTransform, canvas_rect: egui::Rect) {
    let light = egui::Color32::from_rgba_premultiplied(80, 80, 80, 40);
    let heavy = egui::Color32::from_rgba_premultiplied(100, 100, 100, 60);

    let top_left = transform.screen_to_world(canvas_rect.min);
    let bottom_right = transform.screen_to_world(canvas_rect.max);

    let min_gx = (top_left.x / GRID_PX).floor() as i32 - 1;
    let max_gx = (bottom_right.x / GRID_PX).ceil() as i32 + 1;
    let min_gy = (top_left.y / GRID_PX).floor() as i32 - 1;
    let max_gy = (bottom_right.y / GRID_PX).ceil() as i32 + 1;

    for x in min_gx..=max_gx {
        let color = if x % 5 == 0 { heavy } else { light };
        let from = transform.world_to_screen(egui::pos2(grid_to_world(x), grid_to_world(min_gy)));
        let to = transform.world_to_screen(egui::pos2(grid_to_world(x), grid_to_world(max_gy)));
        painter.line_segment([from, to], egui::Stroke::new(1.0, color));
    }
    for y in min_gy..=max_gy {
        let color = if y % 5 == 0 { heavy } else { light };
        let from = transform.world_to_screen(egui::pos2(grid_to_world(min_gx), grid_to_world(y)));
        let to = transform.world_to_screen(egui::pos2(grid_to_world(max_gx), grid_to_world(y)));
        painter.line_segment([from, to], egui::Stroke::new(1.0, color));
    }
}

/// Draw group constraint boxes on the spatial view with draggable corners.
fn draw_groups_spatial(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    state: &SpatialViewState,
) {
    for (gi, group) in graph.groups.iter().enumerate() {
        let Some((gx, gy, gw, gh)) = group.spatial_bounds(layout) else {
            continue;
        };

        // Only draw if group has constraints
        if group.max_width.is_none() && group.max_height.is_none() {
            continue;
        }

        let screen_min = transform.world_to_screen(egui::pos2(
            grid_to_world(gx),
            grid_to_world(gy),
        ));
        let screen_max = transform.world_to_screen(egui::pos2(
            grid_to_world(gx + gw as i32),
            grid_to_world(gy + gh as i32),
        ));
        let rect = egui::Rect::from_min_max(screen_min, screen_max);

        let c = group.color;
        let is_selected = state.selected_group == Some(gi);
        let fill = egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3] / 2);
        let border_color = if is_selected {
            COLOR_SELECTION
        } else {
            egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], 150)
        };

        // Dashed border
        let stroke = egui::Stroke::new(1.5, border_color);
        draw_dashed_line(painter, egui::pos2(rect.min.x, rect.min.y), egui::pos2(rect.max.x, rect.min.y), stroke, 6.0, 3.0);
        draw_dashed_line(painter, egui::pos2(rect.max.x, rect.min.y), egui::pos2(rect.max.x, rect.max.y), stroke, 6.0, 3.0);
        draw_dashed_line(painter, egui::pos2(rect.max.x, rect.max.y), egui::pos2(rect.min.x, rect.max.y), stroke, 6.0, 3.0);
        draw_dashed_line(painter, egui::pos2(rect.min.x, rect.max.y), egui::pos2(rect.min.x, rect.min.y), stroke, 6.0, 3.0);

        // Fill
        painter.rect_filled(rect, 0.0, fill);

        // Label
        if !group.label.is_empty() {
            painter.text(
                egui::pos2(screen_min.x + 3.0, screen_min.y - 12.0),
                egui::Align2::LEFT_BOTTOM,
                &group.label,
                egui::FontId::monospace(10.0 * transform.zoom),
                border_color,
            );
        }

        // Dimension label
        painter.text(
            egui::pos2(rect.center().x, screen_max.y + 10.0),
            egui::Align2::CENTER_TOP,
            format!("{}x{}", gw, gh),
            egui::FontId::monospace(9.0 * transform.zoom),
            border_color,
        );

        // Corner handles
        let corners = [
            egui::pos2(rect.min.x, rect.min.y),
            egui::pos2(rect.max.x, rect.min.y),
            egui::pos2(rect.min.x, rect.max.y),
            egui::pos2(rect.max.x, rect.max.y),
        ];
        let hr = HANDLE_RADIUS * state.view.zoom;
        for (ci, &corner) in corners.iter().enumerate() {
            let is_dragging = matches!(state.drag_target, DragTarget::GroupCorner(g, c) if g == gi && c == ci as u8);
            let color = if is_dragging {
                egui::Color32::from_rgb(255, 220, 80)
            } else {
                border_color
            };
            painter.rect_filled(
                egui::Rect::from_center_size(corner, egui::vec2(hr * 2.0, hr * 2.0)),
                2.0,
                color,
            );
        }
    }
}

fn draw_bounds(painter: &egui::Painter, transform: &ViewTransform, layout: &SpatialLayout) {
    let color = egui::Color32::from_rgb(200, 160, 60);

    for b in &layout.bounds {
        let min = transform.world_to_screen(egui::pos2(grid_to_world(b.x), grid_to_world(b.y)));
        let max = transform.world_to_screen(egui::pos2(
            grid_to_world(b.x + b.width as i32),
            grid_to_world(b.y + b.height as i32),
        ));

        let stroke = egui::Stroke::new(2.0, color);
        let dash = 8.0;
        let gap = 4.0;

        draw_dashed_line(painter, egui::pos2(min.x, min.y), egui::pos2(max.x, min.y), stroke, dash, gap);
        draw_dashed_line(painter, egui::pos2(max.x, min.y), egui::pos2(max.x, max.y), stroke, dash, gap);
        draw_dashed_line(painter, egui::pos2(max.x, max.y), egui::pos2(min.x, max.y), stroke, dash, gap);
        draw_dashed_line(painter, egui::pos2(min.x, max.y), egui::pos2(min.x, min.y), stroke, dash, gap);

        if !b.label.is_empty() {
            painter.text(
                egui::pos2(min.x + 4.0, min.y - 12.0),
                egui::Align2::LEFT_BOTTOM,
                &b.label,
                egui::FontId::monospace(11.0 * transform.zoom),
                color,
            );
        }
    }
}

fn draw_corridors(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    state: &SpatialViewState,
) {
    for (ci, corridor) in layout.corridors.iter().enumerate() {
        // Floor filtering: dim corridors to lower floors, hide higher
        let dim = if let Some(floor) = state.current_floor {
            if let Some(edge) = graph.connections.iter().find(|e| e.connection.id == corridor.connection_id) {
                let src_visible = graph.room_by_id(&edge.source_room_id)
                    .is_some_and(|r| r.floor.visible_on(floor));
                let tgt_visible = graph.room_by_id(&edge.target_room_id)
                    .is_some_and(|r| r.floor.visible_on(floor));
                if src_visible || tgt_visible {
                    1.0
                } else {
                    // Both rooms not on current floor — check if lower
                    let src_lower = graph.room_by_id(&edge.source_room_id)
                        .is_some_and(|r| is_lower_floor(r, floor));
                    let tgt_lower = graph.room_by_id(&edge.target_room_id)
                        .is_some_and(|r| is_lower_floor(r, floor));
                    if src_lower || tgt_lower {
                        LOWER_FLOOR_DIM
                    } else {
                        continue; // higher floor - hide
                    }
                }
            } else {
                1.0
            }
        } else {
            1.0
        };
        let is_selected = state.selected_corridor == Some(ci);
        let mut color = if corridor.invalid {
            egui::Color32::from_rgb(220, 50, 50)
        } else if is_selected {
            egui::Color32::from_rgb(130, 200, 255)
        } else {
            egui::Color32::from_rgb(180, 180, 180)
        };
        if dim < 1.0 {
            color = dim_color(color, dim);
        }

        let w = corridor.width as i32;
        let half = w / 2; // integer: same offset used by solver

        // Draw each segment as a filled rectangle on the grid.
        // Waypoints are center coords. The block covers cells
        // from (center - half) to (center - half + w).
        for pair in corridor.waypoints.windows(2) {
            let x1 = pair[0].x;
            let y1 = pair[0].y;
            let x2 = pair[1].x;
            let y2 = pair[1].y;

            let min_x = x1.min(x2) - half;
            let min_y = y1.min(y2) - half;
            let max_x = x1.max(x2) - half + w;
            let max_y = y1.max(y2) - half + w;

            let screen_min = transform.world_to_screen(egui::pos2(
                grid_to_world(min_x),
                grid_to_world(min_y),
            ));
            let screen_max = transform.world_to_screen(egui::pos2(
                grid_to_world(max_x),
                grid_to_world(max_y),
            ));

            let rect = egui::Rect::from_min_max(screen_min, screen_max);
            painter.rect_filled(rect, 0.0, color);
        }
    }
}

/// Draw draggable handles at each waypoint of the selected corridor.
fn draw_waypoint_handles(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    state: &SpatialViewState,
) {
    let Some(ci) = state.selected_corridor else {
        return;
    };
    let Some(corridor) = layout.corridors.get(ci) else {
        return;
    };

    let handle_r = HANDLE_RADIUS * state.view.zoom;

    for (wi, wp) in corridor.waypoints.iter().enumerate() {
        let screen = transform.world_to_screen(
            egui::pos2(grid_to_world(wp.x), grid_to_world(wp.y)),
        );

        let is_endpoint = wi == 0 || wi == corridor.waypoints.len() - 1;
        let is_dragging = matches!(state.drag_target, DragTarget::Waypoint(c, w) if c == ci && w == wi);
        let is_selected = state.selected_waypoint == Some(wi);

        let fill = if is_dragging {
            egui::Color32::from_rgb(255, 220, 80)
        } else if is_selected {
            egui::Color32::from_rgb(255, 180, 50)
        } else if is_endpoint {
            egui::Color32::from_rgb(80, 220, 120)
        } else {
            egui::Color32::from_rgb(100, 180, 255)
        };

        let stroke_color = egui::Color32::WHITE;

        if is_endpoint {
            // Diamond shape for endpoints
            let s = handle_r * 1.2;
            let points = vec![
                egui::pos2(screen.x, screen.y - s),
                egui::pos2(screen.x + s, screen.y),
                egui::pos2(screen.x, screen.y + s),
                egui::pos2(screen.x - s, screen.y),
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                fill,
                egui::Stroke::new(1.5, stroke_color),
            ));
        } else {
            // Circle for mid-waypoints
            painter.circle(screen, handle_r, fill, egui::Stroke::new(1.5, stroke_color));
        }
    }
}

/// Compute the default exit position for a room/connection when no exit is stored.
/// Returns the corridor center-line position on the room wall.
fn default_exit_pos(room_rl: &RoomLayout, other_rl: &RoomLayout, _corridor_width: u32) -> ExitPos {
    let rcx = room_rl.x as f32 + room_rl.width as f32 / 2.0;
    let rcy = room_rl.y as f32 + room_rl.height as f32 / 2.0;
    let ocx = other_rl.x as f32 + other_rl.width as f32 / 2.0;
    let ocy = other_rl.y as f32 + other_rl.height as f32 / 2.0;
    let dx = ocx - rcx;
    let dy = ocy - rcy;

    if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            ExitPos { x: room_rl.x as f32 + room_rl.width as f32, y: rcy }
        } else {
            ExitPos { x: room_rl.x as f32, y: rcy }
        }
    } else {
        if dy >= 0.0 {
            ExitPos { x: rcx, y: room_rl.y as f32 + room_rl.height as f32 }
        } else {
            ExitPos { x: rcx, y: room_rl.y as f32 }
        }
    }
}

/// Project a world-space point onto the room perimeter and snap to integer grid coords.
/// Returns the snapped exit position on the room wall.
/// Round to nearest half-grid unit (0.0, 0.5, 1.0, 1.5, ...).
fn snap_half_grid(v: f32) -> f32 {
    (v * 2.0).round() / 2.0
}

fn snap_to_perimeter(world: egui::Pos2, room_rl: &RoomLayout, corridor_width: u32) -> ExitPos {
    let half = corridor_width as f32 / 2.0;
    let rw = room_rl.width as f32;
    let rh = room_rl.height as f32;
    let rx = room_rl.x as f32;
    let ry = room_rl.y as f32;
    // Room edges in world pixels
    let rx_px = rx * GRID_PX;
    let ry_px = ry * GRID_PX;
    let rx2_px = (rx + rw) * GRID_PX;
    let ry2_px = (ry + rh) * GRID_PX;

    // Convert cursor to grid coords, snap to half-grid
    let grid_y = snap_half_grid(world.y / GRID_PX);
    let grid_x = snap_half_grid(world.x / GRID_PX);

    // Clamp so corridor fits within the wall (need half corridor width margin)
    let y_min = ry + half;
    let y_max = ry + rh - half;
    let x_min = rx + half;
    let x_max = rx + rw - half;
    let y_clamped = grid_y.clamp(y_min, y_max);
    let x_clamped = grid_x.clamp(x_min, x_max);

    let faces: [(f32, ExitPos); 4] = [
        // Right face
        (world.distance(egui::pos2(rx2_px, y_clamped * GRID_PX)),
         ExitPos { x: rx + rw, y: y_clamped }),
        // Left face
        (world.distance(egui::pos2(rx_px, y_clamped * GRID_PX)),
         ExitPos { x: rx, y: y_clamped }),
        // Bottom face
        (world.distance(egui::pos2(x_clamped * GRID_PX, ry2_px)),
         ExitPos { x: x_clamped, y: ry + rh }),
        // Top face
        (world.distance(egui::pos2(x_clamped * GRID_PX, ry_px)),
         ExitPos { x: x_clamped, y: ry }),
    ];

    faces.iter()
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .unwrap()
        .1
}

/// Draw exit handles for each connection touching the selected room.
fn draw_exit_handles(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    state: &SpatialViewState,
) {
    let Some(ref selected_room_id) = state.selected_room else { return };
    let Some(room_rl) = layout.room_by_id(selected_room_id) else { return };

    let handle_size = EXIT_HANDLE_SIZE * state.view.zoom;

    for edge in &graph.connections {
        let (is_source, other_room_id) = if edge.source_room_id == *selected_room_id {
            (true, &edge.target_room_id)
        } else if edge.target_room_id == *selected_room_id {
            (false, &edge.source_room_id)
        } else {
            continue;
        };

        let exit_opt = if is_source { &edge.source_exit } else { &edge.target_exit };
        let Some(other_rl) = layout.room_by_id(other_room_id) else { continue };

        let (exit_pos, is_set) = match exit_opt {
            Some(pos) => (*pos, true),
            None => (default_exit_pos(room_rl, other_rl, edge.connection.corridor_width), false),
        };

        let screen = transform.world_to_screen(
            egui::pos2(exit_pos.x * GRID_PX, exit_pos.y * GRID_PX),
        );

        let is_dragging = matches!(&state.drag_target, DragTarget::Exit(cid, src) if *cid == edge.connection.id && *src == is_source);

        let fill = if is_dragging {
            egui::Color32::from_rgb(255, 200, 50)
        } else if is_set {
            egui::Color32::from_rgb(240, 160, 40)
        } else {
            egui::Color32::from_rgba_unmultiplied(180, 140, 80, 120)
        };

        let stroke_color = if is_set {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 100)
        };

        // Draw as a small square rotated 45° (diamond)
        let s = handle_size;
        let points = vec![
            egui::pos2(screen.x, screen.y - s),
            egui::pos2(screen.x + s, screen.y),
            egui::pos2(screen.x, screen.y + s),
            egui::pos2(screen.x - s, screen.y),
        ];
        painter.add(egui::Shape::convex_polygon(
            points,
            fill,
            egui::Stroke::new(1.5, stroke_color),
        ));
    }
}

fn draw_rooms(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    state: &SpatialViewState,
) {
    for rl in &layout.rooms {
        // Floor filtering: dim lower floors, hide higher floors
        let dim = if let Some(floor) = state.current_floor {
            if let Some(room) = graph.room_by_id(&rl.room_id) {
                if room.floor.visible_on(floor) {
                    1.0
                } else if is_lower_floor(room, floor) {
                    LOWER_FLOOR_DIM
                } else {
                    continue; // higher floor - hide
                }
            } else {
                1.0
            }
        } else {
            1.0
        };

        let min = transform.world_to_screen(egui::pos2(grid_to_world(rl.x), grid_to_world(rl.y)));
        let max = transform.world_to_screen(egui::pos2(
            grid_to_world(rl.x + rl.width as i32),
            grid_to_world(rl.y + rl.height as i32),
        ));
        let rect = egui::Rect::from_min_max(min, max);

        let is_selected = state.selected_room.as_deref() == Some(&rl.room_id);
        let room = graph.room_by_id(&rl.room_id);
        let shape = room.map(|r| r.shape).unwrap_or_default();
        let has_violations = !rl.violations.is_empty();

        let mut fill = if has_violations {
            egui::Color32::from_rgb(240, 200, 200)
        } else {
            egui::Color32::from_rgb(220, 220, 220)
        };
        let mut wall_fill = egui::Color32::from_rgb(140, 130, 120);
        let border_color = if is_selected {
            COLOR_SELECTION
        } else if has_violations {
            egui::Color32::from_rgb(220, 60, 60)
        } else {
            egui::Color32::from_rgb(60, 60, 60)
        };
        let mut stroke_color = border_color;
        if dim < 1.0 {
            fill = dim_color(fill, dim);
            wall_fill = dim_color(wall_fill, dim);
            stroke_color = dim_color(border_color, dim);
        }
        let stroke = egui::Stroke::new(2.0, stroke_color);

        match shape {
            RoomShape::Circle => {
                let center = rect.center();
                let radius = rect.width().min(rect.height()) / 2.0;
                painter.circle_filled(center, radius, fill);
                painter.circle_stroke(center, radius, stroke);
            }
            RoomShape::Cave => {
                // Draw cave cells individually
                if let Some(cave) = room.and_then(|r| r.cave_data.as_ref()) {
                    if !cave.cells.is_empty() {
                        let w = rl.width as usize;
                        for ly in 0..rl.height as usize {
                            for lx in 0..w {
                                let gx = rl.x + lx as i32;
                                let gy = rl.y + ly as i32;
                                let cell_min = transform.world_to_screen(
                                    egui::pos2(grid_to_world(gx), grid_to_world(gy)),
                                );
                                let cell_max = transform.world_to_screen(
                                    egui::pos2(grid_to_world(gx + 1), grid_to_world(gy + 1)),
                                );
                                let cell_rect = egui::Rect::from_min_max(cell_min, cell_max);
                                let is_floor = cave.cells.get(ly * w + lx).copied().unwrap_or(false);
                                let c = if is_floor { fill } else { wall_fill };
                                painter.rect_filled(cell_rect, 0.0, c);
                            }
                        }
                        // Draw baked marching squares contour
                        let contour_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(40, 40, 40));
                        for &(x1, y1, x2, y2) in &cave.contour_segments {
                            let s1 = transform.world_to_screen(egui::pos2(x1, y1));
                            let s2 = transform.world_to_screen(egui::pos2(x2, y2));
                            painter.line_segment([s1, s2], contour_stroke);
                        }
                    } else {
                        // No cells yet — draw as rectangle
                        painter.rect_filled(rect, 0.0, fill);
                    }
                } else {
                    painter.rect_filled(rect, 0.0, fill);
                }
                // Always draw AABB border (dashed for caves)
                let aabb_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(60, 60, 60, 80));
                painter.rect_stroke(rect, 0.0, aabb_stroke, egui::StrokeKind::Middle);
            }
            RoomShape::Rectangle => {
                painter.rect_filled(rect, 0.0, fill);
                painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Middle);
            }
        }

        // Grid lines on all rooms
        {
            let grid_stroke = egui::Stroke::new(0.5, egui::Color32::from_rgba_unmultiplied(80, 80, 80, 60));
            for ly in 1..rl.height as i32 {
                let y1 = transform.world_to_screen(egui::pos2(grid_to_world(rl.x), grid_to_world(rl.y + ly)));
                let y2 = transform.world_to_screen(egui::pos2(grid_to_world(rl.x + rl.width as i32), grid_to_world(rl.y + ly)));
                painter.line_segment([y1, y2], grid_stroke);
            }
            for lx in 1..rl.width as i32 {
                let x1 = transform.world_to_screen(egui::pos2(grid_to_world(rl.x + lx), grid_to_world(rl.y)));
                let x2 = transform.world_to_screen(egui::pos2(grid_to_world(rl.x + lx), grid_to_world(rl.y + rl.height as i32)));
                painter.line_segment([x1, x2], grid_stroke);
            }
        }

        if let Some(room) = room {
            let label_color = if dim < 1.0 {
                dim_color(egui::Color32::from_rgb(30, 30, 30), dim)
            } else {
                egui::Color32::from_rgb(30, 30, 30)
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &room.label,
                egui::FontId::monospace(11.0 * transform.zoom),
                label_color,
            );

            // Draw elevation sections
            let room_px_x = rl.x as f32 * GRID_PX;
            let room_px_y = rl.y as f32 * GRID_PX;
            for (si, section) in room.sections.iter().enumerate() {
                let sec_min = transform.world_to_screen(egui::pos2(
                    room_px_x + section.x * GRID_PX,
                    room_px_y + section.y * GRID_PX,
                ));
                let sec_max = transform.world_to_screen(egui::pos2(
                    room_px_x + (section.x + section.width) * GRID_PX,
                    room_px_y + (section.y + section.length) * GRID_PX,
                ));
                let sec_rect = egui::Rect::from_min_max(sec_min, sec_max);

                // Fill based on elevation type
                let is_water = section.elevation == ElevationType::Water;
                let (fill_alpha, tick_dir) = match section.elevation {
                    ElevationType::Raised => (30u8, 1.0f32),  // ticks outward
                    ElevationType::Lowered => (50, -1.0),       // ticks inward
                    ElevationType::Steps | ElevationType::Slope => (20, 0.0),
                    ElevationType::BottomlessPit => (160, 0.0),
                    ElevationType::Hole => (90, 0.0),
                    ElevationType::Water => (0, 0.0), // handled separately
                };
                let section_fill = if is_water {
                    egui::Color32::from_rgba_unmultiplied(80, 130, 200, 60)
                } else {
                    egui::Color32::from_rgba_unmultiplied(60, 60, 60, fill_alpha)
                };
                painter.rect_filled(sec_rect, 0.0, section_fill);

                // Border
                let is_sel_section = state.selected_section.as_ref()
                    .is_some_and(|(rid, idx)| rid == &rl.room_id && *idx == si);
                if is_sel_section {
                    painter.rect_stroke(sec_rect, 0.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 200, 50)),
                        egui::StrokeKind::Middle);
                } else if !is_water {
                    painter.rect_stroke(sec_rect, 0.0,
                        egui::Stroke::new(1.5, egui::Color32::from_rgb(80, 80, 80)),
                        egui::StrokeKind::Middle);
                }

                // Tick marks for raised/lowered
                if tick_dir != 0.0 {
                    let tick_len = 3.0 * transform.zoom;
                    let spacing = 8.0 * transform.zoom;
                    let tick_color = egui::Color32::from_rgb(80, 80, 80);
                    let tick_stroke = egui::Stroke::new(1.0, tick_color);

                    // Top/bottom ticks
                    let mut tx = sec_rect.min.x + spacing;
                    while tx < sec_rect.max.x - spacing * 0.5 {
                        // Top edge
                        painter.line_segment(
                            [egui::pos2(tx, sec_rect.min.y), egui::pos2(tx, sec_rect.min.y - tick_len * tick_dir)],
                            tick_stroke,
                        );
                        // Bottom edge
                        painter.line_segment(
                            [egui::pos2(tx, sec_rect.max.y), egui::pos2(tx, sec_rect.max.y + tick_len * tick_dir)],
                            tick_stroke,
                        );
                        tx += spacing;
                    }
                    // Left/right ticks
                    let mut ty = sec_rect.min.y + spacing;
                    while ty < sec_rect.max.y - spacing * 0.5 {
                        painter.line_segment(
                            [egui::pos2(sec_rect.min.x, ty), egui::pos2(sec_rect.min.x - tick_len * tick_dir, ty)],
                            tick_stroke,
                        );
                        painter.line_segment(
                            [egui::pos2(sec_rect.max.x, ty), egui::pos2(sec_rect.max.x + tick_len * tick_dir, ty)],
                            tick_stroke,
                        );
                        ty += spacing;
                    }
                }

                // Steps: draw parallel lines
                if section.elevation == ElevationType::Steps {
                    let step_count = 4;
                    let step_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 80));
                    if sec_rect.width() >= sec_rect.height() {
                        for i in 1..step_count {
                            let lx = sec_rect.min.x + (i as f32 / step_count as f32) * sec_rect.width();
                            painter.line_segment(
                                [egui::pos2(lx, sec_rect.min.y), egui::pos2(lx, sec_rect.max.y)],
                                step_stroke,
                            );
                        }
                    } else {
                        for i in 1..step_count {
                            let ly = sec_rect.min.y + (i as f32 / step_count as f32) * sec_rect.height();
                            painter.line_segment(
                                [egui::pos2(sec_rect.min.x, ly), egui::pos2(sec_rect.max.x, ly)],
                                step_stroke,
                            );
                        }
                    }
                }

                // Bottomless Pit: inset border for depth
                if section.elevation == ElevationType::BottomlessPit {
                    let inset = 3.0 * transform.zoom;
                    let inset_rect = sec_rect.shrink(inset);
                    painter.rect_stroke(inset_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 80)), egui::StrokeKind::Middle);
                }

                // Hole: diagonal cross
                if section.elevation == ElevationType::Hole {
                    let cross_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 80));
                    painter.line_segment([sec_rect.left_top(), sec_rect.right_bottom()], cross_stroke);
                    painter.line_segment([sec_rect.right_top(), sec_rect.left_bottom()], cross_stroke);
                }

                // Slope: gradient strips along the longer axis
                if section.elevation == ElevationType::Slope {
                    let strips = 8;
                    if sec_rect.width() >= sec_rect.height() {
                        let strip_w = sec_rect.width() / strips as f32;
                        for i in 0..strips {
                            let alpha = ((i as f32 + 1.0) / strips as f32 * 60.0) as u8;
                            let strip_fill = egui::Color32::from_rgba_unmultiplied(60, 60, 60, alpha);
                            let strip_rect = egui::Rect::from_min_size(
                                egui::pos2(sec_rect.min.x + i as f32 * strip_w, sec_rect.min.y),
                                egui::vec2(strip_w, sec_rect.height()),
                            );
                            painter.rect_filled(strip_rect, 0.0, strip_fill);
                        }
                    } else {
                        let strip_h = sec_rect.height() / strips as f32;
                        for i in 0..strips {
                            let alpha = ((i as f32 + 1.0) / strips as f32 * 60.0) as u8;
                            let strip_fill = egui::Color32::from_rgba_unmultiplied(60, 60, 60, alpha);
                            let strip_rect = egui::Rect::from_min_size(
                                egui::pos2(sec_rect.min.x, sec_rect.min.y + i as f32 * strip_h),
                                egui::vec2(sec_rect.width(), strip_h),
                            );
                            painter.rect_filled(strip_rect, 0.0, strip_fill);
                        }
                    }
                }

                // Water: wavy lines
                if is_water {
                    let wave_color = egui::Color32::from_rgba_unmultiplied(60, 100, 170, 140);
                    let wave_stroke = egui::Stroke::new(0.8, wave_color);
                    let wave_count = ((sec_rect.height() / (20.0 * transform.zoom)).max(2.0)) as i32;
                    for i in 1..wave_count {
                        let base_y = sec_rect.min.y + (i as f32 / wave_count as f32) * sec_rect.height();
                        let segments = ((sec_rect.width() / (5.0 * transform.zoom)).max(8.0)) as i32;
                        let amp = 2.0 * transform.zoom;
                        let mut points = Vec::with_capacity(segments as usize + 1);
                        for j in 0..=segments {
                            let t = j as f32 / segments as f32;
                            let x = sec_rect.min.x + t * sec_rect.width();
                            let y = base_y + amp * (t * std::f32::consts::TAU * 2.0).sin();
                            points.push(egui::pos2(x, y));
                        }
                        for pair in points.windows(2) {
                            painter.line_segment([pair[0], pair[1]], wave_stroke);
                        }
                    }
                    // Blue-tinted border
                    painter.rect_stroke(sec_rect, 0.0,
                        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(60, 100, 170, 200)),
                        egui::StrokeKind::Middle);
                }

            }
        }
    }
}

/// Draw door symbols at the room wall where corridors connect.
/// The door is a white rectangle with black border, placed ON the room wall.
fn draw_doors(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    state: &SpatialViewState,
) {
    for edge in &graph.connections {
        // Floor filtering: dim doors to lower floors, hide higher
        let dim = if let Some(floor) = state.current_floor {
            let src_visible = graph.room_by_id(&edge.source_room_id)
                .is_some_and(|r| r.floor.visible_on(floor));
            let tgt_visible = graph.room_by_id(&edge.target_room_id)
                .is_some_and(|r| r.floor.visible_on(floor));
            if src_visible || tgt_visible {
                1.0
            } else {
                let src_lower = graph.room_by_id(&edge.source_room_id)
                    .is_some_and(|r| is_lower_floor(r, floor));
                let tgt_lower = graph.room_by_id(&edge.target_room_id)
                    .is_some_and(|r| is_lower_floor(r, floor));
                if src_lower || tgt_lower {
                    LOWER_FLOOR_DIM
                } else {
                    continue;
                }
            }
        } else {
            1.0
        };
        let white = if dim < 1.0 { dim_color(egui::Color32::WHITE, dim) } else { egui::Color32::WHITE };
        let dark = if dim < 1.0 { dim_color(egui::Color32::from_rgb(30, 30, 30), dim) } else { egui::Color32::from_rgb(30, 30, 30) };
        if edge.connection.connection_type == ConnectionType::Open {
            continue;
        }

        let corridor = layout.corridors.iter().find(|c| c.connection_id == edge.connection.id);
        let Some(corridor) = corridor else { continue };
        if corridor.waypoints.len() < 2 {
            continue;
        }

        // Door width: 1 square for single, 2 for double
        let dw = edge.connection.door_width() as i32;
        let dw_half = dw as f32 / 2.0;

        // For each end of the corridor, find the room it connects to
        // and place the door on that room's wall.
        let room_ids = [&edge.source_room_id, &edge.target_room_id];
        let wp_ends = [
            &corridor.waypoints[0],
            corridor.waypoints.last().unwrap(),
        ];

        let exits = [edge.source_exit.as_ref(), edge.target_exit.as_ref()];

        for ((room_id, wp), exit) in room_ids.iter().zip(wp_ends.iter()).zip(exits.iter()) {
            let Some(rl) = layout.room_by_id(room_id) else { continue };

            let door_depth = 0.3_f32;
            let (door_x1, door_y1, door_x2, door_y2) =
                crate::render::themed::door_rect(rl, wp, *exit, dw_half * 2.0, door_depth);

            let screen_min = transform.world_to_screen(egui::pos2(
                door_x1 * GRID_PX,
                door_y1 * GRID_PX,
            ));
            let screen_max = transform.world_to_screen(egui::pos2(
                door_x2 * GRID_PX,
                door_y2 * GRID_PX,
            ));
            let door_rect = egui::Rect::from_min_max(screen_min, screen_max);

            match edge.connection.connection_type {
                ConnectionType::Open => {} // already skipped above
                ConnectionType::Door => {
                    painter.rect_filled(door_rect, 0.0, white);
                    painter.rect_stroke(door_rect, 0.0, egui::Stroke::new(1.5, dark), egui::StrokeKind::Middle);
                }
                ConnectionType::Locked => {
                    painter.rect_filled(door_rect, 0.0, white);
                    painter.rect_stroke(door_rect, 0.0, egui::Stroke::new(1.5, dark), egui::StrokeKind::Middle);
                    // Small filled circle in center (lock indicator)
                    let dot_r = door_rect.width().min(door_rect.height()) * 0.2;
                    painter.circle_filled(door_rect.center(), dot_r, dark);
                }
                ConnectionType::Secret => {
                    // No visible door — just an "S" near the wall
                    painter.text(
                        door_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "S",
                        egui::FontId::monospace((8.0 * transform.zoom).max(6.0)),
                        dark,
                    );
                }
                ConnectionType::OneWay => {
                    painter.rect_filled(door_rect, 0.0, white);
                    painter.rect_stroke(door_rect, 0.0, egui::Stroke::new(1.5, dark), egui::StrokeKind::Middle);
                    // Small arrow in the center
                    let horizontal = door_rect.width() < door_rect.height();
                    let arrow_sz = door_rect.width().min(door_rect.height()) * 0.3;
                    let dir = if horizontal {
                        let toward_room = if (wp.x as f32) > (rl.x + rl.width as i32 / 2) as f32 { -1.0 } else { 1.0 };
                        egui::vec2(toward_room, 0.0)
                    } else {
                        let toward_room = if (wp.y as f32) > (rl.y + rl.height as i32 / 2) as f32 { -1.0 } else { 1.0 };
                        egui::vec2(0.0, toward_room)
                    };
                    let c = door_rect.center();
                    let tip = c + dir * arrow_sz;
                    let perp = egui::vec2(-dir.y, dir.x);
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            tip,
                            c - dir * arrow_sz * 0.5 + perp * arrow_sz * 0.5,
                            c - dir * arrow_sz * 0.5 - perp * arrow_sz * 0.5,
                        ],
                        dark,
                        egui::Stroke::NONE,
                    ));
                }
            }
        }
    }
}

pub fn spatial_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut SpatialViewState) {
    let has_selection = state.selected_room.is_some()
        || state.selected_corridor.is_some()
        || state.selected_group.is_some();

    if !has_selection {
        ui.heading("Spatial Layout");
        ui.separator();
    }

    // Layout controls (collapsible when something is selected)
    let mut show_controls = |ui: &mut egui::Ui| {
        ui.label("Density gap:");
        ui.add(egui::Slider::new(&mut state.density_gap, 0..=6));

        ui.add_space(8.0);
        if ui.button("Recompute All").on_hover_text("Re-solve all room positions and corridors from scratch").clicked() {
            state.recompute_requested = true;
        }

        // Floor selector
        ui.add_space(16.0);
        ui.label("Floor:");
        {
            let floors = collect_floors(&dungeon.graph);
            let label = match state.current_floor {
                None => "All Floors".to_string(),
                Some(f) => format!("Floor {}", f),
            };
            egui::ComboBox::from_id_salt("spatial_floor_select")
                .selected_text(&label)
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut state.current_floor, None, "All Floors").changed() {}
                    for f in &floors {
                        let mut val = Some(*f);
                        if ui.selectable_value(&mut val, Some(*f), format!("Floor {}", f)).clicked() {
                            state.current_floor = Some(*f);
                        }
                    }
                });
        }

        // Bounds management
        ui.add_space(16.0);
        ui.label("Bounds:");

        if ui.button("Add Bounds Rectangle").clicked() {
            if let Some(layout) = &mut dungeon.layout {
                let (min_x, min_y, max_x, max_y) = layout.extents();
                let margin = 2;
                layout.bounds.push(BoundsRect {
                    label: format!("Bounds {}", layout.bounds.len() + 1),
                    x: min_x - margin,
                    y: min_y - margin,
                    width: (max_x - min_x + margin * 2) as u32,
                    height: (max_y - min_y + margin * 2) as u32,
                });
            }
        }

        if let Some(layout) = &mut dungeon.layout {
            let mut to_remove = None;
            for (i, b) in layout.bounds.iter_mut().enumerate() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut b.label);
                    if ui.small_button("X").clicked() {
                        to_remove = Some(i);
                    }
                });
                ui.label(format!("  ({}, {}) {}x{}", b.x, b.y, b.width, b.height));
            }
            if let Some(i) = to_remove {
                layout.bounds.remove(i);
            }
        }
    };

    if has_selection {
        // Show layout controls in a collapsible section when something is selected
        egui::CollapsingHeader::new("Layout Controls")
            .default_open(false)
            .show(ui, show_controls);
    } else {
        show_controls(ui);
    }

    // Selected room info
    if let Some(ref room_id) = state.selected_room {
        let room_id = room_id.clone();

        // Room label as heading
        let room_label = dungeon.graph.room_by_id(&room_id)
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "?".to_string());
        ui.heading(&room_label);
        ui.separator();

        if let Some(layout) = &mut dungeon.layout {
            if let Some(rl) = layout.room_by_id_mut(&room_id) {
                ui.horizontal(|ui| {
                    ui.label("Position:");
                    crate::ui::canvas_common::num_input_i32(ui, &mut rl.x, 35.0);
                    crate::ui::canvas_common::num_input_i32(ui, &mut rl.y, 35.0);
                });
                ui.horizontal(|ui| {
                    ui.label("Size:");
                    crate::ui::canvas_common::num_input_u32(ui, &mut rl.width, 35.0);
                    ui.label("x");
                    crate::ui::canvas_common::num_input_u32(ui, &mut rl.height, 35.0);
                });
                ui.label(format!("{}x{} ft", rl.width * 5, rl.height * 5));
                if !rl.violations.is_empty() {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 60, 60), "Constraint violations:");
                    for v in &rl.violations {
                        ui.colored_label(egui::Color32::from_rgb(220, 60, 60), format!("  {}", v));
                    }
                }

                if ui.button("Rotate 90\u{00b0}").clicked() {
                    std::mem::swap(&mut rl.width, &mut rl.height);
                }
            }
        }

        // Connections from this room
        let connections: Vec<_> = dungeon.graph.connections.iter()
            .filter(|e| e.source_room_id == room_id || e.target_room_id == room_id)
            .map(|e| {
                let other_id = if e.source_room_id == room_id { &e.target_room_id } else { &e.source_room_id };
                let other_label = dungeon.graph.room_by_id(other_id)
                    .map(|r| r.label.as_str()).unwrap_or("?");
                (e.connection.connection_type.label(), other_label.to_string())
            })
            .collect();
        if !connections.is_empty() {
            ui.add_space(8.0);
            ui.label("Connections:");
            for (conn_type, other) in &connections {
                ui.label(format!("  {} \u{2192} {}", conn_type, other));
            }
        }

        // Encounters in this room
        let room_encounters: Vec<_> = dungeon.encounters.iter()
            .filter(|e| e.home_room_id == room_id)
            .map(|e| e.name.clone())
            .collect();
        if !room_encounters.is_empty() {
            ui.add_space(8.0);
            ui.label("Encounters:");
            for name in &room_encounters {
                ui.label(format!("  {}", name));
            }
        }

        // Tags
        if let Some(room) = dungeon.graph.room_by_id(&room_id) {
            if !room.tags.is_empty() {
                ui.add_space(8.0);
                let tags_str: Vec<_> = room.tags.iter().map(|t| t.label()).collect();
                ui.label(format!("Tags: {}", tags_str.join(", ")));
            }
            if !room.notes.is_empty() {
                ui.add_space(4.0);
                ui.label("Notes:");
                ui.label(&room.notes);
            }
        }

        // Elevation sections
        ui.add_space(12.0);
        ui.separator();
        ui.label("Elevation Sections:");

        let (room_w, room_h) = dungeon.graph.room_by_id(&room_id)
            .map(|r| r.grid_size()).unwrap_or((4, 4));

        // Add section button
        if ui.button("Add Section").clicked() {
            if let Some(room) = dungeon.graph.room_by_id_mut(&room_id) {
                let w = (room_w as f32 * 0.5).max(1.0);
                let h = (room_h as f32 * 0.5).max(1.0);
                let x = (room_w as f32 - w) / 2.0;
                let y = (room_h as f32 - h) / 2.0;
                room.sections.push(ElevationSection::new(ElevationType::Raised, x, y, w, h));
            }
        }

        // List sections
        let mut remove_section = None;
        {
            let section_info: Vec<(usize, String, ElevationType)> = dungeon.graph.room_by_id(&room_id)
                .map(|r| r.sections.iter().enumerate()
                    .map(|(i, s)| (i, s.id.clone(), s.elevation))
                    .collect())
                .unwrap_or_default();

            for (si, _id, elev) in &section_info {
                let is_sel = state.selected_section.as_ref()
                    .is_some_and(|(rid, idx)| rid == &room_id && *idx == *si);
                ui.horizontal(|ui| {
                    if ui.selectable_label(is_sel, elev.label()).clicked() {
                        state.selected_section = Some((room_id.clone(), *si));
                    }
                    if ui.small_button("X").clicked() {
                        remove_section = Some(*si);
                    }
                });
            }
        }

        if let Some(idx) = remove_section {
            if let Some(room) = dungeon.graph.room_by_id_mut(&room_id) {
                room.sections.remove(idx);
            }
            if state.selected_section.as_ref().is_some_and(|(rid, si)| rid == &room_id && *si == idx) {
                state.selected_section = None;
            }
        }

        // Edit selected section
        if let Some((ref sel_rid, sel_idx)) = state.selected_section.clone() {
            if sel_rid == &room_id {
                if let Some(room) = dungeon.graph.room_by_id_mut(&room_id) {
                    if sel_idx < room.sections.len() {
                        ui.add_space(8.0);
                        let section = &mut room.sections[sel_idx];
                        ui.horizontal(|ui| {
                            ui.label("Type:");
                            egui::ComboBox::from_id_salt("section_elev_type")
                                .selected_text(section.elevation.label())
                                .show_ui(ui, |ui| {
                                    for et in ElevationType::ALL {
                                        ui.selectable_value(&mut section.elevation, et, et.label());
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.label("x");
                            crate::ui::canvas_common::num_input_f32(ui, &mut section.x, 40.0);
                            ui.label("y");
                            crate::ui::canvas_common::num_input_f32(ui, &mut section.y, 40.0);
                        });
                        ui.horizontal(|ui| {
                            ui.label("Width");
                            crate::ui::canvas_common::num_input_f32(ui, &mut section.width, 40.0);
                            ui.label("Length");
                            crate::ui::canvas_common::num_input_f32(ui, &mut section.length, 40.0);
                        });
                        if matches!(section.elevation, ElevationType::Raised | ElevationType::Lowered | ElevationType::Water) {
                            let height_label = if section.elevation == ElevationType::Water { "Depth:" } else { "Height:" };
                            ui.horizontal(|ui| {
                                ui.label(height_label);
                                crate::ui::canvas_common::num_input_f32(ui, &mut section.height, 40.0);
                                ui.label("ft");
                            });
                        }
                    }
                }
            }
        }
    }

    // Selected corridor info
    if let Some(ci) = state.selected_corridor {
        ui.add_space(16.0);
        ui.separator();
        let conn_id = dungeon.layout.as_ref()
            .and_then(|l| l.corridors.get(ci))
            .map(|c| c.connection_id.clone());
        if let Some(layout) = &dungeon.layout {
            if let Some(corridor) = layout.corridors.get(ci) {
                ui.label(format!("Corridor: {} waypoints", corridor.waypoints.len()));
                if corridor.invalid {
                    ui.colored_label(egui::Color32::from_rgb(220, 50, 50), "Invalid (overlapping)");
                }
            }
        }
        if let Some(conn_id) = conn_id {
            if let Some(edge) = dungeon.graph.connection_by_id_mut(&conn_id) {
                ui.add_space(8.0);

                // Connection type
                egui::ComboBox::from_id_salt("spatial_conn_type")
                    .selected_text(edge.connection.connection_type.label())
                    .show_ui(ui, |ui| {
                        for ct in ConnectionType::ALL {
                            ui.selectable_value(&mut edge.connection.connection_type, ct, ct.label());
                        }
                    });

                // Corridor width
                ui.add_space(4.0);
                let old_width = edge.connection.corridor_width;
                ui.horizontal(|ui| {
                    ui.label("Width:");
                    crate::ui::canvas_common::num_input_u32(ui, &mut edge.connection.corridor_width, 40.0);
                    ui.label("sq");
                });
                if edge.connection.corridor_width < 1 {
                    edge.connection.corridor_width = 1;
                }
                if edge.connection.corridor_width != old_width {
                    // Re-route this corridor
                    let affected: std::collections::HashSet<String> = std::collections::HashSet::from([
                        edge.source_room_id.clone(),
                        edge.target_room_id.clone(),
                    ]);
                    if let Some(layout) = &mut dungeon.layout {
                        layout.corridors =
                            crate::solver::corridor::route_corridors_for_rooms(
                                &dungeon.graph, layout, &affected,
                            );
                        layout.recheck_corridor_overlaps();
                    }
                }

                // Double door
                ui.checkbox(&mut dungeon.graph.connection_by_id_mut(&conn_id).unwrap().connection.double_door, "Double door");

                // Exit placement
                let edge = dungeon.graph.connection_by_id_mut(&conn_id).unwrap();
                if edge.source_exit.is_some() || edge.target_exit.is_some() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if edge.source_exit.is_some() {
                            if ui.button("Clear src exit").clicked() {
                                edge.source_exit = None;
                            }
                        }
                        if edge.target_exit.is_some() {
                            if ui.button("Clear tgt exit").clicked() {
                                edge.target_exit = None;
                            }
                        }
                    });
                }
            }
        }
    }

    // Selected group constraints
    if let Some(gi) = state.selected_group {
        if gi < dungeon.graph.groups.len() {
            ui.add_space(16.0);
            ui.separator();
            let group = &mut dungeon.graph.groups[gi];
            ui.label(format!("Group: {}", group.label));

            let mut has_w = group.max_width.is_some();
            let mut w = group.max_width.unwrap_or(20);
            ui.horizontal(|ui| {
                ui.checkbox(&mut has_w, "Max width:");
                if has_w { crate::ui::canvas_common::num_input_u32(ui, &mut w, 40.0); ui.label("sq"); }
            });
            group.max_width = if has_w { Some(w) } else { None };

            let mut has_h = group.max_height.is_some();
            let mut h = group.max_height.unwrap_or(20);
            ui.horizontal(|ui| {
                ui.checkbox(&mut has_h, "Max height:");
                if has_h { crate::ui::canvas_common::num_input_u32(ui, &mut h, 40.0); ui.label("sq"); }
            });
            group.max_height = if has_h { Some(h) } else { None };

            ui.add_space(8.0);

            // Duplicate group
            let group_room_ids = dungeon.graph.groups[gi].room_ids.clone();
            if ui.button("Duplicate Group").clicked() {
                duplicate_group(dungeon, &group_room_ids, gi);
            }

            // Rotate group 90 degrees
            if ui.button("Rotate Group 90\u{00b0}").clicked() {
                rotate_group(dungeon, &group_room_ids);
            }

            ui.horizontal(|ui| {
                if ui.button("Flip Horizontal").clicked() {
                    flip_group(dungeon, &group_room_ids, true);
                }
                if ui.button("Flip Vertical").clicked() {
                    flip_group(dungeon, &group_room_ids, false);
                }
            });
        }
    }
}

fn duplicate_group(dungeon: &mut Dungeon, room_ids: &[String], group_idx: usize) {
    use std::collections::HashMap;

    let room_id_set: std::collections::HashSet<&String> = room_ids.iter().collect();

    // Compute group bounding box in spatial layout
    let mut group_max_x = i32::MIN;
    let mut group_min_x = i32::MAX;
    if let Some(layout) = &dungeon.layout {
        for rid in room_ids {
            if let Some(rl) = layout.room_by_id(rid) {
                group_min_x = group_min_x.min(rl.x);
                group_max_x = group_max_x.max(rl.x + rl.width as i32);
            }
        }
    }
    let offset_x = if group_max_x > group_min_x { group_max_x - group_min_x + 2 } else { 10 };

    // Clone rooms with new IDs
    let mut id_map: HashMap<String, String> = HashMap::new();
    for old_id in room_ids {
        if let Some(old_room) = dungeon.graph.room_by_id(old_id).cloned() {
            let mut new_room = Room::new(old_room.label.clone());
            new_room.tags = old_room.tags;
            new_room.notes = old_room.notes;
            new_room.size_hint = old_room.size_hint;
            new_room.grid_width = old_room.grid_width;
            new_room.grid_height = old_room.grid_height;
            new_room.shape = old_room.shape;
            new_room.allow_rotation = old_room.allow_rotation;
            id_map.insert(old_id.clone(), new_room.id.clone());

            // Copy graph position with offset
            if let Some(&(gx, gy)) = dungeon.graph.graph_positions.get(old_id) {
                dungeon.graph.graph_positions.insert(new_room.id.clone(), (gx + 150.0, gy));
            }

            dungeon.graph.add_room(new_room);
        }
    }

    // Clone connections between group rooms
    let edges_to_clone: Vec<StoredEdge> = dungeon.graph.connections.iter()
        .filter(|e| room_id_set.contains(&e.source_room_id) && room_id_set.contains(&e.target_room_id))
        .cloned()
        .collect();
    for old_edge in &edges_to_clone {
        if let (Some(new_src), Some(new_tgt)) = (
            id_map.get(&old_edge.source_room_id),
            id_map.get(&old_edge.target_room_id),
        ) {
            let mut new_conn = Connection::new(old_edge.connection.connection_type);
            new_conn.corridor_width = old_edge.connection.corridor_width;
            new_conn.double_door = old_edge.connection.double_door;
            new_conn.label = old_edge.connection.label.clone();
            new_conn.min_length = old_edge.connection.min_length;
            new_conn.max_length = old_edge.connection.max_length;
            dungeon.graph.add_connection(new_src.clone(), new_tgt.clone(), new_conn);
        }
    }

    // Clone spatial layout entries
    if let Some(layout) = &mut dungeon.layout {
        let new_rooms: Vec<RoomLayout> = room_ids.iter().filter_map(|old_id| {
            let rl = layout.room_by_id(old_id)?;
            let new_id = id_map.get(old_id)?;
            Some(RoomLayout {
                room_id: new_id.clone(),
                x: rl.x + offset_x,
                y: rl.y,
                width: rl.width,
                height: rl.height,
                violations: Vec::new(),
            })
        }).collect();
        layout.rooms.extend(new_rooms);

        // Clone corridors
        let new_corridors: Vec<CorridorSegment> = edges_to_clone.iter().filter_map(|old_edge| {
            let new_conn_id = dungeon.graph.connections.iter()
                .find(|e| {
                    id_map.get(&old_edge.source_room_id).is_some_and(|s| s == &e.source_room_id)
                    && id_map.get(&old_edge.target_room_id).is_some_and(|t| t == &e.target_room_id)
                })
                .map(|e| e.connection.id.clone())?;
            let old_corridor = layout.corridors.iter().find(|c| c.connection_id == old_edge.connection.id)?;
            Some(CorridorSegment {
                connection_id: new_conn_id,
                waypoints: old_corridor.waypoints.iter().map(|wp| GridPos { x: wp.x + offset_x, y: wp.y }).collect(),
                width: old_corridor.width,
                invalid: false,
                pinned_waypoints: Vec::new(),
            })
        }).collect();
        layout.corridors.extend(new_corridors);
    }

    // Create new group
    let new_room_ids: Vec<String> = id_map.values().cloned().collect();
    let old_group = &dungeon.graph.groups[group_idx];
    let mut new_group = RoomGroup::new(format!("{} (copy)", old_group.label));
    new_group.room_ids = new_room_ids;
    new_group.max_width = old_group.max_width;
    new_group.max_height = old_group.max_height;
    dungeon.graph.groups.push(new_group);
}

fn rotate_group(dungeon: &mut Dungeon, room_ids: &[String]) {
    let Some(layout) = &mut dungeon.layout else { return };

    // Find group center
    let mut sum_cx = 0.0_f32;
    let mut sum_cy = 0.0_f32;
    let mut count = 0;
    for rid in room_ids {
        if let Some(rl) = layout.room_by_id(rid) {
            sum_cx += rl.x as f32 + rl.width as f32 / 2.0;
            sum_cy += rl.y as f32 + rl.height as f32 / 2.0;
            count += 1;
        }
    }
    if count == 0 { return; }
    let center_x = sum_cx / count as f32;
    let center_y = sum_cy / count as f32;

    // Rotate each room 90° CW around center: (x,y) -> (center_x + (y - center_y), center_y - (x - center_x))
    for rid in room_ids {
        if let Some(rl) = layout.room_by_id_mut(rid) {
            let old_cx = rl.x as f32 + rl.width as f32 / 2.0;
            let old_cy = rl.y as f32 + rl.height as f32 / 2.0;
            let new_cx = center_x + (old_cy - center_y);
            let new_cy = center_y - (old_cx - center_x);
            std::mem::swap(&mut rl.width, &mut rl.height);
            rl.x = (new_cx - rl.width as f32 / 2.0).round() as i32;
            rl.y = (new_cy - rl.height as f32 / 2.0).round() as i32;
        }
    }

    // Rotate corridor waypoints
    let room_id_set: std::collections::HashSet<&String> = room_ids.iter().collect();
    let conn_ids: Vec<String> = dungeon.graph.connections.iter()
        .filter(|e| room_id_set.contains(&e.source_room_id) && room_id_set.contains(&e.target_room_id))
        .map(|e| e.connection.id.clone())
        .collect();

    for corridor in &mut layout.corridors {
        if conn_ids.contains(&corridor.connection_id) {
            for wp in &mut corridor.waypoints {
                let old_x = wp.x as f32;
                let old_y = wp.y as f32;
                wp.x = (center_x + (old_y - center_y)).round() as i32;
                wp.y = (center_y - (old_x - center_x)).round() as i32;
            }
            resolve_diagonal_segments_clean(&mut corridor.waypoints);
        }
    }
}

fn flip_group(dungeon: &mut Dungeon, room_ids: &[String], horizontal: bool) {
    let Some(layout) = &mut dungeon.layout else { return };

    // Find group center
    let mut sum_cx = 0.0_f32;
    let mut sum_cy = 0.0_f32;
    let mut count = 0;
    for rid in room_ids {
        if let Some(rl) = layout.room_by_id(rid) {
            sum_cx += rl.x as f32 + rl.width as f32 / 2.0;
            sum_cy += rl.y as f32 + rl.height as f32 / 2.0;
            count += 1;
        }
    }
    if count == 0 { return; }
    let center_x = sum_cx / count as f32;
    let center_y = sum_cy / count as f32;

    // Flip each room around center
    for rid in room_ids {
        if let Some(rl) = layout.room_by_id_mut(rid) {
            let old_cx = rl.x as f32 + rl.width as f32 / 2.0;
            let old_cy = rl.y as f32 + rl.height as f32 / 2.0;
            if horizontal {
                let new_cx = center_x - (old_cx - center_x);
                rl.x = (new_cx - rl.width as f32 / 2.0).round() as i32;
            } else {
                let new_cy = center_y - (old_cy - center_y);
                rl.y = (new_cy - rl.height as f32 / 2.0).round() as i32;
            }
        }
    }

    // Flip corridor waypoints
    let room_id_set: std::collections::HashSet<&String> = room_ids.iter().collect();
    let conn_ids: Vec<String> = dungeon.graph.connections.iter()
        .filter(|e| room_id_set.contains(&e.source_room_id) && room_id_set.contains(&e.target_room_id))
        .map(|e| e.connection.id.clone())
        .collect();

    for corridor in &mut layout.corridors {
        if conn_ids.contains(&corridor.connection_id) {
            for wp in &mut corridor.waypoints {
                if horizontal {
                    wp.x = (2.0 * center_x - wp.x as f32).round() as i32;
                } else {
                    wp.y = (2.0 * center_y - wp.y as f32).round() as i32;
                }
            }
            resolve_diagonal_segments_clean(&mut corridor.waypoints);
        }
    }
}
