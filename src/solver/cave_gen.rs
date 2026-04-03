use rand::prelude::*;
use std::collections::{HashSet, VecDeque};

use crate::model::CaveAlgorithm;

/// Generate cave cell data.
/// Returns a `Vec<bool>` of length `width * height`, indexed as `y * width + x`.
/// `exits` are local cell coordinates that must be floor (corridor connection points).
pub fn generate_cave(
    width: u32,
    height: u32,
    algorithm: CaveAlgorithm,
    seed: u64,
    density: f32,
    smoothing_iterations: u32,
    exits: &[(u32, u32)],
) -> Vec<bool> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return Vec::new();
    }

    let mut cells = match algorithm {
        CaveAlgorithm::CellularAutomata => {
            cellular_automata(w, h, seed, density, smoothing_iterations, exits)
        }
        CaveAlgorithm::DrunkardsWalk => {
            drunkards_walk(w, h, seed, density, exits)
        }
    };

    // Force exit cells to floor
    for &(ex, ey) in exits {
        let idx = ey as usize * w + ex as usize;
        if idx < cells.len() {
            cells[idx] = true;
        }
    }

    // Ensure all exit cells are connected
    ensure_connectivity(&mut cells, w, h, exits);

    cells
}

fn cellular_automata(
    w: usize,
    h: usize,
    seed: u64,
    density: f32,
    iterations: u32,
    exits: &[(u32, u32)],
) -> Vec<bool> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut cells = vec![false; w * h];

    // Random fill
    for y in 0..h {
        for x in 0..w {
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                cells[y * w + x] = false; // border = wall
            } else {
                cells[y * w + x] = rng.gen::<f32>() < density;
            }
        }
    }

    // Force exits and their neighbors to floor
    for &(ex, ey) in exits {
        let ex = ex as usize;
        let ey = ey as usize;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let nx = ex as i32 + dx;
                let ny = ey as i32 + dy;
                if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                    cells[ny as usize * w + nx as usize] = true;
                }
            }
        }
    }

    // Smoothing passes (4-5 rule)
    let exit_set: HashSet<(usize, usize)> = exits.iter().map(|&(x, y)| (x as usize, y as usize)).collect();
    for _ in 0..iterations {
        let mut next = cells.clone();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if exit_set.contains(&(x, y)) {
                    continue; // never change exit cells
                }
                let walls = count_wall_neighbors(&cells, w, h, x, y);
                if walls >= 5 {
                    next[y * w + x] = false;
                } else if walls < 4 {
                    next[y * w + x] = true;
                }
            }
        }
        cells = next;
    }

    cells
}

fn drunkards_walk(
    w: usize,
    h: usize,
    seed: u64,
    density: f32,
    exits: &[(u32, u32)],
) -> Vec<bool> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut cells = vec![false; w * h];
    let total_budget = ((w * h) as f32 * density) as usize;

    if exits.is_empty() {
        // No exits — start from center
        let cx = w / 2;
        let cy = h / 2;
        walk(&mut cells, w, h, cx, cy, total_budget, &mut rng);
    } else {
        let budget_per_walker = total_budget / exits.len().max(1);
        for &(ex, ey) in exits {
            walk(&mut cells, w, h, ex as usize, ey as usize, budget_per_walker, &mut rng);
        }
    }

    cells
}

fn walk(
    cells: &mut [bool],
    w: usize,
    h: usize,
    start_x: usize,
    start_y: usize,
    steps: usize,
    rng: &mut StdRng,
) {
    let mut x = start_x;
    let mut y = start_y;
    let dirs: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

    for _ in 0..steps {
        if x < w && y < h {
            cells[y * w + x] = true;
        }
        let &(dx, dy) = dirs.choose(rng).unwrap();
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        // Stay 1 cell from border
        if nx >= 1 && ny >= 1 && (nx as usize) < w - 1 && (ny as usize) < h - 1 {
            x = nx as usize;
            y = ny as usize;
        }
    }
}

fn count_wall_neighbors(cells: &[bool], w: usize, h: usize, x: usize, y: usize) -> usize {
    let mut count = 0;
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                count += 1; // out of bounds = wall
            } else if !cells[ny as usize * w + nx as usize] {
                count += 1;
            }
        }
    }
    count
}

/// Ensure all exit cells are connected. If disconnected regions exist,
/// carve shortest-path tunnels to connect them.
fn ensure_connectivity(cells: &mut Vec<bool>, w: usize, h: usize, exits: &[(u32, u32)]) {
    if exits.len() < 2 {
        return;
    }

    // Find which component each exit belongs to
    let mut visited = vec![false; w * h];
    let mut components: Vec<HashSet<(usize, usize)>> = Vec::new();
    let mut exit_component: Vec<usize> = vec![0; exits.len()];

    for (ei, &(ex, ey)) in exits.iter().enumerate() {
        let ex = ex as usize;
        let ey = ey as usize;
        if visited[ey * w + ex] {
            // Already part of a component
            for (ci, comp) in components.iter().enumerate() {
                if comp.contains(&(ex, ey)) {
                    exit_component[ei] = ci;
                    break;
                }
            }
            continue;
        }
        // BFS flood fill from this exit
        let mut component = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back((ex, ey));
        visited[ey * w + ex] = true;
        while let Some((cx, cy)) = queue.pop_front() {
            component.insert((cx, cy));
            for &(dx, dy) in &[(0i32, -1i32), (0, 1), (-1, 0), (1, 0)] {
                let nx = cx as i32 + dx;
                let ny = cy as i32 + dy;
                if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                    let ni = ny as usize * w + nx as usize;
                    if cells[ni] && !visited[ni] {
                        visited[ni] = true;
                        queue.push_back((nx as usize, ny as usize));
                    }
                }
            }
        }
        exit_component[ei] = components.len();
        components.push(component);
    }

    // Connect all components to component 0 by carving straight-line tunnels
    let target = exit_component[0];
    for ei in 1..exits.len() {
        if exit_component[ei] == target {
            continue;
        }
        // Carve a tunnel from this exit to the first exit
        let (x0, y0) = (exits[0].0 as i32, exits[0].1 as i32);
        let (x1, y1) = (exits[ei].0 as i32, exits[ei].1 as i32);
        // L-shaped tunnel: horizontal then vertical
        let min_x = x0.min(x1);
        let max_x = x0.max(x1);
        let min_y = y0.min(y1);
        let max_y = y0.max(y1);
        // Horizontal segment at y0
        for x in min_x..=max_x {
            if x >= 0 && (x as usize) < w && (y0 as usize) < h {
                cells[y0 as usize * w + x as usize] = true;
            }
        }
        // Vertical segment at x1
        for y in min_y..=max_y {
            if y >= 0 && (y as usize) < h && (x1 as usize) < w {
                cells[y as usize * w + x1 as usize] = true;
            }
        }
        exit_component[ei] = target;
    }
}

/// Compute marching squares contour segments for a cave room using the global floor set.
/// This produces smooth wall contours that seamlessly merge with adjacent caves and corridors.
/// Segments are in world pixel coordinates.
pub fn compute_contour_segments(
    rl: &crate::model::RoomLayout,
    cave: &crate::model::CaveData,
    floor: &std::collections::HashSet<(i32, i32)>,
) -> Vec<(f32, f32, f32, f32)> {
    use crate::util::GRID_PX;

    let w = rl.width as i32;
    let h = rl.height as i32;
    let mut segments = Vec::new();

    let is_floor = |gx: i32, gy: i32| -> bool {
        let lx = gx - rl.x;
        let ly = gy - rl.y;
        if lx >= 0 && ly >= 0 && lx < w && ly < h {
            cave.cells.get((ly * w + lx) as usize).copied().unwrap_or(false)
        } else {
            floor.contains(&(gx, gy))
        }
    };

    for by in (rl.y - 1)..(rl.y + h) {
        for bx in (rl.x - 1)..(rl.x + w) {
            let tl = is_floor(bx, by) as u8;
            let tr = is_floor(bx + 1, by) as u8;
            let br = is_floor(bx + 1, by + 1) as u8;
            let bl = is_floor(bx, by + 1) as u8;
            let index = (tl << 3) | (tr << 2) | (br << 1) | bl;
            if index == 0 || index == 15 { continue; }

            let cx = (bx + 1) as f32 * GRID_PX;
            let cy = (by + 1) as f32 * GRID_PX;
            let half = GRID_PX / 2.0;
            let top = (cx, cy - half);
            let right = (cx + half, cy);
            let bottom = (cx, cy + half);
            let left = (cx - half, cy);

            let segs: &[((f32, f32), (f32, f32))] = match index {
                1  => &[(left, bottom)],
                2  => &[(bottom, right)],
                3  => &[(left, right)],
                4  => &[(top, right)],
                5  => &[(top, left), (bottom, right)],
                6  => &[(top, bottom)],
                7  => &[(top, left)],
                8  => &[(top, left)],
                9  => &[(top, bottom)],
                10 => &[(top, right), (left, bottom)],
                11 => &[(top, right)],
                12 => &[(left, right)],
                13 => &[(bottom, right)],
                14 => &[(left, bottom)],
                _ => &[],
            };
            for &((x1, y1), (x2, y2)) in segs {
                segments.push((x1, y1, x2, y2));
            }
        }
    }
    segments
}

/// Compute local exit cell coordinates for a room from its corridor connections.
pub fn compute_exit_cells(
    room_id: &str,
    layout: &crate::model::SpatialLayout,
    graph: &crate::model::DungeonGraph,
) -> Vec<(u32, u32)> {
    let Some(rl) = layout.room_by_id(room_id) else { return Vec::new() };
    let mut exits = Vec::new();

    // Find connections involving this room
    for edge in &graph.connections {
        let is_src = edge.source_room_id == room_id;
        let is_tgt = edge.target_room_id == room_id;
        if !is_src && !is_tgt {
            continue;
        }

        // Find the corridor for this connection
        let corridor = layout.corridors.iter().find(|c| c.connection_id == edge.connection.id);
        let Some(corridor) = corridor else { continue };
        if corridor.waypoints.is_empty() {
            continue;
        }

        // The endpoint waypoint closest to this room
        let wp = if is_src {
            &corridor.waypoints[0]
        } else {
            corridor.waypoints.last().unwrap()
        };

        // Convert global grid coords to local room coords
        let lx = wp.x - rl.x;
        let ly = wp.y - rl.y;
        // Clamp to room bounds
        let lx = lx.max(0).min(rl.width as i32 - 1) as u32;
        let ly = ly.max(0).min(rl.height as i32 - 1) as u32;
        exits.push((lx, ly));
    }

    exits
}
