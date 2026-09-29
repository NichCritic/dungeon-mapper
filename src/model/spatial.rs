use serde::{Deserialize, Serialize};

use super::FloorAssignment;
use super::graph::DungeonGraph;

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
    /// Wall positions where corridors pierce this container room's boundary.
    /// Derived during corridor routing for containment groups.
    #[serde(default)]
    pub wall_openings: Vec<GridPos>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CorridorSegment {
    pub connection_id: String,
    pub waypoints: Vec<GridPos>,
    pub width: u32,
    /// True if this corridor overlaps another on the same floor.
    #[serde(default)]
    pub invalid: bool,
    /// User-pinned waypoints that the solver must route through (in order).
    /// Includes start, any mid-goals, and end. Empty means fully auto-solved.
    #[serde(default)]
    pub pinned_waypoints: Vec<GridPos>,
    /// Which floor(s) this corridor belongs to. Cross-floor connections become
    /// half-floor corridors visible on both floors.
    #[serde(default)]
    pub floor: FloorAssignment,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct GridPos {
    pub x: i32,
    pub y: i32,
}

/// Exit position on a room wall, in grid coordinates with half-grid precision.
/// Allows placement on grid lines (integer) and cell centers (half-integer).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExitPos {
    pub x: f32,
    pub y: f32,
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

    /// Room indices in rendering (z) order: sorted by (max floor, nesting depth),
    /// stable by layout index, so containers draw before their children and
    /// later entries appear on top.
    pub fn render_order(&self, graph: &DungeonGraph) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.rooms.len()).collect();
        // Cached: each key is a room lookup plus a nesting walk, too slow to redo per comparison
        order.sort_by_cached_key(|&i| {
            let room_id = &self.rooms[i].room_id;
            let floor = graph.room_by_id(room_id)
                .map(|r| *r.floor.floors().iter().max().unwrap_or(&0))
                .unwrap_or(0);
            (floor, graph.nesting_depth(room_id))
        });
        order
    }

    /// Find the topmost room (in rendering order) whose footprint contains the
    /// given grid cell, so a click lands on whatever is drawn on top, e.g. a
    /// child room rather than its container.
    pub fn room_at_grid(&self, graph: &DungeonGraph, gx: i32, gy: i32) -> Option<&RoomLayout> {
        self.room_at_grid_ordered(&self.render_order(graph), gx, gy)
    }

    /// Same as [`Self::room_at_grid`] with a precomputed [`Self::render_order`],
    /// for callers that test many cells per frame.
    pub fn room_at_grid_ordered(&self, order: &[usize], gx: i32, gy: i32) -> Option<&RoomLayout> {
        order.iter()
            .rev()
            .map(|&i| &self.rooms[i])
            .find(|rl| {
                gx >= rl.x && gx < rl.x + rl.width as i32
                    && gy >= rl.y && gy < rl.y + rl.height as i32
            })
    }

    /// Recompute the `invalid` flag on all corridors based on grid cell overlap.
    /// Two corridors overlap only if they share a grid cell AND at least one floor.
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

        // Pre-compute floor sets for each corridor
        let corridor_floors: Vec<Vec<i32>> = self.corridors.iter()
            .map(|c| c.floor.floors())
            .collect();

        for i in 0..self.corridors.len() {
            let mut overlaps = false;
            for j in 0..self.corridors.len() {
                if i == j {
                    continue;
                }
                // Skip overlap check if corridors share no floors
                if !corridor_floors[i].iter().any(|f| corridor_floors[j].contains(f)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rl(id: &str, x: i32, y: i32, w: u32, h: u32) -> RoomLayout {
        RoomLayout {
            room_id: id.into(),
            x,
            y,
            width: w,
            height: h,
            violations: Vec::new(),
            wall_openings: Vec::new(),
        }
    }

    fn containment_graph() -> DungeonGraph {
        use super::super::graph::RoomGroup;
        use super::super::room::Room;
        let mut graph = DungeonGraph::new();
        graph.rooms.push(Room::new("container".into()));
        graph.rooms.push(Room::new("child".into()));
        let container_id = graph.rooms[0].id.clone();
        let child_id = graph.rooms[1].id.clone();
        let mut group = RoomGroup::new("g".into());
        group.parent_room_id = Some(container_id);
        group.room_ids.push(child_id);
        graph.groups.push(group);
        graph
    }

    #[test]
    fn room_at_grid_picks_topmost_in_render_order() {
        let graph = containment_graph();
        let container_id = graph.rooms[0].id.clone();
        let child_id = graph.rooms[1].id.clone();
        let mut layout = SpatialLayout::new();
        // Container listed before its child, as happens for containment groups.
        layout.rooms.push(rl(&container_id, 88, 6, 54, 62));
        layout.rooms.push(rl(&child_id, 92, 44, 12, 8));

        assert_eq!(layout.render_order(&graph), vec![0, 1]);
        assert_eq!(layout.room_at_grid(&graph, 95, 46).map(|r| &r.room_id), Some(&child_id));
        assert_eq!(layout.room_at_grid(&graph, 90, 10).map(|r| &r.room_id), Some(&container_id));
        assert!(layout.room_at_grid(&graph, 0, 0).is_none());
        // Exclusive far edge of the child.
        assert_eq!(layout.room_at_grid(&graph, 104, 44).map(|r| &r.room_id), Some(&container_id));

        // Same result when the child happens to be listed first: nesting depth wins.
        layout.rooms.swap(0, 1);
        assert_eq!(layout.render_order(&graph), vec![1, 0]);
        assert_eq!(layout.room_at_grid(&graph, 95, 46).map(|r| &r.room_id), Some(&child_id));
    }
}
