//! Room placement by beam search.
//!
//! Rooms are placed one at a time, most-constrained first (the room with the most
//! already-placed neighbours). For each, candidate spots are generated around every
//! placed neighbour and scored on estimated corridor length, crossings with the
//! connections already drawn, connections running through other rooms, and
//! compactness. Rather than committing to the best spot for each room in turn, the
//! search keeps the best few partial layouts and extends all of them, so an early
//! choice that boxes in later rooms can lose to an alternative.
//!
//! Only top-level rooms are placed here; rooms inside a container ride along with it
//! (their connections count as the container's) and are packed inside afterwards.

use std::collections::HashMap;

use crate::model::*;

/// A placed room footprint, in grid cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn center(&self) -> (f32, f32) {
        (self.x as f32 + self.w as f32 / 2.0, self.y as f32 + self.h as f32 / 2.0)
    }

    /// Whether the two rects come within `margin` cells of each other.
    fn near(&self, o: &Rect, margin: i32) -> bool {
        self.x < o.x + o.w + margin && o.x < self.x + self.w + margin
            && self.y < o.y + o.h + margin && o.y < self.y + self.h + margin
    }

    /// Empty cells between the rects along x and y (0 where they overlap on that axis).
    fn gaps(&self, o: &Rect) -> (i32, i32) {
        let gx = (o.x - (self.x + self.w)).max(self.x - (o.x + o.w)).max(0);
        let gy = (o.y - (self.y + self.h)).max(self.y - (o.y + o.h)).max(0);
        (gx, gy)
    }

    /// How far the rects overlap when projected on each axis (negative = apart).
    fn spans(&self, o: &Rect) -> (i32, i32) {
        ((self.x + self.w).min(o.x + o.w) - self.x.max(o.x), (self.y + self.h).min(o.y + o.h) - self.y.max(o.y))
    }
}

/// Which wall of a room a hand-set exit is on.
#[derive(Clone, Copy, PartialEq)]
enum Wall {
    Left,
    Right,
    Top,
    Bottom,
}

/// A connection between two top-level rooms (by index).
struct Link {
    a: usize,
    b: usize,
    /// Walls the corridor must leave `a` and `b` by (hand-set exits)
    wall_a: Option<Wall>,
    wall_b: Option<Wall>,
    width: i32,
    flush: bool,
    min_len: Option<u32>,
    max_len: Option<u32>,
}

/// A size-capped group of top-level rooms.
struct GroupCap {
    members: Vec<usize>,
    max_w: Option<u32>,
    max_h: Option<u32>,
}

/// What the search works on: the top-level rooms, their links and constraints.
pub struct Problem<'a> {
    graph: &'a DungeonGraph,
    ids: Vec<String>,
    sizes: Vec<(i32, i32)>,
    can_rotate: Vec<bool>,
    floors: Vec<FloorAssignment>,
    links: Vec<Link>,
    /// Links touching each room.
    adj: Vec<Vec<usize>>,
    groups: Vec<GroupCap>,
    gap: i32,
}

/// Search breadth: how many partial layouts survive each step, and how many spots per
/// layout are carried forward.
const BEAM: usize = 12;
const PER_STATE: usize = 6;

// Score weights (lower total is better)
const CROSSING: f32 = 80.0;
const THROUGH_ROOM: f32 = 50.0;
const BEND: f32 = 6.0;
const COMPACT: f32 = 0.05;
const WRONG_WALL: f32 = 40.0;

impl<'a> Problem<'a> {
    /// Build the problem for a set of sibling rooms (the top level, or one container's
    /// children), with sizes from `sizes` (containers already sized to fit their
    /// children). A connection from a room nested inside a sibling counts as that
    /// sibling's; connections leaving the set are ignored.
    ///
    /// Hand-set exits are read against the `previous` layout to learn which wall they
    /// are on; the search then tries to put the neighbour on that side.
    pub fn new(
        graph: &'a DungeonGraph,
        ids: Vec<String>,
        sizes: &HashMap<String, (u32, u32)>,
        gap: u32,
        previous: Option<&SpatialLayout>,
    ) -> Self {
        let member: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let top_of = |id: &str| -> Option<String> {
            let mut cur = id;
            for _ in 0..64 {
                if member.contains(cur) {
                    return Some(cur.to_string());
                }
                cur = graph.parent_of(cur)?;
            }
            None
        };
        let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
        let size_of = |id: &str| sizes.get(id).copied().or_else(|| graph.room_by_id(id).map(|r| r.grid_size())).unwrap_or((4, 4));
        let sizes_v: Vec<(i32, i32)> = ids.iter().map(|id| { let (w, h) = size_of(id); (w as i32, h as i32) }).collect();
        let rooms: Vec<&Room> = ids.iter().map(|id| graph.room_by_id(id).unwrap()).collect();
        let mut links: Vec<Link> = Vec::new();
        for e in &graph.connections {
            let (Some(ta), Some(tb)) = (top_of(&e.source_room_id), top_of(&e.target_room_id)) else { continue };
            let (Some(&a), Some(&b)) = (index.get(ta.as_str()), index.get(tb.as_str())) else { continue };
            if a == b {
                continue;
            }
            let key = (a.min(b), a.max(b));
            let flush = e.connection.connection_type == ConnectionType::Flush;
            if let Some(l) = links.iter_mut().find(|l| (l.a.min(l.b), l.a.max(l.b)) == key) {
                l.width = l.width.max(e.connection.corridor_width as i32);
                l.flush |= flush;
                continue;
            }
            // Length limits only mean something between the rooms themselves, not
            // between the containers they sit in
            let direct = ids[a] == e.source_room_id || ids[a] == e.target_room_id;
            let direct = direct && (ids[b] == e.source_room_id || ids[b] == e.target_room_id);
            let wall_of = |room: &str, exit: Option<ExitPos>| -> Option<Wall> {
                let (exit, rl) = (exit?, previous?.room_by_id(room)?);
                if rl.is_rotated() {
                    return None;
                }
                let (x0, y0, x1, y1) = (rl.x as f32, rl.y as f32, (rl.x + rl.width as i32) as f32, (rl.y + rl.height as i32) as f32);
                [(Wall::Left, (exit.x - x0).abs()), (Wall::Right, (exit.x - x1).abs()), (Wall::Top, (exit.y - y0).abs()), (Wall::Bottom, (exit.y - y1).abs())]
                    .into_iter()
                    .min_by(|p, q| p.1.partial_cmp(&q.1).unwrap())
                    .filter(|w| w.1 < 0.6)
                    .map(|w| w.0)
            };
            let exit_of = |idx: usize| -> Option<Wall> {
                if !direct { return None; }
                if ids[idx] == e.source_room_id { wall_of(&e.source_room_id, e.source_exit) } else { wall_of(&e.target_room_id, e.target_exit) }
            };
            links.push(Link {
                a, b,
                wall_a: exit_of(a),
                wall_b: exit_of(b),
                width: e.connection.corridor_width as i32, flush,
                min_len: e.connection.min_length.filter(|_| direct),
                max_len: e.connection.max_length.filter(|_| direct),
            });
        }
        let mut adj = vec![Vec::new(); ids.len()];
        for (li, l) in links.iter().enumerate() {
            adj[l.a].push(li);
            adj[l.b].push(li);
        }
        let groups = graph.groups.iter()
            .filter(|g| g.parent_room_id.is_none() && (g.max_width.is_some() || g.max_height.is_some()))
            .map(|g| GroupCap {
                members: g.room_ids.iter().filter_map(|id| index.get(id.as_str()).copied()).collect(),
                max_w: g.max_width,
                max_h: g.max_height,
            })
            .collect();
        Problem {
            graph,
            sizes: sizes_v,
            can_rotate: rooms.iter().map(|r| r.allow_rotation).collect(),
            floors: rooms.iter().map(|r| r.floor).collect(),
            ids,
            links,
            adj,
            groups,
            gap: gap as i32,
        }
    }

    fn other(&self, li: usize, i: usize) -> usize {
        let l = &self.links[li];
        if l.a == i { l.b } else { l.a }
    }

    /// Placement order: the entrance first, then always the room with the most placed
    /// neighbours (ties: more links, then graph order). A new component starts with its
    /// best-connected room.
    fn order(&self) -> Vec<usize> {
        let n = self.ids.len();
        let entrance = self.ids.iter().position(|id| {
            self.graph.room_by_id(id).is_some_and(|r| r.tags.contains(&RoomTag::Entrance))
        }).unwrap_or(0);
        let mut placed = vec![false; n];
        let mut out = Vec::with_capacity(n);
        let mut next = Some(entrance);
        while let Some(i) = next {
            placed[i] = true;
            out.push(i);
            next = (0..n).filter(|&j| !placed[j])
                .max_by_key(|&j| {
                    let placed_nbrs = self.adj[j].iter().filter(|&&li| placed[self.other(li, j)]).count();
                    (placed_nbrs, self.adj[j].len(), std::cmp::Reverse(j))
                });
        }
        out
    }

    /// Clearance between two rooms: none for flush neighbours (they touch), otherwise
    /// room for a corridor to pass.
    fn clearance(&self, i: usize, j: usize) -> i32 {
        let flush = self.adj[i].iter().any(|&li| self.other(li, i) == j && self.links[li].flush);
        if flush { 0 } else { self.gap + 2 }
    }

    fn same_floor(&self, i: usize, j: usize) -> bool {
        self.floors[i].shares_floor(&self.floors[j])
    }
}

/// A partial layout: positions of the rooms placed so far, and its running score.
#[derive(Clone)]
struct State {
    pos: Vec<Option<Rect>>,
    score: f32,
}

/// Proper crossing of segments a–b and c–d.
fn segments_cross(a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)) -> bool {
    let cross = |o: (f32, f32), p: (f32, f32), q: (f32, f32)| (p.0 - o.0) * (q.1 - o.1) - (p.1 - o.1) * (q.0 - o.0);
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    ((d1 > 1e-4 && d2 < -1e-4) || (d1 < -1e-4 && d2 > 1e-4)) && ((d3 > 1e-4 && d4 < -1e-4) || (d3 < -1e-4 && d4 > 1e-4))
}

/// Whether segment a–b passes through the interior of `r` (shrunk a little so grazing
/// a corner doesn't count).
fn segment_hits_rect(a: (f32, f32), b: (f32, f32), r: &Rect) -> bool {
    let (x0, y0, x1, y1) = (r.x as f32 + 0.3, r.y as f32 + 0.3, (r.x + r.w) as f32 - 0.3, (r.y + r.h) as f32 - 0.3);
    if x1 <= x0 || y1 <= y0 {
        return false;
    }
    // Liang–Barsky
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-dx, a.0 - x0), (dx, x1 - a.0), (-dy, a.1 - y0), (dy, y1 - a.1)] {
        if p.abs() < 1e-9 {
            if q < 0.0 { return false; }
        } else {
            let t = q / p;
            if p < 0.0 { t0 = t0.max(t); } else { t1 = t1.min(t); }
            if t0 > t1 { return false; }
        }
    }
    true
}

impl Problem<'_> {
    /// Whether room `i` can sit at `r` given the rooms already placed in `st`.
    fn valid(&self, st: &State, i: usize, r: &Rect) -> bool {
        for (j, p) in st.pos.iter().enumerate() {
            let Some(p) = p else { continue };
            if self.same_floor(i, j) && r.near(p, self.clearance(i, j)) {
                return false;
            }
        }
        // Flush neighbours must share a wall; length limits must hold
        for &li in &self.adj[i] {
            let l = &self.links[li];
            let Some(p) = st.pos[self.other(li, i)] else { continue };
            let (gx, gy) = r.gaps(&p);
            if l.flush {
                let (sx, sy) = r.spans(&p);
                let touching = (gx == 0 && sy >= 1 && (r.x + r.w == p.x || p.x + p.w == r.x))
                    || (gy == 0 && sx >= 1 && (r.y + r.h == p.y || p.y + p.h == r.y));
                if !touching {
                    return false;
                }
            }
            let dist = (gx + gy) as u32;
            if l.min_len.is_some_and(|m| dist < m) || l.max_len.is_some_and(|m| dist > m) {
                return false;
            }
        }
        for g in &self.groups {
            if !g.members.contains(&i) {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
            for &m in &g.members {
                if let Some(p) = st.pos[m] {
                    x0 = x0.min(p.x); y0 = y0.min(p.y); x1 = x1.max(p.x + p.w); y1 = y1.max(p.y + p.h);
                }
            }
            if g.max_w.is_some_and(|m| (x1 - x0) as u32 > m) || g.max_h.is_some_and(|m| (y1 - y0) as u32 > m) {
                return false;
            }
        }
        true
    }

    /// Cost of putting room `i` at `r` (lower is better).
    fn cost(&self, st: &State, i: usize, r: &Rect, centroid: (f32, f32)) -> f32 {
        let c = r.center();
        let mut cost = 0.0;
        // New connections: their length, and whether they can run straight
        let mut new_segs: Vec<((f32, f32), (f32, f32))> = Vec::new();
        for &li in &self.adj[i] {
            let j = self.other(li, i);
            let Some(p) = st.pos[j] else { continue };
            let (gx, gy) = r.gaps(&p);
            cost += (gx + gy) as f32;
            let (sx, sy) = r.spans(&p);
            let w = self.links[li].width;
            if sx < w && sy < w {
                cost += BEND; // no straight run: the corridor must turn
            }
            new_segs.push((c, p.center()));
            // A hand-set exit wants the neighbour out past its wall
            let l = &self.links[li];
            let (mine, theirs) = if l.a == i { (l.wall_a, l.wall_b) } else { (l.wall_b, l.wall_a) };
            if mine.is_some_and(|w| !beyond(r, &p, w)) {
                cost += WRONG_WALL;
            }
            if theirs.is_some_and(|w| !beyond(&p, r, w)) {
                cost += WRONG_WALL;
            }
        }
        // Crossings with connections already drawn, and connections through rooms
        for l in &self.links {
            let (Some(pa), Some(pb)) = (st.pos[l.a], st.pos[l.b]) else { continue };
            if l.a == i || l.b == i {
                continue;
            }
            let (ca, cb) = (pa.center(), pb.center());
            if self.same_floor(i, l.a) && segment_hits_rect(ca, cb, r) {
                cost += THROUGH_ROOM;
            }
            for &(s, t) in &new_segs {
                // Links sharing a room meet there; that is not a crossing
                let shares = self.adj[i].iter().any(|&nl| {
                    let o = self.other(nl, i);
                    (o == l.a || o == l.b) && st.pos[o].is_some_and(|p| p.center() == t)
                });
                if !shares && segments_cross(s, t, ca, cb) {
                    cost += CROSSING;
                }
            }
        }
        for &(s, t) in &new_segs {
            for (j, p) in st.pos.iter().enumerate() {
                let Some(p) = p else { continue };
                if p.center() == t || !self.same_floor(i, j) {
                    continue;
                }
                if segment_hits_rect(s, t, p) {
                    cost += THROUGH_ROOM;
                }
            }
        }
        cost + COMPACT * ((c.0 - centroid.0).abs() + (c.1 - centroid.1).abs())
    }

    /// Candidate spots for room `i` (size `w`x`h`) around its placed neighbours.
    fn candidates(&self, st: &State, i: usize, (w, h): (i32, i32)) -> Vec<Rect> {
        let mut out: Vec<Rect> = Vec::new();
        let mut push = |r: Rect| if !out.contains(&r) { out.push(r) };
        let placed_nbrs: Vec<(usize, Rect)> = self.adj[i].iter()
            .filter_map(|&li| st.pos[self.other(li, i)].map(|p| (li, p)))
            .collect();
        for &(li, p) in &placed_nbrs {
            let lw = self.links[li].width;
            let base = if self.links[li].flush { 0 } else { self.gap + lw };
            let dists: &[i32] = if self.links[li].flush { &[0] } else { &[0, 2, 5, 9] };
            for &extra in dists {
                let d = base + extra;
                // Slide along each side, keeping enough overlap for a straight corridor
                let need = if self.links[li].flush { 1 } else { lw };
                let ys = spread_offsets(p.y - h + need, p.y + p.h - need, p.y, p.y + (p.h - h) / 2);
                let xs = spread_offsets(p.x - w + need, p.x + p.w - need, p.x, p.x + (p.w - w) / 2);
                for &y in &ys {
                    push(Rect { x: p.x + p.w + d, y, w, h });
                    push(Rect { x: p.x - w - d, y, w, h });
                }
                for &x in &xs {
                    push(Rect { x, y: p.y + p.h + d, w, h });
                    push(Rect { x, y: p.y - h - d, w, h });
                }
            }
        }
        // Between several placed neighbours: a lattice around their centroid
        if placed_nbrs.len() >= 2 {
            let (sx, sy) = placed_nbrs.iter().fold((0.0, 0.0), |a, (_, p)| (a.0 + p.center().0, a.1 + p.center().1));
            let n = placed_nbrs.len() as f32;
            let (cx, cy) = ((sx / n) as i32 - w / 2, (sy / n) as i32 - h / 2);
            for dy in (-14..=14).step_by(2) {
                for dx in (-14..=14).step_by(2) {
                    push(Rect { x: cx + dx, y: cy + dy, w, h });
                }
            }
        }
        out
    }

    /// Run the search; returns the order rooms were placed in and the best few complete
    /// layouts, best first.
    pub fn solve(&self) -> (Vec<usize>, Vec<Vec<Rect>>) {
        let order = self.order();
        let n = self.ids.len();
        let mut beam = vec![State { pos: vec![None; n], score: 0.0 }];
        for &i in &order {
            let mut next: Vec<State> = Vec::new();
            for st in &beam {
                let orientations: Vec<(i32, i32)> = if self.can_rotate[i] && self.sizes[i].0 != self.sizes[i].1 {
                    vec![self.sizes[i], (self.sizes[i].1, self.sizes[i].0)]
                } else {
                    vec![self.sizes[i]]
                };
                let placed: Vec<Rect> = st.pos.iter().flatten().copied().collect();
                let centroid = if placed.is_empty() { (0.0, 0.0) } else {
                    let s = placed.iter().fold((0.0, 0.0), |a, p| (a.0 + p.center().0, a.1 + p.center().1));
                    (s.0 / placed.len() as f32, s.1 / placed.len() as f32)
                };
                let mut scored: Vec<(f32, Rect)> = Vec::new();
                for &size in &orientations {
                    for r in self.candidates(st, i, size) {
                        if self.valid(st, i, &r) {
                            scored.push((self.cost(st, i, &r, centroid), r));
                        }
                    }
                }
                if scored.is_empty() {
                    // First room of a component (or nowhere near its neighbours fits):
                    // beside everything placed so far
                    let r = self.free_spot(st, i, orientations[0]);
                    scored.push((if self.adj[i].iter().any(|&li| st.pos[self.other(li, i)].is_some()) { 500.0 } else { 0.0 }, r));
                }
                scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                for &(c, r) in scored.iter().take(PER_STATE) {
                    let mut s2 = st.clone();
                    s2.pos[i] = Some(r);
                    s2.score += c;
                    next.push(s2);
                }
            }
            next.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
            // Drop layouts identical to a better one
            let mut kept: Vec<State> = Vec::new();
            for s in next {
                if !kept.iter().any(|k| k.pos == s.pos) {
                    kept.push(s);
                }
                if kept.len() == BEAM {
                    break;
                }
            }
            beam = kept;
        }
        let layouts = beam.into_iter().map(|s| s.pos.into_iter().map(|p| p.unwrap()).collect()).collect();
        (order, layouts)
    }

    /// A clear spot for room `i` to the right of everything placed (scanning down).
    fn free_spot(&self, st: &State, i: usize, (w, h): (i32, i32)) -> Rect {
        let placed: Vec<Rect> = st.pos.iter().flatten().copied().collect();
        if placed.is_empty() {
            return Rect { x: 0, y: 0, w, h };
        }
        let max_x = placed.iter().map(|p| p.x + p.w).max().unwrap();
        let min_y = placed.iter().map(|p| p.y).min().unwrap();
        let x = max_x + self.gap + 4;
        let mut y = min_y;
        loop {
            let r = Rect { x, y, w, h };
            if self.valid(st, i, &r) || y > min_y + 400 {
                return r;
            }
            y += 1;
        }
    }

    pub fn room_id(&self, i: usize) -> &str {
        &self.ids[i]
    }

    pub fn floor(&self, i: usize) -> FloorAssignment {
        self.floors[i]
    }
}

/// Whether `other` lies past wall `w` of `r`.
fn beyond(r: &Rect, other: &Rect, w: Wall) -> bool {
    match w {
        Wall::Left => other.x + other.w <= r.x,
        Wall::Right => other.x >= r.x + r.w,
        Wall::Top => other.y + other.h <= r.y,
        Wall::Bottom => other.y >= r.y + r.h,
    }
}

/// Up to seven offsets in `lo..=hi`: both ends, the two preferred alignments, and
/// evenly between.
fn spread_offsets(lo: i32, hi: i32, aligned: i32, centered: i32) -> Vec<i32> {
    if hi < lo {
        return vec![centered];
    }
    let mut v = vec![aligned.clamp(lo, hi), centered.clamp(lo, hi), lo, hi];
    for k in 1..4 {
        v.push(lo + (hi - lo) * k / 4);
    }
    v.sort();
    v.dedup();
    v
}
