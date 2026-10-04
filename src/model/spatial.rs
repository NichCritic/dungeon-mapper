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
    /// Clockwise rotation in degrees about the room's center. The room's contents
    /// (cave cells, decor, sections) are laid out in its unrotated local frame:
    /// x, y, width, height describe that frame before rotation.
    #[serde(default)]
    pub rotation: f32,
}

impl RoomLayout {
    /// True when rotated by something other than a multiple of 360°.
    pub fn is_rotated(&self) -> bool {
        let r = self.rotation.rem_euclid(360.0);
        r > 1e-3 && r < 360.0 - 1e-3
    }

    /// Center of the room in grid units (the rotation pivot).
    pub fn center(&self) -> (f32, f32) {
        (self.x as f32 + self.width as f32 / 2.0, self.y as f32 + self.height as f32 / 2.0)
    }

    fn sin_cos(&self) -> (f32, f32) {
        self.rotation.to_radians().sin_cos()
    }

    /// Room-local point (offset from the unrotated top-left, grid units) to world.
    pub fn to_world(&self, lx: f32, ly: f32) -> (f32, f32) {
        if !self.is_rotated() {
            // Exact for unrotated rooms (no rounding through the center)
            return (self.x as f32 + lx, self.y as f32 + ly);
        }
        let (cx, cy) = self.center();
        let (dx, dy) = (lx - self.width as f32 / 2.0, ly - self.height as f32 / 2.0);
        let (s, c) = self.sin_cos();
        (cx + dx * c - dy * s, cy + dx * s + dy * c)
    }

    /// World point (grid units) to room-local.
    pub fn to_local(&self, x: f32, y: f32) -> (f32, f32) {
        if !self.is_rotated() {
            return (x - self.x as f32, y - self.y as f32);
        }
        let (cx, cy) = self.center();
        let (dx, dy) = (x - cx, y - cy);
        let (s, c) = self.sin_cos();
        (dx * c + dy * s + self.width as f32 / 2.0, -dx * s + dy * c + self.height as f32 / 2.0)
    }

    /// Corners in world grid units: top-left, top-right, bottom-right, bottom-left
    /// of the unrotated frame.
    pub fn corners(&self) -> [(f32, f32); 4] {
        let (w, h) = (self.width as f32, self.height as f32);
        [self.to_world(0.0, 0.0), self.to_world(w, 0.0), self.to_world(w, h), self.to_world(0.0, h)]
    }

    /// Whether a world point lies inside the room's (rotated) rectangle.
    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        let (lx, ly) = self.to_local(x, y);
        lx >= 0.0 && ly >= 0.0 && lx < self.width as f32 && ly < self.height as f32
    }

    /// World-space bounding box (min_x, min_y, max_x, max_y) in grid units.
    pub fn aabb(&self) -> (f32, f32, f32, f32) {
        if !self.is_rotated() {
            return (self.x as f32, self.y as f32, (self.x + self.width as i32) as f32, (self.y + self.height as i32) as f32);
        }
        self.corners().iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, p| {
            (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))
        })
    }

    /// Grid cells overlapped by the bounding box: (min_x, min_y, max_x, max_y), max exclusive.
    pub fn cell_bounds(&self) -> (i32, i32, i32, i32) {
        let (x0, y0, x1, y1) = self.aabb();
        (x0.floor() as i32, y0.floor() as i32, x1.ceil() as i32, y1.ceil() as i32)
    }
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

impl CorridorSegment {
    /// The cells each run covers, as (min_x, min_y, max_x, max_y) with max exclusive.
    /// Waypoints mark a corridor's cells as `wp - width/2 .. wp - width/2 + width`.
    pub fn run_boxes(&self) -> impl Iterator<Item = (i32, i32, i32, i32)> + '_ {
        let (cw, half) = (self.width as i32, self.width as i32 / 2);
        self.waypoints.windows(2).map(move |w| (
            w[0].x.min(w[1].x) - half,
            w[0].y.min(w[1].y) - half,
            w[0].x.max(w[1].x) - half + cw,
            w[0].y.max(w[1].y) - half + cw,
        ))
    }
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
                if rl.is_rotated() {
                    return rl.contains_point(gx as f32 + 0.5, gy as f32 + 0.5);
                }
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
                    if pair[0].x != pair[1].x && pair[0].y != pair[1].y {
                        cells.extend(super::geometry::swept_cells(pair[0], pair[1], w, 0));
                        continue;
                    }
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
            let (x0, y0, x1, y1) = rl.cell_bounds();
            min_x = min_x.min(x0);
            min_y = min_y.min(y0);
            max_x = max_x.max(x1);
            max_y = max_y.max(y1);
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
            rotation: 0.0,
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
