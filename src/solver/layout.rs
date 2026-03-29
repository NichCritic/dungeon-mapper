use std::collections::{HashSet, VecDeque};

use crate::model::*;

/// BFS greedy placer: takes a DungeonGraph and produces a SpatialLayout.
pub fn solve_layout(
    graph: &DungeonGraph,
    gap: u32,
) -> Result<SpatialLayout, String> {
    if graph.rooms.is_empty() {
        return Err("No rooms to layout".to_string());
    }

    let (pg, node_map) = graph.build_petgraph();
    let mut layout = SpatialLayout::new();
    let mut placed: HashSet<String> = HashSet::new();
    let mut placed_rects: Vec<(i32, i32, u32, u32)> = Vec::new();

    // Find entrance room
    let entrance = graph
        .rooms
        .iter()
        .find(|r| r.tags.contains(&RoomTag::Entrance))
        .unwrap_or(&graph.rooms[0]);

    let (ew, eh) = entrance.grid_size();

    // Place entrance near origin
    let start_x = 0_i32;
    let start_y = 0_i32;

    layout.rooms.push(RoomLayout {
        room_id: entrance.id.clone(),
        x: start_x,
        y: start_y,
        width: ew,
        height: eh,
    });
    placed.insert(entrance.id.clone());
    placed_rects.push((start_x, start_y, ew, eh));

    // BFS from entrance, then handle any disconnected components
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

                // Build list of orientations to try
                let mut orientations = vec![(nw, nh)];
                if neighbor_room.allow_rotation && nw != nh {
                    orientations.push((nh, nw));
                }

                // Max corridor width among connections to this neighbor
                let cw_i = graph.connections.iter()
                    .filter(|e| {
                        (e.source_room_id == *current_id && e.target_room_id == *neighbor_id)
                        || (e.target_room_id == *current_id && e.source_room_id == *neighbor_id)
                    })
                    .map(|e| e.connection.corridor_width as i32)
                    .max()
                    .unwrap_or(2);
                let g = gap as i32;

                let mut did_place = false;
                'orient: for &(tw, th) in &orientations {
                    let adjacent_candidates = [
                        (cx + cw as i32, cy),
                        (cx, cy + ch as i32),
                        (cx - tw as i32, cy),
                        (cx, cy - th as i32),
                    ];
                    let spaced_candidates = [
                        (cx + cw as i32 + g + cw_i, cy),
                        (cx, cy + ch as i32 + g + cw_i),
                        (cx - tw as i32 - g - cw_i, cy),
                        (cx, cy - th as i32 - g - cw_i),
                    ];

                    // Try adjacent first (only when gap is 0)
                    if g == 0 {
                        for &(px, py) in &adjacent_candidates {
                            if !overlaps_any(px, py, tw, th, &placed_rects, 0) {
                                layout.rooms.push(RoomLayout {
                                    room_id: neighbor_id.clone(),
                                    x: px,
                                    y: py,
                                    width: tw,
                                    height: th,
                                });
                                placed.insert(neighbor_id.clone());
                                placed_rects.push((px, py, tw, th));
                                queue.push_back(neighbor_idx);
                                did_place = true;
                                break 'orient;
                            }
                        }
                    }

                    // Spaced: with gap for corridor room
                    for &(px, py) in &spaced_candidates {
                        if !overlaps_any(px, py, tw, th, &placed_rects, gap) {
                            layout.rooms.push(RoomLayout {
                                room_id: neighbor_id.clone(),
                                x: px,
                                y: py,
                                width: tw,
                                height: th,
                            });
                            placed.insert(neighbor_id.clone());
                            placed_rects.push((px, py, tw, th));
                            queue.push_back(neighbor_idx);
                            did_place = true;
                            break 'orient;
                        }
                    }
                }

                if !did_place {
                    let (tw, th) = orientations[0]; // use primary orientation for offset attempts
                    'outer: for offset_mult in 2..=10 {
                        let om = offset_mult as i32;
                        let extra_candidates = [
                            (cx + (cw as i32 + g + cw_i) * om, cy),
                            (cx, cy + (ch as i32 + g + cw_i) * om),
                            (cx - (tw as i32 + g + cw_i) * om, cy),
                            (cx, cy - (th as i32 + g + cw_i) * om),
                        ];
                        for &(px, py) in &extra_candidates {
                            if !overlaps_any(px, py, tw, th, &placed_rects, gap) {
                                layout.rooms.push(RoomLayout {
                                    room_id: neighbor_id.clone(),
                                    x: px,
                                    y: py,
                                    width: tw,
                                    height: th,
                                });
                                placed.insert(neighbor_id.clone());
                                placed_rects.push((px, py, tw, th));
                                queue.push_back(neighbor_idx);
                                did_place = true;
                                break 'outer;
                            }
                        }
                    }
                }

                if !did_place {
                    eprintln!("Warning: Could not place room '{}'", neighbor_room.label);
                }
            }
        }

        // Check for unplaced rooms (disconnected components or isolated nodes)
        let unplaced_room = graph.rooms.iter().find(|r| !placed.contains(&r.id));
        match unplaced_room {
            Some(room) => {
                let (nw, nh) = room.grid_size();
                // Find a free spot by scanning outward from origin
                let mut did_place = false;
                let step = (nh + gap) as i32;
                'scan: for ring in 0..50 {
                    for sy in (-ring * step..=ring * step).step_by(step as usize) {
                        for sx in (-ring * step..=ring * step).step_by((nw + gap) as usize) {
                            if !overlaps_any(sx, sy, nw, nh, &placed_rects, gap) {
                                layout.rooms.push(RoomLayout {
                                    room_id: room.id.clone(),
                                    x: sx,
                                    y: sy,
                                    width: nw,
                                    height: nh,
                                });
                                placed.insert(room.id.clone());
                                placed_rects.push((sx, sy, nw, nh));
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

    // Route corridors
    layout.corridors = crate::solver::corridor::route_corridors(graph, &layout);

    Ok(layout)
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
