//! Walls where two connected rooms overlap. A connection's [`OverlapWalls`] says whose
//! outline survives inside the overlap; a room whose walls are hidden loses every part
//! of its outline that lies strictly inside the other room. Shared by the map renderer,
//! the player view and line of sight so they always agree. Everything here is in grid
//! units.

use crate::model::{DungeonGraph, Room, RoomLayout, RoomShape, SpatialLayout};

pub type Pt = (f32, f32);

/// The open interior of a room: the region its own walls bound.
pub enum Interior<'a> {
    Rect { x0: f32, y0: f32, x1: f32, y1: f32 },
    Circle { cx: f32, cy: f32, r: f32 },
    /// Cave floor cells over the room's footprint.
    Cells { x: i32, y: i32, w: i32, h: i32, cells: &'a [bool] },
    /// Any simple polygon: a rotated room, or a freeform corridor's floor.
    Polygon(Vec<Pt>),
    /// Cave floor cells of a rotated room, in its local frame.
    RotatedCells { rl: &'a RoomLayout, cells: &'a [bool] },
}

impl Interior<'_> {
    pub fn of<'a>(rl: &'a RoomLayout, room: Option<&'a Room>) -> Interior<'a> {
        if rl.is_rotated() {
            return match room.map(|r| r.shape).unwrap_or_default() {
                // A circle is unchanged by turning about its center
                RoomShape::Circle => {
                    let (cx, cy) = rl.center();
                    Interior::Circle { cx, cy, r: rl.width.min(rl.height) as f32 / 2.0 }
                }
                RoomShape::Cave => match room.and_then(|r| r.cave_data.as_ref()) {
                    Some(cave) if cave.cells.len() == (rl.width * rl.height) as usize => Interior::RotatedCells { rl, cells: &cave.cells },
                    _ => Interior::Polygon(rl.corners().to_vec()),
                },
                RoomShape::Rectangle => Interior::Polygon(rl.corners().to_vec()),
            };
        }
        let (x0, y0) = (rl.x as f32, rl.y as f32);
        let (x1, y1) = (x0 + rl.width as f32, y0 + rl.height as f32);
        match room.map(|r| r.shape).unwrap_or_default() {
            RoomShape::Circle => Interior::Circle {
                cx: (x0 + x1) / 2.0,
                cy: (y0 + y1) / 2.0,
                r: rl.width.min(rl.height) as f32 / 2.0,
            },
            RoomShape::Cave => match room.and_then(|r| r.cave_data.as_ref()) {
                Some(cave) if cave.cells.len() == (rl.width * rl.height) as usize => Interior::Cells {
                    x: rl.x, y: rl.y, w: rl.width as i32, h: rl.height as i32, cells: &cave.cells,
                },
                _ => Interior::Rect { x0, y0, x1, y1 },
            },
            RoomShape::Rectangle => Interior::Rect { x0, y0, x1, y1 },
        }
    }

    fn cell_is_floor(&self, gx: i32, gy: i32) -> bool {
        match *self {
            Interior::Cells { x, y, w, h, cells } => {
                let (lx, ly) = (gx - x, gy - y);
                lx >= 0 && ly >= 0 && lx < w && ly < h && cells[(ly * w + lx) as usize]
            }
            _ => false,
        }
    }

    /// Sub-ranges of `t` in [0, 1] where `a + t (b - a)` lies strictly inside.
    fn inside_spans(&self, a: Pt, b: Pt, out: &mut Vec<(f32, f32)>) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        match *self {
            Interior::Polygon(ref poly) => {
                // Split where the segment crosses an edge; each piece is wholly in or out.
                // Pieces lying on an edge count as outside (the interior is open).
                let n = poly.len();
                let mut ts = vec![0.0, 1.0];
                for i in 0..n {
                    let (c, d) = (poly[i], poly[(i + 1) % n]);
                    let (ex, ey) = (d.0 - c.0, d.1 - c.1);
                    let den = dx * ey - dy * ex;
                    if den.abs() < 1e-12 {
                        continue;
                    }
                    let t = ((c.0 - a.0) * ey - (c.1 - a.1) * ex) / den;
                    let u = ((c.0 - a.0) * dy - (c.1 - a.1) * dx) / den;
                    if t > 0.0 && t < 1.0 && (-1e-6..=1.0 + 1e-6).contains(&u) {
                        ts.push(t);
                    }
                }
                ts.sort_by(|x, y| x.partial_cmp(y).unwrap());
                for w in ts.windows(2) {
                    if w[1] - w[0] < 1e-6 {
                        continue;
                    }
                    let tm = (w[0] + w[1]) / 2.0;
                    let m = (a.0 + dx * tm, a.1 + dy * tm);
                    let on_edge = (0..n).any(|i| crate::model::geometry::dist_to_segment(m, poly[i], poly[(i + 1) % n]) < 1e-4);
                    if !on_edge && crate::model::geometry::point_in_polygon(m, poly) {
                        out.push((w[0], w[1]));
                    }
                }
            }
            Interior::RotatedCells { rl, cells } => {
                // The room transform is rigid, so the segment's parameter is the same locally
                let (la, lb) = (rl.to_local(a.0, a.1), rl.to_local(b.0, b.1));
                Interior::Cells { x: 0, y: 0, w: rl.width as i32, h: rl.height as i32, cells }.inside_spans(la, lb, out);
            }
            Interior::Rect { x0, y0, x1, y1 } => {
                // Open box: a segment lying on the boundary is not inside
                let axis = |p: f32, d: f32, lo: f32, hi: f32| -> Option<(f32, f32)> {
                    if d.abs() < 1e-9 {
                        (p > lo + 1e-6 && p < hi - 1e-6).then_some((f32::MIN, f32::MAX))
                    } else {
                        let (t0, t1) = ((lo - p) / d, (hi - p) / d);
                        Some((t0.min(t1), t0.max(t1)))
                    }
                };
                if let (Some(tx), Some(ty)) = (axis(a.0, dx, x0, x1), axis(a.1, dy, y0, y1)) {
                    let (t0, t1) = (tx.0.max(ty.0).max(0.0), tx.1.min(ty.1).min(1.0));
                    if t1 > t0 {
                        out.push((t0, t1));
                    }
                }
            }
            Interior::Circle { cx, cy, r } => {
                let (fx, fy) = (a.0 - cx, a.1 - cy);
                let qa = dx * dx + dy * dy;
                if qa < 1e-12 {
                    return;
                }
                let qb = 2.0 * (fx * dx + fy * dy);
                let qc = fx * fx + fy * fy - r * r;
                let disc = qb * qb - 4.0 * qa * qc;
                if disc <= 0.0 {
                    return;
                }
                let s = disc.sqrt();
                let (t0, t1) = (((-qb - s) / (2.0 * qa)).max(0.0), ((-qb + s) / (2.0 * qa)).min(1.0));
                if t1 > t0 {
                    out.push((t0, t1));
                }
            }
            Interior::Cells { .. } => {
                // Split at every grid line the segment crosses; each piece lies in one cell
                // (or along a grid line, where it is inside only if both sides are floor).
                let mut ts = vec![0.0, 1.0];
                for (p, d) in [(a.0, dx), (a.1, dy)] {
                    if d.abs() > 1e-9 {
                        let (lo, hi) = (p.min(p + d), p.max(p + d));
                        for g in (lo.ceil() as i32)..=(hi.floor() as i32) {
                            let t = (g as f32 - p) / d;
                            if t > 0.0 && t < 1.0 {
                                ts.push(t);
                            }
                        }
                    }
                }
                ts.sort_by(|x, y| x.partial_cmp(y).unwrap());
                for w in ts.windows(2) {
                    let (t0, t1) = (w[0], w[1]);
                    if t1 - t0 < 1e-6 {
                        continue;
                    }
                    let tm = (t0 + t1) / 2.0;
                    let (mx, my) = (a.0 + dx * tm, a.1 + dy * tm);
                    let on_x_line = (mx - mx.round()).abs() < 1e-4;
                    let on_y_line = (my - my.round()).abs() < 1e-4;
                    let inside = if on_x_line {
                        let gx = mx.round() as i32;
                        let gy = my.floor() as i32;
                        self.cell_is_floor(gx - 1, gy) && self.cell_is_floor(gx, gy)
                    } else if on_y_line {
                        let gx = mx.floor() as i32;
                        let gy = my.round() as i32;
                        self.cell_is_floor(gx, gy - 1) && self.cell_is_floor(gx, gy)
                    } else {
                        self.cell_is_floor(mx.floor() as i32, my.floor() as i32)
                    };
                    if inside {
                        out.push((t0, t1));
                    }
                }
            }
        }
    }
}

/// Interiors of the rooms that hide parts of `room_id`'s walls: the other end of each
/// connection whose rooms overlap on a shared floor and whose [`OverlapWalls`] hides
/// this room's walls.
pub fn wall_suppressors<'a>(room_id: &str, graph: &'a DungeonGraph, layout: &'a SpatialLayout) -> Vec<Interior<'a>> {
    overlap_partners(room_id, graph, layout, true)
}

/// Interiors of every room overlapping `room_id` through a connection, whatever the
/// wall mode. Line of sight uses this to find cave walls standing on another room's floor.
pub fn overlap_partners<'a>(room_id: &str, graph: &'a DungeonGraph, layout: &'a SpatialLayout, only_hiding: bool) -> Vec<Interior<'a>> {
    let mut out = Vec::new();
    let Some(rl) = layout.room_by_id(room_id) else { return out };
    let floor = graph.room_by_id(room_id).map(|r| r.floor);
    for e in &graph.connections {
        let is_source = e.source_room_id == room_id;
        if !is_source && e.target_room_id != room_id {
            continue;
        }
        if only_hiding && !e.connection.overlap_walls.hides(is_source) {
            continue;
        }
        let other_id = if is_source { &e.target_room_id } else { &e.source_room_id };
        let Some(orl) = layout.room_by_id(other_id) else { continue };
        let other = graph.room_by_id(other_id);
        if !rects_overlap(rl, orl) {
            continue;
        }
        if let (Some(f), Some(o)) = (floor, other) {
            if !f.shares_floor(&o.floor) {
                continue;
            }
        }
        out.push(Interior::of(orl, other));
    }
    out
}

/// Floors of the freeform corridors ending at `room_id` (for which `include` holds).
/// Each reaches half a width into the room, so it opens the room's wall where it meets it.
pub fn attached_corridor_interiors(
    room_id: &str,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    include: impl Fn(&crate::model::StoredEdge) -> bool,
) -> Vec<Interior<'static>> {
    let mut out = Vec::new();
    for e in &graph.connections {
        if (e.source_room_id != room_id && e.target_room_id != room_id) || !include(e) {
            continue;
        }
        let Some(c) = layout.corridor_for(&e.connection.id) else { continue };
        if let Some(shape) = crate::model::geometry::corridor_shape(c, layout, graph) {
            out.push(Interior::Polygon(shape.polygon));
        }
    }
    out
}

/// The visible walls of freeform corridor `ci` (grid units): its sides, minus the parts
/// inside rooms or other corridors. Like the cell-based corridor walls (which skip any
/// floor cell), this ignores floors: a single-floor view only holds that floor anyway.
pub fn freeform_corridor_walls(
    ci: usize,
    shapes: &[Option<crate::model::geometry::CorridorShape>],
    graph: &DungeonGraph,
    layout: &SpatialLayout,
) -> Vec<(Pt, Pt)> {
    let Some(shape) = &shapes[ci] else { return Vec::new() };
    let (bx0, by0, bx1, by1) = crate::model::geometry::bounds(&shape.polygon);
    let near = |x0: f32, y0: f32, x1: f32, y1: f32| x1 >= bx0 && x0 <= bx1 && y1 >= by0 && y0 <= by1;
    let mut hidden_by: Vec<Interior<'_>> = Vec::new();
    for rl in &layout.rooms {
        let (x0, y0, x1, y1) = rl.aabb();
        if near(x0, y0, x1, y1) {
            hidden_by.push(Interior::of(rl, graph.room_by_id(&rl.room_id)));
        }
    }
    for (j, other) in layout.corridors.iter().enumerate() {
        if j == ci {
            continue;
        }
        match &shapes[j] {
            Some(s) => {
                let (x0, y0, x1, y1) = crate::model::geometry::bounds(&s.polygon);
                if near(x0, y0, x1, y1) {
                    hidden_by.push(Interior::Polygon(s.polygon.clone()));
                }
            }
            None => {
                for (x0, y0, x1, y1) in other.run_boxes() {
                    let (x0, y0, x1, y1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
                    if near(x0, y0, x1, y1) {
                        hidden_by.push(Interior::Rect { x0, y0, x1, y1 });
                    }
                }
            }
        }
    }
    // Its own floor too: where runs overlap (a short jog, a sharp turn) one run's side
    // lies inside another's floor. A wall on a piece's edge is not inside it, so this
    // removes only the overlaps and leaves the outline.
    hidden_by.extend(shape.floor_pieces().into_iter().map(Interior::Polygon));
    shape.wall_segments().into_iter().flat_map(|(a, b)| visible_parts(a, b, &hidden_by)).collect()
}

/// Connections drawn as an opening in the room wall rather than a door on it.
pub fn is_open_passage(e: &crate::model::StoredEdge) -> bool {
    e.connection.connection_type.is_passage()
}

/// Whether two room footprints overlap with positive area (rotated rooms by their
/// actual outlines).
pub fn rects_overlap(a: &RoomLayout, b: &RoomLayout) -> bool {
    if a.is_rotated() || b.is_rotated() {
        let (ax0, ay0, ax1, ay1) = a.aabb();
        let (bx0, by0, bx1, by1) = b.aabb();
        return ax0 < bx1 && bx0 < ax1 && ay0 < by1 && by0 < ay1
            && crate::model::geometry::polygons_overlap(&a.corners(), &b.corners());
    }
    a.x < b.x + b.width as i32 && b.x < a.x + a.width as i32
        && a.y < b.y + b.height as i32 && b.y < a.y + a.height as i32
}

/// Whether `p` lies strictly inside any of the interiors.
pub fn point_inside_any(p: Pt, interiors: &[Interior<'_>]) -> bool {
    let mut spans = Vec::new();
    // A tiny segment through the point reuses the span logic for every shape
    let (a, b) = ((p.0 - 1e-3, p.1 - 1e-4), (p.0 + 1e-3, p.1 + 1e-4));
    interiors.iter().any(|i| {
        spans.clear();
        i.inside_spans(a, b, &mut spans);
        spans.iter().any(|&(t0, t1)| t0 <= 0.5 && t1 >= 0.5)
    })
}

/// The parts of segment `a`–`b` not hidden by any interior.
pub fn visible_parts(a: Pt, b: Pt, hidden_by: &[Interior<'_>]) -> Vec<(Pt, Pt)> {
    if hidden_by.is_empty() {
        return vec![(a, b)];
    }
    let mut spans = Vec::new();
    for i in hidden_by {
        i.inside_spans(a, b, &mut spans);
    }
    if spans.is_empty() {
        return vec![(a, b)];
    }
    spans.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    let at = |t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    let mut out = Vec::new();
    let mut cur = 0.0;
    for (t0, t1) in spans {
        if t0 > cur + 1e-5 {
            out.push((at(cur), at(t0)));
        }
        cur = f32::max(cur, t1);
    }
    if cur < 1.0 - 1e-5 {
        out.push((at(cur), b));
    }
    out
}

/// A circle's rim as segments, fine enough to clip against other rooms.
pub fn circle_rim(cx: f32, cy: f32, r: f32) -> Vec<(Pt, Pt)> {
    let n = 48;
    let pt = |i: usize| {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        (cx + r * a.cos(), cy + r * a.sin())
    };
    (0..n).map(|i| (pt(i), pt(i + 1))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len(parts: &[(Pt, Pt)]) -> f32 {
        parts.iter().map(|(a, b)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()).sum()
    }

    #[test]
    fn rect_hides_only_the_strictly_inside_part() {
        let rect = [Interior::Rect { x0: 2.0, y0: 0.0, x1: 6.0, y1: 4.0 }];
        // Horizontal wall crossing the rect: middle 4 units hidden
        let parts = visible_parts((0.0, 2.0), (10.0, 2.0), &rect);
        assert_eq!(parts.len(), 2);
        assert!((len(&parts) - 6.0).abs() < 1e-4);
        // A wall lying on the rect's edge is not inside it
        assert_eq!(visible_parts((0.0, 4.0), (10.0, 4.0), &rect).len(), 1);
        // Fully inside: nothing left
        assert!(visible_parts((3.0, 1.0), (5.0, 1.0), &rect).is_empty());
    }

    #[test]
    fn circle_hides_its_chord() {
        let circle = [Interior::Circle { cx: 5.0, cy: 5.0, r: 2.0 }];
        let parts = visible_parts((0.0, 5.0), (10.0, 5.0), &circle);
        assert!((len(&parts) - 6.0).abs() < 1e-3);
    }

    #[test]
    fn cells_hide_floor_and_edges_between_floor() {
        // 3x1 cave at (0,0): floor, rock, floor
        let cells = [true, false, true];
        let cave = [Interior::Cells { x: 0, y: 0, w: 3, h: 1, cells: &cells }];
        // A line through the row's middle is hidden over the two floor cells only
        let parts = visible_parts((0.0, 0.5), (3.0, 0.5), &cave);
        assert!((len(&parts) - 1.0).abs() < 1e-4);
        // The cave's top edge borders outside: not hidden
        assert!((len(&visible_parts((0.0, 0.0), (3.0, 0.0), &cave)) - 3.0).abs() < 1e-4);
    }

    /// Two 6x4 rooms overlapping in x 4..6, joined by a Merge connection with `mode`.
    fn overlapping_pair(mode: crate::model::OverlapWalls) -> (crate::model::Dungeon, SpatialLayout) {
        use crate::model::*;
        let mut d = Dungeon::new("t".into());
        let a = Room::new("A".into());
        let b = Room::new("B".into());
        let (aid, bid) = (a.id.clone(), b.id.clone());
        d.graph.add_room(a);
        d.graph.add_room(b);
        let mut conn = Connection::new(ConnectionType::Merge);
        conn.overlap_walls = mode;
        d.graph.add_connection(aid.clone(), bid.clone(), conn);
        let mut layout = SpatialLayout::new();
        let rl = |id: &str, x: i32| RoomLayout { room_id: id.into(), x, y: 0, width: 6, height: 4, violations: Vec::new(), wall_openings: Vec::new(), rotation: 0.0 };
        layout.rooms = vec![rl(&aid, 0), rl(&bid, 4)];
        d.layout = Some(layout.clone());
        (d, layout)
    }

    /// Drawn and line-of-sight wall length near x (within half a cell), over 0 < y < 4.
    /// A spans x 0..6 and B spans x 4..10, so B's west wall sits at x=4 inside A and
    /// A's east wall at x=6 inside B; each is 4 long.
    fn wall_length_near(mode: crate::model::OverlapWalls, x: f32) -> (f32, f32) {
        use crate::render::recording::{RecordingRenderer, RenderCommand};
        let (d, layout) = overlapping_pair(mode);
        let g = crate::util::GRID_PX;
        let band = [Interior::Rect { x0: x - 0.5, y0: 0.0, x1: x + 0.5, y1: 4.0 }];
        let inside = |a: Pt, b: Pt| len(&[(a, b)]) - len(&visible_parts(a, b, &band));
        let mut rendered = 0.0;
        for rl in &layout.rooms {
            let mut rec = RecordingRenderer::new();
            crate::render::themed::render_room_walls(&mut rec, rl, &d.graph, &layout, &d.theme);
            for cmd in &rec.commands {
                if let RenderCommand::Line { x1, y1, x2, y2, .. } = cmd {
                    rendered += inside((x1 / g, y1 / g), (x2 / g, y2 / g));
                }
            }
        }
        let pres = crate::presentation::PresentationState::new_from_dungeon(&d);
        let occ = crate::presentation::los::build_static_occluders(&d, &layout, &pres);
        let los = occ.walls.iter().map(|w| inside(w.a, w.b)).sum();
        (rendered, los)
    }

    #[test]
    fn overlap_modes_control_walls_in_render_and_line_of_sight() {
        use crate::model::OverlapWalls;
        // (mode, B's west wall at x=4 kept, A's east wall at x=6 kept); A is the source
        for (mode, b_west, a_east) in [
            (OverlapWalls::Both, true, true),
            (OverlapWalls::Source, false, true),
            (OverlapWalls::Target, true, false),
            (OverlapWalls::Neither, false, false),
        ] {
            for (x, kept) in [(4.0, b_west), (6.0, a_east)] {
                let (rendered, los) = wall_length_near(mode, x);
                let want = if kept { 4.0 } else { 0.0 };
                assert!((rendered - want).abs() < 1e-3, "{mode:?} render x={x}: {rendered}");
                assert!((los - want).abs() < 1e-3, "{mode:?} LOS x={x}: {los}");
            }
        }
    }

    #[test]
    fn overlap_mode_decides_who_is_hidden() {
        use crate::model::OverlapWalls;
        assert!(!OverlapWalls::Both.hides(true) && !OverlapWalls::Both.hides(false));
        assert!(OverlapWalls::Neither.hides(true) && OverlapWalls::Neither.hides(false));
        assert!(!OverlapWalls::Source.hides(true) && OverlapWalls::Source.hides(false));
        assert!(OverlapWalls::Target.hides(true) && !OverlapWalls::Target.hides(false));
    }
}
