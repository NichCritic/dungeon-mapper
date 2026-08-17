use std::collections::{HashMap, HashSet};

use crate::model::*;

/// Compute the floor assignment for a corridor by inheriting from its rooms.
/// The corridor belongs to the union of both rooms' floors.
fn corridor_floor(graph: &DungeonGraph, edge: &StoredEdge) -> FloorAssignment {
    let src_floor = graph.room_by_id(&edge.source_room_id)
        .map(|r| r.floor)
        .unwrap_or_default();
    let tgt_floor = graph.room_by_id(&edge.target_room_id)
        .map(|r| r.floor)
        .unwrap_or_default();

    let mut all_floors: Vec<i32> = src_floor.floors();
    for f in tgt_floor.floors() {
        if !all_floors.contains(&f) {
            all_floors.push(f);
        }
    }
    all_floors.sort();

    match all_floors.len() {
        0 => FloorAssignment::default(),
        1 => FloorAssignment::Single(all_floors[0]),
        _ => FloorAssignment::Half(all_floors[0], all_floors[all_floors.len() - 1]),
    }
}

/// Merge per-floor forbidden sets for the given floors into a single set for routing.
fn merged_forbidden(
    floors: &[i32],
    per_floor: &HashMap<i32, HashSet<(i32, i32)>>,
) -> HashSet<(i32, i32)> {
    let mut merged = HashSet::new();
    for f in floors {
        if let Some(cells) = per_floor.get(f) {
            merged.extend(cells);
        }
    }
    merged
}

/// Stamp corridor cells into the per-floor forbidden sets for the given floors.
fn stamp_corridor_floors(
    waypoints: &[GridPos],
    w: i32,
    floors: &[i32],
    per_floor: &mut HashMap<i32, HashSet<(i32, i32)>>,
) {
    // Collect the cells once, then insert into each floor
    let mut cells = Vec::new();
    for pair in waypoints.windows(2) {
        let min_x = pair[0].x.min(pair[1].x);
        let max_x = pair[0].x.max(pair[1].x);
        let min_y = pair[0].y.min(pair[1].y);
        let max_y = pair[0].y.max(pair[1].y);
        for y in (min_y - 1)..=(max_y + w) {
            for x in (min_x - 1)..=(max_x + w) {
                cells.push((x, y));
            }
        }
    }
    for f in floors {
        let set = per_floor.entry(*f).or_default();
        for &cell in &cells {
            set.insert(cell);
        }
    }
}

/// Initialize per-floor forbidden sets with room interiors on their respective floors.
/// When `exclude_container_ids` is provided, those rooms' interiors are NOT added
/// to the forbidden set (so corridors can route through container interiors).
fn init_per_floor_forbidden_with_exclusions(
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    exclude_room_ids: &HashSet<String>,
) -> HashMap<i32, HashSet<(i32, i32)>> {
    let mut per_floor: HashMap<i32, HashSet<(i32, i32)>> = HashMap::new();
    for rl in &layout.rooms {
        if exclude_room_ids.contains(&rl.room_id) {
            continue;
        }
        let room_floors = graph.room_by_id(&rl.room_id)
            .map(|r| r.floor.floors())
            .unwrap_or_else(|| vec![0]);
        let mut cells = Vec::new();
        for y in rl.y..(rl.y + rl.height as i32) {
            for x in rl.x..(rl.x + rl.width as i32) {
                cells.push((x, y));
            }
        }
        for f in room_floors {
            let set = per_floor.entry(f).or_default();
            for &cell in &cells {
                set.insert(cell);
            }
        }
    }
    per_floor
}

/// Collect all container room IDs that need their interiors excluded from the
/// forbidden set so corridors between their children can route through them.
fn collect_container_ids(graph: &DungeonGraph) -> HashSet<String> {
    graph.groups.iter()
        .filter_map(|g| g.parent_room_id.clone())
        .collect()
}

/// Grid-based corridor router.
/// The A* pathfinder moves a width×width block through the grid,
/// cell by cell. Every cell a corridor occupies is marked forbidden
/// for future corridors on the same floor. No floating point, no exemptions.
pub fn route_corridors(
    graph: &DungeonGraph,
    layout: &SpatialLayout,
) -> Vec<CorridorSegment> {
    // Collect pinned waypoints
    let mut pinned_map: HashMap<String, Vec<GridPos>> = HashMap::new();
    for c in &layout.corridors {
        if !c.pinned_waypoints.is_empty() {
            pinned_map.insert(c.connection_id.clone(), c.pinned_waypoints.clone());
        }
    }

    // Exclude container room interiors from forbidden set so corridors can
    // route through the parent's empty space between children.
    let container_ids = collect_container_ids(graph);
    let mut per_floor = init_per_floor_forbidden_with_exclusions(graph, layout, &container_ids);

    // Sort edges by distance (shorter first)
    let mut sorted_edges: Vec<&StoredEdge> = graph.connections.iter().collect();
    sorted_edges.sort_by_key(|edge| {
        let src = layout.room_by_id(&edge.source_room_id);
        let tgt = layout.room_by_id(&edge.target_room_id);
        match (src, tgt) {
            (Some(s), Some(t)) => {
                let dx = (s.x + s.width as i32 / 2) - (t.x + t.width as i32 / 2);
                let dy = (s.y + s.height as i32 / 2) - (t.y + t.height as i32 / 2);
                dx.abs() + dy.abs()
            }
            _ => i32::MAX,
        }
    });

    let mut corridors = Vec::new();

    for edge in &sorted_edges {
        // Flush connections have no corridor
        if edge.connection.connection_type == ConnectionType::Flush {
            continue;
        }

        let src_rl = layout.room_by_id(&edge.source_room_id);
        let tgt_rl = layout.room_by_id(&edge.target_room_id);

        let Some((src_rl, tgt_rl)) = src_rl.zip(tgt_rl) else {
            continue;
        };

        let pinned = pinned_map.get(&edge.connection.id).cloned().unwrap_or_default();
        let cw = edge.connection.corridor_width;
        let w = cw as i32;
        let half = w / 2;

        let floor = corridor_floor(graph, edge);
        let c_floors = floor.floors();
        let forbidden = merged_forbidden(&c_floors, &per_floor);

        let has_src_exit = edge.source_exit.is_some();
        let has_tgt_exit = edge.target_exit.is_some();

        // Detect child-to-parent connections: create a short stub exit
        let src_is_child_of_tgt = graph.parent_of(&edge.source_room_id)
            .map(|p| p == edge.target_room_id).unwrap_or(false);
        let tgt_is_child_of_src = graph.parent_of(&edge.target_room_id)
            .map(|p| p == edge.source_room_id).unwrap_or(false);

        let result = if pinned.len() >= 2 {
            let pinned_tl: Vec<GridPos> = pinned.iter()
                .map(|p| GridPos { x: p.x - half, y: p.y - half })
                .collect();
            route_through_pinned(&pinned_tl, w, &forbidden)
        } else if has_src_exit || has_tgt_exit {
            // User-specified exits: use fixed positions
            let src_exits = if let Some(exit) = edge.source_exit {
                vec![exit_to_tl(exit, src_rl, w)]
            } else {
                edge_exits(src_rl, tgt_rl, w)
            };
            let tgt_exits = if let Some(exit) = edge.target_exit {
                vec![exit_to_tl(exit, tgt_rl, w)]
            } else {
                edge_exits(tgt_rl, src_rl, w)
            };
            find_best_route(&src_exits, &tgt_exits, w, &forbidden)
        } else if src_is_child_of_tgt {
            // Source is a child room inside target (parent) — create stub exit
            try_child_parent_exit(src_rl, tgt_rl, w)
        } else if tgt_is_child_of_src {
            // Target is a child room inside source (parent) — create stub exit (reversed)
            try_child_parent_exit(tgt_rl, src_rl, w)
                .map(|mut wps| { wps.reverse(); wps })
        } else if let Some(wall_path) = try_shared_wall(src_rl, tgt_rl, w) {
            Some(wall_path)
        } else if let Some(close_path) = try_close_rooms(src_rl, tgt_rl, w) {
            // Rooms are close enough that normal exits overlap — span the gap directly
            Some(close_path)
        } else {
            let src_exits = edge_exits(src_rl, tgt_rl, w);
            let tgt_exits = edge_exits(tgt_rl, src_rl, w);
            find_best_route(&src_exits, &tgt_exits, w, &forbidden)
        };

        let to_center = |wps: Vec<GridPos>| -> Vec<GridPos> {
            wps.iter().map(|p| GridPos { x: p.x + half, y: p.y + half }).collect()
        };

        let mk = |waypoints: Vec<GridPos>, invalid: bool| CorridorSegment {
            pinned_waypoints: pinned.clone(),
            connection_id: edge.connection.id.clone(),
            waypoints,
            width: cw,
            invalid,
            floor,
        };

        // Helper to fix up corridor endpoints to match user-set exits exactly
        let fix_endpoints = |wps: &mut Vec<GridPos>| {
            if let Some(exit) = edge.source_exit {
                if let Some(first) = wps.first_mut() {
                    *first = exit_to_center(exit, src_rl, w);
                }
            }
            if let Some(exit) = edge.target_exit {
                if let Some(last) = wps.last_mut() {
                    *last = exit_to_center(exit, tgt_rl, w);
                }
            }
        };

        if let Some(waypoints) = result {
            stamp_corridor_floors(&waypoints, w, &c_floors, &mut per_floor);
            let mut centered = to_center(waypoints);
            fix_endpoints(&mut centered);
            corridors.push(mk(centered, false));
        } else {
            // Fallback: L-shaped corridor
            let src_exits = if let Some(exit) = edge.source_exit {
                vec![exit_to_tl(exit, src_rl, w)]
            } else {
                edge_exits(src_rl, tgt_rl, w)
            };
            let tgt_exits = if let Some(exit) = edge.target_exit {
                vec![exit_to_tl(exit, tgt_rl, w)]
            } else {
                edge_exits(tgt_rl, src_rl, w)
            };
            if let (Some(&(sx, sy)), Some(&(tx, ty))) =
                (src_exits.first(), tgt_exits.first())
            {
                let waypoints = vec![
                    GridPos { x: sx, y: sy },
                    GridPos { x: tx, y: sy },
                    GridPos { x: tx, y: ty },
                ];
                stamp_corridor_floors(&waypoints, w, &c_floors, &mut per_floor);
                let mut centered = to_center(waypoints);
                fix_endpoints(&mut centered);
                corridors.push(mk(centered, true));
            }
        }
    }

    corridors
}

/// Re-route only corridors connected to a specific set of rooms.
/// Unaffected corridors are kept as-is and stamped into the forbidden set first.
pub fn route_corridors_for_rooms(
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    affected_room_ids: &HashSet<String>,
) -> Vec<CorridorSegment> {
    // Collect pinned waypoints
    let mut pinned_map: HashMap<String, Vec<GridPos>> = HashMap::new();
    for c in &layout.corridors {
        if !c.pinned_waypoints.is_empty() {
            pinned_map.insert(c.connection_id.clone(), c.pinned_waypoints.clone());
        }
    }

    // Exclude container room interiors from forbidden set
    let container_ids = collect_container_ids(graph);
    let mut per_floor = init_per_floor_forbidden_with_exclusions(graph, layout, &container_ids);

    // Partition edges into affected vs unaffected
    let mut affected_edges: Vec<&StoredEdge> = Vec::new();
    let mut unaffected_corridors: Vec<CorridorSegment> = Vec::new();

    let affected_conn_ids: HashSet<String> = graph.connections.iter()
        .filter(|e| {
            affected_room_ids.contains(&e.source_room_id)
                || affected_room_ids.contains(&e.target_room_id)
        })
        .map(|e| e.connection.id.clone())
        .collect();

    for edge in &graph.connections {
        if affected_conn_ids.contains(&edge.connection.id) {
            affected_edges.push(edge);
        }
    }

    // Keep unaffected corridors and stamp them into per-floor forbidden
    for c in &layout.corridors {
        if !affected_conn_ids.contains(&c.connection_id) {
            let w = c.width as i32;
            let half = w / 2;
            let tl_waypoints: Vec<GridPos> = c.waypoints.iter()
                .map(|p| GridPos { x: p.x - half, y: p.y - half })
                .collect();
            let c_floors = c.floor.floors();
            stamp_corridor_floors(&tl_waypoints, w, &c_floors, &mut per_floor);
            unaffected_corridors.push(c.clone());
        }
    }

    // Sort affected edges by distance (shorter first)
    affected_edges.sort_by_key(|edge| {
        let src = layout.room_by_id(&edge.source_room_id);
        let tgt = layout.room_by_id(&edge.target_room_id);
        match (src, tgt) {
            (Some(s), Some(t)) => {
                let dx = (s.x + s.width as i32 / 2) - (t.x + t.width as i32 / 2);
                let dy = (s.y + s.height as i32 / 2) - (t.y + t.height as i32 / 2);
                dx.abs() + dy.abs()
            }
            _ => i32::MAX,
        }
    });

    // Route affected corridors
    let mut new_corridors = Vec::new();
    for edge in &affected_edges {
        let src_rl = layout.room_by_id(&edge.source_room_id);
        let tgt_rl = layout.room_by_id(&edge.target_room_id);

        let Some((src_rl, tgt_rl)) = src_rl.zip(tgt_rl) else {
            continue;
        };

        let pinned = pinned_map.get(&edge.connection.id).cloned().unwrap_or_default();
        let cw = edge.connection.corridor_width;
        let w = cw as i32;
        let half = w / 2;

        let floor = corridor_floor(graph, edge);
        let c_floors = floor.floors();
        let forbidden = merged_forbidden(&c_floors, &per_floor);

        let has_src_exit = edge.source_exit.is_some();
        let has_tgt_exit = edge.target_exit.is_some();

        // Detect child-to-parent connections
        let src_is_child_of_tgt = graph.parent_of(&edge.source_room_id)
            .map(|p| p == edge.target_room_id).unwrap_or(false);
        let tgt_is_child_of_src = graph.parent_of(&edge.target_room_id)
            .map(|p| p == edge.source_room_id).unwrap_or(false);

        let result = if pinned.len() >= 2 {
            let pinned_tl: Vec<GridPos> = pinned.iter()
                .map(|p| GridPos { x: p.x - half, y: p.y - half })
                .collect();
            route_through_pinned(&pinned_tl, w, &forbidden)
        } else if has_src_exit || has_tgt_exit {
            let src_exits = if let Some(exit) = edge.source_exit {
                vec![exit_to_tl(exit, src_rl, w)]
            } else {
                edge_exits(src_rl, tgt_rl, w)
            };
            let tgt_exits = if let Some(exit) = edge.target_exit {
                vec![exit_to_tl(exit, tgt_rl, w)]
            } else {
                edge_exits(tgt_rl, src_rl, w)
            };
            find_best_route(&src_exits, &tgt_exits, w, &forbidden)
        } else if src_is_child_of_tgt {
            try_child_parent_exit(src_rl, tgt_rl, w)
        } else if tgt_is_child_of_src {
            try_child_parent_exit(tgt_rl, src_rl, w)
                .map(|mut wps| { wps.reverse(); wps })
        } else if let Some(wall_path) = try_shared_wall(src_rl, tgt_rl, w) {
            Some(wall_path)
        } else if let Some(close_path) = try_close_rooms(src_rl, tgt_rl, w) {
            Some(close_path)
        } else {
            let src_exits = edge_exits(src_rl, tgt_rl, w);
            let tgt_exits = edge_exits(tgt_rl, src_rl, w);
            find_best_route(&src_exits, &tgt_exits, w, &forbidden)
        };

        let to_center = |wps: Vec<GridPos>| -> Vec<GridPos> {
            wps.iter().map(|p| GridPos { x: p.x + half, y: p.y + half }).collect()
        };

        let mk = |waypoints: Vec<GridPos>, invalid: bool| CorridorSegment {
            pinned_waypoints: pinned.clone(),
            connection_id: edge.connection.id.clone(),
            waypoints,
            width: cw,
            invalid,
            floor,
        };

        let fix_endpoints = |wps: &mut Vec<GridPos>| {
            if let Some(exit) = edge.source_exit {
                if let Some(first) = wps.first_mut() {
                    *first = exit_to_center(exit, src_rl, w);
                }
            }
            if let Some(exit) = edge.target_exit {
                if let Some(last) = wps.last_mut() {
                    *last = exit_to_center(exit, tgt_rl, w);
                }
            }
        };

        if let Some(waypoints) = result {
            stamp_corridor_floors(&waypoints, w, &c_floors, &mut per_floor);
            let mut centered = to_center(waypoints);
            fix_endpoints(&mut centered);
            new_corridors.push(mk(centered, false));
        } else {
            let src_exits = if let Some(exit) = edge.source_exit {
                vec![exit_to_tl(exit, src_rl, w)]
            } else {
                edge_exits(src_rl, tgt_rl, w)
            };
            let tgt_exits = if let Some(exit) = edge.target_exit {
                vec![exit_to_tl(exit, tgt_rl, w)]
            } else {
                edge_exits(tgt_rl, src_rl, w)
            };
            if let (Some(&(sx, sy)), Some(&(tx, ty))) =
                (src_exits.first(), tgt_exits.first())
            {
                let waypoints = vec![
                    GridPos { x: sx, y: sy },
                    GridPos { x: tx, y: sy },
                    GridPos { x: tx, y: ty },
                ];
                stamp_corridor_floors(&waypoints, w, &c_floors, &mut per_floor);
                let mut centered = to_center(waypoints);
                fix_endpoints(&mut centered);
                new_corridors.push(mk(centered, true));
            }
        }
    }

    // Combine: unaffected first, then newly routed
    unaffected_corridors.extend(new_corridors);
    unaffected_corridors
}

/// Mark all grid cells occupied by a corridor as forbidden.
/// For each segment, fills the w-wide rect between waypoints,
/// plus a 1-cell border around the entire corridor to prevent adjacency.
/// Check if two rooms share a wall and return a direct corridor through it.
/// Returns waypoints in top-left coordinates (pre-center-offset).
fn try_shared_wall(src: &RoomLayout, tgt: &RoomLayout, w: i32) -> Option<Vec<GridPos>> {
    let sw = src.width as i32;
    let sh = src.height as i32;
    let tw = tgt.width as i32;
    let th = tgt.height as i32;

    // Check each possible shared edge
    // src's right edge == tgt's left edge
    if src.x + sw == tgt.x {
        let overlap_min = src.y.max(tgt.y);
        let overlap_max = (src.y + sh).min(tgt.y + th);
        if overlap_max - overlap_min >= w {
            let mid_y = (overlap_min + overlap_max - w) / 2;
            let wall_x = src.x + sw;
            return Some(vec![
                GridPos { x: wall_x - 1, y: mid_y },
                GridPos { x: wall_x, y: mid_y },
            ]);
        }
    }
    // src's left edge == tgt's right edge
    if tgt.x + tw == src.x {
        let overlap_min = src.y.max(tgt.y);
        let overlap_max = (src.y + sh).min(tgt.y + th);
        if overlap_max - overlap_min >= w {
            let mid_y = (overlap_min + overlap_max - w) / 2;
            let wall_x = src.x;
            return Some(vec![
                GridPos { x: wall_x, y: mid_y },
                GridPos { x: wall_x - 1, y: mid_y },
            ]);
        }
    }
    // src's bottom edge == tgt's top edge
    if src.y + sh == tgt.y {
        let overlap_min = src.x.max(tgt.x);
        let overlap_max = (src.x + sw).min(tgt.x + tw);
        if overlap_max - overlap_min >= w {
            let mid_x = (overlap_min + overlap_max - w) / 2;
            let wall_y = src.y + sh;
            return Some(vec![
                GridPos { x: mid_x, y: wall_y - 1 },
                GridPos { x: mid_x, y: wall_y },
            ]);
        }
    }
    // src's top edge == tgt's bottom edge
    if tgt.y + th == src.y {
        let overlap_min = src.x.max(tgt.x);
        let overlap_max = (src.x + sw).min(tgt.x + tw);
        if overlap_max - overlap_min >= w {
            let mid_x = (overlap_min + overlap_max - w) / 2;
            let wall_y = src.y;
            return Some(vec![
                GridPos { x: mid_x, y: wall_y },
                GridPos { x: mid_x, y: wall_y - 1 },
            ]);
        }
    }

    None
}

/// Handle rooms that are close (gap <= corridor width) but not touching.
/// Creates a corridor spanning directly from one room wall to the other.
fn try_close_rooms(src: &RoomLayout, tgt: &RoomLayout, w: i32) -> Option<Vec<GridPos>> {
    let sw = src.width as i32;
    let sh = src.height as i32;
    let tw = tgt.width as i32;
    let th = tgt.height as i32;

    // Right-left gap
    let gap_rl = tgt.x - (src.x + sw);
    if gap_rl > 0 && gap_rl <= w {
        let overlap_min = src.y.max(tgt.y);
        let overlap_max = (src.y + sh).min(tgt.y + th);
        if overlap_max - overlap_min >= w {
            let mid_y = (overlap_min + overlap_max - w) / 2;
            return Some(vec![
                GridPos { x: src.x + sw, y: mid_y },
                GridPos { x: tgt.x - w, y: mid_y },
            ]);
        }
    }

    // Left-right gap
    let gap_lr = src.x - (tgt.x + tw);
    if gap_lr > 0 && gap_lr <= w {
        let overlap_min = src.y.max(tgt.y);
        let overlap_max = (src.y + sh).min(tgt.y + th);
        if overlap_max - overlap_min >= w {
            let mid_y = (overlap_min + overlap_max - w) / 2;
            return Some(vec![
                GridPos { x: src.x - w, y: mid_y },
                GridPos { x: tgt.x + tw, y: mid_y },
            ]);
        }
    }

    // Bottom-top gap
    let gap_bt = tgt.y - (src.y + sh);
    if gap_bt > 0 && gap_bt <= w {
        let overlap_min = src.x.max(tgt.x);
        let overlap_max = (src.x + sw).min(tgt.x + tw);
        if overlap_max - overlap_min >= w {
            let mid_x = (overlap_min + overlap_max - w) / 2;
            return Some(vec![
                GridPos { x: mid_x, y: src.y + sh },
                GridPos { x: mid_x, y: tgt.y - w },
            ]);
        }
    }

    // Top-bottom gap
    let gap_tb = src.y - (tgt.y + th);
    if gap_tb > 0 && gap_tb <= w {
        let overlap_min = src.x.max(tgt.x);
        let overlap_max = (src.x + sw).min(tgt.x + tw);
        if overlap_max - overlap_min >= w {
            let mid_x = (overlap_min + overlap_max - w) / 2;
            return Some(vec![
                GridPos { x: mid_x, y: src.y - w },
                GridPos { x: mid_x, y: tgt.y + th },
            ]);
        }
    }

    None
}

/// Create a short stub corridor from a child room's wall into its parent's interior.
/// The corridor is just 1 cell deep — enough to render a door on the child's wall.
/// `child` is the room inside `parent`. Returns waypoints in top-left coordinates.
fn try_child_parent_exit(
    child: &RoomLayout,
    parent: &RoomLayout,
    w: i32,
) -> Option<Vec<GridPos>> {
    let cw = child.width as i32;
    let ch = child.height as i32;
    let pw = parent.width as i32;
    let ph = parent.height as i32;

    // Check that the child is actually inside the parent
    if child.x < parent.x || child.y < parent.y
        || child.x + cw > parent.x + pw
        || child.y + ch > parent.y + ph
    {
        return None;
    }

    // Pick the face with the most space to the parent wall
    let space_right = (parent.x + pw) - (child.x + cw);
    let space_left = child.x - parent.x;
    let space_bottom = (parent.y + ph) - (child.y + ch);
    let space_top = child.y - parent.y;

    let max_space = space_right.max(space_left).max(space_bottom).max(space_top);
    if max_space < 1 {
        return None;
    }

    // Choose best face and generate a stub exit
    if space_right == max_space && ch >= w {
        let mid_y = child.y + (ch - w) / 2;
        Some(vec![
            GridPos { x: child.x + cw - 1, y: mid_y },
            GridPos { x: child.x + cw, y: mid_y },
        ])
    } else if space_left == max_space && ch >= w {
        let mid_y = child.y + (ch - w) / 2;
        Some(vec![
            GridPos { x: child.x, y: mid_y },
            GridPos { x: child.x - 1, y: mid_y },
        ])
    } else if space_bottom == max_space && cw >= w {
        let mid_x = child.x + (cw - w) / 2;
        Some(vec![
            GridPos { x: mid_x, y: child.y + ch - 1 },
            GridPos { x: mid_x, y: child.y + ch },
        ])
    } else if space_top == max_space && cw >= w {
        let mid_x = child.x + (cw - w) / 2;
        Some(vec![
            GridPos { x: mid_x, y: child.y },
            GridPos { x: mid_x, y: child.y - 1 },
        ])
    } else {
        None
    }
}

/// Check if a w×w block at position (x,y) (top-left corner) is clear.
fn block_clear(x: i32, y: i32, w: i32, forbidden: &HashSet<(i32, i32)>) -> bool {
    for dy in 0..w {
        for dx in 0..w {
            if forbidden.contains(&(x + dx, y + dy)) {
                return false;
            }
        }
    }
    true
}

/// A* pathfinding moving a w×w block through the grid.
/// Position is the top-left corner of the block.
/// Iteration budget scales with manhattan distance so failures are cheap.
fn astar_path(
    sx: i32,
    sy: i32,
    tx: i32,
    ty: i32,
    w: i32,
    forbidden: &HashSet<(i32, i32)>,
) -> Option<Vec<GridPos>> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let start = (sx, sy);
    let goal = (tx, ty);

    if start == goal {
        return Some(vec![GridPos { x: sx, y: sy }, GridPos { x: sx, y: sy }]);
    }

    let manhattan = (sx - tx).abs() + (sy - ty).abs();
    // Budget: proportional to distance. A valid path through obstacles rarely
    // needs more than ~8x the manhattan distance in explored cells.
    let max_iterations = (manhattan * 8).clamp(200, 50_000) as usize;

    let heuristic = |x: i32, y: i32| -> i32 {
        (x - tx).abs() + (y - ty).abs()
    };

    let mut g_score: HashMap<(i32, i32), i32> = HashMap::new();
    let mut came_from: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    let mut closed: HashSet<(i32, i32)> = HashSet::new();
    let mut open: BinaryHeap<Reverse<(i32, (i32, i32))>> = BinaryHeap::new();

    g_score.insert(start, 0);
    open.push(Reverse((heuristic(sx, sy), start)));

    let directions = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let mut iterations = 0;

    while let Some(Reverse((_, (cx, cy)))) = open.pop() {
        if !closed.insert((cx, cy)) {
            continue;
        }

        iterations += 1;
        if iterations > max_iterations {
            break;
        }

        if (cx, cy) == goal {
            let mut path = Vec::new();
            let mut cur = goal;
            while cur != start {
                path.push(GridPos { x: cur.0, y: cur.1 });
                cur = came_from[&cur];
            }
            path.push(GridPos { x: start.0, y: start.1 });
            path.reverse();
            return Some(path);
        }

        let current_g = g_score[&(cx, cy)];

        for (dx, dy) in &directions {
            let nx = cx + dx;
            let ny = cy + dy;

            if closed.contains(&(nx, ny)) {
                continue;
            }

            if !block_clear(nx, ny, w, forbidden) {
                continue;
            }

            let tentative_g = current_g + 1;
            if tentative_g < *g_score.get(&(nx, ny)).unwrap_or(&i32::MAX) {
                came_from.insert((nx, ny), (cx, cy));
                g_score.insert((nx, ny), tentative_g);
                let f = tentative_g + heuristic(nx, ny);
                open.push(Reverse((f, (nx, ny))));
            }
        }
    }

    None
}

fn simplify_path(path: &[GridPos]) -> Vec<GridPos> {
    if path.len() <= 2 {
        return path.to_vec();
    }

    let mut result = vec![path[0]];
    for i in 1..path.len() - 1 {
        let prev = path[i - 1];
        let curr = path[i];
        let next = path[i + 1];
        if (curr.x - prev.x) != (next.x - curr.x) || (curr.y - prev.y) != (next.y - curr.y) {
            result.push(curr);
        }
    }
    result.push(*path.last().unwrap());
    result
}

/// Try start/end candidate pairs, return the shortest valid path.
/// Stops early once a good-enough path is found (within 2x manhattan distance).
fn find_best_route(
    src_exits: &[(i32, i32)],
    tgt_exits: &[(i32, i32)],
    w: i32,
    forbidden: &HashSet<(i32, i32)>,
) -> Option<Vec<GridPos>> {
    let mut best: Option<Vec<GridPos>> = None;
    let mut best_len = i32::MAX;

    // Compute overall manhattan between room centers for "good enough" threshold
    let (avg_sx, avg_sy) = if src_exits.is_empty() {
        (0, 0)
    } else {
        let n = src_exits.len() as i32;
        (src_exits.iter().map(|e| e.0).sum::<i32>() / n,
         src_exits.iter().map(|e| e.1).sum::<i32>() / n)
    };
    let (avg_tx, avg_ty) = if tgt_exits.is_empty() {
        (0, 0)
    } else {
        let n = tgt_exits.len() as i32;
        (tgt_exits.iter().map(|e| e.0).sum::<i32>() / n,
         tgt_exits.iter().map(|e| e.1).sum::<i32>() / n)
    };
    let overall_manhattan = (avg_sx - avg_tx).abs() + (avg_sy - avg_ty).abs();
    // A path within 2x manhattan distance is good enough — stop searching
    let good_enough = overall_manhattan * 2;

    'outer: for &(sx, sy) in src_exits {
        if !block_clear(sx, sy, w, forbidden) {
            continue;
        }
        for &(tx, ty) in tgt_exits {
            if !block_clear(tx, ty, w, forbidden) {
                continue;
            }
            let min_possible = (sx - tx).abs() + (sy - ty).abs();
            if min_possible >= best_len {
                continue;
            }
            if let Some(raw) = astar_path(sx, sy, tx, ty, w, forbidden) {
                let simplified = simplify_path(&raw);
                let len: i32 = simplified
                    .windows(2)
                    .map(|p| (p[1].x - p[0].x).abs() + (p[1].y - p[0].y).abs())
                    .sum();
                if len < best_len {
                    best_len = len;
                    best = Some(simplified);
                    if best_len <= good_enough {
                        break 'outer;
                    }
                }
            }
        }
    }

    best
}

/// Route through pinned waypoints as intermediate goals.
fn route_through_pinned(
    waypoints: &[GridPos],
    w: i32,
    forbidden: &HashSet<(i32, i32)>,
) -> Option<Vec<GridPos>> {
    if waypoints.len() < 2 {
        return None;
    }

    let mut full_path: Vec<GridPos> = Vec::new();
    for pair in waypoints.windows(2) {
        let seg = astar_path(pair[0].x, pair[0].y, pair[1].x, pair[1].y, w, forbidden)?;
        let simplified = simplify_path(&seg);
        if full_path.is_empty() {
            full_path.extend_from_slice(&simplified);
        } else {
            full_path.extend_from_slice(&simplified[1..]);
        }
    }

    Some(full_path)
}

// ============================================================
// Edge exit generation
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Face {
    Right,
    Left,
    Bottom,
    Top,
}

/// Determine which face of the room an exit point lies on.
fn exit_face_f(exit: ExitPos, room: &RoomLayout) -> Face {
    let rw = room.width as f32;
    let rh = room.height as f32;
    let rx = room.x as f32;
    let ry = room.y as f32;
    // Pick face by which wall coordinate matches (within epsilon)
    let eps = 0.01;
    if (exit.x - (rx + rw)).abs() < eps { Face::Right }
    else if (exit.x - rx).abs() < eps { Face::Left }
    else if (exit.y - (ry + rh)).abs() < eps { Face::Bottom }
    else { Face::Top }
}

/// Convert a wall exit position (corridor center-line at wall) to a top-left
/// corridor block position just outside the room. Rounds to integer grid for A*.
///
/// The corridor spans [tl, tl+w) in the free axis. We pick tl so that the exit
/// point falls inside (or on the edge of) that range: tl = floor(exit - w/2.0).
fn exit_to_tl(exit: ExitPos, room: &RoomLayout, w: i32) -> (i32, i32) {
    let half_f = w as f32 / 2.0;
    let snap_tl = |v: f32| -> i32 { (v - half_f).floor() as i32 };
    match exit_face_f(exit, room) {
        Face::Right => (room.x + room.width as i32, snap_tl(exit.y)),
        Face::Left => (room.x - w, snap_tl(exit.y)),
        Face::Bottom => (snap_tl(exit.x), room.y + room.height as i32),
        Face::Top => (snap_tl(exit.x), room.y - w),
    }
}

/// Compute the corridor center waypoint that should correspond to a wall exit.
/// Uses integer half (w/2) since waypoints are GridPos (i32).
fn exit_to_center(exit: ExitPos, room: &RoomLayout, w: i32) -> GridPos {
    let half = w / 2;
    let tl = exit_to_tl(exit, room, w);
    GridPos { x: tl.0 + half, y: tl.1 + half }
}

fn facing_face(room: &RoomLayout, other: &RoomLayout) -> Face {
    let rcx = room.x + room.width as i32 / 2;
    let rcy = room.y + room.height as i32 / 2;
    let ocx = other.x + other.width as i32 / 2;
    let ocy = other.y + other.height as i32 / 2;
    let dx = ocx - rcx;
    let dy = ocy - rcy;

    if dx.abs() >= dy.abs() {
        if dx >= 0 { Face::Right } else { Face::Left }
    } else {
        if dy >= 0 { Face::Bottom } else { Face::Top }
    }
}

/// Generate exit positions for a w×w corridor block on all faces of the room.
/// Each position is the top-left corner of the block, placed just outside the room + 1 cell gap.
/// Primary face listed first.
fn edge_exits(room: &RoomLayout, other: &RoomLayout, w: i32) -> Vec<(i32, i32)> {
    let primary = facing_face(room, other);
    let all_faces = match primary {
        Face::Right => [Face::Right, Face::Top, Face::Bottom, Face::Left],
        Face::Left => [Face::Left, Face::Top, Face::Bottom, Face::Right],
        Face::Bottom => [Face::Bottom, Face::Right, Face::Left, Face::Top],
        Face::Top => [Face::Top, Face::Right, Face::Left, Face::Bottom],
    };

    let mut exits = Vec::new();
    for &face in &all_faces {
        exits.extend(face_exits(room, face, w));
    }
    exits
}

/// Generate exit positions on one face.
/// The block is placed right at the room wall — its edge touches the room edge.
fn face_exits(room: &RoomLayout, face: Face, w: i32) -> Vec<(i32, i32)> {
    match face {
        Face::Right => {
            let x = room.x + room.width as i32;
            let min_y = room.y;
            let max_y = room.y + room.height as i32 - w;
            spread(min_y, max_y, w).into_iter().map(|y| (x, y)).collect()
        }
        Face::Left => {
            let x = room.x - w;
            let min_y = room.y;
            let max_y = room.y + room.height as i32 - w;
            spread(min_y, max_y, w).into_iter().map(|y| (x, y)).collect()
        }
        Face::Bottom => {
            let y = room.y + room.height as i32;
            let min_x = room.x;
            let max_x = room.x + room.width as i32 - w;
            spread(min_x, max_x, w).into_iter().map(|x| (x, y)).collect()
        }
        Face::Top => {
            let y = room.y - w;
            let min_x = room.x;
            let max_x = room.x + room.width as i32 - w;
            spread(min_x, max_x, w).into_iter().map(|x| (x, y)).collect()
        }
    }
}

/// Generate positions spread along a range, center first then alternating outward.
fn spread(min: i32, max: i32, step: i32) -> Vec<i32> {
    if min > max {
        return vec![(min + max) / 2];
    }

    let center = (min + max) / 2;
    let step = step.max(1);
    let mut result = vec![center];

    let mut offset = step;
    loop {
        let lo = center - offset;
        let hi = center + offset;
        let mut added = false;
        if lo >= min {
            result.push(lo);
            added = true;
        }
        if hi <= max && hi != lo {
            result.push(hi);
            added = true;
        }
        if !added {
            break;
        }
        offset += step;
    }

    result
}

/// Compute wall openings for rooms based on routed corridors.
/// A wall opening occurs where a corridor's waypoint passes near a room's boundary.
/// Tracked for: container rooms (cross-boundary corridors) and child rooms (child-to-parent exits).
pub fn compute_wall_openings(
    graph: &DungeonGraph,
    layout: &mut SpatialLayout,
) {
    // Clear existing wall openings
    for rl in &mut layout.rooms {
        rl.wall_openings.clear();
    }

    let container_ids = collect_container_ids(graph);

    // Snapshot all room rects for boundary checking
    let room_rects: Vec<(String, i32, i32, i32, i32)> = layout.rooms.iter()
        .map(|rl| (rl.room_id.clone(), rl.x, rl.y, rl.x + rl.width as i32, rl.y + rl.height as i32))
        .collect();

    // Collect openings per room
    let mut openings: HashMap<String, Vec<GridPos>> = HashMap::new();

    for corridor in &layout.corridors {
        let edge = graph.connections.iter()
            .find(|e| e.connection.id == corridor.connection_id);
        let Some(edge) = edge else { continue };

        // Determine which rooms need wall opening detection for this corridor
        let src_parent = graph.parent_of(&edge.source_room_id);
        let tgt_parent = graph.parent_of(&edge.target_room_id);
        let src_is_child_of_tgt = src_parent.map(|p| p == edge.target_room_id).unwrap_or(false);
        let tgt_is_child_of_src = tgt_parent.map(|p| p == edge.source_room_id).unwrap_or(false);

        // Rooms whose walls might have openings from this corridor
        let mut check_rooms: Vec<&str> = Vec::new();

        // Container rooms that this corridor crosses
        if let Some(p) = src_parent {
            if !check_rooms.contains(&p) { check_rooms.push(p); }
        }
        if let Some(p) = tgt_parent {
            if !check_rooms.contains(&p) { check_rooms.push(p); }
        }
        if container_ids.contains(&edge.source_room_id) {
            let id = edge.source_room_id.as_str();
            if !check_rooms.contains(&id) { check_rooms.push(id); }
        }
        if container_ids.contains(&edge.target_room_id) {
            let id = edge.target_room_id.as_str();
            if !check_rooms.contains(&id) { check_rooms.push(id); }
        }

        // Child rooms with exits to their parent
        if src_is_child_of_tgt {
            let id = edge.source_room_id.as_str();
            if !check_rooms.contains(&id) { check_rooms.push(id); }
        }
        if tgt_is_child_of_src {
            let id = edge.target_room_id.as_str();
            if !check_rooms.contains(&id) { check_rooms.push(id); }
        }

        // Merge connections: both endpoint rooms need wall openings
        if edge.connection.connection_type == ConnectionType::Merge {
            let src_id = edge.source_room_id.as_str();
            let tgt_id = edge.target_room_id.as_str();
            if !check_rooms.contains(&src_id) { check_rooms.push(src_id); }
            if !check_rooms.contains(&tgt_id) { check_rooms.push(tgt_id); }
        }

        // Check each corridor waypoint against each relevant room's boundary
        for room_id in &check_rooms {
            let Some((_, rx, ry, rx2, ry2)) = room_rects.iter().find(|(id, ..)| id == *room_id) else {
                continue;
            };

            let w = corridor.width as i32;
            let half = w / 2;
            for wp in &corridor.waypoints {
                let on_left = (wp.x - half..=wp.x + half).contains(rx);
                let on_right = (wp.x - half..=wp.x + half).contains(rx2);
                let on_top = (wp.y - half..=wp.y + half).contains(ry);
                let on_bottom = (wp.y - half..=wp.y + half).contains(ry2);

                let in_y_range = wp.y >= *ry && wp.y <= *ry2;
                let in_x_range = wp.x >= *rx && wp.x <= *rx2;

                if ((on_left || on_right) && in_y_range) || ((on_top || on_bottom) && in_x_range) {
                    let entry = openings.entry(room_id.to_string()).or_default();
                    if !entry.contains(wp) {
                        entry.push(*wp);
                    }
                }
            }
        }
    }

    // Apply openings to room layouts
    for rl in &mut layout.rooms {
        if let Some(ops) = openings.remove(&rl.room_id) {
            rl.wall_openings = ops;
        }
    }
}
