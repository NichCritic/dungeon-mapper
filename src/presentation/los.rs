//! Line of sight and 2024-rules cover.
//!
//! A port of the user's `cover-sim` geometry: from the best of the attacker's four
//! corners, draw a line to each of the target's four corners and count how many are
//! blocked. Obstacles are solid cells (rock), zero-thickness walls (room edges),
//! objects (decor with a cover value) and other creatures. Only obstacles inside the
//! convex hull of the two spaces count.
//!
//! Levels: 0 blocked = none, 1-2 = half, 3-4 = three-quarters, total only when all
//! four lines are blocked by `Full` obstacles. The result is then capped by the
//! highest cover value among the blocking obstacles, so a low table or a creature
//! (half) never grants more than half cover on its own.

use crate::util::CellSet;
use std::sync::Arc;

use crate::model::{
    CoverKind, CoverLevel, Dungeon, RoomShape, SpatialLayout, TokenKind,
};
use crate::presentation::tokens::TokenInfo;
use crate::presentation::VisibilityProvider;
use crate::render::themed::{build_floor_set, flush_walls_with_layout};
use crate::util::{DECOR_HALF_SIZE, GRID_PX};

pub type Pt = (f32, f32);

/// Axis-aligned square in grid units: (min x, min y, max x, max y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Square {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Square {
    pub fn centered(cx: f32, cy: f32, size: f32) -> Self {
        let h = size / 2.0;
        Square { x0: cx - h, y0: cy - h, x1: cx + h, y1: cy + h }
    }

    pub fn cell(gx: i32, gy: i32) -> Self {
        Square { x0: gx as f32, y0: gy as f32, x1: gx as f32 + 1.0, y1: gy as f32 + 1.0 }
    }

    /// TL, TR, BL, BR — the simulator's corner order.
    pub fn corners(&self) -> [Pt; 4] {
        [(self.x0, self.y0), (self.x1, self.y0), (self.x0, self.y1), (self.x1, self.y1)]
    }

    pub fn center(&self) -> Pt {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
}

/// Zero-thickness, axis-aligned wall piece.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: Pt,
    pub b: Pt,
}

/// An object footprint with the most cover it can grant.
#[derive(Clone, Debug)]
pub struct Obstacle {
    pub poly: Vec<Pt>,
    pub cover: CoverKind,
}

impl Obstacle {
    pub fn blocks_light(&self) -> bool {
        self.cover == CoverKind::Full
    }
}

/// Everything that can block a line. The static geometry is shared (`Arc`) so a
/// per-frame copy with fresh creature positions costs nothing.
#[derive(Clone, Debug, Default)]
pub struct Occluders {
    /// Floor cells of the map (rooms and corridors).
    pub floor: Arc<CellSet>,
    /// Solid cells (the simulator's wall cells).
    pub solid: Arc<CellSet>,
    /// Zero-thickness walls with door gaps already cut.
    pub walls: Arc<Vec<Segment>>,
    pub objects: Arc<Vec<Obstacle>>,
    /// Creatures occupy their whole space.
    pub creatures: Vec<(TokenKind, Square)>,
    /// Fingerprint of the static parts, computed once when they are built.
    pub static_hash: u64,
}

impl Occluders {
    /// Same static geometry with the current creature squares.
    pub fn with_creatures(&self, dungeon: &Dungeon, token_infos: &[TokenInfo]) -> Occluders {
        let creatures = dungeon.tokens.iter().zip(token_infos)
            .filter(|(_, info)| !info.dead && info.size >= 1.0)
            .map(|(t, info)| (t.kind.clone(), Square::centered(t.x, t.y, info.size)))
            .collect();
        Occluders {
            floor: Arc::clone(&self.floor),
            solid: Arc::clone(&self.solid),
            walls: Arc::clone(&self.walls),
            objects: Arc::clone(&self.objects),
            creatures,
            static_hash: self.static_hash,
        }
    }
}

impl Occluders {
    /// A copy holding only the walls, objects and creatures that touch the given box
    /// (grid units). Exact for any query whose lines and spaces stay inside the box,
    /// and saves `cover_between` re-filtering the whole map per query.
    pub fn restricted_to(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Occluders {
        let touches = |ax0: f32, ay0: f32, ax1: f32, ay1: f32| ax1 >= x0 && ax0 <= x1 && ay1 >= y0 && ay0 <= y1;
        let walls = self.walls.iter()
            .filter(|w| touches(w.a.0.min(w.b.0), w.a.1.min(w.b.1), w.a.0.max(w.b.0), w.a.1.max(w.b.1)))
            .copied()
            .collect();
        let objects = self.objects.iter()
            .filter(|o| {
                let (lo, hi) = o.poly.iter().fold(((f32::MAX, f32::MAX), (f32::MIN, f32::MIN)), |(lo, hi), p| {
                    ((lo.0.min(p.0), lo.1.min(p.1)), (hi.0.max(p.0), hi.1.max(p.1)))
                });
                touches(lo.0, lo.1, hi.0, hi.1)
            })
            .cloned()
            .collect();
        Occluders {
            floor: Arc::clone(&self.floor),
            solid: Arc::clone(&self.solid),
            walls: Arc::new(walls),
            objects: Arc::new(objects),
            creatures: self.creatures.iter().filter(|(_, s)| touches(s.x0, s.y0, s.x1, s.y1)).cloned().collect(),
            static_hash: self.static_hash,
        }
    }
}

/// Cheap fingerprint of everything the static occluders are built from: rooms,
/// corridors, connections and door state, and decor. Used to know when to rebuild.
pub fn geometry_key(dungeon: &Dungeon, layout: &SpatialLayout, presentation: &dyn VisibilityProvider) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for rl in &layout.rooms {
        rl.room_id.hash(&mut h);
        (rl.x, rl.y, rl.width, rl.height).hash(&mut h);
        rl.rotation.to_bits().hash(&mut h);
        for wp in &rl.wall_openings { (wp.x, wp.y).hash(&mut h); }
        if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
            (room.shape as u8).hash(&mut h);
            (room.open_walls.north, room.open_walls.south, room.open_walls.east, room.open_walls.west).hash(&mut h);
            if let Some(cave) = &room.cave_data { cave.cells.hash(&mut h); }
            for d in &room.decor {
                (d.decor_type as u8).hash(&mut h);
                d.x.to_bits().hash(&mut h); d.y.to_bits().hash(&mut h);
                d.rotation.to_bits().hash(&mut h);
                d.scale_x.to_bits().hash(&mut h); d.scale_y.to_bits().hash(&mut h);
                (d.cover_kind() as u8).hash(&mut h);
            }
        }
    }
    for c in &layout.corridors {
        c.connection_id.hash(&mut h);
        c.width.hash(&mut h);
        for wp in &c.waypoints { (wp.x, wp.y).hash(&mut h); }
    }
    for e in &dungeon.graph.connections {
        e.connection.id.hash(&mut h);
        (e.connection.connection_type as u8).hash(&mut h);
        e.connection.door_width().hash(&mut h);
        e.connection.keep_walls.hash(&mut h);
        e.connection.overlap_walls.hash(&mut h);
        e.connection.corridor_angle.hash(&mut h);
        presentation.is_door_open(&e.connection.id).hash(&mut h);
        if let Some(x) = &e.source_exit { x.x.to_bits().hash(&mut h); x.y.to_bits().hash(&mut h); }
        if let Some(x) = &e.target_exit { x.x.to_bits().hash(&mut h); x.y.to_bits().hash(&mut h); }
    }
    h.finish()
}

/// What blocked a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Clear,
    Wall,
    Object(CoverKind),
    Creature,
}

impl Block {
    fn cap(self) -> CoverKind {
        match self {
            Block::Clear => CoverKind::None,
            Block::Wall => CoverKind::Full,
            Block::Object(k) => k,
            Block::Creature => CoverKind::Half,
        }
    }
}

/// Result of a cover query.
#[derive(Clone, Debug)]
pub struct CoverResult {
    pub level: CoverLevel,
    pub blocked: u8,
    /// Index of the attacker corner used (TL, TR, BL, BR).
    pub corner: usize,
    pub lines: [(Pt, Pt, Block); 4],
}

// ---------------------------------------------------------------------------
// Building occluders from the map
// ---------------------------------------------------------------------------

/// Derive the static occluders (cells, walls, objects) from the map, door state and decor.
/// Creatures are added per frame with [`Occluders::with_creatures`].
pub fn build_static_occluders(
    dungeon: &Dungeon,
    layout: &SpatialLayout,
    presentation: &dyn VisibilityProvider,
) -> Occluders {
    let graph = &dungeon.graph;
    let floor = build_floor_set(layout, graph);
    let shapes: Vec<Option<crate::model::geometry::CorridorShape>> = layout.corridors.iter()
        .map(|c| crate::model::geometry::corridor_shape(c, layout, graph))
        .collect();
    let freeform = shapes.iter().any(Option::is_some) || layout.rooms.iter().any(|rl| rl.is_rotated());
    // Rotated rooms and angled corridors don't sit on the grid: only cells they don't
    // touch at all are rock, and their exact walls (added below) do the blocking.
    let open_cells;
    let open: &CellSet = if freeform {
        open_cells = crate::render::themed::rasterize_floor(layout, graph, crate::render::themed::Coverage::Touched);
        &open_cells
    } else {
        &floor
    };

    // Solid cells: everything inside the layout extents (plus a 1-cell rim) that is not floor.
    let (min_x, min_y, max_x, max_y) = layout.extents();
    let mut solid = CellSet::default();
    for y in (min_y - 1)..=(max_y + 1) {
        for x in (min_x - 1)..=(max_x + 1) {
            if !open.contains(&(x, y)) {
                solid.insert((x, y));
            }
        }
    }
    // Freeform corridors that let sight through where they meet a room: open passages,
    // and doors that are open
    let passable = |e: &crate::model::StoredEdge| {
        crate::render::overlap::is_open_passage(e) || presentation.is_door_open(&e.connection.id)
    };

    // Door apertures: (room_id, span rect) for every open doorway. A rotated room's
    // wall opens by clipping against the corridor instead (below).
    let mut apertures: Vec<(String, (f32, f32, f32, f32))> = Vec::new();
    for end in crate::model::geometry::door_ends(graph, layout) {
        let ct = end.edge.connection.connection_type;
        if !(ct.is_passage() || presentation.is_door_open(&end.edge.connection.id)) {
            continue;
        }
        // Open passages are as wide as the corridor; doors as wide as the door.
        let dw = if ct.is_passage() { end.corridor.width as f32 } else { end.edge.connection.door_width() as f32 };
        if let Some(crate::model::geometry::DoorShape::Rect(x0, y0, x1, y1)) = end.shape(graph, layout, dw, 0.3) {
            apertures.push((end.rl.room_id.clone(), (x0, y0, x1, y1)));
        }
    }

    let mut walls_all = Vec::new();
    for rl in &layout.rooms {
        let room = graph.room_by_id(&rl.room_id);
        let shape = room.map(|r| r.shape).unwrap_or_default();
        // This room's walls, before removing the parts hidden inside overlapping rooms
        let mut room_walls: Vec<Segment> = Vec::new();
        let walls = &mut room_walls;
        let x0 = rl.x as f32;
        let y0 = rl.y as f32;
        let x1 = x0 + rl.width as f32;
        let y1 = y0 + rl.height as f32;
        if rl.is_rotated() && shape != RoomShape::Circle {
            let cave = room.and_then(|r| r.cave_data.as_ref()).filter(|c| shape == RoomShape::Cave && !c.contour_segments.is_empty());
            if let Some(cave) = cave {
                // Turned cave cells don't line up with the grid's rock cells: use the contour
                let g = crate::util::GRID_PX;
                for &(ax, ay, bx, by) in &cave.contour_segments {
                    walls.push(Segment { a: (ax / g, ay / g), b: (bx / g, by / g) });
                }
            } else {
                let open = room.map(|r| r.open_walls).unwrap_or_default();
                let c = rl.corners();
                for (i, is_open) in [open.north, open.east, open.south, open.west].into_iter().enumerate() {
                    if !is_open {
                        walls.push(Segment { a: c[i], b: c[(i + 1) % 4] });
                    }
                }
            }
        } else {
            match shape {
                RoomShape::Rectangle => {
                    let open = room.map(|r| r.open_walls).unwrap_or_default();
                    let flush = flush_walls_with_layout(&rl.room_id, rl, graph, layout);
                    // Gaps along each wall: wall openings (corridor crossings) and open doors.
                    let mut gaps_h_top = Vec::new();
                    let mut gaps_h_bot = Vec::new();
                    let mut gaps_v_left = Vec::new();
                    let mut gaps_v_right = Vec::new();
                    for wp in &rl.wall_openings {
                        let cw = layout.corridors.iter()
                            .find(|c| c.waypoints.iter().any(|p| *p == *wp))
                            .map(|c| c.width as f32)
                            .unwrap_or(2.0);
                        let (wx, wy) = (wp.x as f32, wp.y as f32);
                        if (wy - y0).abs() < 1.0 { gaps_h_top.push((wx - cw / 2.0, wx + cw / 2.0)); }
                        if (wy - y1).abs() < 1.0 { gaps_h_bot.push((wx - cw / 2.0, wx + cw / 2.0)); }
                        if (wx - x0).abs() < 1.0 { gaps_v_left.push((wy - cw / 2.0, wy + cw / 2.0)); }
                        if (wx - x1).abs() < 1.0 { gaps_v_right.push((wy - cw / 2.0, wy + cw / 2.0)); }
                    }
                    for (room_id, (dx0, dy0, dx1, dy1)) in &apertures {
                        if *room_id != rl.room_id { continue; }
                        let (mx, my) = ((dx0 + dx1) / 2.0, (dy0 + dy1) / 2.0);
                        if (my - y0).abs() < 0.5 { gaps_h_top.push((*dx0, *dx1)); }
                        else if (my - y1).abs() < 0.5 { gaps_h_bot.push((*dx0, *dx1)); }
                        else if (mx - x0).abs() < 0.5 { gaps_v_left.push((*dy0, *dy1)); }
                        else if (mx - x1).abs() < 0.5 { gaps_v_right.push((*dy0, *dy1)); }
                    }
                    if !open.north && !flush.north { push_wall_with_gaps(walls, (x0, y0), (x1, y0), &gaps_h_top); }
                    if !open.south && !flush.south { push_wall_with_gaps(walls, (x0, y1), (x1, y1), &gaps_h_bot); }
                    if !open.west && !flush.west { push_wall_with_gaps(walls, (x0, y0), (x0, y1), &gaps_v_left); }
                    if !open.east && !flush.east { push_wall_with_gaps(walls, (x1, y0), (x1, y1), &gaps_v_right); }
                }
                RoomShape::Circle => {
                    // Rim as a polygon; door gaps are handled by the corridor floor cells touching it.
                    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                    let r = (rl.width.min(rl.height) as f32) / 2.0;
                    let n = 24;
                    let mut prev = (cx + r, cy);
                    for i in 1..=n {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU;
                        let p = (cx + r * a.cos(), cy + r * a.sin());
                        // Skip rim pieces that sit on an aperture of this room
                        let mid = ((prev.0 + p.0) / 2.0, (prev.1 + p.1) / 2.0);
                        let in_aperture = apertures.iter().any(|(rid, (ax0, ay0, ax1, ay1))| {
                            *rid == rl.room_id && mid.0 >= ax0 - 0.5 && mid.0 <= ax1 + 0.5 && mid.1 >= ay0 - 0.5 && mid.1 <= ay1 + 0.5
                        });
                        if !in_aperture {
                            walls.push(Segment { a: prev, b: p });
                        }
                        prev = p;
                    }
                }
                RoomShape::Cave => {
                    // Cave walls are solid cells already (non-floor inside the rect), except
                    // where the cave overlaps another room's floor: there its contour stands
                    // on open floor, so it becomes a wall segment wherever it stays visible.
                    let partners = crate::render::overlap::overlap_partners(&rl.room_id, graph, layout, false);
                    if let Some(cave) = room.and_then(|r| r.cave_data.as_ref()).filter(|_| !partners.is_empty()) {
                        let g = crate::util::GRID_PX;
                        for &(ax, ay, bx, by) in &cave.contour_segments {
                            let (a, b) = ((ax / g, ay / g), (bx / g, by / g));
                            let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                            if crate::render::overlap::point_inside_any(mid, &partners) {
                                walls.push(Segment { a, b });
                            }
                        }
                    }
                }
            }
        }
        let mut hidden_by = crate::render::overlap::wall_suppressors(&rl.room_id, graph, layout);
        hidden_by.extend(crate::render::overlap::attached_corridor_interiors(&rl.room_id, graph, layout, passable));
        for w in room_walls {
            for (a, b) in crate::render::overlap::visible_parts(w.a, w.b, &hidden_by) {
                walls_all.push(Segment { a, b });
            }
        }
    }

    // Objects
    let mut objects = Vec::new();
    for rl in &layout.rooms {
        let Some(room) = graph.room_by_id(&rl.room_id) else { continue };
        for d in &room.decor {
            let cover = d.cover_kind();
            if cover == CoverKind::None {
                continue;
            }
            let (ex, ey) = d.decor_type.local_extent();
            let unit = DECOR_HALF_SIZE / GRID_PX; // 0.4 cell
            let hx = ex * d.scale_x * unit;
            let hy = ey * d.scale_y * unit;
            // Decor sits in the room's local frame and turns with the room
            let (cx, cy) = rl.to_world(d.x, d.y);
            let (s, c) = (d.rotation + rl.rotation).to_radians().sin_cos();
            let rot = |lx: f32, ly: f32| (cx + lx * c - ly * s, cy + lx * s + ly * c);
            objects.push(Obstacle {
                poly: vec![rot(-hx, -hy), rot(hx, -hy), rot(hx, hy), rot(-hx, hy)],
                cover,
            });
        }
    }

    for ci in 0..shapes.len() {
        for (a, b) in crate::render::overlap::freeform_corridor_walls(ci, &shapes, graph, layout) {
            walls_all.push(Segment { a, b });
        }
    }
    let walls = merge_collinear_walls(walls_all);
    let mut occ = Occluders {
        floor: Arc::new(floor),
        solid: Arc::new(solid),
        walls: Arc::new(walls),
        objects: Arc::new(objects),
        creatures: Vec::new(),
        static_hash: 0,
    };
    occ.static_hash = hash_static_parts(&occ);
    occ
}

/// Build or reuse the static occluders cached on the presentation state, then add
/// the current creatures. Rebuilds only when [`geometry_key`] changes.
pub fn ensure_occluders(
    presentation: &mut super::PresentationState,
    dungeon: &Dungeon,
    layout: &SpatialLayout,
    token_infos: &[TokenInfo],
) -> Occluders {
    let key = geometry_key(dungeon, layout, presentation);
    let stale = presentation.occ_cache.as_ref().map(|(k, _)| *k != key).unwrap_or(true);
    if stale {
        let snapshot = super::PresentationSnapshot {
            room_visibility: presentation.room_visibility.clone(),
            doors_open: presentation.doors_open.clone(),
        };
        let built = build_static_occluders(dungeon, layout, &snapshot);
        presentation.occ_cache = Some((key, built));
    }
    presentation.occ_cache.as_ref().unwrap().1.with_creatures(dungeon, token_infos)
}

/// Join wall pieces that lie on the same line and touch or overlap, so a point where
/// two rooms' walls meet is wall interior rather than a wall end (which the corner rule
/// would treat as clear).
pub fn merge_collinear_walls(walls: Vec<Segment>) -> Vec<Segment> {
    const E: f32 = 1e-4;
    let mut out = Vec::new();
    // Key: (is_horizontal, line coordinate as bits)
    let mut groups: std::collections::HashMap<(bool, u32), Vec<(f32, f32)>> = std::collections::HashMap::new();
    let mut others = Vec::new();
    for w in walls {
        if (w.a.1 - w.b.1).abs() < E {
            groups.entry((true, w.a.1.to_bits())).or_default().push((w.a.0.min(w.b.0), w.a.0.max(w.b.0)));
        } else if (w.a.0 - w.b.0).abs() < E {
            groups.entry((false, w.a.0.to_bits())).or_default().push((w.a.1.min(w.b.1), w.a.1.max(w.b.1)));
        } else {
            others.push(w);
        }
    }
    for ((horizontal, coord_bits), mut ranges) in groups {
        let coord = f32::from_bits(coord_bits);
        ranges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut cur: Option<(f32, f32)> = None;
        for (lo, hi) in ranges {
            match cur {
                Some((clo, chi)) if lo <= chi + E => cur = Some((clo, chi.max(hi))),
                Some(done) => { out.push(range_to_segment(done, horizontal, coord)); cur = Some((lo, hi)); }
                None => cur = Some((lo, hi)),
            }
        }
        if let Some(done) = cur {
            out.push(range_to_segment(done, horizontal, coord));
        }
    }
    out.extend(others);
    out
}

fn range_to_segment((lo, hi): (f32, f32), horizontal: bool, coord: f32) -> Segment {
    if horizontal { Segment { a: (lo, coord), b: (hi, coord) } } else { Segment { a: (coord, lo), b: (coord, hi) } }
}

fn push_wall_with_gaps(walls: &mut Vec<Segment>, a: Pt, b: Pt, gaps: &[(f32, f32)]) {
    let horizontal = (a.1 - b.1).abs() < 1e-6;
    let (lo, hi) = if horizontal { (a.0.min(b.0), a.0.max(b.0)) } else { (a.1.min(b.1), a.1.max(b.1)) };
    let mut gaps: Vec<(f32, f32)> = gaps.iter().map(|&(g0, g1)| (g0.max(lo), g1.min(hi))).filter(|(g0, g1)| g1 > g0).collect();
    gaps.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    let mut cur = lo;
    let mk = |s: f32, e: f32| if horizontal { Segment { a: (s, a.1), b: (e, a.1) } } else { Segment { a: (a.0, s), b: (a.0, e) } };
    for (g0, g1) in gaps {
        if g0 > cur + 1e-6 {
            walls.push(mk(cur, g0));
        }
        cur = cur.max(g1);
    }
    if hi > cur + 1e-6 {
        walls.push(mk(cur, hi));
    }
}

// ---------------------------------------------------------------------------
// Geometry primitives (ported from cover_sim.py)
// ---------------------------------------------------------------------------

fn cross2d(o: Pt, a: Pt, b: Pt) -> f32 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Convex hull, CCW (Andrew's monotone chain).
pub fn convex_hull(points: &[Pt]) -> Vec<Pt> {
    let mut pts: Vec<Pt> = points.to_vec();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
    pts.dedup();
    if pts.len() <= 1 {
        return pts;
    }
    let mut lower: Vec<Pt> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross2d(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Pt> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross2d(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Does an axis-aligned rect strictly overlap a convex polygon (touching does not count)?
pub fn rect_intersects_convex(x0: f32, y0: f32, x1: f32, y1: f32, hull: &[Pt]) -> bool {
    if hull.len() < 3 {
        return false;
    }
    let rc = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let n = hull.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let nx = hull[j].1 - hull[i].1;
        let ny = hull[i].0 - hull[j].0;
        let proj = |p: Pt| nx * p.0 + ny * p.1;
        let (min_h, max_h) = hull.iter().map(|&p| proj(p)).fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let (min_r, max_r) = rc.iter().map(|&p| proj(p)).fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        if max_r <= min_h || max_h <= min_r {
            return false;
        }
    }
    let (hx0, hx1) = hull.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
    let (hy0, hy1) = hull.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    if hx1 <= x0 || hx0 >= x1 || hy1 <= y0 || hy0 >= y1 {
        return false;
    }
    true
}

/// Liang-Barsky: does the segment pass through the box interior?
fn segment_hits_aabb(p1: Pt, p2: Pt, rx0: f32, ry0: f32, rx1: f32, ry1: f32) -> bool {
    let dx = p2.0 - p1.0;
    let dy = p2.1 - p1.1;
    let checks = [
        (-dx, -(rx0 - p1.0)),
        (dx, rx1 - p1.0),
        (-dy, -(ry0 - p1.1)),
        (dy, ry1 - p1.1),
    ];
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in checks {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 { t0 = t0.max(r); } else { t1 = t1.min(r); }
            if t0 > t1 {
                return false;
            }
        }
    }
    t0 < t1
}

/// Segment through a cell's interior (inward epsilon so grazing the face is clear).
fn segment_hits_cell(p1: Pt, p2: Pt, c: (i32, i32)) -> bool {
    const EPS: f32 = 1e-5;
    segment_hits_aabb(p1, p2, c.0 as f32 + EPS, c.1 as f32 + EPS, c.0 as f32 + 1.0 - EPS, c.1 as f32 + 1.0 - EPS)
}

/// Interior overlap of two squares (touching edges do not count).
fn squares_overlap(a: &Square, b: &Square) -> bool {
    const E: f32 = 1e-4;
    a.x0 < b.x1 - E && b.x0 < a.x1 - E && a.y0 < b.y1 - E && b.y0 < a.y1 - E
}

fn segment_hits_square(p1: Pt, p2: Pt, s: &Square) -> bool {
    const EPS: f32 = 1e-5;
    segment_hits_aabb(p1, p2, s.x0 + EPS, s.y0 + EPS, s.x1 - EPS, s.y1 - EPS)
}

fn segments_intersect(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let cross = |o: Pt, p: Pt, q: Pt| (p.0 - o.0) * (q.1 - o.1) - (p.1 - o.1) * (q.0 - o.0);
    let d1 = cross(c, d, a);
    let d2 = cross(c, d, b);
    let d3 = cross(a, b, c);
    let d4 = cross(a, b, d);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    let on_seg = |p: Pt, q: Pt, r: Pt| {
        p.0.min(q.0) <= r.0 && r.0 <= p.0.max(q.0) && p.1.min(q.1) <= r.1 && r.1 <= p.1.max(q.1)
    };
    const E: f32 = 1e-7;
    (d1.abs() < E && on_seg(c, d, a))
        || (d2.abs() < E && on_seg(c, d, b))
        || (d3.abs() < E && on_seg(a, b, c))
        || (d4.abs() < E && on_seg(a, b, d))
}


fn segment_hits_polygon(p1: Pt, p2: Pt, poly: &[Pt]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    if crate::model::geometry::point_in_polygon(p1, poly) || crate::model::geometry::point_in_polygon(p2, poly) {
        return true;
    }
    let n = poly.len();
    (0..n).any(|i| segments_intersect(p1, p2, poly[i], poly[(i + 1) % n]))
}

fn point_on_segment(p: Pt, a: Pt, b: Pt) -> bool {
    let cross = (p.0 - a.0) * (b.1 - a.1) - (p.1 - a.1) * (b.0 - a.0);
    if cross.abs() > 1e-6 {
        return false;
    }
    const E: f32 = 1e-6;
    a.0.min(b.0) - E <= p.0 && p.0 <= a.0.max(b.0) + E && a.1.min(b.1) - E <= p.1 && p.1 <= a.1.max(b.1) + E
}

fn same_pt(a: Pt, b: Pt) -> bool {
    (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
}

/// The simulator's wall-corner rule for solid cells.
fn segment_hits_solid_corner(p1: Pt, p2: Pt, active: &CellSet, all: &CellSet) -> bool {
    let mut checked: CellSet = CellSet::default();
    for &(wc, wr) in active {
        for (cx, cy) in [(wc, wr), (wc + 1, wr), (wc, wr + 1), (wc + 1, wr + 1)] {
            if !checked.insert((cx, cy)) {
                continue;
            }
            let corner = (cx as f32, cy as f32);
            if same_pt(corner, p2) {
                continue; // a line TO a wall corner is not blocked
            }
            if same_pt(corner, p1) {
                let dx = p2.0 - p1.0;
                let dy = p2.1 - p1.1;
                if dx.abs() < 1e-6 && dy.abs() < 1e-6 {
                    continue;
                }
                const EPS2: f32 = 1e-4;
                let fwd = ((p1.0 + dx * EPS2).floor() as i32, (p1.1 + dy * EPS2).floor() as i32);
                if !active.contains(&fwd) {
                    // Along a wall face? Probe both sides just ahead of the corner.
                    let len = (dx * dx + dy * dy).sqrt();
                    let (nx, ny) = (-dy / len, dx / len);
                    let mx = corner.0 + dx / len * EPS2;
                    let my = corner.1 + dy / len * EPS2;
                    let pa = ((mx + nx * EPS2).floor() as i32, (my + ny * EPS2).floor() as i32);
                    let pb = ((mx - nx * EPS2).floor() as i32, (my - ny * EPS2).floor() as i32);
                    if !active.contains(&pa) && !active.contains(&pb) {
                        continue; // leaving the wall — clear
                    }
                }
            }
            // Fully interior corners (all four cells solid) never matter
            let neighbors = [(cx - 1, cy - 1), (cx, cy - 1), (cx - 1, cy), (cx, cy)];
            if neighbors.iter().all(|n| all.contains(n)) {
                continue;
            }
            if point_on_segment(corner, p1, p2) {
                return true;
            }
        }
    }
    false
}

/// Axis-aligned ray running along the seam between two solid cells.
fn segment_on_solid_seam(p1: Pt, p2: Pt, active: &CellSet) -> bool {
    const EPS: f32 = 1e-6;
    if (p1.0 - p2.0).abs() < EPS {
        let gx = p1.0;
        if (gx - gx.round()).abs() < EPS {
            let igx = gx.round() as i32;
            let (y0, y1) = (p1.1.min(p2.1), p1.1.max(p2.1));
            let r0 = (y0 + EPS).floor() as i32;
            let r1 = (y1 - EPS).floor() as i32;
            for row in r0..=r1 {
                if active.contains(&(igx - 1, row)) && active.contains(&(igx, row)) {
                    return true;
                }
            }
        }
    } else if (p1.1 - p2.1).abs() < EPS {
        let gy = p1.1;
        if (gy - gy.round()).abs() < EPS {
            let igy = gy.round() as i32;
            let (x0, x1) = (p1.0.min(p2.0), p1.0.max(p2.0));
            let c0 = (x0 + EPS).floor() as i32;
            let c1 = (x1 - EPS).floor() as i32;
            for col in c0..=c1 {
                if active.contains(&(col, igy - 1)) && active.contains(&(col, igy)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Zero-thickness wall test with the same corner semantics as solid cells:
/// crossing, running along the wall, or passing through a wall end mid-line blocks.
/// A line that starts or ends on the wall's face is blocked only when the two
/// spaces (`sides` = attacker center, target center) are on opposite sides of it;
/// touching a wall *end* is the "line to a corner" case and stays clear.
fn segment_hits_wall(p1: Pt, p2: Pt, w: &Segment, sides: Option<(Pt, Pt)>) -> bool {
    let cross = |o: Pt, p: Pt, q: Pt| (p.0 - o.0) * (q.1 - o.1) - (p.1 - o.1) * (q.0 - o.0);
    const E: f32 = 1e-6;
    let d1 = cross(w.a, w.b, p1);
    let d2 = cross(w.a, w.b, p2);
    let d3 = cross(p1, p2, w.a);
    let d4 = cross(p1, p2, w.b);
    // Proper crossing
    if ((d1 > E && d2 < -E) || (d1 < -E && d2 > E)) && ((d3 > E && d4 < -E) || (d3 < -E && d4 > E)) {
        return true;
    }
    let collinear = d1.abs() < E && d2.abs() < E;
    if collinear && !same_pt(p1, p2) {
        // Overlap of positive length along the wall line?
        let horizontal = (w.a.1 - w.b.1).abs() < E;
        let (a0, a1, b0, b1) = if horizontal {
            (p1.0.min(p2.0), p1.0.max(p2.0), w.a.0.min(w.b.0), w.a.0.max(w.b.0))
        } else {
            (p1.1.min(p2.1), p1.1.max(p2.1), w.a.1.min(w.b.1), w.a.1.max(w.b.1))
        };
        if a1.min(b1) - a0.max(b0) > E {
            return true;
        }
    }
    // Wall end strictly inside the line (not at the line's endpoints)
    for end in [w.a, w.b] {
        if point_on_segment(end, p1, p2) && !same_pt(end, p1) && !same_pt(end, p2) {
            return true;
        }
    }
    // An endpoint resting on the wall's face (not at a wall end): the wall is between
    // the two spaces if their centers lie on opposite sides of it.
    let touches_face = |p: Pt, d: f32| d.abs() < E && point_on_segment(p, w.a, w.b) && !same_pt(p, w.a) && !same_pt(p, w.b);
    if touches_face(p1, d1) || touches_face(p2, d2) {
        if let Some((ac, tc)) = sides {
            let sa = cross(w.a, w.b, ac);
            let st = cross(w.a, w.b, tc);
            return (sa > E && st < -E) || (sa < -E && st > E);
        }
    }
    false
}

/// The obstacles relevant to one cover query, already narrowed to the hull.
pub struct LineGeo<'a> {
    /// Solid cells inside the hull.
    pub active_solid: &'a CellSet,
    /// Solid cells near them (for the interior-corner rule).
    pub nearby_solid: &'a CellSet,
    pub walls: &'a [Segment],
    pub objects: &'a [Obstacle],
    pub creatures: &'a [(TokenKind, Square)],
}

/// Classify one line. `sides` gives the attacker and target centers so walls touched
/// at an endpoint resolve correctly.
pub fn classify(p1: Pt, p2: Pt, geo: &LineGeo<'_>, exclude: &[TokenKind], sides: Option<(Pt, Pt)>) -> Block {
    for &c in geo.active_solid {
        if segment_hits_cell(p1, p2, c) {
            return Block::Wall;
        }
    }
    if segment_hits_solid_corner(p1, p2, geo.active_solid, geo.nearby_solid) {
        return Block::Wall;
    }
    if segment_on_solid_seam(p1, p2, geo.active_solid) {
        return Block::Wall;
    }
    for w in geo.walls {
        if segment_hits_wall(p1, p2, w, sides) {
            return Block::Wall;
        }
    }
    // Lines that start or end exactly where walls / solid cells meet (a room corner,
    // two rock cells touching diagonally, a T-junction) are blocked when the two
    // spaces sit in different sectors between the wall rays at that point.
    if let Some((ac, tc)) = sides {
        if joint_separates(p1, geo, ac, tc) || joint_separates(p2, geo, ac, tc) {
            return Block::Wall;
        }
    }
    let mut best_obj: Option<CoverKind> = None;
    for o in geo.objects {
        if segment_hits_polygon(p1, p2, &o.poly) {
            best_obj = Some(best_obj.map_or(o.cover, |b| b.max(o.cover)));
        }
    }
    if let Some(k) = best_obj {
        return Block::Object(k);
    }
    for (kind, sq) in geo.creatures {
        if exclude.contains(kind) {
            continue;
        }
        if segment_hits_square(p1, p2, sq) {
            return Block::Creature;
        }
    }
    Block::Clear
}

/// Directions (angles) of every wall ray leaving point `p`: edges of solid cells that
/// have `p` as a corner, and walls that end at or pass through `p`.
fn rays_at(p: Pt, geo: &LineGeo<'_>) -> Vec<f32> {
    const E: f32 = 1e-4;
    let mut rays: Vec<f32> = Vec::new();
    let mut push = |a: f32| {
        if !rays.iter().any(|r| (r - a).abs() < 1e-3 || ((r - a).abs() - std::f32::consts::TAU).abs() < 1e-3) {
            rays.push(a);
        }
    };
    let (w_ang, e_ang, n_ang, s_ang) = (std::f32::consts::PI, 0.0, -std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
    if (p.0 - p.0.round()).abs() < E && (p.1 - p.1.round()).abs() < E {
        let (ix, iy) = (p.0.round() as i32, p.1.round() as i32);
        let solid = |c: (i32, i32)| geo.active_solid.contains(&c) || geo.nearby_solid.contains(&c);
        if solid((ix - 1, iy - 1)) { push(w_ang); push(n_ang); }
        if solid((ix, iy - 1)) { push(e_ang); push(n_ang); }
        if solid((ix - 1, iy)) { push(w_ang); push(s_ang); }
        if solid((ix, iy)) { push(e_ang); push(s_ang); }
    }
    for w in geo.walls {
        let toward = |q: Pt| (q.1 - p.1).atan2(q.0 - p.0);
        if same_pt(p, w.a) {
            push(toward(w.b));
        } else if same_pt(p, w.b) {
            push(toward(w.a));
        } else if point_on_segment(p, w.a, w.b) {
            push(toward(w.a));
            push(toward(w.b));
        }
    }
    rays
}

/// Do the attacker and target centers fall in different sectors between the wall
/// rays meeting at `p`? (False when fewer than two rays meet there.)
fn joint_separates(p: Pt, geo: &LineGeo<'_>, ac: Pt, tc: Pt) -> bool {
    let mut rays = rays_at(p, geo);
    if rays.len() < 2 {
        return false;
    }
    rays.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = rays.len();
    let sector = |q: Pt| {
        let a = (q.1 - p.1).atan2(q.0 - p.0);
        // The arc above the last ray wraps around to the arc below the first one
        rays.iter().filter(|&&r| r <= a).count() % n
    };
    sector(ac) != sector(tc)
}

/// Turn a blocked-line tally into a cover level (2024 rules plus the obstacle cap).
pub fn cover_level(lines: &[(Pt, Pt, Block); 4]) -> CoverLevel {
    let blocked: Vec<Block> = lines.iter().map(|l| l.2).filter(|b| *b != Block::Clear).collect();
    let n = blocked.len();
    if n == 0 {
        return CoverLevel::None;
    }
    let all_full = blocked.iter().all(|b| b.cap() == CoverKind::Full);
    let by_count = if n == 4 && all_full {
        CoverLevel::Total
    } else if n >= 3 {
        CoverLevel::ThreeQuarters
    } else {
        CoverLevel::Half
    };
    let cap = blocked.iter().map(|b| b.cap()).max().unwrap_or(CoverKind::None).max_level();
    by_count.min(cap)
}

/// Cover the target has from the attacker, from the attacker's best corner.
pub fn cover_between(attacker: &Square, target: &Square, occ: &Occluders, exclude: &[TokenKind]) -> CoverResult {
    let ac = attacker.corners();
    let tc = target.corners();

    // Only obstacles inside the convex hull of both spaces provide cover.
    let mut pts: Vec<Pt> = ac.to_vec();
    pts.extend_from_slice(&tc);
    let hull = convex_hull(&pts);
    // Only cells under the hull's bounding box can intersect it: walk that box
    // instead of scanning every solid cell on the map.
    let (bx0, bx1) = hull.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
    let (by0, by1) = hull.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    let mut active_solid: CellSet = CellSet::default();
    for y in (by0.floor() as i32 - 1)..=(by1.ceil() as i32) {
        for x in (bx0.floor() as i32 - 1)..=(bx1.ceil() as i32) {
            if occ.solid.contains(&(x, y))
                && rect_intersects_convex(x as f32, y as f32, x as f32 + 1.0, y as f32 + 1.0, &hull)
            {
                active_solid.insert((x, y));
            }
        }
    }
    // Quick bounding-box rejection before the exact hull test
    let bbox_touches = |x0: f32, y0: f32, x1: f32, y1: f32| x1 >= bx0 && x0 <= bx1 && y1 >= by0 && y0 <= by1;
    let walls: Vec<Segment> = occ.walls.iter()
        .filter(|w| {
            let (x0, x1) = (w.a.0.min(w.b.0), w.a.0.max(w.b.0));
            let (y0, y1) = (w.a.1.min(w.b.1), w.a.1.max(w.b.1));
            bbox_touches(x0, y0, x1, y1) && rect_intersects_convex(x0 - 0.01, y0 - 0.01, x1 + 0.01, y1 + 0.01, &hull)
        })
        .copied()
        .collect();
    let objects: Vec<Obstacle> = occ.objects.iter()
        .filter(|o| {
            let (x0, x1) = o.poly.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
            let (y0, y1) = o.poly.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
            bbox_touches(x0, y0, x1, y1) && rect_intersects_convex(x0, y0, x1, y1, &hull)
        })
        .cloned()
        .collect();
    let creatures: Vec<(TokenKind, Square)> = occ.creatures.iter()
        .filter(|(k, _)| !exclude.contains(k))
        // A creature standing in either space is the attacker or the target itself
        .filter(|(_, s)| !squares_overlap(s, attacker) && !squares_overlap(s, target))
        .filter(|(_, s)| bbox_touches(s.x0, s.y0, s.x1, s.y1) && rect_intersects_convex(s.x0, s.y0, s.x1, s.y1, &hull))
        .cloned()
        .collect();
    let sides = Some((attacker.center(), target.center()));
    // `classify` reads corner rules against the full solid set for interior-corner checks.
    // Only the neighbourhood of the active cells matters, so copy just that.
    let mut nearby_solid = CellSet::default();
    for &(x, y) in &active_solid {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if occ.solid.contains(&(x + dx, y + dy)) {
                    nearby_solid.insert((x + dx, y + dy));
                }
            }
        }
    }
    let geo = LineGeo {
        active_solid: &active_solid,
        nearby_solid: &nearby_solid,
        walls: &walls,
        objects: &objects,
        creatures: &creatures,
    };

    let mut best: Option<CoverResult> = None;
    for (ci, &a) in ac.iter().enumerate() {
        let lines = [
            (a, tc[0], classify(a, tc[0], &geo, exclude, sides)),
            (a, tc[1], classify(a, tc[1], &geo, exclude, sides)),
            (a, tc[2], classify(a, tc[2], &geo, exclude, sides)),
            (a, tc[3], classify(a, tc[3], &geo, exclude, sides)),
        ];
        let blocked = lines.iter().filter(|l| l.2 != Block::Clear).count() as u8;
        let level = cover_level(&lines);
        let better = match &best {
            None => true,
            Some(b) => level < b.level || (level == b.level && blocked < b.blocked),
        };
        if better {
            best = Some(CoverResult { level, blocked, corner: ci, lines });
        }
        if blocked == 0 {
            break;
        }
    }
    best.unwrap()
}

/// Cells within `radius` of `from` whose centers are in line of sight
/// (walls, solid cells, and light-blocking objects only).
pub fn visible_cells(from: Pt, radius: f32, occ: &Occluders, floor: &CellSet) -> CellSet {
    let mut out = CellSet::default();
    let r = radius.max(0.0);
    let (fx, fy) = from;
    let min_x = (fx - r).floor() as i32;
    let max_x = (fx + r).ceil() as i32;
    let min_y = (fy - r).floor() as i32;
    let max_y = (fy + r).ceil() as i32;
    let r2 = r * r;
    // Only geometry inside the sweep box can block anything
    let (bx0, by0, bx1, by1) = (min_x as f32, min_y as f32, max_x as f32 + 1.0, max_y as f32 + 1.0);
    // Each object with its bounding box, so most lines skip the polygon test
    let light_objs: Vec<(&Obstacle, (f32, f32, f32, f32))> = occ.objects.iter()
        .filter(|o| o.blocks_light())
        .filter(|o| o.poly.iter().any(|p| p.0 >= bx0 && p.0 <= bx1 && p.1 >= by0 && p.1 <= by1))
        .map(|o| {
            let bb = o.poly.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, p| {
                (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))
            });
            (o, bb)
        })
        .collect();
    // Memo of solid cells over the sweep box, filled on first probe: the line walk below
    // probes cells several times per cell crossed, far too often for hash lookups.
    // 0 = unknown, 1 = clear, 2 = solid.
    let grid_w = (max_x - min_x + 1) as usize;
    let grid_h = (max_y - min_y + 1) as usize;
    let solid_memo = vec![std::cell::Cell::new(0u8); grid_w * grid_h];
    let is_solid = |gx: i32, gy: i32| {
        if gx < min_x || gx > max_x || gy < min_y || gy > max_y {
            return occ.solid.contains(&(gx, gy));
        }
        let slot = &solid_memo[(gy - min_y) as usize * grid_w + (gx - min_x) as usize];
        if slot.get() == 0 {
            slot.set(if occ.solid.contains(&(gx, gy)) { 2 } else { 1 });
        }
        slot.get() == 2
    };
    let walls: Vec<&Segment> = occ.walls.iter()
        .filter(|w| {
            let (wx0, wx1) = (w.a.0.min(w.b.0), w.a.0.max(w.b.0));
            let (wy0, wy1) = (w.a.1.min(w.b.1), w.a.1.max(w.b.1));
            wx1 >= bx0 && wx0 <= bx1 && wy1 >= by0 && wy0 <= by1
        })
        .collect();
    for gy in min_y..=max_y {
        for gx in min_x..=max_x {
            if !floor.contains(&(gx, gy)) {
                continue;
            }
            let (cx, cy) = (gx as f32 + 0.5, gy as f32 + 0.5);
            let dx = cx - fx;
            let dy = cy - fy;
            if dx * dx + dy * dy > r2 {
                continue;
            }
            // Only the origin's own cell is visible unconditionally; neighbours must
            // pass the line test so an adjacent wall still blocks.
            if gx == fx.floor() as i32 && gy == fy.floor() as i32 {
                out.insert((gx, gy));
                continue;
            }
            let p1 = (fx, fy);
            let p2 = (cx, cy);
            const E: f32 = 1e-3;
            let (lx0, lx1, ly0, ly1) = (p1.0.min(p2.0) - E, p1.0.max(p2.0) + E, p1.1.min(p2.1) - E, p1.1.max(p2.1) + E);
            let blocked = line_crosses_solid(p1, p2, &is_solid)
                || walls.iter().any(|w| segment_hits_wall(p1, p2, w, None))
                || light_objs.iter().any(|(o, bb)| {
                    bb.2 >= lx0 && bb.0 <= lx1 && bb.3 >= ly0 && bb.1 <= ly1
                        && segment_hits_polygon(p1, p2, &o.poly)
                });
            if !blocked {
                out.insert((gx, gy));
            }
        }
    }
    out
}

/// Does the line pass through the interior of any solid cell? Sampled along the
/// line (every 0.2 cells) so cost is proportional to length, not to the number of
/// solid cells. Samples on a cell edge do not count, so grazing a face stays clear.
fn line_crosses_solid(p1: Pt, p2: Pt, is_solid: impl Fn(i32, i32) -> bool) -> bool {
    let dx = p2.0 - p1.0;
    let dy = p2.1 - p1.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return false;
    }
    let steps = (len / 0.2).ceil().max(1.0) as i32;
    const EPS: f32 = 1e-3;
    for i in 1..steps {
        let t = i as f32 / steps as f32;
        let x = p1.0 + dx * t;
        let y = p1.1 + dy * t;
        let fx = x - x.floor();
        let fy = y - y.floor();
        if fx < EPS || fx > 1.0 - EPS || fy < EPS || fy > 1.0 - EPS {
            continue;
        }
        if is_solid(x.floor() as i32, y.floor() as i32) {
            return true;
        }
    }
    false
}

/// Hash the static parts (done once per rebuild). Cells are combined order-independently.
fn hash_static_parts(occ: &Occluders) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    occ.solid.len().hash(&mut h);
    let mut acc: u64 = 0;
    for &(x, y) in occ.solid.iter() {
        let mut ch = std::collections::hash_map::DefaultHasher::new();
        (x, y).hash(&mut ch);
        acc = acc.wrapping_add(ch.finish());
    }
    acc.hash(&mut h);
    for w in occ.walls.iter() {
        w.a.0.to_bits().hash(&mut h); w.a.1.to_bits().hash(&mut h);
        w.b.0.to_bits().hash(&mut h); w.b.1.to_bits().hash(&mut h);
    }
    for o in occ.objects.iter() {
        (o.cover as u8).hash(&mut h);
        for p in &o.poly { p.0.to_bits().hash(&mut h); p.1.to_bits().hash(&mut h); }
    }
    h.finish()
}

/// Fingerprint of the static geometry only (walls, cells, objects) — what light depends on.
pub fn occluder_hash_static(occ: &Occluders) -> u64 {
    occ.static_hash
}

/// Fingerprint of everything that affects cover: static geometry plus creature squares.
pub fn occluder_hash(occ: &Occluders) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    occ.static_hash.hash(&mut h);
    for (k, s) in &occ.creatures {
        k.hash(&mut h);
        s.x0.to_bits().hash(&mut h); s.y0.to_bits().hash(&mut h);
        s.x1.to_bits().hash(&mut h); s.y1.to_bits().hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occ(walls: &[(i32, i32)], creatures: &[(i32, i32)]) -> Occluders {
        Occluders {
            floor: Arc::new(CellSet::default()),
            solid: Arc::new(walls.iter().copied().collect()),
            walls: Arc::new(Vec::new()),
            objects: Arc::new(Vec::new()),
            creatures: creatures.iter().enumerate()
                .map(|(i, &(x, y))| (TokenKind::Player(format!("c{}", i)), Square::cell(x, y)))
                .collect(),
            static_hash: 0,
        }
    }

    fn geo_all(o: &Occluders) -> LineGeo<'_> {
        LineGeo { active_solid: &o.solid, nearby_solid: &o.solid, walls: &o.walls, objects: &o.objects, creatures: &o.creatures }
    }

    fn blocked_best(o: &Occluders, attacker: (i32, i32), target: (i32, i32)) -> u8 {
        cover_between(&Square::cell(attacker.0, attacker.1), &Square::cell(target.0, target.1), o, &[]).blocked
    }

    fn level(o: &Occluders, attacker: (i32, i32), target: (i32, i32)) -> CoverLevel {
        cover_between(&Square::cell(attacker.0, attacker.1), &Square::cell(target.0, target.1), o, &[]).level
    }

    fn classify_all(o: &Occluders, p1: Pt, p2: Pt) -> Block {
        classify(p1, p2, &geo_all(o), &[], None)
    }

    #[test]
    fn thresholds() {
        let mk = |blocks: [Block; 4]| {
            let z = (0.0, 0.0);
            cover_level(&[(z, z, blocks[0]), (z, z, blocks[1]), (z, z, blocks[2]), (z, z, blocks[3])])
        };
        use Block::*;
        assert_eq!(mk([Clear, Clear, Clear, Clear]), CoverLevel::None);
        assert_eq!(mk([Wall, Clear, Clear, Clear]), CoverLevel::Half);
        assert_eq!(mk([Wall, Wall, Clear, Clear]), CoverLevel::Half);
        assert_eq!(mk([Wall, Wall, Wall, Clear]), CoverLevel::ThreeQuarters);
        assert_eq!(mk([Wall, Wall, Wall, Wall]), CoverLevel::Total);
        // Creatures cap at half; creature + wall still reaches three-quarters
        assert_eq!(mk([Creature, Creature, Creature, Creature]), CoverLevel::Half);
        assert_eq!(mk([Creature, Wall, Wall, Clear]), CoverLevel::ThreeQuarters);
        assert_eq!(mk([Creature, Wall, Wall, Wall]), CoverLevel::ThreeQuarters);
        // A low table blocking everything is still only half
        assert_eq!(mk([Object(CoverKind::Half); 4]), CoverLevel::Half);
        assert_eq!(mk([Object(CoverKind::ThreeQuarters); 4]), CoverLevel::ThreeQuarters);
        assert_eq!(mk([Object(CoverKind::Full); 4]), CoverLevel::Total);
    }

    #[test]
    fn no_wall_no_cover() {
        let o = occ(&[], &[]);
        for t in [(5, 3), (3, 5), (7, 5), (5, 7), (3, 3), (7, 7)] {
            assert_eq!(blocked_best(&o, (5, 5), t), 0, "{:?}", t);
        }
    }

    #[test]
    fn wall_between_and_full_cover() {
        let o = occ(&[(5, 5)], &[]);
        assert!(blocked_best(&o, (5, 3), (5, 7)) >= 1);
        let o = occ(&[(3, 5), (4, 5), (5, 5), (6, 5), (7, 5)], &[]);
        assert_eq!(blocked_best(&o, (5, 3), (5, 7)), 4);
        assert_eq!(level(&o, (5, 3), (5, 7)), CoverLevel::Total);
    }

    #[test]
    fn corner_rules() {
        let o = occ(&[(5, 5)], &[]);
        assert_eq!(classify_all(&o, (4.0, 4.0), (6.0, 6.0)), Block::Wall); // through TL corner mid-segment
        assert_eq!(classify_all(&o, (5.0, 5.0), (7.0, 7.0)), Block::Wall); // through BR corner
        assert_eq!(classify_all(&o, (6.0, 4.0), (4.0, 6.0)), Block::Wall); // other diagonal
        assert_eq!(classify_all(&o, (4.0, 4.0), (5.0, 5.0)), Block::Clear); // TO a corner
        assert_eq!(classify_all(&o, (5.0, 5.0), (4.0, 4.0)), Block::Clear); // FROM a corner, away
        assert_eq!(classify_all(&o, (7.0, 7.0), (6.0, 6.0)), Block::Clear);
        assert_eq!(classify_all(&o, (5.0, 5.0), (6.0, 6.0)), Block::Wall); // from corner into the wall
    }

    #[test]
    fn seams_and_edges() {
        let o = occ(&[(4, 4), (5, 4)], &[]);
        assert_eq!(classify_all(&o, (5.0, 3.0), (5.0, 5.0)), Block::Wall);
        let o = occ(&[(4, 3), (4, 4)], &[]);
        assert_eq!(classify_all(&o, (3.0, 4.0), (6.0, 4.0)), Block::Wall);
        let o = occ(&[(5, 4)], &[]);
        assert_eq!(classify_all(&o, (5.0, 3.0), (5.0, 5.0)), Block::Wall);
    }

    #[test]
    fn creature_blocks_and_caps_at_half() {
        let o = occ(&[], &[(5, 5)]);
        assert_eq!(classify_all(&o, (5.0, 3.0), (6.0, 6.0)), Block::Creature);
        // Creature directly between attacker and target: at most half cover
        assert!(level(&o, (5, 3), (5, 7)) <= CoverLevel::Half);
        assert!(blocked_best(&o, (5, 3), (5, 7)) >= 1);
    }

    #[test]
    fn creature_uses_whole_square() {
        // A Small creature drawn at 80% still blocks lines that skirt its cell
        let mut o = occ(&[], &[]);
        o.creatures.push((TokenKind::Player("s".into()), Square::centered(5.5, 5.5, 1.0)));
        // Line passing 0.05 inside the cell edge
        assert_eq!(classify_all(&o, (5.05, 3.0), (5.05, 8.0)), Block::Creature);
    }

    #[test]
    fn best_corner_is_minimum() {
        let o = occ(&[(5, 5), (6, 5)], &[]);
        for t in [(4, 6), (5, 6), (6, 6), (7, 6)] {
            let best = cover_between(&Square::cell(5, 3), &Square::cell(t.0, t.1), &o, &[]);
            let ac = Square::cell(5, 3).corners();
            let tc = Square::cell(t.0, t.1).corners();
            for a in ac {
                let n = tc.iter().filter(|&&c| classify(a, c, &geo_all(&o), &[], None) != Block::Clear).count() as u8;
                assert!(best.blocked <= n);
            }
        }
    }

    #[test]
    fn recorded_cases() {
        // From cover-sim/test_cases.json
        let cases: Vec<(Vec<(i32, i32)>, (i32, i32), Vec<((i32, i32), (u8, u8))>)> = vec![
            (
                (0..=9).map(|x| (x, 7)).chain((10..=15).flat_map(|x| [(x, 5), (x, 6), (x, 7)]))
                    .chain((16..=19).flat_map(|x| [(x, 5), (x, 6)])).collect(),
                (9, 5),
                vec![((10, 4), (1, 2))],
            ),
            (vec![(7, 3)], (6, 3), vec![((8, 3), (4, 4)), ((7, 2), (1, 2)), ((7, 4), (1, 2))]),
            (vec![(7, 3)], (7, 2), vec![((6, 3), (1, 2)), ((8, 3), (1, 2)), ((7, 4), (4, 4))]),
        ];
        for (walls, attacker, expectations) in cases {
            let o = occ(&walls, &[]);
            for (target, (lo, hi)) in expectations {
                let b = blocked_best(&o, attacker, target);
                assert!(lo <= b && b <= hi, "attacker {:?} target {:?}: {} blocked, expected {}-{}", attacker, target, b, lo, hi);
            }
        }
    }

    #[test]
    fn zero_thickness_walls() {
        let mut o = occ(&[], &[]);
        Arc::make_mut(&mut o.walls).push(Segment { a: (5.0, 3.0), b: (5.0, 8.0) });
        assert_eq!(classify_all(&o, (4.0, 5.0), (6.0, 5.0)), Block::Wall); // crossing
        assert_eq!(classify_all(&o, (5.0, 4.0), (5.0, 6.0)), Block::Wall); // along
        assert_eq!(classify_all(&o, (5.0, 4.0), (3.0, 6.0)), Block::Clear); // leaving
        assert_eq!(classify_all(&o, (3.0, 4.0), (5.0, 6.0)), Block::Clear); // arriving
        assert_eq!(classify_all(&o, (4.0, 2.0), (6.0, 9.0)), Block::Wall); // crossing through interior
        assert_eq!(classify_all(&o, (4.0, 1.0), (6.0, 2.0)), Block::Clear); // misses entirely
        assert_eq!(classify_all(&o, (4.0, 2.0), (6.0, 4.0)), Block::Wall); // through the wall's end mid-line
        // Two creatures hugging the same wall: attacker's off-wall corner sees the target
        let a = Square::cell(4, 4);
        let t = Square::cell(4, 6);
        assert_eq!(cover_between(&a, &t, &o, &[]).level, CoverLevel::None);
        // Through a door gap
        let mut o2 = occ(&[], &[]);
        push_wall_with_gaps(Arc::make_mut(&mut o2.walls), (5.0, 0.0), (5.0, 10.0), &[(4.0, 6.0)]);
        assert_eq!(o2.walls.len(), 2);
        assert_eq!(level(&o2, (3, 4), (7, 4)), CoverLevel::None);
        assert_eq!(level(&o2, (3, 1), (7, 1)), CoverLevel::Total);
    }

    #[test]
    fn cell_right_behind_a_room_wall_is_total_cover() {
        // Wall along x = 5; attacker inside at (3,4), target cells hugging the far side
        let mut o = occ(&[], &[]);
        Arc::make_mut(&mut o.walls).push(Segment { a: (5.0, 0.0), b: (5.0, 10.0) });
        assert_eq!(level(&o, (3, 4), (5, 4)), CoverLevel::Total);
        assert_eq!(level(&o, (3, 4), (5, 6)), CoverLevel::Total);
        // Same side, hugging the wall: no cover
        assert_eq!(level(&o, (3, 4), (4, 7)), CoverLevel::None);
        // Through a wall end (door jamb): the corner rule applies, not total
        let mut o2 = occ(&[], &[]);
        Arc::make_mut(&mut o2.walls).push(Segment { a: (5.0, 0.0), b: (5.0, 5.0) });
        assert!(level(&o2, (3, 4), (5, 4)) < CoverLevel::Total);
    }

    #[test]
    fn target_never_blocks_itself() {
        // The creature standing on the scored cell must not count as an obstacle,
        // so the heatmap and the token badge agree.
        let o = occ(&[], &[(7, 4)]);
        let heat = cover_between(&Square::cell(3, 4), &Square::cell(7, 4), &o, &[]);
        assert_eq!(heat.level, CoverLevel::None);
        let badge = cover_between(&Square::cell(3, 4), &Square::cell(7, 4), &o, &[TokenKind::Player("c0".into())]);
        assert_eq!(badge.level, heat.level);
    }

    #[test]
    fn walls_meeting_at_a_room_corner_still_block() {
        // Room A's east wall x=5 (y 0..5) and room B's east wall x=5 (y 5..10) meet at (5,5).
        let pieces = vec![
            Segment { a: (5.0, 0.0), b: (5.0, 5.0) },
            Segment { a: (5.0, 5.0), b: (5.0, 10.0) },
        ];
        let merged = merge_collinear_walls(pieces);
        assert_eq!(merged.len(), 1);
        let mut o = occ(&[], &[]);
        *Arc::make_mut(&mut o.walls) = merged;
        // Attacker in the cell whose corner is exactly the meeting point, target across the wall
        let r = cover_between(&Square::cell(4, 4), &Square::cell(5, 5), &o, &[]);
        assert_eq!(r.level, CoverLevel::Total, "{:?}", r);
        assert_eq!(level(&o, (4, 4), (5, 4)), CoverLevel::Total);
        // Pieces with a real gap between them stay separate
        let gapped = merge_collinear_walls(vec![
            Segment { a: (5.0, 0.0), b: (5.0, 4.0) },
            Segment { a: (5.0, 6.0), b: (5.0, 10.0) },
        ]);
        assert_eq!(gapped.len(), 2);
    }

    #[test]
    fn concave_room_corner_blocks_but_convex_corner_does_not() {
        // Room x<5, y<5 with rock outside: walls meet at (5,5), rock at (5,4) and (4,5)
        let mut o = occ(&[(5, 4), (4, 5)], &[]);
        *Arc::make_mut(&mut o.walls) = vec![
            Segment { a: (0.0, 5.0), b: (5.0, 5.0) },
            Segment { a: (5.0, 0.0), b: (5.0, 5.0) },
        ];
        // Attacker in the corner cell, target diagonally outside the corner and further out
        assert_eq!(level(&o, (4, 4), (5, 5)), CoverLevel::Total);
        assert_eq!(level(&o, (4, 4), (6, 6)), CoverLevel::Total);
        assert_eq!(level(&o, (3, 3), (6, 6)), CoverLevel::Total);
        // Outside the convex corner, looking around it stays clear
        let o2 = {
            let mut o2 = occ(&[], &[]);
            *Arc::make_mut(&mut o2.walls) = vec![
                Segment { a: (0.0, 5.0), b: (5.0, 5.0) },
                Segment { a: (5.0, 0.0), b: (5.0, 5.0) },
            ];
            o2
        };
        assert_eq!(level(&o2, (5, 5), (5, 3)), CoverLevel::None);
        assert_eq!(level(&o2, (6, 6), (5, 3)), CoverLevel::None);
        // Two rock cells touching diagonally: nothing squeezes through the pinch
        let o3 = occ(&[(5, 4), (4, 5)], &[]);
        assert_eq!(level(&o3, (4, 4), (5, 5)), CoverLevel::Total);
    }

    #[test]
    fn light_does_not_leak_to_the_neighbour_across_a_wall() {
        let floor: CellSet = (0..10).flat_map(|y| (0..10).map(move |x| (x, y))).collect();
        let mut o = occ(&[], &[]);
        Arc::make_mut(&mut o.walls).push(Segment { a: (5.0, 0.0), b: (5.0, 10.0) });
        // Token adjacent to the wall
        let seen = visible_cells((4.5, 5.5), 6.0, &o, &floor);
        assert!(seen.contains(&(4, 5)));
        assert!(!seen.contains(&(5, 5)), "neighbour across the wall must stay dark");
        assert!(!seen.contains(&(5, 4)));
    }

    #[test]
    fn visible_cells_respect_walls() {
        let floor: CellSet = (0..10).flat_map(|y| (0..10).map(move |x| (x, y))).collect();
        let mut o = occ(&[], &[]);
        Arc::make_mut(&mut o.walls).push(Segment { a: (5.0, 0.0), b: (5.0, 10.0) });
        let seen = visible_cells((2.5, 5.5), 6.0, &o, &floor);
        assert!(seen.contains(&(4, 5)));
        assert!(!seen.contains(&(6, 5)));
        assert!(seen.contains(&(2, 5)));
    }
}
