use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomLayout {
    pub room_id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Constraint violations for this room's placement.
    #[serde(default)]
    pub violations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CorridorSegment {
    pub connection_id: String,
    pub waypoints: Vec<GridPos>,
    pub width: u32,
    /// True if this corridor overlaps another.
    #[serde(default)]
    pub invalid: bool,
    /// User-pinned waypoints that the solver must route through (in order).
    /// Includes start, any mid-goals, and end. Empty means fully auto-solved.
    #[serde(default)]
    pub pinned_waypoints: Vec<GridPos>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct GridPos {
    pub x: i32,
    pub y: i32,
}

/// A first-class bounds rectangle that can be placed on the map.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoundsRect {
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpatialLayout {
    pub rooms: Vec<RoomLayout>,
    pub corridors: Vec<CorridorSegment>,
    pub bounds: Vec<BoundsRect>,
}

impl SpatialLayout {
    pub fn new() -> Self {
        Self {
            rooms: Vec::new(),
            corridors: Vec::new(),
            bounds: Vec::new(),
        }
    }

    pub fn room_by_id(&self, room_id: &str) -> Option<&RoomLayout> {
        self.rooms.iter().find(|r| r.room_id == room_id)
    }

    pub fn room_by_id_mut(&mut self, room_id: &str) -> Option<&mut RoomLayout> {
        self.rooms.iter_mut().find(|r| r.room_id == room_id)
    }

    /// Recompute the `invalid` flag on all corridors based on grid cell overlap.
    /// Two corridors overlap if they share any grid cell.
    pub fn recheck_corridor_overlaps(&mut self) {
        use std::collections::HashSet;

        // Compute the grid cells each corridor occupies
        let corridor_cells: Vec<HashSet<(i32, i32)>> = self.corridors.iter()
            .map(|c| {
                let w = c.width as i32;
                let mut cells = HashSet::new();
                for pair in c.waypoints.windows(2) {
                    let min_x = pair[0].x.min(pair[1].x);
                    let max_x = pair[0].x.max(pair[1].x);
                    let min_y = pair[0].y.min(pair[1].y);
                    let max_y = pair[0].y.max(pair[1].y);
                    for y in min_y..=(max_y + w - 1) {
                        for x in min_x..=(max_x + w - 1) {
                            cells.insert((x, y));
                        }
                    }
                }
                cells
            })
            .collect();

        for i in 0..self.corridors.len() {
            let mut overlaps = false;
            for j in 0..self.corridors.len() {
                if i == j {
                    continue;
                }
                if !corridor_cells[i].is_disjoint(&corridor_cells[j]) {
                    overlaps = true;
                    break;
                }
            }
            self.corridors[i].invalid = overlaps;
        }
    }

    /// Compute the bounding box of all rooms and corridors.
    /// Returns (min_x, min_y, max_x, max_y) in grid coordinates.
    pub fn extents(&self) -> (i32, i32, i32, i32) {
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;

        for rl in &self.rooms {
            min_x = min_x.min(rl.x);
            min_y = min_y.min(rl.y);
            max_x = max_x.max(rl.x + rl.width as i32);
            max_y = max_y.max(rl.y + rl.height as i32);
        }

        for corridor in &self.corridors {
            for wp in &corridor.waypoints {
                min_x = min_x.min(wp.x - corridor.width as i32);
                min_y = min_y.min(wp.y - corridor.width as i32);
                max_x = max_x.max(wp.x + corridor.width as i32);
                max_y = max_y.max(wp.y + corridor.width as i32);
            }
        }

        for b in &self.bounds {
            min_x = min_x.min(b.x);
            min_y = min_y.min(b.y);
            max_x = max_x.max(b.x + b.width as i32);
            max_y = max_y.max(b.y + b.height as i32);
        }

        if min_x > max_x {
            (0, 0, 10, 10)
        } else {
            (min_x, min_y, max_x, max_y)
        }
    }
}

impl Default for SpatialLayout {
    fn default() -> Self {
        Self::new()
    }
}
