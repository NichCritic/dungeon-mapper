use std::collections::{HashSet, VecDeque};

use crate::model::*;

/// An axis-aligned rectangle on the grid.
#[derive(Clone, Copy)]
struct GridRect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

/// A placed rectangle with its floor assignment, used for floor-aware overlap checks.
#[derive(Clone, Copy)]
struct PlacedRect {
    rect: GridRect,
    floor: FloorAssignment,
}

/// Mutable state accumulated during placement.
struct PlacementState {
    layout: SpatialLayout,
    placed: HashSet<String>,
    placed_rects: Vec<PlacedRect>,
    placed_rooms: Vec<(String, GridRect)>,
}

impl PlacementState {
    fn new() -> Self {
        Self {
            layout: SpatialLayout::new(),
            placed: HashSet::new(),
            placed_rects: Vec::new(),
            placed_rooms: Vec::new(),
        }
    }

    fn place_room(&mut self, room_id: &str, rect: GridRect, floor: FloorAssignment) {
        self.layout.rooms.push(RoomLayout {
            room_id: room_id.to_string(),
            x: rect.x,
            y: rect.y,
            width: rect.w,
            height: rect.h,
            violations: Vec::new(),
        });
        self.placed.insert(room_id.to_string());
        self.placed_rects.push(PlacedRect { rect, floor });
        self.placed_rooms.push((room_id.to_string(), rect));
    }
}

/// Immutable context for constraint checking during placement.
struct PlacementContext<'a> {
    gap: u32,
    groups: &'a [RoomGroup],
    connections: &'a [StoredEdge],
    graph: &'a DungeonGraph,
}

/// Check if placing a room would violate any group constraint.
fn violates_group_constraints(
    room_id: &str,
    rect: GridRect,
    groups: &[RoomGroup],
    placed_rooms: &[(String, GridRect)],
) -> bool {
    for group in groups {
        if !group.room_ids.contains(&room_id.to_string()) {
            continue;
        }
        if group.max_width.is_none() && group.max_height.is_none() {
            continue;
        }

        let mut min_x = rect.x;
        let mut min_y = rect.y;
        let mut max_x = rect.x + rect.w as i32;
        let mut max_y = rect.y + rect.h as i32;

        for (pid, pr) in placed_rooms {
            if group.room_ids.contains(pid) {
                min_x = min_x.min(pr.x);
                min_y = min_y.min(pr.y);
                max_x = max_x.max(pr.x + pr.w as i32);
                max_y = max_y.max(pr.y + pr.h as i32);
            }
        }

        if let Some(mw) = group.max_width {
            if (max_x - min_x) as u32 > mw {
                return true;
            }
        }
        if let Some(mh) = group.max_height {
            if (max_y - min_y) as u32 > mh {
                return true;
            }
        }
    }
    false
}

/// Manhattan edge-to-edge distance between two rooms.
/// For each axis, the gap is 0 if the rooms overlap on that axis, otherwise
/// it's the distance between the closest edges.
fn edge_to_edge_manhattan(a: GridRect, b: GridRect) -> u32 {
    let gap_x = if a.x + a.w as i32 <= b.x {
        (b.x - (a.x + a.w as i32)) as u32
    } else if b.x + b.w as i32 <= a.x {
        (a.x - (b.x + b.w as i32)) as u32
    } else {
        0 // overlapping on x axis
    };
    let gap_y = if a.y + a.h as i32 <= b.y {
        (b.y - (a.y + a.h as i32)) as u32
    } else if b.y + b.h as i32 <= a.y {
        (a.y - (b.y + b.h as i32)) as u32
    } else {
        0 // overlapping on y axis
    };
    gap_x + gap_y
}

/// Check if placing a room violates any connection length constraint.
fn violates_length_constraints(
    room_id: &str,
    rect: GridRect,
    connections: &[StoredEdge],
    placed_rooms: &[(String, GridRect)],
) -> bool {
    for edge in connections {
        let other_id = if edge.source_room_id == room_id {
            &edge.target_room_id
        } else if edge.target_room_id == room_id {
            &edge.source_room_id
        } else {
            continue;
        };

        let Some((_, other_rect)) = placed_rooms.iter().find(|(id, _)| id == other_id) else {
            continue;
        };

        let dist = edge_to_edge_manhattan(rect, *other_rect);

        if let Some(min) = edge.connection.min_length {
            if dist < min {
                return true;
            }
        }
        if let Some(max) = edge.connection.max_length {
            if dist > max {
                return true;
            }
        }
    }
    false
}

fn try_place(rect: GridRect, room_id: &str, floor: FloorAssignment, state: &PlacementState, ctx: &PlacementContext) -> bool {
    !overlaps_any(rect, floor, &state.placed_rects, ctx.gap)
        && !violates_group_constraints(room_id, rect, ctx.groups, &state.placed_rooms)
        && !violates_length_constraints(room_id, rect, ctx.connections, &state.placed_rooms)
}

/// Try placement with only overlap checking (no constraint checks).
fn try_place_unconstrained(rect: GridRect, floor: FloorAssignment, placed_rects: &[PlacedRect], gap: u32) -> bool {
    !overlaps_any(rect, floor, placed_rects, gap)
}

/// Collect violation descriptions for a room placement.
fn collect_violations(room_id: &str, rect: GridRect, state: &PlacementState, ctx: &PlacementContext) -> Vec<String> {
    let mut violations = Vec::new();

    for edge in ctx.connections {
        let other_id = if edge.source_room_id == room_id {
            &edge.target_room_id
        } else if edge.target_room_id == room_id {
            &edge.source_room_id
        } else {
            continue;
        };

        let Some((_, other_rect)) = state.placed_rooms.iter().find(|(id, _)| id == other_id) else {
            continue;
        };

        let dist = edge_to_edge_manhattan(rect, *other_rect);
        let other_label = ctx.graph.room_by_id(other_id).map(|r| r.label.as_str()).unwrap_or("?");

        if let Some(min) = edge.connection.min_length {
            if dist < min {
                violations.push(format!("Too close to {} ({} < min {})", other_label, dist, min));
            }
        }
        if let Some(max) = edge.connection.max_length {
            if dist > max {
                violations.push(format!("Too far from {} ({} > max {})", other_label, dist, max));
            }
        }
    }

    // Group constraints
    for group in ctx.groups {
        if !group.room_ids.contains(&room_id.to_string()) {
            continue;
        }
        let mut min_x = rect.x;
        let mut min_y = rect.y;
        let mut max_x = rect.x + rect.w as i32;
        let mut max_y = rect.y + rect.h as i32;
        for (pid, pr) in &state.placed_rooms {
            if group.room_ids.contains(pid) {
                min_x = min_x.min(pr.x);
                min_y = min_y.min(pr.y);
                max_x = max_x.max(pr.x + pr.w as i32);
                max_y = max_y.max(pr.y + pr.h as i32);
            }
        }
        let bbox_w = (max_x - min_x) as u32;
        let bbox_h = (max_y - min_y) as u32;
        if let Some(mw) = group.max_width {
            if bbox_w > mw {
                violations.push(format!("Group '{}' width {} > max {}", group.label, bbox_w, mw));
            }
        }
        if let Some(mh) = group.max_height {
            if bbox_h > mh {
                violations.push(format!("Group '{}' height {} > max {}", group.label, bbox_h, mh));
            }
        }
    }

    violations
}

/// Sort candidate positions by distance to a preferred position.
fn sort_by_preference(candidates: &mut [(i32, i32)], pref_x: i32, pref_y: i32) {
    candidates.sort_by_key(|&(x, y)| (x - pref_x).abs() + (y - pref_y).abs());
}

/// BFS greedy placer. Uses graph view positions as hints for relative placement.
pub fn solve_layout(
    graph: &DungeonGraph,
    gap: u32,
) -> Result<SpatialLayout, String> {
    if graph.rooms.is_empty() {
        return Err("No rooms to layout".to_string());
    }

    // Convert graph view positions to grid-scale hints.
    // Scale factor: graph positions are in ~pixels (NODE_WIDTH=120),
    // grid positions are in grid squares. We normalize relative to the entrance.
    let graph_pos = &graph.graph_positions;
    let scale = 0.05_f32; // rough conversion from graph pixels to grid squares

    let (pg, node_map) = graph.build_petgraph();
    let mut state = PlacementState::new();
    let ctx = PlacementContext {
        gap,
        groups: &graph.groups,
        connections: &graph.connections,
        graph,
    };

    // Find entrance room
    let entrance = graph.rooms.iter()
        .find(|r| r.tags.contains(&RoomTag::Entrance))
        .unwrap_or(&graph.rooms[0]);

    let (ew, eh) = entrance.grid_size();
    let entrance_graph_pos = graph_pos.get(&entrance.id).copied().unwrap_or((0.0, 0.0));

    state.place_room(&entrance.id, GridRect { x: 0, y: 0, w: ew, h: eh }, entrance.floor);

    let mut queue = VecDeque::new();
    if let Some(&start_idx) = node_map.get(&entrance.id) {
        queue.push_back(start_idx);
    }

    loop {
        while let Some(current) = queue.pop_front() {
            let current_id = &pg[current];
            let current_layout = state.layout.room_by_id(current_id).unwrap();
            let cx = current_layout.x;
            let cy = current_layout.y;
            let cw = current_layout.width;
            let ch = current_layout.height;

            for neighbor_idx in pg.neighbors(current) {
                let neighbor_id = &pg[neighbor_idx];
                if state.placed.contains(neighbor_id) {
                    continue;
                }

                let neighbor_room = graph.room_by_id(neighbor_id).unwrap();
                let neighbor_floor = neighbor_room.floor;
                let (nw, nh) = neighbor_room.grid_size();

                let mut orientations = vec![(nw, nh)];
                if neighbor_room.allow_rotation && nw != nh {
                    orientations.push((nh, nw));
                }

                let cw_i = graph.connections.iter()
                    .filter(|e| {
                        (e.source_room_id == *current_id && e.target_room_id == *neighbor_id)
                        || (e.target_room_id == *current_id && e.source_room_id == *neighbor_id)
                    })
                    .map(|e| e.connection.corridor_width as i32)
                    .max()
                    .unwrap_or(2);
                let g = gap as i32;

                // Compute preferred position from graph view hint
                let (pref_x, pref_y) = if let Some(&(nx, ny)) = graph_pos.get(neighbor_id.as_str()) {
                    let dx = (nx - entrance_graph_pos.0) * scale;
                    let dy = (ny - entrance_graph_pos.1) * scale;
                    (dx.round() as i32, dy.round() as i32)
                } else {
                    (cx + cw as i32, cy) // default: to the right
                };

                let mut did_place = false;
                'orient: for &(tw, th) in &orientations {
                    // Build all candidate positions
                    let mut adjacent = vec![
                        (cx + cw as i32, cy),
                        (cx, cy + ch as i32),
                        (cx - tw as i32, cy),
                        (cx, cy - th as i32),
                    ];
                    // Sort adjacent by closeness to graph hint
                    sort_by_preference(&mut adjacent, pref_x, pref_y);

                    let mut spaced = vec![
                        (cx + cw as i32 + g + cw_i, cy),
                        (cx, cy + ch as i32 + g + cw_i),
                        (cx - tw as i32 - g - cw_i, cy),
                        (cx, cy - th as i32 - g - cw_i),
                    ];
                    sort_by_preference(&mut spaced, pref_x, pref_y);

                    // Try adjacent only when gap is 0
                    if g == 0 {
                        for &(px, py) in &adjacent {
                            let rect = GridRect { x: px, y: py, w: tw, h: th };
                            if try_place(rect, neighbor_id, neighbor_floor, &state, &ctx) {
                                state.place_room(neighbor_id, rect, neighbor_floor);
                                queue.push_back(neighbor_idx);
                                did_place = true;
                                break 'orient;
                            }
                        }
                    }

                    // Then try spaced
                    for &(px, py) in &spaced {
                        let rect = GridRect { x: px, y: py, w: tw, h: th };
                        if try_place(rect, neighbor_id, neighbor_floor, &state, &ctx) {
                            state.place_room(neighbor_id, rect, neighbor_floor);
                            queue.push_back(neighbor_idx);
                            did_place = true;
                            break 'orient;
                        }
                    }
                }

                // Fallback: try further out
                if !did_place {
                    let (tw, th) = orientations[0];
                    'outer: for om in 2..=10 {
                        let mut extra = vec![
                            (cx + (cw as i32 + g + cw_i) * om, cy),
                            (cx, cy + (ch as i32 + g + cw_i) * om),
                            (cx - (tw as i32 + g + cw_i) * om, cy),
                            (cx, cy - (th as i32 + g + cw_i) * om),
                        ];
                        sort_by_preference(&mut extra, pref_x, pref_y);
                        for &(px, py) in &extra {
                            let rect = GridRect { x: px, y: py, w: tw, h: th };
                            if try_place(rect, neighbor_id, neighbor_floor, &state, &ctx) {
                                state.place_room(neighbor_id, rect, neighbor_floor);
                                queue.push_back(neighbor_idx);
                                did_place = true;
                                break 'outer;
                            }
                        }
                    }
                }

                // If constrained placement failed, try unconstrained and record violations
                if !did_place {
                    let (tw, th) = orientations[0];
                    let fallback_candidates = vec![
                        (cx + cw as i32 + g + cw_i, cy),
                        (cx, cy + ch as i32 + g + cw_i),
                        (cx - tw as i32 - g - cw_i, cy),
                        (cx, cy - th as i32 - g - cw_i),
                    ];
                    for &(px, py) in &fallback_candidates {
                        let rect = GridRect { x: px, y: py, w: tw, h: th };
                        if try_place_unconstrained(rect, neighbor_floor, &state.placed_rects, gap) {
                            let v = collect_violations(neighbor_id, rect, &state, &ctx);
                            state.place_room(neighbor_id, rect, neighbor_floor);
                            if let Some(room_layout) = state.layout.rooms.last_mut() {
                                room_layout.violations = v;
                            }
                            queue.push_back(neighbor_idx);
                            did_place = true;
                            break;
                        }
                    }
                }

                if !did_place {
                    eprintln!("Warning: Could not place room '{}'", neighbor_room.label);
                }
            }
        }

        // Handle disconnected components
        let unplaced_room = graph.rooms.iter().find(|r| !state.placed.contains(&r.id));
        match unplaced_room {
            Some(room) => {
                let (nw, nh) = room.grid_size();
                let mut did_place = false;
                let step = (nh + gap).max(1) as i32;
                'scan: for ring in 0..50 {
                    for sy in (-ring * step..=ring * step).step_by(step as usize) {
                        for sx in (-ring * step..=ring * step).step_by((nw + gap).max(1) as usize) {
                            let rect = GridRect { x: sx, y: sy, w: nw, h: nh };
                            if try_place(rect, &room.id, room.floor, &state, &ctx) {
                                state.place_room(&room.id, rect, room.floor);
                                if let Some(&idx) = node_map.get(&room.id) {
                                    queue.push_back(idx);
                                }
                                did_place = true;
                                break 'scan;
                            }
                        }
                    }
                }
                if !did_place {
                    eprintln!("Warning: Could not place room '{}'", room.label);
                    state.placed.insert(room.id.clone());
                }
            }
            None => break,
        }
    }

    state.layout.corridors = crate::solver::corridor::route_corridors(graph, &state.layout);

    Ok(state.layout)
}

/// Incremental layout update: only places new rooms and routes new corridors.
/// Rooms already present in the layout keep their positions.
/// Removed rooms/connections are cleaned up.
pub fn solve_incremental(
    graph: &DungeonGraph,
    existing: &SpatialLayout,
    gap: u32,
) -> Result<SpatialLayout, String> {
    if graph.rooms.is_empty() {
        return Ok(SpatialLayout {
            rooms: Vec::new(),
            corridors: Vec::new(),
            bounds: existing.bounds.clone(),
        });
    }

    let existing_room_ids: HashSet<String> = existing.rooms.iter()
        .map(|rl| rl.room_id.clone())
        .collect();
    let graph_room_ids: HashSet<String> = graph.rooms.iter()
        .map(|r| r.id.clone())
        .collect();

    // Keep rooms that still exist in the graph
    let mut layout = SpatialLayout {
        rooms: existing.rooms.iter()
            .filter(|rl| graph_room_ids.contains(&rl.room_id))
            .cloned()
            .collect(),
        corridors: Vec::new(), // corridors will be re-routed
        bounds: existing.bounds.clone(),
    };

    // Find new rooms that need placement
    let new_room_ids: Vec<String> = graph.rooms.iter()
        .filter(|r| !existing_room_ids.contains(&r.id))
        .map(|r| r.id.clone())
        .collect();

    if !new_room_ids.is_empty() {
        // Build placement state from existing rooms
        let mut placed: HashSet<String> = layout.rooms.iter()
            .map(|rl| rl.room_id.clone())
            .collect();
        let mut placed_rects: Vec<PlacedRect> = layout.rooms.iter()
            .map(|rl| {
                let floor = graph.room_by_id(&rl.room_id)
                    .map(|r| r.floor)
                    .unwrap_or_default();
                PlacedRect {
                    rect: GridRect { x: rl.x, y: rl.y, w: rl.width, h: rl.height },
                    floor,
                }
            })
            .collect();
        let placed_rooms: Vec<(String, GridRect)> = layout.rooms.iter()
            .map(|rl| (rl.room_id.clone(), GridRect { x: rl.x, y: rl.y, w: rl.width, h: rl.height }))
            .collect();

        let ctx = PlacementContext {
            gap,
            groups: &graph.groups,
            connections: &graph.connections,
            graph,
        };

        // Graph positions for hints
        let graph_pos = &graph.graph_positions;
        let scale = 0.05_f32;
        let entrance = graph.rooms.iter()
            .find(|r| r.tags.contains(&RoomTag::Entrance))
            .unwrap_or(&graph.rooms[0]);
        let entrance_graph_pos = graph_pos.get(&entrance.id).copied().unwrap_or((0.0, 0.0));

        for room_id in &new_room_ids {
            let room = graph.room_by_id(room_id).unwrap();
            let (nw, nh) = room.grid_size();

            // Find a placed neighbor to anchor placement
            let anchor = graph.connections.iter()
                .filter_map(|e| {
                    let neighbor = if e.source_room_id == *room_id {
                        &e.target_room_id
                    } else if e.target_room_id == *room_id {
                        &e.source_room_id
                    } else {
                        return None;
                    };
                    layout.room_by_id(neighbor).map(|rl| (rl, e.connection.corridor_width as i32))
                })
                .next();

            let (cx, cy, cw, ch, cw_i) = if let Some((anchor_rl, corridor_w)) = anchor {
                (anchor_rl.x, anchor_rl.y, anchor_rl.width, anchor_rl.height, corridor_w)
            } else {
                // No placed neighbor — place near origin
                (0, 0, 4, 4, 2)
            };

            let g = gap as i32;

            // Compute preferred position from graph hint
            let (pref_x, pref_y) = if let Some(&(nx, ny)) = graph_pos.get(room_id.as_str()) {
                let dx = (nx - entrance_graph_pos.0) * scale;
                let dy = (ny - entrance_graph_pos.1) * scale;
                (dx.round() as i32, dy.round() as i32)
            } else {
                (cx + cw as i32, cy)
            };

            // Build a temporary PlacementState for constraint checking
            let mut all_placed_rooms = placed_rooms.clone();
            for rl in &layout.rooms {
                if !all_placed_rooms.iter().any(|pr| pr.0 == rl.room_id) {
                    all_placed_rooms.push((rl.room_id.clone(), GridRect { x: rl.x, y: rl.y, w: rl.width, h: rl.height }));
                }
            }
            let temp_state = PlacementState {
                layout: SpatialLayout::new(),
                placed: placed.clone(),
                placed_rects: placed_rects.clone(),
                placed_rooms: all_placed_rooms,
            };

            let mut did_place = false;
            let orientations = if room.allow_rotation && nw != nh {
                vec![(nw, nh), (nh, nw)]
            } else {
                vec![(nw, nh)]
            };

            'orient: for &(tw, th) in &orientations {
                let mut candidates = Vec::new();
                if g == 0 {
                    candidates.extend_from_slice(&[
                        (cx + cw as i32, cy),
                        (cx, cy + ch as i32),
                        (cx - tw as i32, cy),
                        (cx, cy - th as i32),
                    ]);
                }
                candidates.extend_from_slice(&[
                    (cx + cw as i32 + g + cw_i, cy),
                    (cx, cy + ch as i32 + g + cw_i),
                    (cx - tw as i32 - g - cw_i, cy),
                    (cx, cy - th as i32 - g - cw_i),
                ]);
                sort_by_preference(&mut candidates, pref_x, pref_y);

                for &(px, py) in &candidates {
                    let rect = GridRect { x: px, y: py, w: tw, h: th };
                    if try_place(rect, room_id, room.floor, &temp_state, &ctx) {
                        layout.rooms.push(RoomLayout {
                            room_id: room_id.clone(),
                            x: px,
                            y: py,
                            width: tw,
                            height: th,
                            violations: Vec::new(),
                        });
                        placed.insert(room_id.clone());
                        placed_rects.push(PlacedRect { rect, floor: room.floor });
                        did_place = true;
                        break 'orient;
                    }
                }
            }

            // Fallback: try further out
            if !did_place {
                let (tw, th) = orientations[0];
                'far: for om in 2..=10 {
                    let mut extra = vec![
                        (cx + (cw as i32 + g + cw_i) * om, cy),
                        (cx, cy + (ch as i32 + g + cw_i) * om),
                        (cx - (tw as i32 + g + cw_i) * om, cy),
                        (cx, cy - (th as i32 + g + cw_i) * om),
                    ];
                    sort_by_preference(&mut extra, pref_x, pref_y);
                    for &(px, py) in &extra {
                        let rect = GridRect { x: px, y: py, w: tw, h: th };
                        if !overlaps_any(rect, room.floor, &placed_rects, gap) {
                            layout.rooms.push(RoomLayout {
                                room_id: room_id.clone(),
                                x: px,
                                y: py,
                                width: tw,
                                height: th,
                                violations: Vec::new(),
                            });
                            placed.insert(room_id.clone());
                            placed_rects.push(PlacedRect { rect, floor: room.floor });
                            did_place = true;
                            break 'far;
                        }
                    }
                }
            }

            if !did_place {
                eprintln!("Warning: Could not incrementally place room '{}'", room.label);
            }
        }
    }

    // Route all corridors (re-route is cheap compared to placement)
    layout.corridors = crate::solver::corridor::route_corridors(graph, &layout);

    Ok(layout)
}

/// Check if a rect overlaps any placed rect that shares at least one floor.
fn overlaps_any(rect: GridRect, floor: FloorAssignment, placed: &[PlacedRect], gap: u32) -> bool {
    let g = gap as i32;
    let floors = floor.floors();
    for pr in placed {
        // Skip overlap check if rooms are on entirely different floors
        if !pr.floor.floors().iter().any(|f| floors.contains(f)) {
            continue;
        }
        let r = &pr.rect;
        if rect.x < r.x + r.w as i32 + g
            && rect.x + rect.w as i32 + g > r.x
            && rect.y < r.y + r.h as i32 + g
            && rect.y + rect.h as i32 + g > r.y
        {
            return true;
        }
    }
    false
}
