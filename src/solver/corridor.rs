use std::collections::{HashMap, HashSet};

use crate::model::*;

/// Grid-based corridor router.
/// The A* pathfinder moves a width×width block through the grid,
/// cell by cell. Every cell a corridor occupies is marked forbidden
/// for all future corridors. No floating point, no exemptions.
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

    // Initialize forbidden cells with room interiors only.
    // No border — corridors should reach the room wall.
    let mut forbidden = HashSet::new();
    for rl in &layout.rooms {
        for y in rl.y..(rl.y + rl.height as i32) {
            for x in rl.x..(rl.x + rl.width as i32) {
                forbidden.insert((x, y));
            }
        }
    }

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
        let src_rl = layout.room_by_id(&edge.source_room_id);
        let tgt_rl = layout.room_by_id(&edge.target_room_id);

        let Some((src_rl, tgt_rl)) = src_rl.zip(tgt_rl) else {
            continue;
        };

        let pinned = pinned_map.get(&edge.connection.id).cloned().unwrap_or_default();
        let cw = edge.connection.corridor_width;
        let w = cw as i32;
        let half = w / 2;

        let result = if pinned.len() >= 2 {
            let pinned_tl: Vec<GridPos> = pinned.iter()
                .map(|p| GridPos { x: p.x - half, y: p.y - half })
                .collect();
            route_through_pinned(&pinned_tl, w, &forbidden)
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
        };

        if let Some(waypoints) = result {
            stamp_corridor(&waypoints, w, &mut forbidden);
            corridors.push(mk(to_center(waypoints), false));
        } else {
            let src_exits = edge_exits(src_rl, tgt_rl, w);
            let tgt_exits = edge_exits(tgt_rl, src_rl, w);
            if let (Some(&(sx, sy)), Some(&(tx, ty))) =
                (src_exits.first(), tgt_exits.first())
            {
                let waypoints = vec![
                    GridPos { x: sx, y: sy },
                    GridPos { x: tx, y: sy },
                    GridPos { x: tx, y: ty },
                ];
                stamp_corridor(&waypoints, w, &mut forbidden);
                corridors.push(mk(to_center(waypoints), true));
            }
        }
    }

    corridors
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

fn stamp_corridor(waypoints: &[GridPos], w: i32, forbidden: &mut HashSet<(i32, i32)>) {
    for pair in waypoints.windows(2) {
        let min_x = pair[0].x.min(pair[1].x);
        let max_x = pair[0].x.max(pair[1].x);
        let min_y = pair[0].y.min(pair[1].y);
        let max_y = pair[0].y.max(pair[1].y);
        // Corridor occupies cells [min, max+w-1] in each axis from the top-left,
        // plus 1-cell border
        for y in (min_y - 1)..=(max_y + w) {
            for x in (min_x - 1)..=(max_x + w) {
                forbidden.insert((x, y));
            }
        }
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
        // Two identical points so windows(2) produces a segment for rendering
        return Some(vec![GridPos { x: sx, y: sy }, GridPos { x: sx, y: sy }]);
    }

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
    let max_iterations = 100_000;
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

/// Try all start/end candidate pairs, return the shortest valid path.
fn find_best_route(
    src_exits: &[(i32, i32)],
    tgt_exits: &[(i32, i32)],
    w: i32,
    forbidden: &HashSet<(i32, i32)>,
) -> Option<Vec<GridPos>> {
    let mut best: Option<Vec<GridPos>> = None;
    let mut best_len = i32::MAX;

    for &(sx, sy) in src_exits {
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
