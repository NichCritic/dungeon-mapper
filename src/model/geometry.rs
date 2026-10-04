//! Shapes in world grid units for what the grid cells can't express exactly: rotated
//! rooms and freeform corridors (angled runs, or attached to a rotated room). Shared by
//! rendering, line of sight, fog and the editors. Orthogonal corridors between unrotated
//! rooms keep the cell-based path and never come through here.

use super::{CorridorAngle, CorridorSegment, DungeonGraph, GridPos, RoomLayout, SpatialLayout};

pub type Pt = (f32, f32);

/// Even-odd point-in-polygon test.
pub fn point_in_polygon(p: Pt, poly: &[Pt]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Distance from `p` to segment `a`–`b`.
pub fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-12 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (a.0 + dx * t, a.1 + dy * t);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Bounding box of points: (min_x, min_y, max_x, max_y).
pub fn bounds(pts: &[Pt]) -> (f32, f32, f32, f32) {
    pts.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)))
}

/// Centerline position of a corridor waypoint. Waypoints mark the corridor's cells as
/// `wp - width/2 .. wp - width/2 + width`, so odd widths are centered on a cell.
pub fn corridor_center(wp: GridPos, width: u32) -> Pt {
    let off = width as f32 / 2.0 - (width / 2) as f32;
    (wp.x as f32 + off, wp.y as f32 + off)
}

/// Where a corridor meets a rotated room's wall.
#[derive(Clone, Debug)]
pub struct Attach {
    pub room_id: String,
    /// Point on the wall, on the corridor's centerline.
    pub point: Pt,
    /// Unit vector along the wall.
    pub tangent: Pt,
    /// Unit vector out of the room.
    pub normal: Pt,
}

/// A freeform corridor: centerline, side walls and floor outline.
#[derive(Clone, Debug)]
pub struct CorridorShape {
    pub centerline: Vec<Pt>,
    pub left: Vec<Pt>,
    pub right: Vec<Pt>,
    /// Floor outline (left side, then the right side reversed).
    pub polygon: Vec<Pt>,
    pub half_width: f32,
    /// Attachment to a rotated room at the start and end, if any.
    pub ends: [Option<Attach>; 2],
}

impl CorridorShape {
    pub fn contains(&self, p: Pt) -> bool {
        point_in_polygon(p, &self.polygon)
    }

    /// The side walls as segments.
    pub fn wall_segments(&self) -> Vec<(Pt, Pt)> {
        self.left.windows(2).chain(self.right.windows(2)).map(|w| (w[0], w[1])).collect()
    }

    /// Convex pieces covering the floor (renderers fill convex polygons): a quad per
    /// run, plus a kite (or triangle, at a bevelled turn) on each side of every bend.
    pub fn floor_pieces(&self) -> Vec<Vec<Pt>> {
        let pts = &self.centerline;
        let d = self.half_width;
        let normal = |a: Pt, b: Pt| { let u = unit((b.0 - a.0, b.1 - a.1)); (u.1, -u.0) };
        let off = |p: Pt, n: Pt, k: f32| (p.0 + n.0 * k, p.1 + n.1 * k);
        let mut out = Vec::new();
        for w in pts.windows(2) {
            let n = normal(w[0], w[1]);
            out.push(vec![off(w[0], n, d), off(w[1], n, d), off(w[1], n, -d), off(w[0], n, -d)]);
        }
        for i in 1..pts.len().saturating_sub(1) {
            let (p, n1, n2) = (pts[i], normal(pts[i - 1], pts[i]), normal(pts[i], pts[i + 1]));
            let m = unit((n1.0 + n2.0, n1.1 + n2.1));
            let cos_half = m.0 * n1.0 + m.1 * n1.1;
            for k in [d, -d] {
                if cos_half > 0.35 {
                    out.push(vec![p, off(p, n1, k), off(p, m, k / cos_half), off(p, n2, k)]);
                } else {
                    out.push(vec![p, off(p, n1, k), off(p, n2, k)]);
                }
            }
        }
        out
    }

    /// Whether the floor fully covers a grid cell.
    pub fn covers_cell(&self, gx: i32, gy: i32) -> bool {
        let (x, y) = (gx as f32, gy as f32);
        [(x + 0.5, y + 0.5), (x + 0.01, y + 0.01), (x + 0.99, y + 0.01), (x + 0.99, y + 0.99), (x + 0.01, y + 0.99)]
            .iter().all(|&p| self.contains(p))
    }
}

/// A door's outline where a corridor meets a rotated room: `width` along the wall and
/// `depth` across it, centered on the wall.
pub fn door_quad(a: &Attach, width: f32, depth: f32) -> [Pt; 4] {
    let (t, n, p) = (a.tangent, a.normal, a.point);
    let (hw, hd) = (width / 2.0, depth / 2.0);
    let at = |u: f32, v: f32| (p.0 + t.0 * u + n.0 * v, p.1 + t.1 * u + n.1 * v);
    [at(-hw, -hd), at(hw, -hd), at(hw, hd), at(-hw, hd)]
}

/// Whether a corridor needs freeform geometry: an angle setting other than orthogonal,
/// a non-orthogonal run, or an end on a rotated room.
pub fn is_freeform(corridor: &CorridorSegment, layout: &SpatialLayout, graph: &DungeonGraph) -> bool {
    let edge = graph.connection_by_id(&corridor.connection_id);
    if edge.is_some_and(|e| e.connection.corridor_angle != CorridorAngle::Orthogonal) {
        return true;
    }
    if corridor.waypoints.windows(2).any(|w| w[0].x != w[1].x && w[0].y != w[1].y) {
        return true;
    }
    edge.is_some_and(|e| {
        [&e.source_room_id, &e.target_room_id].iter()
            .any(|id| layout.room_by_id(id).is_some_and(|rl| rl.is_rotated()))
    })
}

/// [`corridor_shape`] for every corridor of the layout, by index.
pub fn corridor_shapes(layout: &SpatialLayout, graph: &DungeonGraph) -> Vec<Option<CorridorShape>> {
    layout.corridors.iter().map(|c| corridor_shape(c, layout, graph)).collect()
}

/// Freeform geometry for a corridor, or None when it uses the cell-based path.
pub fn corridor_shape(corridor: &CorridorSegment, layout: &SpatialLayout, graph: &DungeonGraph) -> Option<CorridorShape> {
    if corridor.waypoints.len() < 2 || !is_freeform(corridor, layout, graph) {
        return None;
    }
    let half = corridor.width.max(1) as f32 / 2.0;
    let mut pts: Vec<Pt> = corridor.waypoints.iter().map(|&wp| corridor_center(wp, corridor.width)).collect();
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4);
    if pts.len() < 2 {
        return None;
    }

    // The end rooms, nearest end first
    let edge = graph.connection_by_id(&corridor.connection_id);
    let end_rooms: Vec<&RoomLayout> = edge
        .map(|e| [&e.source_room_id, &e.target_room_id].iter().filter_map(|id| layout.room_by_id(id)).collect())
        .unwrap_or_default();
    let nearest_room = |p: Pt| -> Option<&RoomLayout> {
        end_rooms.iter().copied().min_by(|a, b| {
            let d = |rl: &RoomLayout| { let c = rl.center(); (c.0 - p.0).powi(2) + (c.1 - p.1).powi(2) };
            d(a).partial_cmp(&d(b)).unwrap()
        })
    };

    // Ends: run in through the nearest point of the end room's wall, square to it and
    // reaching past it, so the floor meets the room and the walls (clipped by the room)
    // stop at its edge. Only a rotated room records the attachment (for its door).
    let mut ends: [Option<Attach>; 2] = [None, None];
    for (slot, at_start) in [(0, true), (1, false)] {
        let (p, q) = if at_start { (pts[0], pts[1]) } else { (pts[pts.len() - 1], pts[pts.len() - 2]) };
        let mut prefix = Vec::new();
        if let Some(rl) = nearest_room(p) {
            let depth = half + 0.5;
            let heading = unit((p.0 - q.0, p.1 - q.1));
            // Heading into the room already: carry straight on through the wall rather
            // than bending to meet it square on
            let a = match ray_into_room(rl, graph, p, heading, half + 2.0) {
                Some(a) => {
                    prefix.push((a.point.0 + heading.0 * depth, a.point.1 + heading.1 * depth));
                    a
                }
                None => {
                    let a = attach_to_room(rl, graph, p);
                    prefix.push((a.point.0 - a.normal.0 * depth, a.point.1 - a.normal.1 * depth));
                    a
                }
            };
            if dist(a.point, p) > 1e-3 {
                prefix.push(a.point);
            }
            if rl.is_rotated() {
                ends[slot] = Some(a);
            }
        } else {
            let d = unit((p.0 - q.0, p.1 - q.1));
            prefix.push((p.0 + d.0 * half, p.1 + d.1 * half));
        }
        if at_start {
            prefix.extend(pts.drain(..));
            pts = prefix;
        } else {
            prefix.reverse();
            pts.extend(prefix);
        }
    }

    let (left, right) = offset_polylines(&pts, half);
    let mut polygon = left.clone();
    polygon.extend(right.iter().rev().copied());
    Some(CorridorShape { centerline: pts, left, right, polygon, half_width: half, ends })
}

/// Where a corridor at `p` heading `d` enters the room, if it reaches the wall within
/// `reach` and meets it within 60° of square on.
fn ray_into_room(rl: &RoomLayout, graph: &DungeonGraph, p: Pt, d: Pt, reach: f32) -> Option<Attach> {
    let circle = graph.room_by_id(&rl.room_id).is_some_and(|r| r.shape == super::RoomShape::Circle);
    let hit = if circle {
        let c = rl.center();
        let r = rl.width.min(rl.height) as f32 / 2.0;
        let (fx, fy) = (p.0 - c.0, p.1 - c.1);
        let b = fx * d.0 + fy * d.1;
        let disc = b * b - (fx * fx + fy * fy - r * r);
        if disc < 0.0 {
            return None;
        }
        let t = -b - disc.sqrt();
        if !(0.0..=reach).contains(&t) {
            return None;
        }
        let point = (p.0 + d.0 * t, p.1 + d.1 * t);
        let n = unit((point.0 - c.0, point.1 - c.1));
        Attach { room_id: rl.room_id.clone(), point, tangent: (-n.1, n.0), normal: n }
    } else {
        // Slab test in the room's local frame
        let (lp, lq) = (rl.to_local(p.0, p.1), rl.to_local(p.0 + d.0, p.1 + d.1));
        let ld = (lq.0 - lp.0, lq.1 - lp.1);
        let (w, h) = (rl.width as f32, rl.height as f32);
        let mut best: Option<(f32, Pt)> = None;
        for (axis, bound, n) in [(0, 0.0, (-1.0, 0.0)), (0, w, (1.0, 0.0)), (1, 0.0, (0.0, -1.0)), (1, h, (0.0, 1.0))] {
            let (o, v) = if axis == 0 { (lp.0, ld.0) } else { (lp.1, ld.1) };
            if v.abs() < 1e-9 {
                continue;
            }
            let t = (bound - o) / v;
            let (hx, hy) = (lp.0 + ld.0 * t, lp.1 + ld.1 * t);
            let along = if axis == 0 { (0.0..=h).contains(&hy) } else { (0.0..=w).contains(&hx) };
            // Entering through this face: moving against its outward normal
            if t >= 0.0 && t <= reach && along && (ld.0 * n.0 + ld.1 * n.1) < 0.0 && best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, n));
            }
        }
        let (t, n_local) = best?;
        let (s, c) = rl.rotation.to_radians().sin_cos();
        let rot = |v: Pt| (v.0 * c - v.1 * s, v.0 * s + v.1 * c);
        let n = rot(n_local);
        Attach { room_id: rl.room_id.clone(), point: (p.0 + d.0 * t, p.1 + d.1 * t), tangent: (-n.1, n.0), normal: n }
    };
    // Within 60° of square on (the heading against the outward normal)
    (-(d.0 * hit.normal.0 + d.1 * hit.normal.1) >= 0.5).then_some(hit)
}

/// Where a corridor ending near `p` meets a rotated room: on the rim for a circle
/// (which turning doesn't change), otherwise on the nearest wall.
pub fn attach_to_room(rl: &RoomLayout, graph: &DungeonGraph, p: Pt) -> Attach {
    if graph.room_by_id(&rl.room_id).is_some_and(|r| r.shape == super::RoomShape::Circle) {
        let c = rl.center();
        let r = rl.width.min(rl.height) as f32 / 2.0;
        let n = unit((p.0 - c.0, p.1 - c.1));
        return Attach { room_id: rl.room_id.clone(), point: (c.0 + n.0 * r, c.1 + n.1 * r), tangent: (-n.1, n.0), normal: n };
    }
    attach_point(rl, p)
}

/// The nearest point on a rotated room's wall to `p`, with the wall's frame.
pub fn attach_point(rl: &RoomLayout, p: Pt) -> Attach {
    let (w, h) = (rl.width as f32, rl.height as f32);
    let (lx, ly) = rl.to_local(p.0, p.1);
    let (cx, cy) = (lx.clamp(0.0, w), ly.clamp(0.0, h));
    // Nearest edge to the clamped point: (distance, local normal, local tangent)
    let edges = [
        (cx, (-1.0, 0.0), (0.0, 1.0)),
        (w - cx, (1.0, 0.0), (0.0, 1.0)),
        (cy, (0.0, -1.0), (1.0, 0.0)),
        (h - cy, (0.0, 1.0), (1.0, 0.0)),
    ];
    let &(_, n, t) = edges.iter().min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()).unwrap();
    let local = match n {
        (x, _) if x < 0.0 => (0.0, cy),
        (x, _) if x > 0.0 => (w, cy),
        (_, y) if y < 0.0 => (cx, 0.0),
        _ => (cx, h),
    };
    let (s, c) = rl.rotation.to_radians().sin_cos();
    let rot = |v: Pt| (v.0 * c - v.1 * s, v.0 * s + v.1 * c);
    Attach { room_id: rl.room_id.clone(), point: rl.to_world(local.0, local.1), tangent: rot(t), normal: rot(n) }
}

fn dist(a: Pt, b: Pt) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn unit(v: Pt) -> Pt {
    let l = (v.0 * v.0 + v.1 * v.1).sqrt();
    if l < 1e-9 { (1.0, 0.0) } else { (v.0 / l, v.1 / l) }
}

/// Left and right offsets of a polyline at distance `d`, with miter joins (bevelled
/// when the turn is too sharp for a miter).
fn offset_polylines(pts: &[Pt], d: f32) -> (Vec<Pt>, Vec<Pt>) {
    let n = pts.len();
    let dirs: Vec<Pt> = pts.windows(2).map(|w| unit((w[1].0 - w[0].0, w[1].1 - w[0].1))).collect();
    // Left normal in screen coordinates (y down): rotate the direction 90° counter-clockwise
    let normal = |v: Pt| (v.1, -v.0);
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    for i in 0..n {
        let p = pts[i];
        if i == 0 || i == n - 1 {
            let nv = normal(dirs[if i == 0 { 0 } else { n - 2 }]);
            left.push((p.0 + nv.0 * d, p.1 + nv.1 * d));
            right.push((p.0 - nv.0 * d, p.1 - nv.1 * d));
            continue;
        }
        let (n1, n2) = (normal(dirs[i - 1]), normal(dirs[i]));
        let m = unit((n1.0 + n2.0, n1.1 + n2.1));
        let cos_half = m.0 * n1.0 + m.1 * n1.1;
        if cos_half > 0.35 {
            let l = d / cos_half;
            left.push((p.0 + m.0 * l, p.1 + m.1 * l));
            right.push((p.0 - m.0 * l, p.1 - m.1 * l));
        } else {
            left.push((p.0 + n1.0 * d, p.1 + n1.1 * d));
            left.push((p.0 + n2.0 * d, p.1 + n2.1 * d));
            right.push((p.0 - n1.0 * d, p.1 - n1.1 * d));
            right.push((p.0 - n2.0 * d, p.1 - n2.1 * d));
        }
    }
    (left, right)
}

/// Cells covered by a w×w block (top-left at the point) sliding straight from `a` to
/// `b`, grown by `border` cells on every side.
pub fn swept_cells(a: GridPos, b: GridPos, w: i32, border: i32) -> Vec<(i32, i32)> {
    let (dx, dy) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
    let steps = ((dx.abs().max(dy.abs())) * 4.0).ceil().max(1.0) as i32;
    let mut cells = crate::util::CellSet::default();
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (px, py) = (a.x as f32 + dx * t, a.y as f32 + dy * t);
        let (x0, y0) = (px.floor() as i32 - border, py.floor() as i32 - border);
        let (x1, y1) = ((px + w as f32).ceil() as i32 + border, (py + w as f32).ceil() as i32 + border);
        for y in y0..y1 {
            for x in x0..x1 {
                cells.insert((x, y));
            }
        }
    }
    cells.into_iter().collect()
}

/// Compute door rectangle in grid coordinates given an exit position or waypoint fallback.
/// Returns (x1, y1, x2, y2) in grid coords for the door rectangle.
pub fn door_rect(
    rl: &RoomLayout,
    wp: &GridPos,
    exit: Option<&super::ExitPos>,
    dw: f32,
    door_depth: f32,
) -> (f32, f32, f32, f32) {
    let dw_half = dw / 2.0;

    if let Some(exit) = exit {
        // Use stored exit to determine face and position
        let rw = rl.width as f32;
        let rh = rl.height as f32;
        let rx = rl.x as f32;
        let ry = rl.y as f32;
        let ex = exit.x;
        let ey = exit.y;
        let eps = 0.01;
        if (ex - (rx + rw)).abs() < eps {
            // Right wall
            let wall_x = rx + rw;
            (wall_x - door_depth / 2.0, ey - dw_half, wall_x + door_depth / 2.0, ey + dw_half)
        } else if (ex - rx).abs() < eps {
            // Left wall
            (rx - door_depth / 2.0, ey - dw_half, rx + door_depth / 2.0, ey + dw_half)
        } else if (ey - (ry + rh)).abs() < eps {
            // Bottom wall
            let wall_y = ry + rh;
            (ex - dw_half, wall_y - door_depth / 2.0, ex + dw_half, wall_y + door_depth / 2.0)
        } else {
            // Top wall
            (ex - dw_half, ry - door_depth / 2.0, ex + dw_half, ry + door_depth / 2.0)
        }
    } else {
        // Fallback: nearest-wall heuristic from waypoint
        let wp_cx = wp.x as f32;
        let wp_cy = wp.y as f32;
        let dist_right = (wp_cx - (rl.x + rl.width as i32) as f32).abs();
        let dist_left = (wp_cx - rl.x as f32).abs();
        let dist_bottom = (wp_cy - (rl.y + rl.height as i32) as f32).abs();
        let dist_top = (wp_cy - rl.y as f32).abs();
        let min_dist = dist_right.min(dist_left).min(dist_bottom).min(dist_top);

        if min_dist == dist_right {
            let wall_x = (rl.x + rl.width as i32) as f32;
            (wall_x - door_depth / 2.0, wp_cy - dw_half, wall_x + door_depth / 2.0, wp_cy + dw_half)
        } else if min_dist == dist_left {
            let wall_x = rl.x as f32;
            (wall_x - door_depth / 2.0, wp_cy - dw_half, wall_x + door_depth / 2.0, wp_cy + dw_half)
        } else if min_dist == dist_bottom {
            let wall_y = (rl.y + rl.height as i32) as f32;
            (wp_cx - dw_half, wall_y - door_depth / 2.0, wp_cx + dw_half, wall_y + door_depth / 2.0)
        } else {
            let wall_y = rl.y as f32;
            (wp_cx - dw_half, wall_y - door_depth / 2.0, wp_cx + dw_half, wall_y + door_depth / 2.0)
        }
    }
}

/// One end of a connection's corridor, where its door sits on a room's wall.
pub struct DoorEnd<'a> {
    pub edge: &'a super::StoredEdge,
    pub corridor: &'a CorridorSegment,
    pub rl: &'a RoomLayout,
    /// The corridor's waypoint at this end.
    pub wp: GridPos,
    /// The user-placed exit at this end, if any.
    pub exit: Option<&'a super::ExitPos>,
}

/// A door's outline: a grid rectangle (x0, y0, x1, y1) on an unrotated room, or a
/// quad along a rotated room's turned wall with its center.
pub enum DoorShape {
    Rect(f32, f32, f32, f32),
    Quad([Pt; 4], Pt),
}

impl DoorShape {
    pub fn center(&self) -> Pt {
        match *self {
            DoorShape::Rect(x0, y0, x1, y1) => ((x0 + x1) / 2.0, (y0 + y1) / 2.0),
            DoorShape::Quad(_, c) => c,
        }
    }
}

impl DoorEnd<'_> {
    /// The door's outline here, `width` along the wall and `depth` across it.
    pub fn shape(&self, graph: &DungeonGraph, layout: &SpatialLayout, width: f32, depth: f32) -> Option<DoorShape> {
        if self.rl.is_rotated() {
            let shape = corridor_shape(self.corridor, layout, graph)?;
            let attach = shape.ends.iter().flatten().find(|a| a.room_id == self.rl.room_id)?;
            return Some(DoorShape::Quad(door_quad(attach, width, depth), attach.point));
        }
        let (x0, y0, x1, y1) = door_rect(self.rl, &self.wp, self.exit, width, depth);
        Some(DoorShape::Rect(x0, y0, x1, y1))
    }
}

/// Both ends of every connection with a routed corridor, in connection order. The
/// parent's end of a child-to-parent connection is left out: that corridor ends
/// inside the parent, not at its wall.
pub fn door_ends<'a>(graph: &'a DungeonGraph, layout: &'a SpatialLayout) -> impl Iterator<Item = DoorEnd<'a>> + 'a {
    graph.connections.iter().flat_map(move |edge| {
        let corridor = layout.corridor_for(&edge.connection.id).filter(|c| c.waypoints.len() >= 2);
        let src_is_child = graph.parent_of(&edge.source_room_id).is_some_and(|p| p == edge.target_room_id);
        let tgt_is_child = graph.parent_of(&edge.target_room_id).is_some_and(|p| p == edge.source_room_id);
        corridor.into_iter().flat_map(move |c| {
            let ends = [
                (&edge.source_room_id, c.waypoints[0], edge.source_exit.as_ref(), tgt_is_child),
                (&edge.target_room_id, c.waypoints[c.waypoints.len() - 1], edge.target_exit.as_ref(), src_is_child),
            ];
            ends.into_iter()
                .filter(|&(_, _, _, parent_side)| !parent_side)
                .filter_map(move |(room_id, wp, exit, _)| {
                    layout.room_by_id(room_id).map(|rl| DoorEnd { edge, corridor: c, rl, wp, exit })
                })
        })
    })
}

/// Grid cells whose centers lie inside a polygon.
pub fn polygon_cells(poly: &[Pt]) -> Vec<(i32, i32)> {
    let (x0, y0, x1, y1) = bounds(poly);
    let mut out = Vec::new();
    for gy in (y0.floor() as i32)..(y1.ceil() as i32) {
        for gx in (x0.floor() as i32)..(x1.ceil() as i32) {
            if point_in_polygon((gx as f32 + 0.5, gy as f32 + 0.5), poly) {
                out.push((gx, gy));
            }
        }
    }
    out
}

/// Grid cells a polygon overlaps at all (for line of sight, where a cell partly open
/// must not count as solid rock; the exact walls do the blocking).
pub fn polygon_touched_cells(poly: &[Pt]) -> Vec<(i32, i32)> {
    let (x0, y0, x1, y1) = bounds(poly);
    let mut out = Vec::new();
    for gy in (y0.floor() as i32)..(y1.ceil() as i32) {
        for gx in (x0.floor() as i32)..(x1.ceil() as i32) {
            let cell = [(gx as f32, gy as f32), (gx as f32 + 1.0, gy as f32), (gx as f32 + 1.0, gy as f32 + 1.0), (gx as f32, gy as f32 + 1.0)];
            if polygons_overlap(&cell, poly) {
                out.push((gx, gy));
            }
        }
    }
    out
}

/// Whether two polygons overlap with positive area (approximately: a vertex of one
/// inside the other, the cell center inside, or crossing edges).
pub fn polygons_overlap(a: &[Pt], b: &[Pt]) -> bool {
    let center = { let (x0, y0, x1, y1) = bounds(a); ((x0 + x1) / 2.0, (y0 + y1) / 2.0) };
    if point_in_polygon(center, b) || a.iter().any(|&p| point_in_polygon(p, b)) || b.iter().any(|&p| point_in_polygon(p, a)) {
        return true;
    }
    let edges = |p: &[Pt]| -> Vec<(Pt, Pt)> { (0..p.len()).map(|i| (p[i], p[(i + 1) % p.len()])).collect() };
    let (ea, eb) = (edges(a), edges(b));
    ea.iter().any(|&(p, q)| eb.iter().any(|&(r, s)| segments_cross(p, q, r, s)))
}

/// Proper crossing of two segments (not just touching).
fn segments_cross(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let cross = |o: Pt, p: Pt, q: Pt| (p.0 - o.0) * (q.1 - o.1) - (p.1 - o.1) * (q.0 - o.0);
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    ((d1 > 1e-6 && d2 < -1e-6) || (d1 < -1e-6 && d2 > 1e-6)) && ((d3 > 1e-6 && d4 < -1e-6) || (d3 < -1e-6 && d4 > 1e-6))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rl(x: i32, y: i32, w: u32, h: u32, rotation: f32) -> RoomLayout {
        RoomLayout { room_id: "r".into(), x, y, width: w, height: h, violations: Vec::new(), wall_openings: Vec::new(), rotation }
    }

    #[test]
    fn room_transform_round_trips_and_rotates_about_center() {
        let r = rl(0, 0, 4, 2, 90.0);
        let (wx, wy) = r.to_world(0.0, 0.0);
        // Top-left of a 4x2 room centered at (2,1), turned 90° clockwise, lands at (3,-1)
        assert!((wx - 3.0).abs() < 1e-5 && (wy + 1.0).abs() < 1e-5, "{wx},{wy}");
        let (lx, ly) = r.to_local(wx, wy);
        assert!(lx.abs() < 1e-5 && ly.abs() < 1e-5);
        assert!(r.contains_point(2.0, 1.0));
        assert!(!r.contains_point(0.2, 0.5)); // inside the unrotated rect only
        let unrotated = rl(1, 2, 3, 4, 0.0);
        assert_eq!(unrotated.aabb(), (1.0, 2.0, 4.0, 6.0));
    }

    #[test]
    fn straight_corridor_offsets_by_half_width() {
        let (l, r) = offset_polylines(&[(0.0, 0.0), (10.0, 0.0)], 1.0);
        assert_eq!(l, vec![(0.0, -1.0), (10.0, -1.0)]);
        assert_eq!(r, vec![(0.0, 1.0), (10.0, 1.0)]);
    }

    #[test]
    fn miter_keeps_width_through_a_45_degree_turn() {
        let (l, r) = offset_polylines(&[(0.0, 0.0), (10.0, 0.0), (20.0, 10.0)], 1.0);
        // Both mitered corners are at distance 1 from the lines of both runs
        let line_dist = |p: Pt, a: Pt, b: Pt| {
            let d = unit((b.0 - a.0, b.1 - a.1));
            ((p.0 - a.0) * d.1 - (p.1 - a.1) * d.0).abs()
        };
        for corner in [l[1], r[1]] {
            assert!((line_dist(corner, (0.0, 0.0), (10.0, 0.0)) - 1.0).abs() < 1e-4);
            assert!((line_dist(corner, (10.0, 0.0), (20.0, 10.0)) - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn attach_point_is_on_the_nearest_rotated_wall() {
        let r = rl(0, 0, 4, 4, 45.0);
        let c = r.center();
        // A point far to the right of the center attaches to a wall, with an outward normal
        let a = attach_point(&r, (c.0 + 10.0, c.1));
        let (lx, ly) = r.to_local(a.point.0, a.point.1);
        assert!(lx.abs() < 1e-4 || (lx - 4.0).abs() < 1e-4 || ly.abs() < 1e-4 || (ly - 4.0).abs() < 1e-4);
        let out = (a.point.0 + a.normal.0 * 0.1, a.point.1 + a.normal.1 * 0.1);
        assert!(!r.contains_point(out.0, out.1));
    }

    #[test]
    fn polygon_cells_rasterize_by_center() {
        let square = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)];
        let mut cells = polygon_cells(&square);
        cells.sort();
        assert_eq!(cells, vec![(0, 0), (0, 1), (1, 0), (1, 1)]);
        let tri = [(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)];
        assert!(polygon_touched_cells(&tri).len() > polygon_cells(&tri).len());
    }
}
