use std::collections::{HashSet, VecDeque};

use crate::model::*;

/// Check if placing a room would violate any group constraint.
fn violates_group_constraints(
    room_id: &str,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    groups: &[RoomGroup],
    placed_rooms: &[(String, i32, i32, u32, u32)],
) -> bool {
    for group in groups {
        if !group.room_ids.contains(&room_id.to_string()) {
            continue;
        }
        if group.max_width.is_none() && group.max_height.is_none() {
            continue;
        }

        let mut min_x = x;
        let mut min_y = y;
        let mut max_x = x + w as i32;
        let mut max_y = y + h as i32;

        for (pid, px, py, pw, ph) in placed_rooms {
            if group.room_ids.contains(pid) {
                min_x = min_x.min(*px);
                min_y = min_y.min(*py);
                max_x = max_x.max(px + *pw as i32);
                max_y = max_y.max(py + *ph as i32);
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
fn edge_to_edge_manhattan(
    x1: i32, y1: i32, w1: u32, h1: u32,
    x2: i32, y2: i32, w2: u32, h2: u32,
) -> u32 {
    let gap_x = if x1 + w1 as i32 <= x2 {
        (x2 - (x1 + w1 as i32)) as u32
    } else if x2 + w2 as i32 <= x1 {
        (x1 - (x2 + w2 as i32)) as u32
    } else {
        0 // overlapping on x axis
    };
    let gap_y = if y1 + h1 as i32 <= y2 {
        (y2 - (y1 + h1 as i32)) as u32
    } else if y2 + h2 as i32 <= y1 {
        (y1 - (y2 + h2 as i32)) as u32
    } else {
        0 // overlapping on y axis
    };
    gap_x + gap_y
}

/// Check if placing a room violates any connection length constraint.
fn violates_length_constraints(
    room_id: &str,
    x: i32, y: i32, w: u32, h: u32,
    connections: &[StoredEdge],
    placed_rooms: &[(String, i32, i32, u32, u32)],
) -> bool {
    for edge in connections {
        let other_id = if edge.source_room_id == room_id {
            &edge.target_room_id
        } else if edge.target_room_id == room_id {
            &edge.source_room_id
        } else {
            continue;
        };

        let Some((_, ox, oy, ow, oh)) = placed_rooms.iter().find(|(id, _, _, _, _)| id == other_id) else {
            continue;
        };

        let dist = edge_to_edge_manhattan(x, y, w, h, *ox, *oy, *ow, *oh);

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

fn try_place(
    px: i32, py: i32, tw: u32, th: u32,
    room_id: &str,
    placed_rects: &[(i32, i32, u32, u32)],
    placed_rooms: &[(String, i32, i32, u32, u32)],
    gap: u32,
    groups: &[RoomGroup],
    connections: &[StoredEdge],
) -> bool {
    !overlaps_any(px, py, tw, th, placed_rects, gap)
        && !violates_group_constraints(room_id, px, py, tw, th, groups, placed_rooms)
        && !violates_length_constraints(room_id, px, py, tw, th, connections, placed_rooms)
}

/// Try placement with only overlap checking (no constraint checks).
fn try_place_unconstrained(
    px: i32, py: i32, tw: u32, th: u32,
    placed_rects: &[(i32, i32, u32, u32)],
    gap: u32,
) -> bool {
    !overlaps_any(px, py, tw, th, placed_rects, gap)
}

/// Collect violation descriptions for a room placement.
fn collect_violations(
    room_id: &str,
    x: i32, y: i32, w: u32, h: u32,
    connections: &[StoredEdge],
    placed_rooms: &[(String, i32, i32, u32, u32)],
    groups: &[RoomGroup],
    graph: &crate::model::DungeonGraph,
) -> Vec<String> {
    let mut violations = Vec::new();

    for edge in connections {
        let other_id = if edge.source_room_id == room_id {
            &edge.target_room_id
        } else if edge.target_room_id == room_id {
            &edge.source_room_id
        } else {
            continue;
        };

        let Some((_, ox, oy, ow, oh)) = placed_rooms.iter().find(|(id, _, _, _, _)| id == other_id) else {
            continue;
        };

        let dist = edge_to_edge_manhattan(x, y, w, h, *ox, *oy, *ow, *oh);
        let other_label = graph.room_by_id(other_id).map(|r| r.label.as_str()).unwrap_or("?");

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
    for group in groups {
        if !group.room_ids.contains(&room_id.to_string()) {
            continue;
        }
        let mut min_x = x;
        let mut min_y = y;
        let mut max_x = x + w as i32;
        let mut max_y = y + h as i32;
        for (pid, px, py, pw, ph) in placed_rooms {
            if group.room_ids.contains(pid) {
                min_x = min_x.min(*px);
                min_y = min_y.min(*py);
                max_x = max_x.max(px + *pw as i32);
                max_y = max_y.max(py + *ph as i32);
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
fn sort_by_preference(candidates: &mut Vec<(i32, i32)>, pref_x: i32, pref_y: i32) {
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
    let mut layout = SpatialLayout::new();
    let mut placed: HashSet<String> = HashSet::new();
    let mut placed_rects: Vec<(i32, i32, u32, u32)> = Vec::new();
    let mut placed_rooms: Vec<(String, i32, i32, u32, u32)> = Vec::new();

    let groups = &graph.groups;

    // Find entrance room
    let entrance = graph.rooms.iter()
        .find(|r| r.tags.contains(&RoomTag::Entrance))
        .unwrap_or(&graph.rooms[0]);

    let (ew, eh) = entrance.grid_size();
    let entrance_graph_pos = graph_pos.get(&entrance.id).copied().unwrap_or((0.0, 0.0));

    place_room(&mut layout, &mut placed, &mut placed_rects, &mut placed_rooms,
        &entrance.id, 0, 0, ew, eh);

    let mut queue = VecDeque::new();
    if let Some(&start_idx) = node_map.get(&entrance.id) {
        queue.push_back(start_idx);
    }

    loop {
        while let Some(current) = queue.pop_front() {
            let current_id = &pg[current];
            let current_layout = layout.room_by_id(current_id).unwrap();
            let cx = current_layout.x;
            let cy = current_layout.y;
            let cw = current_layout.width;
            let ch = current_layout.height;

            for neighbor_idx in pg.neighbors(current) {
                let neighbor_id = &pg[neighbor_idx];
                if placed.contains(neighbor_id) {
                    continue;
                }

                let neighbor_room = graph.room_by_id(neighbor_id).unwrap();
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
                            if try_place(px, py, tw, th, neighbor_id, &placed_rects, &placed_rooms, 0, groups, &graph.connections) {
                                place_room(&mut layout, &mut placed, &mut placed_rects, &mut placed_rooms,
                                    neighbor_id, px, py, tw, th);
                                queue.push_back(neighbor_idx);
                                did_place = true;
                                break 'orient;
                            }
                        }
                    }

                    // Then try spaced
                    for &(px, py) in &spaced {
                        if try_place(px, py, tw, th, neighbor_id, &placed_rects, &placed_rooms, gap, groups, &graph.connections) {
                            place_room(&mut layout, &mut placed, &mut placed_rects, &mut placed_rooms,
                                neighbor_id, px, py, tw, th);
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
                            if try_place(px, py, tw, th, neighbor_id, &placed_rects, &placed_rooms, gap, groups, &graph.connections) {
                                place_room(&mut layout, &mut placed, &mut placed_rects, &mut placed_rooms,
                                    neighbor_id, px, py, tw, th);
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
                        if try_place_unconstrained(px, py, tw, th, &placed_rects, gap) {
                            let v = collect_violations(
                                neighbor_id, px, py, tw, th,
                                &graph.connections, &placed_rooms, groups, graph,
                            );
                            place_room(&mut layout, &mut placed, &mut placed_rects, &mut placed_rooms,
                                neighbor_id, px, py, tw, th);
                            if let Some(room_layout) = layout.rooms.last_mut() {
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
        let unplaced_room = graph.rooms.iter().find(|r| !placed.contains(&r.id));
        match unplaced_room {
            Some(room) => {
                let (nw, nh) = room.grid_size();
                let mut did_place = false;
                let step = (nh + gap).max(1) as i32;
                'scan: for ring in 0..50 {
                    for sy in (-ring * step..=ring * step).step_by(step as usize) {
                        for sx in (-ring * step..=ring * step).step_by((nw + gap).max(1) as usize) {
                            if try_place(sx, sy, nw, nh, &room.id, &placed_rects, &placed_rooms, gap, groups, &graph.connections) {
                                place_room(&mut layout, &mut placed, &mut placed_rects, &mut placed_rooms,
                                    &room.id, sx, sy, nw, nh);
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
                    placed.insert(room.id.clone());
                }
            }
            None => break,
        }
    }

    layout.corridors = crate::solver::corridor::route_corridors(graph, &layout);

    Ok(layout)
}

fn place_room(
    layout: &mut SpatialLayout,
    placed: &mut HashSet<String>,
    placed_rects: &mut Vec<(i32, i32, u32, u32)>,
    placed_rooms: &mut Vec<(String, i32, i32, u32, u32)>,
    room_id: &str,
    x: i32, y: i32, w: u32, h: u32,
) {
    layout.rooms.push(RoomLayout {
        room_id: room_id.to_string(),
        x, y, width: w, height: h,
        violations: Vec::new(),
    });
    placed.insert(room_id.to_string());
    placed_rects.push((x, y, w, h));
    placed_rooms.push((room_id.to_string(), x, y, w, h));
}

fn overlaps_any(x: i32, y: i32, w: u32, h: u32, rects: &[(i32, i32, u32, u32)], gap: u32) -> bool {
    let g = gap as i32;
    for &(rx, ry, rw, rh) in rects {
        if x < rx + rw as i32 + g
            && x + w as i32 + g > rx
            && y < ry + rh as i32 + g
            && y + h as i32 + g > ry
        {
            return true;
        }
    }
    false
}
