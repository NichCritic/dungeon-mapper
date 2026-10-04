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
    /// Dragging the selected room's rotation handle.
    Rotate(String),
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
    /// When true, clicking/dragging on the selected cave paints cells instead of moving it.
    pub cave_edit_mode: bool,
    /// Last cell painted in the current cave stroke (room-local), so a fast drag fills
    /// the cells in between. None when no stroke is in progress.
    cave_stroke_last: Option<(i32, i32)>,
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
            cave_edit_mode: false,
            cave_stroke_last: None,
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
const LOWER_FLOOR_DIM: f32 = 0.35;

/// How brightly to draw something at `rel` to the viewed floor: in full, dimmed
/// (below), or not at all (above).
fn floor_dim(rel: FloorRel) -> Option<f32> {
    match rel {
        FloorRel::On => Some(1.0),
        FloorRel::Below => Some(LOWER_FLOOR_DIM),
        FloorRel::Above => None,
    }
}

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
        draw_rooms(&painter, &transform, layout, &dungeon.graph, state, dungeon.theme.floor_color);
        draw_corridors(&painter, &transform, layout, &dungeon.graph, state);
        draw_doors(&painter, &transform, layout, &dungeon.graph, state);
        draw_waypoint_handles(&painter, &transform, layout, state);
        draw_exit_handles(&painter, &transform, layout, &dungeon.graph, state);
        if !state.cave_edit_mode {
            if let Some(rl) = state.selected_room.as_ref().and_then(|id| layout.room_by_id(id)) {
                draw_rotation_handle(&painter, &transform, rl);
            }
        }
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
    // Cave painting takes the pointer before anything else, so strokes never select
    // or move rooms.
    if state.cave_edit_mode && handle_cave_paint(ui, response, transform, dungeon, state) {
        return;
    }

    // === DRAG START ===
    if response.drag_started_by(egui::PointerButton::Primary) {
        if let Some(pos) = response.hover_pos() {
            let world = transform.screen_to_world(pos);

            // Rotation handle of the selected room
            if let (Some(room_id), Some(layout)) = (&state.selected_room, &dungeon.layout) {
                if let Some(rl) = layout.room_by_id(room_id) {
                    let (hx, hy) = rotation_handle_pos(rl);
                    if pos.distance(transform.world_to_screen(egui::pos2(hx, hy))) < HANDLE_HIT_RADIUS {
                        state.drag_target = DragTarget::Rotate(room_id.clone());
                        return;
                    }
                }
            }

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
                                let (lx, ly) = rl.to_local(world.x / GRID_PX, world.y / GRID_PX);
                                if lx >= sec.x && lx <= sec.x + sec.width
                                    && ly >= sec.y && ly <= sec.y + sec.length
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

            // Check rooms — prefer deepest-nested (smallest) room at click point
            let gx = world_to_grid(world.x);
            let gy = world_to_grid(world.y);
            if let Some(layout) = &dungeon.layout {
                let mut best_hit: Option<(&RoomLayout, u32)> = None; // (room_layout, nesting_depth)
                for rl in &layout.rooms {
                    if let Some(floor) = state.current_floor {
                        if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
                            if !room.floor.visible_on(floor) {
                                continue;
                            }
                        }
                    }

                    if room_hit(rl, world, 0.4) {
                        let depth = dungeon.graph.nesting_depth(&rl.room_id);
                        let area = rl.width * rl.height;
                        let is_better = match &best_hit {
                            None => true,
                            Some((prev, prev_depth)) => {
                                depth > *prev_depth
                                    || (depth == *prev_depth && area < prev.width * prev.height)
                            }
                        };
                        if is_better {
                            best_hit = Some((rl, depth));
                        }
                    }
                }

                if let Some((rl, _)) = best_hit {
                    let rl_id = rl.room_id.clone();
                    if state.selected_room.as_deref() != Some(&rl_id) {
                        state.cave_edit_mode = false;
                    }
                    state.selected_room = Some(rl_id.clone());
                    state.selected_corridor = None;
                    state.selected_waypoint = None;
                    state.drag_target = DragTarget::Room(rl_id);
                    state.drag_accum = egui::Vec2::ZERO;
                    return;
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
                    if is_orthogonal(&dungeon.graph, &layout.corridors[ci]) {
                        resolve_diagonal_segments(&mut layout.corridors[ci].waypoints);
                    }
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
                    // Floor filtering: skip corridors not on the current floor
                    if let (Some(floor), Some(edge)) = (state.current_floor, dungeon.graph.connection_by_id(&corridor.connection_id)) {
                        if dungeon.graph.edge_floor_relation(edge, floor) != FloorRel::On {
                            continue;
                        }
                    }
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
                    // Check room hit — prefer deepest-nested room
                    let mut hit_room = false;
                    let mut best_hit: Option<(&RoomLayout, u32)> = None;
                    for rl in &layout.rooms {
                        if let Some(floor) = state.current_floor {
                            if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
                                if !room.floor.visible_on(floor) {
                                    continue;
                                }
                            }
                        }
                        if room_hit(rl, world, 0.4) {
                            let depth = dungeon.graph.nesting_depth(&rl.room_id);
                            let area = rl.width * rl.height;
                            let is_better = match &best_hit {
                                None => true,
                                Some((prev, prev_depth)) => {
                                    depth > *prev_depth
                                        || (depth == *prev_depth && area < prev.width * prev.height)
                                }
                            };
                            if is_better {
                                best_hit = Some((rl, depth));
                            }
                        }
                    }
                    if let Some((rl, _)) = best_hit {
                        let rl_id = rl.room_id.clone();
                        if !hit_room {
                            // Check if click is on an elevation section
                            let room_px_x = rl.x as f32 * GRID_PX;
                            let room_px_y = rl.y as f32 * GRID_PX;
                            let mut hit_section = None;
                            if let Some(room) = dungeon.graph.room_by_id(&rl_id) {
                                for (si, sec) in room.sections.iter().enumerate() {
                                    let sx = room_px_x + sec.x * GRID_PX;
                                    let sy = room_px_y + sec.y * GRID_PX;
                                    let sw = sec.width * GRID_PX;
                                    let sh = sec.length * GRID_PX;
                                    if world.x >= sx && world.x <= sx + sw
                                        && world.y >= sy && world.y <= sy + sh
                                    {
                                        hit_section = Some((rl_id.clone(), si));
                                        break;
                                    }
                                }
                            }

                            state.selected_room = Some(rl_id);
                            state.selected_corridor = None;
                            state.selected_waypoint = None;
                            state.selected_group = None;
                            state.selected_section = hit_section;
                            hit_room = true;
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
                            state.cave_edit_mode = false;
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
        // Rotation follows the cursor's angle around the room's center
        if let DragTarget::Rotate(ref room_id) = state.drag_target {
            let snap = ui.input(|i| i.modifiers.shift);
            if let (Some(ptr), Some(layout)) = (response.interact_pointer_pos(), &mut dungeon.layout) {
                if let Some(rl) = layout.room_by_id_mut(room_id) {
                    let world = transform.screen_to_world(ptr);
                    let (cx, cy) = rl.center();
                    // The handle sits straight above the center at 0°
                    let deg = (world.y / GRID_PX - cy).atan2(world.x / GRID_PX - cx).to_degrees() + 90.0;
                    let step = if snap { 15.0 } else { 1.0 };
                    let deg = (deg / step).round() * step;
                    rl.rotation = (deg + 180.0).rem_euclid(360.0) - 180.0;
                }
            }
            return;
        }
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

        let mut delta = response.drag_delta() / state.view.zoom;
        // A section of a rotated room moves in the room's own frame
        if let (DragTarget::Section(room_id, _), Some(layout)) = (&state.drag_target, &dungeon.layout) {
            if let Some(rl) = layout.room_by_id(room_id).filter(|rl| rl.is_rotated()) {
                let (s, c) = rl.rotation.to_radians().sin_cos();
                delta = egui::vec2(delta.x * c + delta.y * s, -delta.x * s + delta.y * c);
            }
        }
        state.drag_accum += delta;

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

                    // Collect children to move along with this room (recursive)
                    let mut children_to_move: Vec<String> = Vec::new();
                    {
                        let mut stack = vec![room_id.clone()];
                        while let Some(rid) = stack.pop() {
                            for child_id in dungeon.graph.children_of(&rid) {
                                children_to_move.push(child_id.to_string());
                                stack.push(child_id.to_string());
                            }
                        }
                    }

                    // Also collect connections for children (for waypoint/exit shifting)
                    let mut child_connected_ids: Vec<(String, String, bool, bool)> = Vec::new(); // (conn_id, room_id, is_src, is_tgt)
                    for child_id in &children_to_move {
                        for e in &dungeon.graph.connections {
                            let is_src = e.source_room_id == *child_id;
                            let is_tgt = e.target_room_id == *child_id;
                            if is_src || is_tgt {
                                child_connected_ids.push((e.connection.id.clone(), child_id.clone(), is_src, is_tgt));
                            }
                        }
                    }

                    if let Some(layout) = &mut dungeon.layout {
                        // Move the room
                        if let Some(rl) = layout.room_by_id_mut(&room_id) {
                            rl.x += grid_steps_x;
                            rl.y += grid_steps_y;
                        }

                        // Move all children along with the container
                        for child_id in &children_to_move {
                            if let Some(rl) = layout.room_by_id_mut(child_id) {
                                rl.x += grid_steps_x;
                                rl.y += grid_steps_y;
                            }
                        }

                        // Shift pinned waypoints for children's corridors
                        for corridor in &mut layout.corridors {
                            for (conn_id, _, is_src, is_tgt) in &child_connected_ids {
                                if corridor.connection_id != *conn_id { continue; }
                                if !corridor.pinned_waypoints.is_empty() {
                                    if *is_src {
                                        corridor.pinned_waypoints.first_mut().unwrap().x += grid_steps_x;
                                        corridor.pinned_waypoints.first_mut().unwrap().y += grid_steps_y;
                                    }
                                    if *is_tgt {
                                        corridor.pinned_waypoints.last_mut().unwrap().x += grid_steps_x;
                                        corridor.pinned_waypoints.last_mut().unwrap().y += grid_steps_y;
                                    }
                                }
                            }
                        }

                        // Clamp child rooms to stay within parent bounds
                        if let Some(parent_id) = dungeon.graph.parent_of(&room_id) {
                            let padding = dungeon.graph.containment_group(parent_id)
                                .map(|g| g.containment_padding as i32)
                                .unwrap_or(1);
                            let parent_bounds = layout.room_by_id(parent_id).map(|p| (p.x, p.y, p.width, p.height));
                            if let (Some((px, py, pw, ph)), Some(rl)) = (parent_bounds, layout.room_by_id_mut(&room_id)) {
                                let min_x = px + padding;
                                let min_y = py + padding;
                                let max_x = px + pw as i32 - padding - rl.width as i32;
                                let max_y = py + ph as i32 - padding - rl.height as i32;
                                rl.x = rl.x.clamp(min_x, max_x);
                                rl.y = rl.y.clamp(min_y, max_y);
                            }
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

                    // Shift exit positions for connected edges (room + children)
                    let all_exit_shifts: Vec<(String, bool, bool)> = connected_ids.iter()
                        .map(|(c, s, t)| (c.clone(), *s, *t))
                        .chain(child_connected_ids.iter().map(|(c, _, s, t)| (c.clone(), *s, *t)))
                        .collect();
                    for (conn_id, is_src, is_tgt) in &all_exit_shifts {
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
                        // Angled corridors keep their angles: only orthogonal ones drag
                        // neighbours along and get their corners squared
                        let ortho = is_orthogonal(&dungeon.graph, &layout.corridors[ci]);
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
                            if ortho && wi > 0 && wi - 1 != 0 {
                                if prev_horizontal {
                                    wps[wi - 1].y += grid_steps_y;
                                }
                                if prev_vertical {
                                    wps[wi - 1].x += grid_steps_x;
                                }
                            }

                            // Pull the next neighbor along the shared axis,
                            // but never move the last endpoint
                            if ortho && wi + 1 < wps.len() && wi + 1 != last {
                                if next_horizontal {
                                    wps[wi + 1].y += grid_steps_y;
                                }
                                if next_vertical {
                                    wps[wi + 1].x += grid_steps_x;
                                }
                            }

                            // Clean up stale auto-corners and resolve new diagonals
                            if ortho {
                                resolve_diagonal_segments_clean(wps);
                            }

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
                DragTarget::Exit(_, _) | DragTarget::Rotate(_) => {} // handled above, before grid-step check
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
                // Cave contours are stored in world space and depend on the neighbours
                state.cave_contours_dirty = true;
            }
            DragTarget::Waypoint(ci, _) => {
                let ci = *ci;
                if let Some(layout) = &mut dungeon.layout {
                    if ci < layout.corridors.len() {
                        if is_orthogonal(&dungeon.graph, &layout.corridors[ci]) {
                            resolve_diagonal_segments(&mut layout.corridors[ci].waypoints);
                        }
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
                state.cave_contours_dirty = true;
            }
            DragTarget::Section(_, _) => {} // position already updated during drag
            DragTarget::Rotate(room_id) => {
                reroute_room(dungeon, room_id);
                state.cave_contours_dirty = true;
            }
            DragTarget::Exit(conn_id, _) => {
                // Re-route the corridor for this connection
                let conn_id = conn_id.clone();
                if let Some(edge) = dungeon.graph.connection_by_id(&conn_id) {
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

/// Paint the selected cave's cells: left button digs (floor), right button fills (wall).
/// A stroke must start inside the cave; it then paints every cell the pointer passes
/// over, interpolating between frames. Returns true when the pointer was used for
/// painting this frame (including the release that ends a stroke).
fn handle_cave_paint(
    ui: &egui::Ui,
    response: &egui::Response,
    transform: &ViewTransform,
    dungeon: &mut Dungeon,
    state: &mut SpatialViewState,
) -> bool {
    let Some(room_id) = state.selected_room.clone() else { return false };
    let Some(rl) = dungeon.layout.as_ref().and_then(|l| l.room_by_id(&room_id)).cloned() else { return false };
    let (rw, rh) = (rl.width as i32, rl.height as i32);
    let Some(cave) = dungeon.graph.room_by_id_mut(&room_id)
        .filter(|r| r.shape == RoomShape::Cave)
        .and_then(|r| r.cave_data.as_mut())
        .filter(|c| c.cells.len() == (rw * rh) as usize)
    else {
        return false;
    };

    // The cell under the pointer in the cave's own (possibly rotated) frame
    let local = |pos: egui::Pos2| {
        let world = transform.screen_to_world(pos);
        let (lx, ly) = rl.to_local(world.x / GRID_PX, world.y / GRID_PX);
        (lx.floor() as i32, ly.floor() as i32)
    };
    let inside = |(x, y): (i32, i32)| x >= 0 && y >= 0 && x < rw && y < rh;

    let (primary, secondary, origin) = ui.input(|i| (i.pointer.primary_down(), i.pointer.secondary_down(), i.pointer.press_origin()));
    let paint_val = if primary { Some(true) } else if secondary { Some(false) } else { None };
    let Some(paint_val) = paint_val.filter(|_| response.is_pointer_button_down_on()) else {
        // Button released: swallow the release frame so it doesn't also count as a click
        return state.cave_stroke_last.take().is_some();
    };
    let Some(pos) = response.interact_pointer_pos() else { return false };
    let cell = local(pos);

    let from = match state.cave_stroke_last {
        Some(last) => last,
        // A new stroke only starts inside the cave; elsewhere the click behaves normally
        None if origin.map(local).is_some_and(inside) => cell,
        None => return false,
    };

    let mut changed = false;
    for (x, y) in grid_line(from, cell) {
        if !inside((x, y)) {
            continue;
        }
        let c = &mut cave.cells[(y * rw + x) as usize];
        if *c != paint_val {
            *c = paint_val;
            changed = true;
        }
    }
    if changed {
        cave.generation += 1;
        state.cave_contours_dirty = true;
    }
    state.cave_stroke_last = Some(cell);
    true
}

/// Grid cells on the line from `a` to `b` inclusive (Bresenham), 4-connected: a
/// diagonal step goes through an edge-adjacent cell, so a dug stroke is walkable.
fn grid_line(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut x, mut y) = a;
    let (dx, dy) = ((b.0 - x).abs(), -(b.1 - y).abs());
    let (sx, sy) = ((b.0 - x).signum(), (b.1 - y).signum());
    let mut err = dx + dy;
    let mut out = vec![(x, y)];
    while (x, y) != b {
        let e2 = 2 * err;
        let (step_x, step_y) = (e2 >= dy, e2 <= dx);
        if step_x {
            err += dy;
            x += sx;
            out.push((x, y));
        }
        if step_y {
            err += dx;
            y += sy;
            out.push((x, y));
        }
    }
    out
}

/// Whether a world point (pixels) lies on a room, with `margin` cells of slack, in the
/// room's own (possibly rotated) frame.
fn room_hit(rl: &RoomLayout, world: egui::Pos2, margin: f32) -> bool {
    let (lx, ly) = rl.to_local(world.x / GRID_PX, world.y / GRID_PX);
    lx >= -margin && ly >= -margin && lx <= rl.width as f32 + margin && ly <= rl.height as f32 + margin
}

/// World position (pixels) of a room's rotation handle: a cell above its top edge.
fn rotation_handle_pos(rl: &RoomLayout) -> (f32, f32) {
    let (x, y) = rl.to_world(rl.width as f32 / 2.0, -1.2);
    (x * GRID_PX, y * GRID_PX)
}

/// Re-route the corridors of a room that moved or turned.
fn reroute_room(dungeon: &mut Dungeon, room_id: &str) {
    if let Some(layout) = &mut dungeon.layout {
        let affected = std::collections::HashSet::from([room_id.to_string()]);
        layout.corridors = crate::solver::corridor::route_corridors_for_rooms(&dungeon.graph, layout, &affected);
        crate::solver::corridor::compute_wall_openings(&dungeon.graph, layout);
        layout.recheck_corridor_overlaps();
    }
}

/// Whether a corridor's connection keeps to horizontal and vertical runs.
fn is_orthogonal(graph: &DungeonGraph, corridor: &CorridorSegment) -> bool {
    graph.connections.iter()
        .find(|e| e.connection.id == corridor.connection_id)
        .is_none_or(|e| e.connection.corridor_angle == CorridorAngle::Orthogonal)
}

/// Draw a rotated room: its turned outline (or circle), cave cells, local grid, label
/// and elevation sections.
fn draw_rotated_room(
    painter: &egui::Painter,
    transform: &ViewTransform,
    rl: &RoomLayout,
    room: Option<&Room>,
    fill: egui::Color32,
    wall_fill: egui::Color32,
    stroke: egui::Stroke,
    cave_outline: egui::Stroke,
    label_color: egui::Color32,
) {
    let sp = |lx: f32, ly: f32| {
        let (x, y) = rl.to_world(lx, ly);
        transform.world_to_screen(egui::pos2(x * GRID_PX, y * GRID_PX))
    };
    let quad = |x0: f32, y0: f32, x1: f32, y1: f32| vec![sp(x0, y0), sp(x1, y0), sp(x1, y1), sp(x0, y1)];
    let (w, h) = (rl.width as f32, rl.height as f32);
    match room.map(|r| r.shape).unwrap_or_default() {
        RoomShape::Circle => {
            let r = w.min(h) / 2.0 * GRID_PX * transform.zoom;
            painter.circle_filled(sp(w / 2.0, h / 2.0), r, fill);
            painter.circle_stroke(sp(w / 2.0, h / 2.0), r, stroke);
        }
        RoomShape::Cave if room.and_then(|r| r.cave_data.as_ref()).is_some_and(|c| c.cells.len() == (rl.width * rl.height) as usize) => {
            let cave = room.and_then(|r| r.cave_data.as_ref()).unwrap();
            for ly in 0..rl.height as usize {
                for lx in 0..rl.width as usize {
                    let floor = cave.cells[ly * rl.width as usize + lx];
                    let (x, y) = (lx as f32, ly as f32);
                    painter.add(egui::Shape::convex_polygon(quad(x, y, x + 1.0, y + 1.0), if floor { fill } else { wall_fill }, egui::Stroke::NONE));
                }
            }
            let contour = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(40, 40, 40));
            for &(x1, y1, x2, y2) in &cave.contour_segments {
                painter.line_segment([transform.world_to_screen(egui::pos2(x1, y1)), transform.world_to_screen(egui::pos2(x2, y2))], contour);
            }
            painter.add(egui::Shape::closed_line(quad(0.0, 0.0, w, h), cave_outline));
        }
        _ => {
            painter.add(egui::Shape::convex_polygon(quad(0.0, 0.0, w, h), fill, egui::Stroke::NONE));
            painter.add(egui::Shape::closed_line(quad(0.0, 0.0, w, h), stroke));
        }
    }
    let grid = egui::Stroke::new(0.5_f32, egui::Color32::from_rgba_unmultiplied(80, 80, 80, 60));
    for ly in 1..rl.height {
        painter.line_segment([sp(0.0, ly as f32), sp(w, ly as f32)], grid);
    }
    for lx in 1..rl.width {
        painter.line_segment([sp(lx as f32, 0.0), sp(lx as f32, h)], grid);
    }
    if let Some(room) = room {
        for section in &room.sections {
            let q = quad(section.x, section.y, section.x + section.width, section.y + section.length);
            painter.add(egui::Shape::convex_polygon(q.clone(), egui::Color32::from_rgba_unmultiplied(80, 80, 80, 30), egui::Stroke::NONE));
            painter.add(egui::Shape::closed_line(q, egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(90, 90, 90))));
        }
        painter.text(sp(w / 2.0, h / 2.0), egui::Align2::CENTER_CENTER, &room.label, egui::FontId::monospace(11.0 * transform.zoom), label_color);
    }
}

/// The selected room's rotation handle: a knob above its top edge, on a stalk.
fn draw_rotation_handle(painter: &egui::Painter, transform: &ViewTransform, rl: &RoomLayout) {
    let (hx, hy) = rotation_handle_pos(rl);
    let (tx, ty) = rl.to_world(rl.width as f32 / 2.0, 0.0);
    let knob = transform.world_to_screen(egui::pos2(hx, hy));
    let top = transform.world_to_screen(egui::pos2(tx * GRID_PX, ty * GRID_PX));
    painter.line_segment([top, knob], egui::Stroke::new(1.5_f32, COLOR_SELECTION));
    painter.circle_filled(knob, 5.0, egui::Color32::WHITE);
    painter.circle_stroke(knob, 5.0, egui::Stroke::new(2.0_f32, COLOR_SELECTION));
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
        painter.line_segment([from, to], egui::Stroke::new(1.0_f32, color));
    }
    for y in min_gy..=max_gy {
        let color = if y % 5 == 0 { heavy } else { light };
        let from = transform.world_to_screen(egui::pos2(grid_to_world(min_gx), grid_to_world(y)));
        let to = transform.world_to_screen(egui::pos2(grid_to_world(max_gx), grid_to_world(y)));
        painter.line_segment([from, to], egui::Stroke::new(1.0_f32, color));
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

        if group.is_containment() {
            // Solid border for containment groups
            let stroke = egui::Stroke::new(2.0_f32, border_color);
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Middle);
        } else {
            // Dashed border for constraint groups
            let stroke = egui::Stroke::new(1.5_f32, border_color);
            draw_dashed_line(painter, egui::pos2(rect.min.x, rect.min.y), egui::pos2(rect.max.x, rect.min.y), stroke, 6.0, 3.0);
            draw_dashed_line(painter, egui::pos2(rect.max.x, rect.min.y), egui::pos2(rect.max.x, rect.max.y), stroke, 6.0, 3.0);
            draw_dashed_line(painter, egui::pos2(rect.max.x, rect.max.y), egui::pos2(rect.min.x, rect.max.y), stroke, 6.0, 3.0);
            draw_dashed_line(painter, egui::pos2(rect.min.x, rect.max.y), egui::pos2(rect.min.x, rect.min.y), stroke, 6.0, 3.0);
        }

        // Fill (only for non-containment; containers show the room's floor)
        if !group.is_containment() {
            painter.rect_filled(rect, 0.0, fill);
        }

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

        let stroke = egui::Stroke::new(2.0_f32, color);
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
    let shapes = crate::model::geometry::corridor_shapes(layout, graph);
    for (ci, corridor) in layout.corridors.iter().enumerate() {
        // Floor filtering: dim corridors to lower floors, hide higher
        let dim = match (state.current_floor, graph.connection_by_id(&corridor.connection_id)) {
            (Some(floor), Some(edge)) => match floor_dim(graph.edge_floor_relation(edge, floor)) {
                Some(dim) => dim,
                None => continue,
            },
            _ => 1.0,
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

        if let Some(shape) = &shapes[ci] {
            for piece in shape.floor_pieces() {
                let pts: Vec<egui::Pos2> = piece.iter()
                    .map(|&(x, y)| transform.world_to_screen(egui::pos2(x * GRID_PX, y * GRID_PX)))
                    .collect();
                painter.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
            }
            continue;
        }

        // Draw each segment as a filled rectangle on the grid.
        for (min_x, min_y, max_x, max_y) in corridor.run_boxes() {

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
                egui::Stroke::new(1.5_f32, stroke_color),
            ));
        } else {
            // Circle for mid-waypoints
            painter.circle(screen, handle_r, fill, egui::Stroke::new(1.5_f32, stroke_color));
        }
    }
}

/// Compute the default exit position for a room/connection when no exit is stored.
/// Returns the corridor center-line position on the room wall.
fn default_exit_pos(room_rl: &RoomLayout, other_rl: &RoomLayout, corridor_width: u32) -> ExitPos {
    if room_rl.is_rotated() {
        // The point of the turned perimeter facing the other room
        let (ox, oy) = other_rl.center();
        return snap_to_perimeter(egui::pos2(ox * GRID_PX, oy * GRID_PX), room_rl, corridor_width);
    }
    let (rcx, rcy) = room_rl.center();
    let (ocx, ocy) = other_rl.center();
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
    if room_rl.is_rotated() {
        // Snap in the room's own frame, then turn the exit back into place
        let local_rl = RoomLayout { x: 0, y: 0, rotation: 0.0, ..room_rl.clone() };
        let (lx, ly) = room_rl.to_local(world.x / GRID_PX, world.y / GRID_PX);
        let e = snap_to_perimeter(egui::pos2(lx * GRID_PX, ly * GRID_PX), &local_rl, corridor_width);
        let (x, y) = room_rl.to_world(e.x, e.y);
        return ExitPos { x, y };
    }
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
            egui::Stroke::new(1.5_f32, stroke_color),
        ));
    }
}

fn draw_rooms(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    state: &SpatialViewState,
    floor_color: [u8; 4],
) {
    // Rooms in z-order so containers render before children
    let room_order = layout.render_order(graph);
    for ri in room_order {
        let rl = &layout.rooms[ri];
        // Floor filtering: dim lower floors, hide higher floors
        let dim = match (state.current_floor, graph.room_by_id(&rl.room_id)) {
            (Some(floor), Some(room)) => match floor_dim(room.floor.relation(floor)) {
                Some(dim) => dim,
                None => continue,
            },
            _ => 1.0,
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
        let stroke = egui::Stroke::new(2.0_f32, stroke_color);

        if rl.is_rotated() {
            let cave_outline = if is_selected && state.cave_edit_mode {
                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(80, 180, 255))
            } else {
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(60, 60, 60, 80))
            };
            let label_color = if dim < 1.0 { dim_color(egui::Color32::from_rgb(30, 30, 30), dim) } else { egui::Color32::from_rgb(30, 30, 30) };
            draw_rotated_room(painter, transform, rl, room, fill, wall_fill, stroke, cave_outline, label_color);
            continue;
        }

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
                        let contour_stroke = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(40, 40, 40));
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
                // AABB border — highlighted when in cave edit mode
                let aabb_stroke = if is_selected && state.cave_edit_mode {
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(80, 180, 255))
                } else {
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(60, 60, 60, 80))
                };
                painter.rect_stroke(rect, 0.0, aabb_stroke, egui::StrokeKind::Middle);
            }
            RoomShape::Rectangle => {
                painter.rect_filled(rect, 0.0, fill);
                painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Middle);
            }
        }

        // Grid lines on all rooms
        {
            let grid_stroke = egui::Stroke::new(0.5_f32, egui::Color32::from_rgba_unmultiplied(80, 80, 80, 60));
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
                if section.opaque {
                    painter.rect_filled(sec_rect, 0.0, egui::Color32::from_rgba_unmultiplied(floor_color[0], floor_color[1], floor_color[2], floor_color[3]));
                }
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
                        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 200, 50)),
                        egui::StrokeKind::Middle);
                } else if !is_water {
                    painter.rect_stroke(sec_rect, 0.0,
                        egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(80, 80, 80)),
                        egui::StrokeKind::Middle);
                }

                // Tick marks for raised/lowered
                if tick_dir != 0.0 {
                    let tick_len = 3.0 * transform.zoom;
                    let spacing = 8.0 * transform.zoom;
                    let tick_color = egui::Color32::from_rgb(80, 80, 80);
                    let tick_stroke = egui::Stroke::new(1.0_f32, tick_color);

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
                    let step_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(80, 80, 80));
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
                    painter.rect_stroke(inset_rect, 0.0, egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(80, 80, 80)), egui::StrokeKind::Middle);
                }

                // Hole: diagonal cross
                if section.elevation == ElevationType::Hole {
                    let cross_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(80, 80, 80));
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
                    let wave_stroke = egui::Stroke::new(0.8_f32, wave_color);
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
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(60, 100, 170, 200)),
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
        let dim = match state.current_floor {
            Some(floor) => match floor_dim(graph.edge_floor_relation(edge, floor)) {
                Some(dim) => dim,
                None => continue,
            },
            None => 1.0,
        };
        let white = if dim < 1.0 { dim_color(egui::Color32::WHITE, dim) } else { egui::Color32::WHITE };
        let dark = if dim < 1.0 { dim_color(egui::Color32::from_rgb(30, 30, 30), dim) } else { egui::Color32::from_rgb(30, 30, 30) };
        if edge.connection.connection_type.is_passage() {
            continue;
        }

        let corridor = layout.corridor_for(&edge.connection.id);
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

        // For child-to-parent connections, only draw door on the child side
        let src_is_child_of_tgt = graph.parent_of(&edge.source_room_id)
            .map(|p| p == edge.target_room_id).unwrap_or(false);
        let tgt_is_child_of_src = graph.parent_of(&edge.target_room_id)
            .map(|p| p == edge.source_room_id).unwrap_or(false);

        for (i, ((room_id, wp), exit)) in room_ids.iter().zip(wp_ends.iter()).zip(exits.iter()).enumerate() {
            // Skip door on the parent side of child-to-parent connections
            if i == 1 && src_is_child_of_tgt { continue; } // target is parent
            if i == 0 && tgt_is_child_of_src { continue; } // source is parent

            let Some(rl) = layout.room_by_id(room_id) else { continue };

            let door_depth = 0.3_f32;
            if rl.is_rotated() {
                // Along the turned wall where the corridor attaches
                let attach = crate::model::geometry::corridor_shape(corridor, layout, graph)
                    .and_then(|sh| sh.ends.into_iter().flatten().find(|a| a.room_id == rl.room_id));
                if let Some(a) = attach {
                    let quad: Vec<egui::Pos2> = crate::model::geometry::door_quad(&a, dw_half * 2.0, door_depth).iter()
                        .map(|&(x, y)| transform.world_to_screen(egui::pos2(x * GRID_PX, y * GRID_PX)))
                        .collect();
                    let center = transform.world_to_screen(egui::pos2(a.point.0 * GRID_PX, a.point.1 * GRID_PX));
                    if edge.connection.connection_type == ConnectionType::Secret {
                        painter.text(center, egui::Align2::CENTER_CENTER, "S", egui::FontId::monospace((8.0 * transform.zoom).max(6.0)), dark);
                    } else {
                        painter.add(egui::Shape::convex_polygon(quad.clone(), white, egui::Stroke::NONE));
                        painter.add(egui::Shape::closed_line(quad, egui::Stroke::new(1.5_f32, dark)));
                        if edge.connection.connection_type == ConnectionType::Locked {
                            painter.circle_filled(center, 0.12 * GRID_PX * transform.zoom, dark);
                        }
                    }
                }
                continue;
            }
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
                ConnectionType::Open | ConnectionType::Flush | ConnectionType::Merge => {} // already skipped above
                ConnectionType::Door => {
                    painter.rect_filled(door_rect, 0.0, white);
                    painter.rect_stroke(door_rect, 0.0, egui::Stroke::new(1.5_f32, dark), egui::StrokeKind::Middle);
                }
                ConnectionType::Locked => {
                    painter.rect_filled(door_rect, 0.0, white);
                    painter.rect_stroke(door_rect, 0.0, egui::Stroke::new(1.5_f32, dark), egui::StrokeKind::Middle);
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
                    painter.rect_stroke(door_rect, 0.0, egui::Stroke::new(1.5_f32, dark), egui::StrokeKind::Middle);
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

        let mut rotation_changed = false;
        if let Some(layout) = &mut dungeon.layout {
            if let Some(rl) = layout.room_by_id_mut(&room_id) {
                ui.horizontal(|ui| {
                    ui.label("Position:");
                    crate::ui::canvas_common::num_input_i32(ui, &mut rl.x, 35.0);
                    crate::ui::canvas_common::num_input_i32(ui, &mut rl.y, 35.0);
                });
                // Size edits the room's own size (what the layout solver places) and its
                // footprint here together, so a re-solve keeps it.
                if let Some(room) = dungeon.graph.room_by_id_mut(&room_id) {
                    let (old_w, old_h) = (rl.width, rl.height);
                    ui.horizontal(|ui| {
                        ui.label("Size:");
                        egui::ComboBox::from_id_salt("spatial_size_hint")
                            .selected_text(if room.grid_width.is_some() || room.grid_height.is_some() { "Custom" } else { room.size_hint.label() })
                            .show_ui(ui, |ui| {
                                for hint in SizeHint::ALL {
                                    if ui.selectable_label(room.size_hint == hint && room.grid_width.is_none() && room.grid_height.is_none(), hint.label()).clicked() {
                                        room.size_hint = hint;
                                        room.grid_width = None;
                                        room.grid_height = None;
                                        (rl.width, rl.height) = hint.grid_size();
                                    }
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("W:");
                        if crate::ui::canvas_common::num_input_u32(ui, &mut rl.width, 35.0) {
                            rl.width = rl.width.max(1);
                            room.grid_width = Some(rl.width);
                        }
                        ui.label("L:");
                        if crate::ui::canvas_common::num_input_u32(ui, &mut rl.height, 35.0) {
                            rl.height = rl.height.max(1);
                            room.grid_height = Some(rl.height);
                        }
                    });
                    // Cave cells are laid out for the old footprint; regenerate them
                    if (rl.width, rl.height) != (old_w, old_h) {
                        if let Some(cave) = room.cave_data.as_mut() {
                            cave.cells.clear();
                        }
                    }
                }
                ui.label(format!("{}x{} ft", rl.width * 5, rl.height * 5));
                ui.horizontal(|ui| {
                    ui.label("Rotation:");
                    let before = rl.rotation;
                    ui.add(egui::DragValue::new(&mut rl.rotation).speed(1.0).suffix("°").range(-180.0..=180.0));
                    if ui.small_button("-15°").clicked() { rl.rotation -= 15.0; }
                    if ui.small_button("+15°").clicked() { rl.rotation += 15.0; }
                    if ui.small_button("0°").on_hover_text("Clear rotation").clicked() { rl.rotation = 0.0; }
                    rl.rotation = (rl.rotation + 180.0).rem_euclid(360.0) - 180.0;
                    rotation_changed = rl.rotation != before;
                });
                ui.weak("Drag the handle above the room to turn it (Shift snaps to 15°).");
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

        if rotation_changed {
            reroute_room(dungeon, &room_id);
            state.cave_contours_dirty = true;
        }

        // Cave edit mode toggle
        if let Some(room) = dungeon.graph.room_by_id(&room_id) {
            if room.shape == RoomShape::Cave && room.cave_data.as_ref().is_some_and(|c| !c.cells.is_empty()) {
                ui.add_space(8.0);
                ui.separator();
                let label = if state.cave_edit_mode { "Stop Editing Cave" } else { "Edit Cave Cells" };
                if ui.button(label).clicked() {
                    state.cave_edit_mode = !state.cave_edit_mode;
                }
                if state.cave_edit_mode {
                    ui.label("Left-click/drag digs floor. Right-click/drag fills wall.");
                }
            }
        }

        // Connections from this room; overlapping rooms get a wall picker
        let connections: Vec<_> = dungeon.graph.connections.iter()
            .filter(|e| e.source_room_id == room_id || e.target_room_id == room_id)
            .map(|e| {
                let other_id = if e.source_room_id == room_id { &e.target_room_id } else { &e.source_room_id };
                let other_label = dungeon.graph.room_by_id(other_id)
                    .map(|r| r.label.as_str()).unwrap_or("?");
                let overlap = crate::ui::sidebar::overlapping_room_labels(dungeon, &e.connection.id);
                (e.connection.id.clone(), e.connection.connection_type.label(), other_label.to_string(), overlap)
            })
            .collect();
        if !connections.is_empty() {
            ui.add_space(8.0);
            ui.label("Connections:");
            for (conn_id, conn_type, other, overlap) in connections {
                ui.label(format!("  {} \u{2192} {}", conn_type, other));
                if let Some(edge) = dungeon.graph.connection_by_id_mut(&conn_id) {
                    ui.indent(("angle", &conn_id), |ui| {
                        crate::ui::sidebar::corridor_angle_picker(ui, &format!("corridor_angle_{conn_id}"), &mut edge.connection.corridor_angle);
                    });
                }
                if let Some((source, target)) = overlap {
                    if let Some(edge) = dungeon.graph.connection_by_id_mut(&conn_id) {
                        ui.indent(("overlap", &conn_id), |ui| {
                            crate::ui::sidebar::overlap_walls_picker(
                                ui, &format!("overlap_walls_{conn_id}"), &mut edge.connection.overlap_walls, &source, &target,
                            );
                        });
                    }
                }
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
            if !room.note_excerpt.is_empty() {
                ui.add_space(4.0);
                ui.label("Notes:");
                ui.label(&room.note_excerpt);
                ui.weak("Full note in the drawer below (F9).");
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

        // List sections (drag to reorder)
        let mut remove_section = None;
        let mut reorder: Option<(usize, usize)> = None;
        {
            let section_info: Vec<(usize, String, ElevationType)> = dungeon.graph.room_by_id(&room_id)
                .map(|r| r.sections.iter().enumerate()
                    .map(|(i, s)| (i, s.id.clone(), s.elevation))
                    .collect())
                .unwrap_or_default();

            for (si, id, elev) in &section_info {
                let is_sel = state.selected_section.as_ref()
                    .is_some_and(|(rid, idx)| rid == &room_id && *idx == *si);
                let row_id = egui::Id::new(("section_row", id));
                let resp = ui.horizontal(|ui| {
                    // Drag handle
                    let handle = ui.label("≡");
                    let drag_resp = ui.interact(handle.rect, row_id, egui::Sense::drag());
                    if drag_resp.dragged() {
                        // Show drag cursor
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    }
                    if drag_resp.drag_stopped() {
                        // Find target index from pointer position
                        if let Some(pos) = ui.ctx().pointer_latest_pos() {
                            // Walk the section list to find which row the pointer is over
                            // We use a simple heuristic: the row heights are uniform
                            let row_top = handle.rect.top();
                            let row_h = handle.rect.height() + ui.spacing().item_spacing.y;
                            let delta_rows = ((pos.y - row_top) / row_h).round() as i32;
                            let target = (*si as i32 + delta_rows).clamp(0, section_info.len() as i32 - 1) as usize;
                            if target != *si {
                                reorder = Some((*si, target));
                            }
                        }
                    }
                    if ui.selectable_label(is_sel, elev.label()).clicked() {
                        state.selected_section = Some((room_id.clone(), *si));
                    }
                    if ui.small_button("X").clicked() {
                        remove_section = Some(*si);
                    }
                });
                // Drop target indicator
                if let Some((_from, _to)) = reorder {
                    let _ = resp;
                }
            }
        }

        if let Some((from, to)) = reorder {
            if let Some(room) = dungeon.graph.room_by_id_mut(&room_id) {
                if from < room.sections.len() && to < room.sections.len() {
                    let item = room.sections.remove(from);
                    room.sections.insert(to, item);
                    // Update selection to follow the moved item
                    if let Some((ref rid, ref mut idx)) = state.selected_section {
                        if rid == &room_id {
                            if *idx == from {
                                *idx = to;
                            } else if from < to && *idx > from && *idx <= to {
                                *idx -= 1;
                            } else if from > to && *idx >= to && *idx < from {
                                *idx += 1;
                            }
                        }
                    }
                }
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
                        ui.checkbox(&mut section.opaque, "Opaque");
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
            // The clone is a new room with no note of its own yet.
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
                wall_openings: Vec::new(),
                rotation: rl.rotation,
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
            let old_corridor = layout.corridor_for(&old_edge.connection.id)?;
            Some(CorridorSegment {
                connection_id: new_conn_id,
                waypoints: old_corridor.waypoints.iter().map(|wp| GridPos { x: wp.x + offset_x, y: wp.y }).collect(),
                width: old_corridor.width,
                invalid: false,
                pinned_waypoints: Vec::new(),
                floor: old_corridor.floor,
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
    // 90° about the group's center; room footprints swap width and height
    transform_group(dungeon, room_ids, true, |(cx, cy), (x, y)| (cx + (y - cy), cy - (x - cx)));
}

fn flip_group(dungeon: &mut Dungeon, room_ids: &[String], horizontal: bool) {
    transform_group(dungeon, room_ids, false, move |(cx, cy), (x, y)| {
        if horizontal { (2.0 * cx - x, y) } else { (x, 2.0 * cy - y) }
    });
}

/// Move a group of rooms by `xf` (given the group's center and a point): rooms by
/// their centers, plus the waypoints and pins of corridors inside the group and the
/// exits of every connection touching it. `quarter_turn` swaps room footprints;
/// otherwise `xf` is a mirror, which reverses each room's rotation.
fn transform_group(
    dungeon: &mut Dungeon,
    room_ids: &[String],
    quarter_turn: bool,
    xf: impl Fn((f32, f32), (f32, f32)) -> (f32, f32),
) {
    let Some(layout) = &mut dungeon.layout else { return };
    let centers: Vec<(f32, f32)> = room_ids.iter().filter_map(|rid| layout.room_by_id(rid)).map(|rl| rl.center()).collect();
    if centers.is_empty() {
        return;
    }
    let n = centers.len() as f32;
    let center = (centers.iter().map(|c| c.0).sum::<f32>() / n, centers.iter().map(|c| c.1).sum::<f32>() / n);

    for rid in room_ids {
        if let Some(rl) = layout.room_by_id_mut(rid) {
            let (new_cx, new_cy) = xf(center, rl.center());
            if quarter_turn {
                std::mem::swap(&mut rl.width, &mut rl.height);
            } else {
                // A mirror turns the other way
                rl.rotation = -rl.rotation;
            }
            rl.x = (new_cx - rl.width as f32 / 2.0).round() as i32;
            rl.y = (new_cy - rl.height as f32 / 2.0).round() as i32;
        }
    }

    let in_group: std::collections::HashSet<&String> = room_ids.iter().collect();
    let move_grid = |p: &mut GridPos| {
        let (x, y) = xf(center, (p.x as f32, p.y as f32));
        (p.x, p.y) = (x.round() as i32, y.round() as i32);
    };
    for corridor in &mut layout.corridors {
        let Some(edge) = dungeon.graph.connection_by_id(&corridor.connection_id) else { continue };
        if !(in_group.contains(&edge.source_room_id) && in_group.contains(&edge.target_room_id)) {
            continue;
        }
        corridor.waypoints.iter_mut().for_each(move_grid);
        corridor.pinned_waypoints.iter_mut().for_each(move_grid);
        if edge.connection.corridor_angle == CorridorAngle::Orthogonal {
            resolve_diagonal_segments_clean(&mut corridor.waypoints);
        }
    }
    // Exits sit on a room's wall: move them with the room, then snap back onto the wall
    for edge in &mut dungeon.graph.connections {
        let cw = edge.connection.corridor_width;
        for (room_id, exit) in [(&edge.source_room_id, &mut edge.source_exit), (&edge.target_room_id, &mut edge.target_exit)] {
            let (Some(e), Some(rl)) = (exit.as_mut(), layout.room_by_id(room_id).filter(|_| in_group.contains(room_id))) else { continue };
            let (x, y) = xf(center, (e.x, e.y));
            *e = snap_to_perimeter(egui::pos2(x * GRID_PX, y * GRID_PX), rl, cw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flipping_a_group_moves_pins_and_exits_with_it() {
        let mut d = Dungeon::new("t".into());
        let (a, b) = (Room::new("A".into()), Room::new("B".into()));
        let (aid, bid) = (a.id.clone(), b.id.clone());
        d.graph.add_room(a);
        d.graph.add_room(b);
        d.graph.add_connection(aid.clone(), bid.clone(), Connection::new(ConnectionType::Door));
        d.graph.connections[0].source_exit = Some(ExitPos { x: 4.0, y: 2.0 });
        let rl = |id: &str, x: i32| RoomLayout { room_id: id.into(), x, y: 0, width: 4, height: 4, violations: Vec::new(), wall_openings: Vec::new(), rotation: 0.0 };
        let mut layout = SpatialLayout::new();
        layout.rooms = vec![rl(&aid, 0), rl(&bid, 10)];
        let pins = vec![GridPos { x: 4, y: 2 }, GridPos { x: 7, y: 2 }, GridPos { x: 10, y: 2 }];
        layout.corridors = vec![CorridorSegment {
            connection_id: d.graph.connections[0].connection.id.clone(),
            waypoints: pins.clone(), width: 2, invalid: false, pinned_waypoints: pins, floor: FloorAssignment::default(),
        }];
        d.layout = Some(layout);
        flip_group(&mut d, &[aid.clone(), bid.clone()], true);
        let layout = d.layout.as_ref().unwrap();
        // Mirrored about x = 7: A lands on the right, and the pins run the other way
        assert_eq!(layout.room_by_id(&aid).unwrap().x, 10);
        let xs: Vec<i32> = layout.corridors[0].pinned_waypoints.iter().map(|p| p.x).collect();
        assert_eq!(xs, vec![10, 7, 4]);
        // A's exit was on its east wall; it is now on the moved room's west wall
        let e = d.graph.connections[0].source_exit.unwrap();
        assert_eq!((e.x, e.y), (10.0, 2.0));
    }

    #[test]
    fn grid_line_is_gapless_and_inclusive() {
        assert_eq!(grid_line((2, 2), (2, 2)), vec![(2, 2)]);
        assert_eq!(grid_line((0, 0), (3, 0)), vec![(0, 0), (1, 0), (2, 0), (3, 0)]);
        for (a, b) in [((0, 0), (5, 2)), ((4, 7), (-3, 1)), ((0, 0), (0, -4))] {
            let line = grid_line(a, b);
            assert_eq!(line.first(), Some(&a));
            assert_eq!(line.last(), Some(&b));
            // Each step moves to an edge-adjacent cell, so a dug stroke is walkable
            for w in line.windows(2) {
                assert_eq!((w[1].0 - w[0].0).abs() + (w[1].1 - w[0].1).abs(), 1, "{a:?}->{b:?}: {line:?}");
            }
        }
    }
}
