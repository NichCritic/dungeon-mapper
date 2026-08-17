use std::collections::{HashMap, HashSet, VecDeque};
use std::f32::consts::PI;

use petgraph::graph::{NodeIndex, UnGraph};

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
            wall_openings: Vec::new(),
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

    // Containment violations: check if room is outside its parent's bounds
    if let Some(parent_id) = ctx.graph.parent_of(room_id) {
        if let Some((_, parent_rect)) = state.placed_rooms.iter().find(|(id, _)| id == parent_id) {
            let padding = ctx.graph.containment_group(parent_id)
                .map(|g| g.containment_padding as i32)
                .unwrap_or(1);
            let parent_label = ctx.graph.room_by_id(parent_id).map(|r| r.label.as_str()).unwrap_or("?");
            if rect.x < parent_rect.x + padding
                || rect.y < parent_rect.y + padding
                || rect.x + rect.w as i32 > parent_rect.x + parent_rect.w as i32 - padding
                || rect.y + rect.h as i32 > parent_rect.y + parent_rect.h as i32 - padding
            {
                violations.push(format!("Overflows container '{}'", parent_label));
            }
        }
    }

    violations
}

/// Sort candidate positions by distance to a preferred position.
fn sort_by_preference(candidates: &mut [(i32, i32)], pref_x: i32, pref_y: i32) {
    candidates.sort_by_key(|&(x, y)| (x - pref_x).abs() + (y - pref_y).abs());
}

/// Compute a Tutte embedding for the graph, producing crossing-free positions
/// for planar graphs. Uses the entrance's connected component.
///
/// Algorithm:
/// 1. Find a boundary cycle (outer face) via DFS from the entrance.
///    Falls back to the entrance + its neighbors if no cycle is found.
/// 2. Fix boundary vertices equally spaced on a circle.
/// 3. Iteratively solve for interior vertices as barycentric averages of neighbors.
fn tutte_embedding(
    pg: &UnGraph<String, String>,
    node_map: &HashMap<String, NodeIndex>,
    entrance_id: &str,
) -> HashMap<String, (f32, f32)> {
    let mut positions: HashMap<NodeIndex, (f32, f32)> = HashMap::new();

    let Some(&entrance_idx) = node_map.get(entrance_id) else {
        return HashMap::new();
    };

    // Collect all nodes reachable from entrance (connected component)
    let mut component: Vec<NodeIndex> = Vec::new();
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut stack = vec![entrance_idx];
    while let Some(node) = stack.pop() {
        if visited.insert(node) {
            component.push(node);
            for neighbor in pg.neighbors(node) {
                if !visited.contains(&neighbor) {
                    stack.push(neighbor);
                }
            }
        }
    }

    if component.len() <= 2 {
        return HashMap::new();
    }

    // Find a boundary cycle using DFS
    let boundary = find_boundary_cycle(pg, entrance_idx, &component);

    let boundary_set: HashSet<NodeIndex> = boundary.iter().copied().collect();
    let interior: Vec<NodeIndex> = component.iter()
        .filter(|n| !boundary_set.contains(n))
        .copied()
        .collect();

    // Scale radius based on number of nodes and average room size
    let radius = 10.0 * (component.len() as f32).sqrt();

    // Fix boundary vertices on a circle
    let k = boundary.len() as f32;
    for (i, &node) in boundary.iter().enumerate() {
        let angle = 2.0 * PI * i as f32 / k;
        positions.insert(node, (radius * angle.cos(), radius * angle.sin()));
    }

    // Initialize interior vertices at center
    for &node in &interior {
        positions.insert(node, (0.0, 0.0));
    }

    // Gauss-Seidel iteration
    for _ in 0..200 {
        let mut max_delta: f32 = 0.0;
        for &node in &interior {
            let neighbors: Vec<NodeIndex> = pg.neighbors(node).collect();
            if neighbors.is_empty() {
                continue;
            }
            let (sum_x, sum_y) = neighbors.iter()
                .filter_map(|n| positions.get(n))
                .fold((0.0f32, 0.0f32), |(ax, ay), &(bx, by)| (ax + bx, ay + by));
            let count = neighbors.iter().filter(|n| positions.contains_key(n)).count();
            if count == 0 {
                continue;
            }
            let new_x = sum_x / count as f32;
            let new_y = sum_y / count as f32;
            let (old_x, old_y) = positions[&node];
            max_delta = max_delta.max((new_x - old_x).abs().max((new_y - old_y).abs()));
            positions.insert(node, (new_x, new_y));
        }
        if max_delta < 0.01 {
            break;
        }
    }

    // Convert NodeIndex keys back to room ID strings
    let idx_to_id: HashMap<NodeIndex, &str> = node_map.iter()
        .map(|(id, &idx)| (idx, id.as_str()))
        .collect();

    positions.into_iter()
        .filter_map(|(idx, pos)| {
            idx_to_id.get(&idx).map(|&id| (id.to_string(), pos))
        })
        .collect()
}

/// Find a cycle to use as the outer boundary for Tutte embedding.
/// Tries to find the longest cycle reachable from the start node.
/// Falls back to the start node + its neighbors.
fn find_boundary_cycle(
    pg: &UnGraph<String, String>,
    start: NodeIndex,
    component: &[NodeIndex],
) -> Vec<NodeIndex> {
    // Strategy: find a cycle via DFS, then try to expand it.
    // For most dungeon graphs this produces a good outer face.

    if component.len() <= 3 {
        // Small graph: use all nodes as boundary
        return component.to_vec();
    }

    // DFS to find the first back-edge cycle
    let mut parent: HashMap<NodeIndex, NodeIndex> = HashMap::new();
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut dfs_stack: Vec<(NodeIndex, Option<NodeIndex>)> = vec![(start, None)];
    let mut cycle: Option<Vec<NodeIndex>> = None;

    'dfs: while let Some((node, from)) = dfs_stack.pop() {
        if !visited.insert(node) {
            continue;
        }
        if let Some(p) = from {
            parent.insert(node, p);
        }

        for neighbor in pg.neighbors(node) {
            if !visited.contains(&neighbor) {
                dfs_stack.push((neighbor, Some(node)));
            } else if from.is_some() && Some(neighbor) != from {
                // Back edge found — extract cycle
                let mut path_a = vec![node];
                let mut cur = node;
                while cur != start && parent.contains_key(&cur) {
                    cur = parent[&cur];
                    path_a.push(cur);
                }

                let mut path_b = vec![neighbor];
                cur = neighbor;
                while cur != start && parent.contains_key(&cur) {
                    cur = parent[&cur];
                    path_b.push(cur);
                }

                // Find common ancestor and build cycle
                let set_a: HashSet<NodeIndex> = path_a.iter().copied().collect();
                let mut lca_idx_b = 0;
                for (i, &n) in path_b.iter().enumerate() {
                    if set_a.contains(&n) {
                        lca_idx_b = i;
                        break;
                    }
                }
                let lca = path_b[lca_idx_b];
                let lca_idx_a = path_a.iter().position(|&n| n == lca).unwrap_or(0);

                let mut c: Vec<NodeIndex> = path_a[..=lca_idx_a].to_vec();
                for &n in path_b[..lca_idx_b].iter().rev() {
                    c.push(n);
                }

                if c.len() >= 3 {
                    cycle = Some(c);
                    break 'dfs;
                }
            }
        }
    }

    // If we found a cycle, use it; otherwise fall back to star from entrance
    if let Some(c) = cycle {
        // Try to find a longer cycle by attempting BFS on the dual,
        // but for now the first cycle is good enough
        if c.len() >= 3 {
            return c;
        }
    }

    // Fallback: entrance + all neighbors
    let mut boundary = vec![start];
    for neighbor in pg.neighbors(start) {
        boundary.push(neighbor);
    }
    if boundary.len() < 3 {
        // Extend with neighbors-of-neighbors
        let first_neighbors: Vec<NodeIndex> = pg.neighbors(start).collect();
        for n in first_neighbors {
            for nn in pg.neighbors(n) {
                if !boundary.contains(&nn) {
                    boundary.push(nn);
                    if boundary.len() >= 3 {
                        break;
                    }
                }
            }
            if boundary.len() >= 3 {
                break;
            }
        }
    }
    boundary
}

/// Compute the effective grid size for a room, enlarging containers to fit their children.
/// Uses a greedy row-packing heuristic.
fn effective_grid_size(
    room: &Room,
    graph: &DungeonGraph,
    size_overrides: &HashMap<String, (u32, u32)>,
) -> (u32, u32) {
    let base = room.grid_size();
    let children: Vec<&str> = graph.children_of(&room.id);
    if children.is_empty() {
        return base;
    }

    let padding = graph.containment_group(&room.id)
        .map(|g| g.containment_padding)
        .unwrap_or(1);
    let gap = 1u32; // 1-square gap between children

    // Collect child sizes (recursively computed)
    let child_sizes: Vec<(u32, u32)> = children.iter()
        .filter_map(|cid| {
            size_overrides.get(*cid).copied()
                .or_else(|| graph.room_by_id(cid).map(|r| effective_grid_size(r, graph, size_overrides)))
        })
        .collect();

    if child_sizes.is_empty() {
        return base;
    }

    // Greedy row-packing: pack children into rows with max width tracking
    let max_child_w = child_sizes.iter().map(|s| s.0).max().unwrap_or(0);
    let target_row_width = max_child_w * 2 + gap; // rough target

    let mut rows: Vec<(u32, u32)> = Vec::new(); // (width, height) of each row
    let mut row_w = 0u32;
    let mut row_h = 0u32;

    for &(cw, ch) in &child_sizes {
        if row_w > 0 && row_w + gap + cw > target_row_width {
            rows.push((row_w, row_h));
            row_w = cw;
            row_h = ch;
        } else {
            if row_w > 0 { row_w += gap; }
            row_w += cw;
            row_h = row_h.max(ch);
        }
    }
    if row_w > 0 {
        rows.push((row_w, row_h));
    }

    let content_w = rows.iter().map(|r| r.0).max().unwrap_or(0);
    let content_h: u32 = rows.iter().map(|r| r.1).sum::<u32>()
        + if rows.len() > 1 { (rows.len() as u32 - 1) * gap } else { 0 };

    let min_w = content_w + padding * 2;
    let min_h = content_h + padding * 2;

    (base.0.max(min_w), base.1.max(min_h))
}

/// Build a map of effective sizes for all rooms, computing containers bottom-up.
fn compute_effective_sizes(graph: &DungeonGraph) -> HashMap<String, (u32, u32)> {
    let mut sizes: HashMap<String, (u32, u32)> = HashMap::new();

    // Process rooms bottom-up: deepest children first
    let mut rooms_by_depth: Vec<(&Room, u32)> = graph.rooms.iter()
        .map(|r| (r, graph.nesting_depth(&r.id)))
        .collect();
    rooms_by_depth.sort_by(|a, b| b.1.cmp(&a.1));

    for (room, _depth) in &rooms_by_depth {
        let size = effective_grid_size(room, graph, &sizes);
        sizes.insert(room.id.clone(), size);
    }

    sizes
}

/// Check if a rect fits within bounds and doesn't overlap siblings (excluding the parent).
fn try_place_bounded(
    rect: GridRect,
    room_id: &str,
    floor: FloorAssignment,
    state: &PlacementState,
    ctx: &PlacementContext,
    bounds: GridRect,
    parent_id: &str,
) -> bool {
    // Must fit within bounds
    if rect.x < bounds.x || rect.y < bounds.y
        || rect.x + rect.w as i32 > bounds.x + bounds.w as i32
        || rect.y + rect.h as i32 > bounds.y + bounds.h as i32
    {
        return false;
    }

    // Overlap check excluding the parent room
    let g = ctx.gap as i32;
    let floors = floor.floors();
    for pr in &state.placed_rects {
        if !pr.floor.floors().iter().any(|f| floors.contains(f)) {
            continue;
        }
        // Find the room_id for this placed rect
        let is_parent = state.placed_rooms.iter().any(|(id, r)|
            id == parent_id && r.x == pr.rect.x && r.y == pr.rect.y
                && r.w == pr.rect.w && r.h == pr.rect.h
        );
        if is_parent { continue; }

        let r = &pr.rect;
        if rect.x < r.x + r.w as i32 + g
            && rect.x + rect.w as i32 + g > r.x
            && rect.y < r.y + r.h as i32 + g
            && rect.y + rect.h as i32 + g > r.y
        {
            return false;
        }
    }

    !violates_group_constraints(room_id, rect, ctx.groups, &state.placed_rooms)
        && !violates_length_constraints(room_id, rect, ctx.connections, &state.placed_rooms)
}

/// Place children inside a container room's bounds using the same BFS/adjacency
/// placement logic as top-level rooms, but constrained within the parent.
fn place_children_in_container(
    parent_id: &str,
    parent_rect: GridRect,
    graph: &DungeonGraph,
    sizes: &HashMap<String, (u32, u32)>,
    state: &mut PlacementState,
    node_map: &HashMap<String, petgraph::graph::NodeIndex>,
    queue: &mut VecDeque<petgraph::graph::NodeIndex>,
    gap: u32,
    pg: &UnGraph<String, String>,
    graph_pos: &HashMap<String, (f32, f32)>,
    scale: f32,
    entrance_graph_pos: (f32, f32),
    ctx: &PlacementContext,
) {
    let padding = graph.containment_group(parent_id)
        .map(|g| g.containment_padding)
        .unwrap_or(1) as i32;

    let children: HashSet<String> = graph.children_of(parent_id).into_iter()
        .map(|s| s.to_string()).collect();
    if children.is_empty() {
        return;
    }

    // Inner bounds
    let bounds = GridRect {
        x: parent_rect.x + padding,
        y: parent_rect.y + padding,
        w: (parent_rect.w as i32 - padding * 2).max(1) as u32,
        h: (parent_rect.h as i32 - padding * 2).max(1) as u32,
    };

    // Place first child at the inner top-left, then BFS from it
    let first_child = children.iter()
        .find(|id| !state.placed.contains(id.as_str()));
    let Some(first_id) = first_child else { return };
    let first_room = graph.room_by_id(first_id).unwrap();
    let (fw, fh) = sizes.get(first_id.as_str()).copied()
        .unwrap_or_else(|| first_room.grid_size());
    let first_rect = GridRect { x: bounds.x, y: bounds.y, w: fw, h: fh };
    state.place_room(first_id, first_rect, first_room.floor);
    if let Some(&idx) = node_map.get(first_id) {
        queue.push_back(idx);
    }
    if graph.is_container(first_id) {
        place_children_in_container(
            first_id, first_rect, graph, sizes, state, node_map, queue, gap,
            pg, graph_pos, scale, entrance_graph_pos, ctx,
        );
    }

    // BFS within children: process queue entries that are children of this container
    let mut child_queue: VecDeque<String> = VecDeque::new();
    child_queue.push_back(first_id.clone());

    while let Some(current_id) = child_queue.pop_front() {
        let current_layout = state.layout.room_by_id(&current_id).unwrap();
        let cx = current_layout.x;
        let cy = current_layout.y;
        let cw = current_layout.width;
        let ch = current_layout.height;

        // Find siblings connected to this child
        if let Some(&current_idx) = node_map.get(&current_id) {
            for neighbor_idx in pg.neighbors(current_idx) {
                let neighbor_id = &pg[neighbor_idx];
                if state.placed.contains(neighbor_id) { continue; }
                if !children.contains(neighbor_id) { continue; }

                let neighbor_room = graph.room_by_id(neighbor_id).unwrap();
                let neighbor_floor = neighbor_room.floor;
                let (nw, nh) = sizes.get(neighbor_id.as_str()).copied()
                    .unwrap_or_else(|| neighbor_room.grid_size());

                let mut orientations = vec![(nw, nh)];
                if neighbor_room.allow_rotation && nw != nh {
                    orientations.push((nh, nw));
                }

                let is_flush = graph.connections.iter().any(|e| {
                    e.connection.connection_type == ConnectionType::Flush
                    && ((e.source_room_id == current_id && e.target_room_id == *neighbor_id)
                        || (e.target_room_id == current_id && e.source_room_id == *neighbor_id))
                });

                let cw_i = graph.connections.iter()
                    .filter(|e| {
                        (e.source_room_id == current_id && e.target_room_id == *neighbor_id)
                        || (e.target_room_id == current_id && e.source_room_id == *neighbor_id)
                    })
                    .map(|e| e.connection.corridor_width as i32)
                    .max()
                    .unwrap_or(2);
                let g = gap as i32;

                let (pref_x, pref_y) = if let Some(&(nx, ny)) = graph_pos.get(neighbor_id.as_str()) {
                    let dx = (nx - entrance_graph_pos.0) * scale;
                    let dy = (ny - entrance_graph_pos.1) * scale;
                    (dx.round() as i32, dy.round() as i32)
                } else {
                    (cx + cw as i32, cy)
                };

                let mut did_place = false;
                'orient: for &(tw, th) in &orientations {
                    let mut adjacent = vec![
                        (cx + cw as i32, cy),
                        (cx, cy + ch as i32),
                        (cx - tw as i32, cy),
                        (cx, cy - th as i32),
                    ];
                    sort_by_preference(&mut adjacent, pref_x, pref_y);

                    let mut spaced = vec![
                        (cx + cw as i32 + g + cw_i, cy),
                        (cx, cy + ch as i32 + g + cw_i),
                        (cx - tw as i32 - g - cw_i, cy),
                        (cx, cy - th as i32 - g - cw_i),
                    ];
                    sort_by_preference(&mut spaced, pref_x, pref_y);

                    if g == 0 || is_flush {
                        for &(px, py) in &adjacent {
                            let rect = GridRect { x: px, y: py, w: tw, h: th };
                            if try_place_bounded(rect, neighbor_id, neighbor_floor, state, ctx, bounds, parent_id) {
                                state.place_room(neighbor_id, rect, neighbor_floor);
                                if let Some(&idx) = node_map.get(neighbor_id.as_str()) {
                                    queue.push_back(idx);
                                }
                                child_queue.push_back(neighbor_id.clone());
                                did_place = true;
                                break 'orient;
                            }
                        }
                    }

                    if !is_flush {
                        for &(px, py) in &spaced {
                            let rect = GridRect { x: px, y: py, w: tw, h: th };
                            if try_place_bounded(rect, neighbor_id, neighbor_floor, state, ctx, bounds, parent_id) {
                                state.place_room(neighbor_id, rect, neighbor_floor);
                                if let Some(&idx) = node_map.get(neighbor_id.as_str()) {
                                    queue.push_back(idx);
                                }
                                child_queue.push_back(neighbor_id.clone());
                                did_place = true;
                                break 'orient;
                            }
                        }
                    }
                }

                // Fallback: further out
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
                            if try_place_bounded(rect, neighbor_id, neighbor_floor, state, ctx, bounds, parent_id) {
                                state.place_room(neighbor_id, rect, neighbor_floor);
                                if let Some(&idx) = node_map.get(neighbor_id.as_str()) {
                                    queue.push_back(idx);
                                }
                                child_queue.push_back(neighbor_id.clone());
                                did_place = true;
                                break 'outer;
                            }
                        }
                    }
                }

                if did_place && graph.is_container(neighbor_id) {
                    let placed_rl = state.layout.room_by_id(neighbor_id).unwrap();
                    let r = GridRect { x: placed_rl.x, y: placed_rl.y, w: placed_rl.width, h: placed_rl.height };
                    place_children_in_container(
                        neighbor_id, r, graph, sizes, state, node_map, queue, gap,
                        pg, graph_pos, scale, entrance_graph_pos, ctx,
                    );
                }
            }
        }
    }

    // Place any remaining unconnected children via scan within bounds
    for child_id in &children {
        if state.placed.contains(child_id.as_str()) { continue; }
        let Some(child_room) = graph.room_by_id(child_id) else { continue };
        let (nw, nh) = sizes.get(child_id.as_str()).copied()
            .unwrap_or_else(|| child_room.grid_size());
        let mut did_place = false;
        'scan: for sy in bounds.y..=bounds.y + bounds.h as i32 - nh as i32 {
            for sx in bounds.x..=bounds.x + bounds.w as i32 - nw as i32 {
                let rect = GridRect { x: sx, y: sy, w: nw, h: nh };
                if try_place_bounded(rect, child_id, child_room.floor, state, ctx, bounds, parent_id) {
                    state.place_room(child_id, rect, child_room.floor);
                    if let Some(&idx) = node_map.get(child_id.as_str()) {
                        queue.push_back(idx);
                    }
                    did_place = true;
                    break 'scan;
                }
            }
        }

        if did_place && graph.is_container(child_id) {
            let placed_rl = state.layout.room_by_id(child_id).unwrap();
            let r = GridRect { x: placed_rl.x, y: placed_rl.y, w: placed_rl.width, h: placed_rl.height };
            place_children_in_container(
                child_id, r, graph, sizes, state, node_map, queue, gap,
                pg, graph_pos, scale, entrance_graph_pos, ctx,
            );
        }

        if !did_place {
            eprintln!("Warning: Could not place child room '{}' inside container", child_room.label);
        }
    }
}

/// BFS greedy placer. Uses graph view positions as hints for relative placement.
pub fn solve_layout(
    graph: &DungeonGraph,
    gap: u32,
) -> Result<SpatialLayout, String> {
    if graph.rooms.is_empty() {
        return Err("No rooms to layout".to_string());
    }

    let (pg, node_map) = graph.build_petgraph();

    // Collect all child room IDs (placed by their containers, not by BFS)
    let child_room_ids: HashSet<String> = graph.groups.iter()
        .filter(|g| g.parent_room_id.is_some())
        .flat_map(|g| g.room_ids.iter().cloned())
        .collect();

    // Find entrance room — must not be a child (children are placed by their container)
    let entrance = graph.rooms.iter()
        .find(|r| r.tags.contains(&RoomTag::Entrance) && !child_room_ids.contains(&r.id))
        .or_else(|| graph.rooms.iter().find(|r| !child_room_ids.contains(&r.id)))
        .unwrap_or(&graph.rooms[0]);

    // Compute Tutte embedding for crossing-free placement hints (planar graphs)
    let tutte_pos = tutte_embedding(&pg, &node_map, &entrance.id);
    // Prefer Tutte positions over raw graph editor positions
    let fallback_pos: HashMap<String, (f32, f32)> = graph.graph_positions.iter().map(|(k, v)| (k.clone(), *v)).collect();
    let graph_pos = if tutte_pos.len() >= 3 { &tutte_pos } else { &fallback_pos };
    let scale = if tutte_pos.len() >= 3 { 1.0_f32 } else { 0.05_f32 };

    let mut state = PlacementState::new();
    let ctx = PlacementContext {
        gap,
        groups: &graph.groups,
        connections: &graph.connections,
        graph,
    };

    // Compute effective sizes (containers enlarged to fit children)
    let effective_sizes = compute_effective_sizes(graph);

    let get_size = |room_id: &str| -> (u32, u32) {
        effective_sizes.get(room_id).copied()
            .unwrap_or_else(|| graph.room_by_id(room_id).map(|r| r.grid_size()).unwrap_or((4, 4)))
    };

    let (ew, eh) = get_size(&entrance.id);
    let entrance_graph_pos = graph_pos.get(&entrance.id).copied().unwrap_or((0.0, 0.0));

    state.place_room(&entrance.id, GridRect { x: 0, y: 0, w: ew, h: eh }, entrance.floor);

    let mut queue = VecDeque::new();
    if let Some(&start_idx) = node_map.get(&entrance.id) {
        queue.push_back(start_idx);
    }

    // If the entrance is a container, place children inside it
    if graph.is_container(&entrance.id) {
        let rect = GridRect { x: 0, y: 0, w: ew, h: eh };
        place_children_in_container(
            &entrance.id, rect, graph, &effective_sizes,
            &mut state, &node_map, &mut queue, gap,
            &pg, graph_pos, scale, entrance_graph_pos, &ctx,
        );
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

                // Skip children — they are placed by their container
                if child_room_ids.contains(neighbor_id) {
                    continue;
                }

                let neighbor_room = graph.room_by_id(neighbor_id).unwrap();
                let neighbor_floor = neighbor_room.floor;
                let (nw, nh) = get_size(neighbor_id);

                let mut orientations = vec![(nw, nh)];
                if neighbor_room.allow_rotation && nw != nh {
                    orientations.push((nh, nw));
                }

                // Check if any connection between these rooms is Flush
                let is_flush = graph.connections.iter().any(|e| {
                    e.connection.connection_type == ConnectionType::Flush
                    && ((e.source_room_id == *current_id && e.target_room_id == *neighbor_id)
                        || (e.target_room_id == *current_id && e.source_room_id == *neighbor_id))
                });

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

                    // Flush connections always try adjacent first; others only when gap is 0
                    if g == 0 || is_flush {
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

                    // Flush connections should not use spaced placement
                    if !is_flush {
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

                // If we just placed a container, place its children inside
                if did_place && graph.is_container(neighbor_id) {
                    let placed_rl = state.layout.room_by_id(neighbor_id).unwrap();
                    let container_rect = GridRect {
                        x: placed_rl.x, y: placed_rl.y,
                        w: placed_rl.width, h: placed_rl.height,
                    };
                    place_children_in_container(
                        neighbor_id, container_rect, graph, &effective_sizes,
                        &mut state, &node_map, &mut queue, gap,
                        &pg, graph_pos, scale, entrance_graph_pos, &ctx,
                    );
                }
            }
        }

        // Handle disconnected components (skip child rooms)
        let unplaced_room = graph.rooms.iter()
            .find(|r| !state.placed.contains(&r.id) && !child_room_ids.contains(&r.id));
        match unplaced_room {
            Some(room) => {
                let (nw, nh) = get_size(&room.id);
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
                if did_place && graph.is_container(&room.id) {
                    let placed_rl = state.layout.room_by_id(&room.id).unwrap();
                    let container_rect = GridRect {
                        x: placed_rl.x, y: placed_rl.y,
                        w: placed_rl.width, h: placed_rl.height,
                    };
                    place_children_in_container(
                        &room.id, container_rect, graph, &effective_sizes,
                        &mut state, &node_map, &mut queue, gap,
                        &pg, graph_pos, scale, entrance_graph_pos, &ctx,
                    );
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
    crate::solver::corridor::compute_wall_openings(graph, &mut state.layout);

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

    // Compute effective sizes for containment
    let effective_sizes = compute_effective_sizes(graph);

    // Resize existing containers that are too small for their children
    for (room_id, &(ew, eh)) in &effective_sizes {
        if let Some(rl) = layout.room_by_id_mut(room_id) {
            if rl.width < ew || rl.height < eh {
                rl.width = rl.width.max(ew);
                rl.height = rl.height.max(eh);
            }
        }
    }

    // Reposition children that are outside their container's bounds
    for group in &graph.groups {
        let Some(parent_id) = &group.parent_room_id else { continue };
        let parent_bounds = layout.room_by_id(parent_id)
            .map(|p| (p.x, p.y, p.width, p.height));
        let Some((px, py, pw, ph)) = parent_bounds else { continue };
        let padding = group.containment_padding as i32;
        let inner_x = px + padding;
        let inner_y = py + padding;
        let inner_w = (pw as i32 - padding * 2).max(1);
        let inner_h = (ph as i32 - padding * 2).max(1);

        // Row-pack all children that are currently outside the container
        let mut needs_repack = false;
        for child_id in &group.room_ids {
            if let Some(child_rl) = layout.room_by_id(child_id) {
                if child_rl.x < inner_x || child_rl.y < inner_y
                    || child_rl.x + child_rl.width as i32 > inner_x + inner_w
                    || child_rl.y + child_rl.height as i32 > inner_y + inner_h
                {
                    needs_repack = true;
                    break;
                }
            }
        }

        if needs_repack {
            let mut cursor_x = inner_x;
            let mut cursor_y = inner_y;
            let mut row_max_h = 0i32;
            for child_id in &group.room_ids {
                let child_size = effective_sizes.get(child_id.as_str()).copied()
                    .or_else(|| graph.room_by_id(child_id).map(|r| r.grid_size()));
                let Some((cw, ch)) = child_size else { continue };
                if cursor_x > inner_x && cursor_x + cw as i32 > inner_x + inner_w {
                    cursor_x = inner_x;
                    cursor_y += row_max_h + 1;
                    row_max_h = 0;
                }
                if let Some(rl) = layout.room_by_id_mut(child_id) {
                    rl.x = cursor_x;
                    rl.y = cursor_y;
                    rl.width = cw;
                    rl.height = ch;
                }
                cursor_x += cw as i32 + 1;
                row_max_h = row_max_h.max(ch as i32);
            }
        }
    }

    let child_room_ids: HashSet<String> = graph.groups.iter()
        .filter(|g| g.parent_room_id.is_some())
        .flat_map(|g| g.room_ids.iter().cloned())
        .collect();

    // Filter out child rooms from new_room_ids (they'll be placed by containers)
    let new_room_ids: Vec<String> = new_room_ids.into_iter()
        .filter(|id| !child_room_ids.contains(id))
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

        // Graph positions for hints — use Tutte embedding when available
        let (pg, node_map) = graph.build_petgraph();
        let entrance = graph.rooms.iter()
            .find(|r| r.tags.contains(&RoomTag::Entrance))
            .unwrap_or(&graph.rooms[0]);
        let tutte_pos = tutte_embedding(&pg, &node_map, &entrance.id);
        let fallback_pos: HashMap<String, (f32, f32)> = graph.graph_positions.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let graph_pos = if tutte_pos.len() >= 3 { &tutte_pos } else { &fallback_pos };
        let scale = if tutte_pos.len() >= 3 { 1.0_f32 } else { 0.05_f32 };
        let entrance_graph_pos = graph_pos.get(&entrance.id).copied().unwrap_or((0.0, 0.0));

        for room_id in &new_room_ids {
            let room = graph.room_by_id(room_id).unwrap();
            let (nw, nh) = effective_sizes.get(room_id.as_str()).copied()
                .unwrap_or_else(|| room.grid_size());

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

            // Check if any connection to this room is Flush
            let is_flush = graph.connections.iter().any(|e| {
                e.connection.connection_type == ConnectionType::Flush
                && (e.source_room_id == *room_id || e.target_room_id == *room_id)
                && anchor.is_some()
            });

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
                // Flush connections always try adjacent; others only when gap is 0
                if g == 0 || is_flush {
                    candidates.extend_from_slice(&[
                        (cx + cw as i32, cy),
                        (cx, cy + ch as i32),
                        (cx - tw as i32, cy),
                        (cx, cy - th as i32),
                    ]);
                }
                if !is_flush {
                    candidates.extend_from_slice(&[
                        (cx + cw as i32 + g + cw_i, cy),
                        (cx, cy + ch as i32 + g + cw_i),
                        (cx - tw as i32 - g - cw_i, cy),
                        (cx, cy - th as i32 - g - cw_i),
                    ]);
                }
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
                            wall_openings: Vec::new(),
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
                                wall_openings: Vec::new(),
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

            // If we just placed a container, place its children inside
            if did_place && graph.is_container(room_id) {
                let placed_rl = layout.room_by_id(room_id).unwrap();
                let container_rect = GridRect {
                    x: placed_rl.x, y: placed_rl.y,
                    w: placed_rl.width, h: placed_rl.height,
                };
                // Use a simple row-packing placement for children
                let children = graph.children_of(room_id);
                let c_padding = graph.containment_group(room_id)
                    .map(|g| g.containment_padding)
                    .unwrap_or(1) as i32;
                let mut cx = container_rect.x + c_padding;
                let mut cy = container_rect.y + c_padding;
                let mut row_max_h = 0i32;
                let inner_right = container_rect.x + container_rect.w as i32 - c_padding;
                for child_id in children {
                    if placed.contains(child_id) { continue; }
                    let Some(child_room) = graph.room_by_id(child_id) else { continue };
                    let (cw, ch) = effective_sizes.get(child_id).copied()
                        .unwrap_or_else(|| child_room.grid_size());
                    if cx > container_rect.x + c_padding && cx + cw as i32 > inner_right {
                        cx = container_rect.x + c_padding;
                        cy += row_max_h + 1;
                        row_max_h = 0;
                    }
                    layout.rooms.push(RoomLayout {
                        room_id: child_id.to_string(),
                        x: cx, y: cy,
                        width: cw, height: ch,
                        violations: Vec::new(),
                        wall_openings: Vec::new(),
                    });
                    placed.insert(child_id.to_string());
                    placed_rects.push(PlacedRect {
                        rect: GridRect { x: cx, y: cy, w: cw, h: ch },
                        floor: child_room.floor,
                    });
                    cx += cw as i32 + 1;
                    row_max_h = row_max_h.max(ch as i32);
                }
            }
        }
    }

    // Route all corridors (re-route is cheap compared to placement)
    layout.corridors = crate::solver::corridor::route_corridors(graph, &layout);
    crate::solver::corridor::compute_wall_openings(graph, &mut layout);

    Ok(layout)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_containment_4_children_fit_inside() {
        let mut graph = DungeonGraph::new();

        // Create a container room and 4 children
        let mut container = Room::new("Great Hall".to_string());
        container.tags.push(RoomTag::Entrance);
        let container_id = container.id.clone();
        graph.add_room(container);

        let mut child_ids = Vec::new();
        for i in 0..4 {
            let child = Room::new(format!("Room {}", i + 1));
            child_ids.push(child.id.clone());
            graph.add_room(child);
        }

        // Create a containment group
        let mut group = RoomGroup::new("Hall Contents".to_string());
        group.parent_room_id = Some(container_id.clone());
        group.room_ids = child_ids.clone();
        graph.groups.push(group);

        // Solve the layout
        let layout = solve_layout(&graph, 1).expect("Layout should succeed");

        // Get container bounds
        let container_rl = layout.room_by_id(&container_id).expect("Container should be placed");
        let cx = container_rl.x;
        let cy = container_rl.y;
        let cw = container_rl.width as i32;
        let ch = container_rl.height as i32;

        // Verify all children are inside the container
        for child_id in &child_ids {
            let child_rl = layout.room_by_id(child_id)
                .unwrap_or_else(|| panic!("Child {} should be placed", child_id));
            let child_right = child_rl.x + child_rl.width as i32;
            let child_bottom = child_rl.y + child_rl.height as i32;

            assert!(child_rl.x >= cx,
                "Child x={} should be >= container x={}", child_rl.x, cx);
            assert!(child_rl.y >= cy,
                "Child y={} should be >= container y={}", child_rl.y, cy);
            assert!(child_right <= cx + cw,
                "Child right={} should be <= container right={}", child_right, cx + cw);
            assert!(child_bottom <= cy + ch,
                "Child bottom={} should be <= container bottom={}", child_bottom, cy + ch);
        }
    }

    #[test]
    fn test_containment_varied_sizes() {
        let mut graph = DungeonGraph::new();

        let mut container = Room::new("Hall".to_string());
        container.tags.push(RoomTag::Entrance);
        let container_id = container.id.clone();
        graph.add_room(container);

        let sizes = [(3, 3), (4, 4), (6, 6), (3, 5)];
        let mut child_ids = Vec::new();
        for (i, (w, h)) in sizes.iter().enumerate() {
            let mut child = Room::new(format!("Room {}", i + 1));
            child.grid_width = Some(*w);
            child.grid_height = Some(*h);
            child_ids.push(child.id.clone());
            graph.add_room(child);
        }

        let mut group = RoomGroup::new("Contents".to_string());
        group.parent_room_id = Some(container_id.clone());
        group.room_ids = child_ids.clone();
        graph.groups.push(group);

        let layout = solve_layout(&graph, 1).expect("Layout should succeed");

        let container_rl = layout.room_by_id(&container_id).expect("Container placed");
        let cx = container_rl.x;
        let cy = container_rl.y;
        let cw = container_rl.width as i32;
        let ch = container_rl.height as i32;

        for child_id in &child_ids {
            let child_rl = layout.room_by_id(child_id)
                .unwrap_or_else(|| panic!("Child {} should be placed", child_id));
            assert!(child_rl.x >= cx, "x overflow");
            assert!(child_rl.y >= cy, "y overflow");
            assert!(child_rl.x + child_rl.width as i32 <= cx + cw, "right overflow");
            assert!(child_rl.y + child_rl.height as i32 <= cy + ch, "bottom overflow");
        }
    }
    #[test]
    fn test_incremental_containment_new_container() {
        // Simulate: rooms exist, then user creates a containment group
        let mut graph = DungeonGraph::new();

        let mut container = Room::new("Hall".to_string());
        container.tags.push(RoomTag::Entrance);
        container.grid_width = Some(14);
        container.grid_height = Some(14);
        let container_id = container.id.clone();
        graph.add_room(container);

        let mut child_ids = Vec::new();
        for i in 0..4 {
            let child = Room::new(format!("Room {}", i + 1));
            child_ids.push(child.id.clone());
            graph.add_room(child);
        }

        // First, solve without containment
        let layout1 = solve_layout(&graph, 1).expect("Initial layout");

        // Now add the containment group
        let mut group = RoomGroup::new("Contents".to_string());
        group.parent_room_id = Some(container_id.clone());
        group.room_ids = child_ids.clone();
        graph.groups.push(group);

        // Full re-solve (what "Recompute All" does)
        let layout2 = solve_layout(&graph, 1).expect("Layout with containment");

        let container_rl = layout2.room_by_id(&container_id).expect("Container placed");
        let cx = container_rl.x;
        let cy = container_rl.y;
        let cw = container_rl.width as i32;
        let ch = container_rl.height as i32;

        for child_id in &child_ids {
            let child_rl = layout2.room_by_id(child_id)
                .unwrap_or_else(|| panic!("Child {} should be placed", child_id));
            assert!(child_rl.x >= cx, "x: {} < {}", child_rl.x, cx);
            assert!(child_rl.y >= cy, "y: {} < {}", child_rl.y, cy);
            assert!(child_rl.x + child_rl.width as i32 <= cx + cw,
                "right: {} > {}", child_rl.x + child_rl.width as i32, cx + cw);
            assert!(child_rl.y + child_rl.height as i32 <= cy + ch,
                "bottom: {} > {}", child_rl.y + child_rl.height as i32, cy + ch);
        }

        // Also test incremental solve from layout1
        let layout3 = solve_incremental(&graph, &layout1, 1).expect("Incremental");

        let container_rl = layout3.room_by_id(&container_id).expect("Container placed");
        let cx = container_rl.x;
        let cy = container_rl.y;
        let cw = container_rl.width as i32;
        let ch = container_rl.height as i32;

        for child_id in &child_ids {
            let child_rl = layout3.room_by_id(child_id)
                .unwrap_or_else(|| panic!("Child {} should be placed in incremental", child_id));
            assert!(child_rl.x >= cx, "incr x: {} < {}", child_rl.x, cx);
            assert!(child_rl.y >= cy, "incr y: {} < {}", child_rl.y, cy);
            assert!(child_rl.x + child_rl.width as i32 <= cx + cw,
                "incr right: {} > {}", child_rl.x + child_rl.width as i32, cx + cw);
            assert!(child_rl.y + child_rl.height as i32 <= cy + ch,
                "incr bottom: {} > {}", child_rl.y + child_rl.height as i32, cy + ch);
        }
    }

    #[test]
    fn test_100x100_container_mixed_children() {
        // Reproduction: 100x100 container, 3x 4x4 children + 1x 8x4 child
        let mut graph = DungeonGraph::new();

        // NO entrance tag — first room becomes entrance by default
        // Children added BEFORE the container
        let child_sizes = [(4u32, 4u32), (4, 4), (4, 4), (8, 4)];
        let mut child_ids = Vec::new();
        for (i, (w, h)) in child_sizes.iter().enumerate() {
            let mut child = Room::new(format!("Child {}", i + 1));
            child.grid_width = Some(*w);
            child.grid_height = Some(*h);
            child_ids.push(child.id.clone());
            graph.add_room(child);
        }

        let mut container = Room::new("Container".to_string());
        container.grid_width = Some(100);
        container.grid_height = Some(100);
        let container_id = container.id.clone();
        graph.add_room(container);

        let mut group = RoomGroup::new("Contents".to_string());
        group.parent_room_id = Some(container_id.clone());
        group.room_ids = child_ids.clone();
        graph.groups.push(group);

        let layout = solve_layout(&graph, 1).expect("Layout should succeed");

        let container_rl = layout.room_by_id(&container_id).expect("Container placed");
        let cx = container_rl.x;
        let cy = container_rl.y;
        let cw = container_rl.width as i32;
        let ch = container_rl.height as i32;

        for (i, child_id) in child_ids.iter().enumerate() {
            let child_rl = layout.room_by_id(child_id)
                .unwrap_or_else(|| panic!("Child {} should be placed", i));
            let (child_w, child_h) = (child_sizes[i].0, child_sizes[i].1);
            eprintln!(
                "Child {} '{}': pos=({}, {}), size={}x{}, container=({}, {})+{}x{}",
                i, format!("Child {}", i + 1),
                child_rl.x, child_rl.y, child_rl.width, child_rl.height,
                cx, cy, cw, ch
            );
            assert!(child_rl.x >= cx,
                "Child {} x={} < container x={}", i, child_rl.x, cx);
            assert!(child_rl.y >= cy,
                "Child {} y={} < container y={}", i, child_rl.y, cy);
            assert!(child_rl.x + child_w as i32 <= cx + cw,
                "Child {} right={} > container right={}", i, child_rl.x + child_w as i32, cx + cw);
            assert!(child_rl.y + child_h as i32 <= cy + ch,
                "Child {} bottom={} > container bottom={}", i, child_rl.y + child_h as i32, cy + ch);
        }
    }
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
