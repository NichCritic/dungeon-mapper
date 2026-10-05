use std::collections::{HashMap, HashSet};


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
            rotation: 0.0,
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
        if !group.room_ids.iter().any(|id| id == room_id) {
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
        if !group.room_ids.iter().any(|id| id == room_id) {
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

/// Compute the effective grid size for a room, enlarging containers to fit their children.
/// Uses a greedy row-packing heuristic.
fn effective_grid_size(
    room: &Room,
    graph: &DungeonGraph,
    size_overrides: &HashMap<String, (u32, u32)>,
    visiting: &mut HashSet<String>,
) -> (u32, u32) {
    let base = room.grid_size();
    let children: Vec<&str> = graph.children_of(&room.id);
    if children.is_empty() {
        return base;
    }

    // A containment cycle (A contains B, B contains A) would recurse here until the
    // stack overflows. Treat an already-visited room as a leaf so the estimate falls
    // back to its base size; the cycle itself is reported to the user by
    // DungeonGraph::containment_cycle before the solve runs.
    if !visiting.insert(room.id.clone()) {
        return base;
    }
    let size = container_grid_size(room, base, &children, graph, size_overrides, visiting);
    visiting.remove(&room.id);
    size
}

/// Size of a container that has at least one child. Split out of
/// [`effective_grid_size`] so the recursion guard has a single exit point.
fn container_grid_size(
    room: &Room,
    base: (u32, u32),
    children: &[&str],
    graph: &DungeonGraph,
    size_overrides: &HashMap<String, (u32, u32)>,
    visiting: &mut HashSet<String>,
) -> (u32, u32) {
    let padding = graph.containment_group(&room.id)
        .map(|g| g.containment_padding)
        .unwrap_or(1);
    let gap = 1u32; // 1-square gap between children

    // Collect child sizes (recursively computed), largest first — the same order
    // place_children_in_container packs them in, so the estimate matches placement.
    let mut child_sizes: Vec<(u32, u32)> = children.iter()
        .filter_map(|cid| {
            size_overrides.get(*cid).copied()
                .or_else(|| graph.room_by_id(cid).map(|r| effective_grid_size(r, graph, size_overrides, visiting)))
        })
        .collect();
    child_sizes.sort_by(|a, b| child_pack_order(*a, *b));

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

/// Packing order for a container's children: tallest first, then widest.
/// Shared by the size estimate and the placement so they agree.
fn child_pack_order(a: (u32, u32), b: (u32, u32)) -> std::cmp::Ordering {
    b.1.cmp(&a.1).then(b.0.cmp(&a.0))
}

/// Build a map of effective sizes for all rooms, computing containers bottom-up.
fn compute_effective_sizes(graph: &DungeonGraph) -> HashMap<String, (u32, u32)> {
    let mut sizes: HashMap<String, (u32, u32)> = HashMap::new();

    // Process rooms bottom-up: deepest children first
    let mut rooms_by_depth: Vec<(&Room, u32)> = graph.rooms.iter()
        .map(|r| (r, graph.nesting_depth(&r.id)))
        .collect();
    rooms_by_depth.sort_by(|a, b| b.1.cmp(&a.1));

    let mut visiting: HashSet<String> = HashSet::new();
    for (room, _depth) in &rooms_by_depth {
        let size = effective_grid_size(room, graph, &sizes, &mut visiting);
        debug_assert!(visiting.is_empty());
        sizes.insert(room.id.clone(), size);
    }

    sizes
}

/// BFS greedy placer. Uses graph view positions as hints for relative placement.
/// How many of the search's best layouts get their corridors routed to pick from.
const ROUTED_CANDIDATES: usize = 4;

/// Solve the whole layout from scratch. Hand-set corridor exits are absolute positions,
/// so they are read against the `previous` layout: the search tries to keep each one's
/// neighbour beyond its wall, and the exit moves along with its room. An exit whose
/// corridor can't be routed from there any more is cleared; without a previous layout
/// all exits are cleared. The graph's exits are updated to match the result.
pub fn solve_layout(
    graph: &mut DungeonGraph,
    gap: u32,
    previous: Option<&SpatialLayout>,
) -> Result<SpatialLayout, String> {
    if graph.rooms.is_empty() {
        return Err("No rooms to layout".to_string());
    }
    use crate::solver::placement::Problem;

    // Containers first, innermost out: lay out each one's children, then size the
    // container to fit them (with room for their corridors), so the level above
    // places it at its real size.
    let mut sizes: HashMap<String, (u32, u32)> = graph.rooms.iter().map(|r| (r.id.clone(), r.grid_size())).collect();
    let mut inner: HashMap<String, Vec<(String, GridRect, FloorAssignment)>> = HashMap::new();
    let mut containers: Vec<&Room> = graph.rooms.iter().filter(|r| graph.is_container(&r.id)).collect();
    containers.sort_by_key(|r| std::cmp::Reverse(graph.nesting_depth(&r.id)));
    for c in containers {
        let children: Vec<String> = graph.children_of(&c.id).into_iter().map(str::to_string).collect();
        let problem = Problem::new(graph, children, &sizes, gap, previous);
        let (_, layouts) = problem.solve();
        let Some(rects) = layouts.first() else { continue };
        let x0 = rects.iter().map(|r| r.x).min().unwrap_or(0);
        let y0 = rects.iter().map(|r| r.y).min().unwrap_or(0);
        let cw = (rects.iter().map(|r| r.x + r.w).max().unwrap_or(0) - x0) as u32;
        let ch = (rects.iter().map(|r| r.y + r.h).max().unwrap_or(0) - y0) as u32;
        let pad = graph.containment_group(&c.id).map(|g| g.containment_padding).unwrap_or(1);
        let (bw, bh) = c.grid_size();
        let (w, h) = (bw.max(cw + 2 * pad), bh.max(ch + 2 * pad));
        // Centered when the container is bigger than its contents
        let (ox, oy) = ((w - cw) as i32 / 2 - x0, (h - ch) as i32 / 2 - y0);
        let placed = rects.iter().enumerate().map(|(i, r)| (
            problem.room_id(i).to_string(),
            GridRect { x: r.x + ox, y: r.y + oy, w: r.w as u32, h: r.h as u32 },
            problem.floor(i),
        )).collect();
        inner.insert(c.id.clone(), placed);
        sizes.insert(c.id.clone(), (w, h));
    }

    let top: Vec<String> = graph.rooms.iter().filter(|r| graph.parent_of(&r.id).is_none()).map(|r| r.id.clone()).collect();
    let problem = Problem::new(graph, top, &sizes, gap, previous);
    let (order, candidates) = problem.solve();
    let ctx = PlacementContext {
        gap,
        groups: &graph.groups,
        connections: &graph.connections,
        graph,
    };

    // Place a room, noting any length limits it breaks, then everything inside it
    fn place(state: &mut PlacementState, ctx: &PlacementContext, inner: &HashMap<String, Vec<(String, GridRect, FloorAssignment)>>, id: &str, rect: GridRect, floor: FloorAssignment) {
        let violations = collect_violations(id, rect, state, ctx);
        state.place_room(id, rect, floor);
        if let Some(rl) = state.layout.rooms.last_mut() {
            rl.violations = violations;
        }
        for (cid, r, f) in inner.get(id).into_iter().flatten() {
            let r = GridRect { x: r.x + rect.x, y: r.y + rect.y, ..*r };
            place(state, ctx, inner, cid, r, *f);
        }
    }

    // The search scores layouts on straight-line estimates; route the best few for real
    // and keep the one whose corridors come out cleanest.
    let mut best: Option<((usize, i32), SpatialLayout, Vec<StoredEdge>)> = None;
    for rects in candidates.iter().take(ROUTED_CANDIDATES) {
        let mut state = PlacementState::new();
        for &i in &order {
            let r = rects[i];
            let rect = GridRect { x: r.x, y: r.y, w: r.w as u32, h: r.h as u32 };
            place(&mut state, &ctx, &inner, problem.room_id(i), rect, problem.floor(i));
        }
        let mut routing = graph.clone();
        match previous {
            Some(prev) => relocate_exits(&mut routing, prev, &state.layout),
            None => routing.connections.iter_mut().for_each(|e| (e.source_exit, e.target_exit) = (None, None)),
        }
        let mut layout = state.layout;
        let mut key = route_and_score(&routing, &mut layout);
        // Exits that no longer route cleanly from where their room ended up are dropped
        let stuck: Vec<String> = layout.corridors.iter()
            .filter(|c| c.invalid)
            .filter_map(|c| routing.connection_by_id(&c.connection_id))
            .filter(|e| e.source_exit.is_some() || e.target_exit.is_some())
            .map(|e| e.connection.id.clone())
            .collect();
        if !stuck.is_empty() {
            let mut freed = routing.clone();
            for e in freed.connections.iter_mut().filter(|e| stuck.contains(&e.connection.id)) {
                (e.source_exit, e.target_exit) = (None, None);
            }
            let mut retry = layout.clone();
            let retry_key = route_and_score(&freed, &mut retry);
            if retry_key < key {
                (layout, routing, key) = (retry, freed, retry_key);
            }
        }
        if best.as_ref().is_none_or(|(k, _, _)| key < *k) {
            best = Some((key, layout, routing.connections));
        }
        if key.0 == 0 {
            break;
        }
    }
    let Some((_, mut layout, connections)) = best else {
        return Err("No layout found".to_string());
    };
    for (edge, solved) in graph.connections.iter_mut().zip(connections) {
        (edge.source_exit, edge.target_exit) = (solved.source_exit, solved.target_exit);
    }
    crate::solver::corridor::compute_wall_openings(graph, &mut layout);
    Ok(layout)
}

/// Route every corridor in `layout`; returns (corridors that couldn't route cleanly,
/// total corridor length) for comparing layouts.
fn route_and_score(graph: &DungeonGraph, layout: &mut SpatialLayout) -> (usize, i32) {
    layout.corridors = crate::solver::corridor::route_corridors(graph, layout);
    layout.recheck_corridor_overlaps();
    let bad = layout.corridors.iter().filter(|c| c.invalid).count();
    let length = layout.corridors.iter()
        .map(|c| c.waypoints.windows(2).map(|w| (w[1].x - w[0].x).abs() + (w[1].y - w[0].y).abs()).sum::<i32>())
        .sum();
    (bad, length)
}

/// Move each user-set corridor exit along with its room from `old` to `new`, keeping its
/// place on the wall. A room that changed shape keeps the exit at the same relative spot,
/// snapped back onto the nearest wall; an exit whose room wasn't laid out is cleared.
pub fn relocate_exits(graph: &mut DungeonGraph, old: &SpatialLayout, new: &SpatialLayout) {
    for edge in &mut graph.connections {
        for (room_id, exit) in [(&edge.source_room_id, &mut edge.source_exit), (&edge.target_room_id, &mut edge.target_exit)] {
            let Some(e) = exit.as_mut() else { continue };
            let (Some(o), Some(n)) = (old.room_by_id(room_id), new.room_by_id(room_id)) else {
                *exit = None;
                continue;
            };
            if (o.width, o.height) == (n.width, n.height) {
                e.x += (n.x - o.x) as f32;
                e.y += (n.y - o.y) as f32;
                continue;
            }
            let half_snap = |v: f32| (v * 2.0).round() / 2.0;
            let fx = ((e.x - o.x as f32) / o.width as f32).clamp(0.0, 1.0);
            let fy = ((e.y - o.y as f32) / o.height as f32).clamp(0.0, 1.0);
            let (nw, nh) = (n.width as f32, n.height as f32);
            // Nearest wall in the new shape
            let (x, y) = (fx * nw, fy * nh);
            let (x, y) = [(x, 0.0, y), (x, nh, nh - y), (0.0, y, x), (nw, y, nw - x)]
                .into_iter()
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
                .map(|(x, y, _)| (x, y))
                .unwrap();
            *e = ExitPos { x: n.x as f32 + half_snap(x), y: n.y as f32 + half_snap(y) };
        }
    }
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

    // Keep rooms that still exist in the graph exactly where they are: only an explicit
    // full solve moves or resizes placed rooms. Corridors are kept too, and only the
    // ones whose connection changed are re-routed (below).
    let mut layout = SpatialLayout {
        rooms: existing.rooms.iter()
            .filter(|rl| graph_room_ids.contains(&rl.room_id))
            .cloned()
            .collect(),
        corridors: existing.corridors.clone(),
        bounds: existing.bounds.clone(),
    };

    // Find new rooms that need placement
    let new_room_ids: Vec<String> = graph.rooms.iter()
        .filter(|r| !existing_room_ids.contains(&r.id))
        .map(|r| r.id.clone())
        .collect();

    // Compute effective sizes for containment
    let effective_sizes = compute_effective_sizes(graph);

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

        let ctx = PlacementContext {
            gap,
            groups: &graph.groups,
            connections: &graph.connections,
            graph,
        };

        let guide = GraphGuide::fit(graph, &layout);
        let corridor_cells = corridor_cells_by_floor(&layout);

        for room_id in &new_room_ids {
            let room = graph.room_by_id(room_id).unwrap();
            let (nw, nh) = effective_sizes.get(room_id.as_str()).copied()
                .unwrap_or_else(|| room.grid_size());
            let orientations = if room.allow_rotation && nw != nh {
                vec![(nw, nh), (nh, nw)]
            } else {
                vec![(nw, nh)]
            };

            // Placed rooms it connects to, with the corridor width and whether flush
            let neighbours: Vec<(GridRect, i32, bool)> = graph.connections.iter()
                .filter_map(|e| {
                    let other = if e.source_room_id == *room_id { &e.target_room_id }
                        else if e.target_room_id == *room_id { &e.source_room_id }
                        else { return None };
                    layout.room_by_id(other).map(|rl| (
                        GridRect { x: rl.x, y: rl.y, w: rl.width, h: rl.height },
                        e.connection.corridor_width as i32,
                        e.connection.connection_type == ConnectionType::Flush,
                    ))
                })
                .collect();

            // Where the Graph view puts it; failing that, beside a neighbour
            let target = guide.as_ref().and_then(|g| g.target(room_id))
                .or_else(|| neighbours.first().map(|(r, w, _)| ((r.x + r.w as i32 + gap as i32 + w) as f32 + nw as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0)))
                .unwrap_or((0.0, 0.0));

            let temp_state = PlacementState {
                layout: SpatialLayout::new(),
                placed: placed.clone(),
                placed_rects: placed_rects.clone(),
                placed_rooms: layout.rooms.iter()
                    .map(|rl| (rl.room_id.clone(), GridRect { x: rl.x, y: rl.y, w: rl.width, h: rl.height }))
                    .collect(),
            };
            let floors = room.floor.floors();
            let clear_of_corridors = |r: &GridRect| floors.iter().all(|f| corridor_cells.get(f).is_none_or(|cells| {
                (r.y - 1..r.y + r.h as i32 + 1).all(|y| (r.x - 1..r.x + r.w as i32 + 1).all(|x| !cells.contains(&(x, y))))
            }));
            let valid = |r: &GridRect| try_place(*r, room_id, room.floor, &temp_state, &ctx) && clear_of_corridors(r);
            // Close to the Graph-view spot first; a shorter corridor breaks near-ties
            let score = |r: &GridRect| {
                let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
                let corridor: u32 = neighbours.iter().map(|(n, _, _)| edge_to_edge_manhattan(*r, *n)).sum();
                (cx - target.0).abs() + (cy - target.1).abs() + 0.25 * corridor as f32
            };

            let mut best: Option<(f32, GridRect)> = None;
            let consider = |r: GridRect, best: &mut Option<(f32, GridRect)>| {
                if valid(&r) {
                    let sc = score(&r);
                    if best.is_none_or(|(b, _)| sc < b) {
                        *best = Some((sc, r));
                    }
                }
            };
            for &(tw, th) in &orientations {
                // Beside each placed neighbour (touching for flush connections)
                for &(n, w, flush) in &neighbours {
                    let d = if flush { 0 } else { gap as i32 + w };
                    for (x, y) in [
                        (n.x + n.w as i32 + d, n.y), (n.x - tw as i32 - d, n.y),
                        (n.x, n.y + n.h as i32 + d), (n.x, n.y - th as i32 - d),
                    ] {
                        consider(GridRect { x, y, w: tw, h: th }, &mut best);
                    }
                }
                // Ring by ring out from the target, a few rings past the first fit
                let (tx, ty) = ((target.0 - tw as f32 / 2.0).round() as i32, (target.1 - th as f32 / 2.0).round() as i32);
                let mut stop_at = MAX_GUIDE_RADIUS;
                let mut radius = 0;
                while radius <= stop_at {
                    for dy in -radius..=radius {
                        for dx in -radius..=radius {
                            if dx.abs().max(dy.abs()) == radius {
                                consider(GridRect { x: tx + dx, y: ty + dy, w: tw, h: th }, &mut best);
                            }
                        }
                    }
                    if best.is_some() && stop_at == MAX_GUIDE_RADIUS {
                        stop_at = radius + 4;
                    }
                    radius += 1;
                }
            }

            let did_place = if let Some((_, rect)) = best {
                layout.rooms.push(RoomLayout {
                    room_id: room_id.clone(),
                    x: rect.x,
                    y: rect.y,
                    width: rect.w,
                    height: rect.h,
                    violations: Vec::new(),
                    wall_openings: Vec::new(),
                    rotation: 0.0,
                });
                placed.insert(room_id.clone());
                placed_rects.push(PlacedRect { rect, floor: room.floor });
                true
            } else {
                false
            };
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
                        rotation: 0.0,
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

    // New rooms inside a container that is already placed: the free spot inside it
    // nearest where the Graph view puts them
    let guide = GraphGuide::fit(graph, &layout);
    for room in &graph.rooms {
        if layout.room_by_id(&room.id).is_some() {
            continue;
        }
        let Some(parent) = graph.parent_of(&room.id).and_then(|p| layout.room_by_id(p)).cloned() else { continue };
        let (w, h) = effective_sizes.get(&room.id).copied().unwrap_or_else(|| room.grid_size());
        let padding = graph.containment_group(&parent.room_id).map(|g| g.containment_padding).unwrap_or(1) as i32;
        let target = guide.as_ref().and_then(|g| g.target(&room.id));
        let (x, y) = free_spot_inside(&layout, graph, &parent, (w, h), padding, room.floor, target);
        layout.rooms.push(RoomLayout {
            room_id: room.id.clone(),
            x, y, width: w, height: h,
            violations: Vec::new(),
            wall_openings: Vec::new(),
            rotation: 0.0,
        });
    }

    // Route only what changed: connections to newly placed rooms, and ones whose width or
    // angle setting differs from what their corridor was routed with. (Connections with
    // no corridor yet are routed too; deleted and flush ones lose theirs.)
    let new_ids: HashSet<&str> = layout.rooms.iter()
        .map(|rl| rl.room_id.as_str())
        .filter(|id| !existing_room_ids.contains(*id))
        .collect();
    layout.corridors = crate::solver::corridor::route_corridors_where(graph, &layout, |e| {
        new_ids.contains(e.source_room_id.as_str())
            || new_ids.contains(e.target_room_id.as_str())
            || layout.corridor_for(&e.connection.id).is_some_and(|c| {
                c.width != e.connection.corridor_width || c.angle != e.connection.corridor_angle
            })
    });
    crate::solver::corridor::compute_wall_openings(graph, &mut layout);

    Ok(layout)
}

/// A spot inside `parent`, `padding` cells in, where a `w`x`h` room overlaps no other
/// placed room on its floor: the one whose center is nearest `target` if given, else the
/// first row by row from the top-left. The inner top-left corner if none is free.
fn free_spot_inside(
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    parent: &RoomLayout,
    (w, h): (u32, u32),
    padding: i32,
    floor: FloorAssignment,
    target: Option<(f32, f32)>,
) -> (i32, i32) {
    let (x0, y0) = (parent.x + padding, parent.y + padding);
    let (x1, y1) = (parent.x + parent.width as i32 - padding - w as i32, parent.y + parent.height as i32 - padding - h as i32);
    let others: Vec<GridRect> = layout.rooms.iter()
        .filter(|rl| rl.room_id != parent.room_id)
        .filter(|rl| graph.room_by_id(&rl.room_id).is_none_or(|r| r.floor.shares_floor(&floor)))
        .map(|rl| GridRect { x: rl.x, y: rl.y, w: rl.width, h: rl.height })
        .collect();
    let mut best: Option<(f32, (i32, i32))> = None;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let free = others.iter().all(|o| {
                x + w as i32 <= o.x || o.x + o.w as i32 <= x || y + h as i32 <= o.y || o.y + o.h as i32 <= y
            });
            if !free {
                continue;
            }
            let Some((tx, ty)) = target else { return (x, y) };
            let d = (x as f32 + w as f32 / 2.0 - tx).abs() + (y as f32 + h as f32 / 2.0 - ty).abs();
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, (x, y)));
            }
        }
    }
    best.map(|(_, p)| p).unwrap_or((x0, y0))
}

/// How far (in cells) from its Graph-view spot a new room is looked for.
const MAX_GUIDE_RADIUS: i32 = 200;

/// Maps Graph-view positions onto the map, learned from the rooms already placed, so a
/// new room can go where it was drawn on the graph. One scale per axis comes from all
/// placed rooms; the offset is local, taken from the placed rooms nearest it on the
/// graph, so wings arranged differently on the two views each map sensibly.
struct GraphGuide<'a> {
    graph: &'a DungeonGraph,
    /// (graph position, center on the map) of each placed room drawn on the graph
    anchors: Vec<((f32, f32), (f32, f32))>,
    scale: (f32, f32),
}

impl<'a> GraphGuide<'a> {
    /// How many placed rooms nearest on the graph set a new room's offset.
    const NEAREST: usize = 4;

    fn fit(graph: &'a DungeonGraph, layout: &SpatialLayout) -> Option<Self> {
        let anchors: Vec<((f32, f32), (f32, f32))> = layout.rooms.iter()
            .filter_map(|rl| {
                let g = *graph.graph_positions.get(&rl.room_id)?;
                Some((g, (rl.x as f32 + rl.width as f32 / 2.0, rl.y as f32 + rl.height as f32 / 2.0)))
            })
            .collect();
        if anchors.is_empty() {
            return None;
        }
        // Least-squares slope of map position against graph position, per axis
        let slope = |pick: fn(&((f32, f32), (f32, f32))) -> (f32, f32)| -> Option<f32> {
            let n = anchors.len() as f32;
            let (mg, mm) = anchors.iter().map(pick).fold((0.0, 0.0), |a, (g, m)| (a.0 + g / n, a.1 + m / n));
            let (cov, var) = anchors.iter().map(pick).fold((0.0, 0.0), |a, (g, m)| (a.0 + (g - mg) * (m - mm), a.1 + (g - mg) * (g - mg)));
            (var > 1e-3).then(|| cov / var).filter(|s| *s > 1e-3)
        };
        let sx = slope(|a| (a.0.0, a.1.0));
        let sy = slope(|a| (a.0.1, a.1.1));
        // A degenerate axis borrows the other's scale; with neither, a typical one
        let scale = match (sx, sy) {
            (Some(x), Some(y)) => (x, y),
            (Some(s), None) | (None, Some(s)) => (s, s),
            (None, None) => (0.1, 0.1),
        };
        Some(GraphGuide { graph, anchors, scale })
    }

    /// Where on the map the room drawn at its Graph-view position should be centered.
    fn target(&self, room_id: &str) -> Option<(f32, f32)> {
        let g = *self.graph.graph_positions.get(room_id)?;
        let mut near: Vec<(f32, (f32, f32))> = self.anchors.iter()
            .map(|&(ag, am)| {
                let d = ((g.0 - ag.0).powi(2) + (g.1 - ag.1).powi(2)).sqrt();
                (d, (am.0 + (g.0 - ag.0) * self.scale.0, am.1 + (g.1 - ag.1) * self.scale.1))
            })
            .collect();
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        near.truncate(Self::NEAREST);
        // Nearer anchors count for more
        let weights: Vec<f32> = near.iter().map(|(d, _)| 1.0 / (d + 1.0)).collect();
        let total: f32 = weights.iter().sum();
        let x = near.iter().zip(&weights).map(|((_, p), w)| p.0 * w).sum::<f32>() / total;
        let y = near.iter().zip(&weights).map(|((_, p), w)| p.1 * w).sum::<f32>() / total;
        Some((x, y))
    }
}

/// The grid cells taken by corridors, per floor.
fn corridor_cells_by_floor(layout: &SpatialLayout) -> HashMap<i32, HashSet<(i32, i32)>> {
    let mut out: HashMap<i32, HashSet<(i32, i32)>> = HashMap::new();
    for c in &layout.corridors {
        let cells = c.cells();
        for f in c.floor.floors() {
            out.entry(f).or_default().extend(cells.iter().copied());
        }
    }
    out
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
        let layout = solve_layout(&mut graph.clone(), 1, None).expect("Layout should succeed");

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

        let layout = solve_layout(&mut graph.clone(), 1, None).expect("Layout should succeed");

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
    /// A chain of four connected rooms, solved once.
    fn chain() -> (DungeonGraph, SpatialLayout) {
        let mut graph = DungeonGraph::new();
        let mut ids = Vec::new();
        for i in 0..4 {
            let r = Room::new(format!("R{i}"));
            ids.push(r.id.clone());
            graph.add_room(r);
        }
        for w in ids.windows(2) {
            graph.add_connection(w[0].clone(), w[1].clone(), Connection::new(ConnectionType::Door));
        }
        let layout = solve_layout(&mut graph.clone(), 1, None).expect("layout");
        (graph, layout)
    }

    fn assert_rooms_kept(before: &SpatialLayout, after: &SpatialLayout) {
        for b in &before.rooms {
            let a = after.room_by_id(&b.room_id).expect("room kept");
            assert_eq!((a.x, a.y, a.width, a.height), (b.x, b.y, b.width, b.height), "room {} moved", b.room_id);
        }
    }

    fn assert_corridors_kept(before: &SpatialLayout, after: &SpatialLayout) {
        for b in &before.corridors {
            let a = after.corridor_for(&b.connection_id).expect("corridor kept");
            assert_eq!(a.waypoints, b.waypoints, "corridor {} re-routed", b.connection_id);
        }
    }

    #[test]
    fn incremental_solve_leaves_placed_rooms_and_corridors_alone() {
        let (mut graph, mut layout) = chain();
        // Hand-edit the layout: move a room and widen another; a graph-only edit follows
        layout.rooms[1].x += 7;
        layout.rooms[2].width += 3;
        graph.rooms[0].tags.push(RoomTag::Trap);
        let after = solve_incremental(&graph, &layout, 1).unwrap();
        assert_rooms_kept(&layout, &after);
        assert_corridors_kept(&layout, &after);
    }

    #[test]
    fn incremental_solve_places_and_connects_only_whats_new() {
        let (mut graph, layout) = chain();
        let new = Room::new("New".into());
        let new_id = new.id.clone();
        graph.add_room(new);
        let anchor = graph.rooms[3].id.clone();
        graph.add_connection(anchor, new_id.clone(), Connection::new(ConnectionType::Door));
        let after = solve_incremental(&graph, &layout, 1).unwrap();
        assert_rooms_kept(&layout, &after);
        assert_corridors_kept(&layout, &after);
        assert!(after.room_by_id(&new_id).is_some(), "new room placed");
        assert_eq!(after.corridors.len(), layout.corridors.len() + 1, "new connection routed");
    }

    #[test]
    fn incremental_solve_reroutes_a_corridor_whose_angle_changed() {
        let (mut graph, layout) = chain();
        graph.connections[1].connection.corridor_angle = CorridorAngle::Any;
        let changed = graph.connections[1].connection.id.clone();
        let after = solve_incremental(&graph, &layout, 1).unwrap();
        assert_rooms_kept(&layout, &after);
        assert_eq!(after.corridor_for(&changed).unwrap().angle, CorridorAngle::Any);
        for b in layout.corridors.iter().filter(|c| c.connection_id != changed) {
            assert_eq!(after.corridor_for(&b.connection_id).unwrap().waypoints, b.waypoints);
        }
    }

    #[test]
    fn incremental_solve_places_a_new_room_inside_its_existing_container() {
        let mut graph = DungeonGraph::new();
        let mut hall = Room::new("Hall".into());
        hall.grid_width = Some(12);
        hall.grid_height = Some(12);
        let hall_id = hall.id.clone();
        graph.add_room(hall);
        let first = Room::new("First".into());
        let first_id = first.id.clone();
        graph.add_room(first);
        let mut group = RoomGroup::new("Inside".into());
        group.parent_room_id = Some(hall_id.clone());
        group.room_ids = vec![first_id];
        graph.groups.push(group);
        let layout = solve_layout(&mut graph.clone(), 1, None).unwrap();

        let second = Room::new("Second".into());
        let second_id = second.id.clone();
        graph.add_room(second);
        graph.groups[0].room_ids.push(second_id.clone());
        let after = solve_incremental(&graph, &layout, 1).unwrap();
        assert_rooms_kept(&layout, &after);
        let (h, c) = (after.room_by_id(&hall_id).unwrap(), after.room_by_id(&second_id).expect("placed"));
        assert!(c.x >= h.x && c.y >= h.y && c.x + c.width as i32 <= h.x + h.width as i32 && c.y + c.height as i32 <= h.y + h.height as i32,
            "inside the container: {:?} in {:?}", (c.x, c.y, c.width, c.height), (h.x, h.y, h.width, h.height));
    }

    /// Four 4x4 rooms on the map in a square 20 cells apart, drawn on the graph in the
    /// same square 200 units apart (so the graph maps onto the map at 1/10 scale).
    fn drawn_square() -> (DungeonGraph, SpatialLayout) {
        let mut graph = DungeonGraph::new();
        let mut layout = SpatialLayout::new();
        for (i, (x, y)) in [(0, 0), (20, 0), (0, 20), (20, 20)].into_iter().enumerate() {
            let mut r = Room::new(format!("R{i}"));
            (r.grid_width, r.grid_height) = (Some(4), Some(4));
            graph.graph_positions.insert(r.id.clone(), (x as f32 * 10.0, y as f32 * 10.0));
            layout.rooms.push(RoomLayout {
                room_id: r.id.clone(), x, y, width: 4, height: 4,
                violations: Vec::new(), wall_openings: Vec::new(), rotation: 0.0,
            });
            graph.add_room(r);
        }
        (graph, layout)
    }

    /// Add a 4x4 room drawn at `at` on the graph; returns its id.
    fn add_drawn_room(graph: &mut DungeonGraph, at: (f32, f32)) -> String {
        let mut r = Room::new("New".into());
        (r.grid_width, r.grid_height) = (Some(4), Some(4));
        let id = r.id.clone();
        graph.graph_positions.insert(id.clone(), at);
        graph.add_room(r);
        id
    }

    #[test]
    fn incremental_solve_puts_a_new_room_where_it_was_drawn_on_the_graph() {
        let (mut graph, layout) = drawn_square();
        // East of the square on the graph: centered at (42, 2) on the map
        let id = add_drawn_room(&mut graph, (400.0, 0.0));
        let after = solve_incremental(&graph, &layout, 1).unwrap();
        assert_rooms_kept(&layout, &after);
        let r = after.room_by_id(&id).expect("placed");
        assert_eq!((r.x, r.y), (40, 0), "where the graph puts it");
    }

    #[test]
    fn incremental_solve_keeps_a_new_room_off_existing_corridors() {
        let (mut graph, mut layout) = drawn_square();
        // A corridor running right through the drawn spot
        layout.corridors.push(CorridorSegment {
            connection_id: "c".into(),
            waypoints: vec![GridPos { x: 30, y: 2 }, GridPos { x: 60, y: 2 }],
            width: 2,
            invalid: false,
            pinned_waypoints: Vec::new(),
            floor: FloorAssignment::default(),
            angle: CorridorAngle::Orthogonal,
        });
        let id = add_drawn_room(&mut graph, (400.0, 0.0));
        let after = solve_incremental(&graph, &layout, 1).unwrap();
        let r = after.room_by_id(&id).expect("placed");
        let cells = layout.corridors[0].cells();
        for y in r.y..r.y + r.height as i32 {
            for x in r.x..r.x + r.width as i32 {
                assert!(!cells.contains(&(x, y)), "room on the corridor at {:?}", (x, y));
            }
        }
        assert!((r.x - 40).abs() + (r.y - 0).abs() <= 8, "still near the drawn spot: {:?}", (r.x, r.y));
    }

    #[test]
    fn incremental_solve_puts_a_new_child_where_it_was_drawn_inside_its_container() {
        let mut graph = DungeonGraph::new();
        let mut hall = Room::new("Hall".into());
        (hall.grid_width, hall.grid_height) = (Some(30), Some(30));
        let hall_id = hall.id.clone();
        graph.graph_positions.insert(hall_id.clone(), (150.0, 150.0));
        graph.add_room(hall);
        let mut first = Room::new("First".into());
        (first.grid_width, first.grid_height) = (Some(4), Some(4));
        let first_id = first.id.clone();
        graph.graph_positions.insert(first_id.clone(), (30.0, 30.0));
        graph.add_room(first);
        let mut group = RoomGroup::new("Inside".into());
        group.parent_room_id = Some(hall_id.clone());
        group.room_ids = vec![first_id.clone()];
        graph.groups.push(group);
        let rl = |id: &str, x, y, w| RoomLayout {
            room_id: id.to_string(), x, y, width: w, height: w,
            violations: Vec::new(), wall_openings: Vec::new(), rotation: 0.0,
        };
        let layout = SpatialLayout { rooms: vec![rl(&hall_id, 0, 0, 30), rl(&first_id, 1, 1, 4)], ..SpatialLayout::new() };

        // Drawn at the bottom-right of the hall: it should go there, not top-left
        let id = add_drawn_room(&mut graph, (250.0, 250.0));
        graph.groups[0].room_ids.push(id.clone());
        let after = solve_incremental(&graph, &layout, 1).unwrap();
        let r = after.room_by_id(&id).expect("placed");
        assert!(r.x >= 18 && r.y >= 18 && r.x + 4 <= 29 && r.y + 4 <= 29, "bottom-right, inside: {:?}", (r.x, r.y));
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
        let layout1 = solve_layout(&mut graph.clone(), 1, None).expect("Initial layout");

        // Now add the containment group
        let mut group = RoomGroup::new("Contents".to_string());
        group.parent_room_id = Some(container_id.clone());
        group.room_ids = child_ids.clone();
        graph.groups.push(group);

        // Full re-solve (what "Recompute All" does)
        let layout2 = solve_layout(&mut graph.clone(), 1, None).expect("Layout with containment");

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

        // The automatic incremental solve leaves already-placed rooms where they are:
        // packing them into the new container waits for an explicit Recompute All
        let layout3 = solve_incremental(&graph, &layout1, 1).expect("Incremental");
        assert_rooms_kept(&layout1, &layout3);
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

        let layout = solve_layout(&mut graph.clone(), 1, None).expect("Layout should succeed");

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

    /// Two rooms that each contain the other used to recurse until the stack
    /// overflowed (SIGABRT, not a catchable panic). The size estimate must treat a
    /// repeat visit as a leaf and return.
    #[test]
    fn effective_sizes_terminate_on_containment_cycle() {
        let mut graph = DungeonGraph::new();
        let a = Room::new("Throne Court".to_string());
        let b = Room::new("The Robe".to_string());
        let (a_id, b_id) = (a.id.clone(), b.id.clone());
        graph.add_room(a);
        graph.add_room(b);

        let mut g1 = RoomGroup::new("Arm A".to_string());
        g1.parent_room_id = Some(a_id.clone());
        g1.room_ids = vec![b_id.clone()];
        graph.groups.push(g1);

        let mut g2 = RoomGroup::new("Court".to_string());
        g2.parent_room_id = Some(b_id.clone());
        g2.room_ids = vec![a_id.clone()];
        graph.groups.push(g2);

        let sizes = compute_effective_sizes(&graph);
        assert_eq!(sizes.len(), 2);
        assert!(sizes.values().all(|&(w, h)| w > 0 && h > 0));
    }
}

/// Check if a rect overlaps any placed rect that shares at least one floor.
fn overlaps_any(rect: GridRect, floor: FloorAssignment, placed: &[PlacedRect], gap: u32) -> bool {
    let g = gap as i32;
    for pr in placed {
        // Skip overlap check if rooms are on entirely different floors
        if !pr.floor.shares_floor(&floor) {
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
