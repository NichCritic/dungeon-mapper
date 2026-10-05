use crate::util::CellSet;
use crate::model::{ShadingStyle, SpatialLayout};
use crate::render::traits::MapRenderer;
use crate::util::GRID_PX;

/// Dense membership grid over a cell set's bounding box: the shading tests cells
/// millions of times, and an array index is much cheaper than a hash lookup.
struct CellGrid {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    bits: Vec<bool>,
}

impl CellGrid {
    fn new(cells: &CellSet) -> Self {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &(x, y) in cells {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let (w, h) = if cells.is_empty() { (0, 0) } else { (x1 - x0 + 1, y1 - y0 + 1) };
        let mut bits = vec![false; (w * h) as usize];
        for &(x, y) in cells {
            bits[((y - y0) * w + (x - x0)) as usize] = true;
        }
        CellGrid { x0, y0, w, h, bits }
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        let (lx, ly) = (x - self.x0, y - self.y0);
        lx >= 0 && ly >= 0 && lx < self.w && ly < self.h && self.bits[(ly * self.w + lx) as usize]
    }
}

/// Parameters controlling exterior shading appearance.
pub struct ShadingParams {
    pub radius: f32,
    pub style: ShadingStyle,
    pub density: f32,
    pub color: [u8; 4],
}

pub fn draw_exterior_shading(
    renderer: &mut dyn MapRenderer,
    layout: &SpatialLayout,
    floor: &CellSet,
    params: &ShadingParams,
    contour_segments: &[(f32, f32, f32, f32)],
) {
    if floor.is_empty() || params.radius <= 0.0 {
        return;
    }

    let radius_px = params.radius * GRID_PX;

    // Find boundary cells (floor cells with at least one non-floor neighbor)
    let mut boundary_cells: CellSet = CellSet::default();
    for &(fx, fy) in floor {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if !floor.contains(&(fx + dx, fy + dy)) {
                    boundary_cells.insert((fx, fy));
                }
            }
        }
    }

    let walls = WallDistance::new(&boundary_cells, contour_segments, radius_px);
    match params.style {
        ShadingStyle::Hatched => {
            draw_dyson_hatching(renderer, floor, &boundary_cells, radius_px, params.density, params.color, &walls);
        }
        ShadingStyle::Solid => {
            let extents = layout.extents();
            let search_r = params.radius.ceil() as i32 + 1;
            let search_extents = (
                extents.0 - search_r,
                extents.1 - search_r,
                extents.2 + search_r,
                extents.3 + search_r,
            );
            draw_solid_shading(renderer, floor, search_extents, radius_px, params.color, &walls);
        }
        ShadingStyle::Stippled => {
            let extents = layout.extents();
            let search_r = params.radius.ceil() as i32 + 1;
            let search_extents = (
                extents.0 - search_r,
                extents.1 - search_r,
                extents.2 + search_r,
                extents.3 + search_r,
            );
            draw_stippled_shading(renderer, floor, search_extents, radius_px, params.density, params.color, &walls);
        }
    }
}

/// Uniform spatial hash over the hatch seeds.
///
/// [`Self::nearest`] returns exactly what a linear scan over `seeds` returns: the
/// seed with the smallest squared distance, ties broken by lowest index. That
/// equivalence is load-bearing — the hatching is a Voronoi partition keyed on the
/// winning seed's index, so a different tie-break would move lines on the page.
///
/// Cells are visited in expanding square rings around the query. After every cell
/// within Chebyshev ring `r` has been visited, any unvisited seed is further than
/// `r * cell` away, so the search can stop as soon as the best match is within that
/// bound. The comparisons below are shaded by `BOUND_MARGIN` in the conservative
/// direction so f32 rounding at a ring boundary can only cost an extra ring, never
/// a different answer.
struct SeedGrid {
    cell: f32,
    /// Bucket `(cx, cy)` holds `members[starts[i]..starts[i + 1]]`, ascending, with
    /// `i` row-major from `(min_cx, min_cy)`
    starts: Vec<u32>,
    members: Vec<u32>,
    min_cx: i32,
    max_cx: i32,
    min_cy: i32,
    max_cy: i32,
}

/// Shrinks/expands the ring bound so a rounding error can't end the search early.
const BOUND_MARGIN: f32 = 0.999;

impl SeedGrid {
    fn build(seeds: &[(f32, f32, f32)], cell: f32) -> Self {
        let cell = cell.max(1.0);
        let key = |x: f32, y: f32| ((x / cell).floor() as i32, (y / cell).floor() as i32);
        let (mut min_cx, mut max_cx) = (i32::MAX, i32::MIN);
        let (mut min_cy, mut max_cy) = (i32::MAX, i32::MIN);
        for &(x, y, _) in seeds {
            let (cx, cy) = key(x, y);
            (min_cx, max_cx, min_cy, max_cy) = (min_cx.min(cx), max_cx.max(cx), min_cy.min(cy), max_cy.max(cy));
        }
        let width = if seeds.is_empty() { 0 } else { (max_cx - min_cx + 1) as usize };
        let height = if seeds.is_empty() { 0 } else { (max_cy - min_cy + 1) as usize };
        let index = |(cx, cy): (i32, i32)| (cy - min_cy) as usize * width + (cx - min_cx) as usize;
        let mut starts = vec![0u32; width * height + 1];
        for &(x, y, _) in seeds {
            starts[index(key(x, y)) + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        // Filled in seed order, so each bucket lists its seeds by ascending index
        let mut fill = starts.clone();
        let mut members = vec![0u32; seeds.len()];
        for (i, &(x, y, _)) in seeds.iter().enumerate() {
            let slot = &mut fill[index(key(x, y))];
            members[*slot as usize] = i as u32;
            *slot += 1;
        }
        Self { cell, starts, members, min_cx, max_cx, min_cy, max_cy }
    }

    /// The seeds in bucket `(cx, cy)`.
    fn bucket(&self, cx: i32, cy: i32) -> &[u32] {
        if cx < self.min_cx || cx > self.max_cx || cy < self.min_cy || cy > self.max_cy {
            return &[];
        }
        let width = (self.max_cx - self.min_cx + 1) as usize;
        let i = (cy - self.min_cy) as usize * width + (cx - self.min_cx) as usize;
        &self.members[self.starts[i] as usize..self.starts[i + 1] as usize]
    }

    /// Nearest seed to `(px, py)`, as `(index, squared distance)`.
    ///
    /// `skip` excludes one index (used for "nearest *other* seed"). `limit` is a
    /// distance past which the caller doesn't care about the answer; the search may
    /// stop once nothing nearer than `limit` can remain, in which case the returned
    /// seed is only guaranteed correct when its distance is below `limit`.
    fn nearest(
        &self,
        seeds: &[(f32, f32, f32)],
        px: f32,
        py: f32,
        skip: Option<usize>,
        limit: f32,
    ) -> Option<(usize, f32)> {
        if self.members.is_empty() {
            return None;
        }
        let qcx = (px / self.cell).floor() as i32;
        let qcy = (py / self.cell).floor() as i32;

        // Skip straight to the first ring that can reach an occupied cell.
        let r_start = [
            self.min_cx - qcx,
            qcx - self.max_cx,
            self.min_cy - qcy,
            qcy - self.max_cy,
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
        .max(0);

        let mut best: Option<(usize, f32)> = None;
        let mut r = r_start;
        loop {
            let exhausted = qcx - r <= self.min_cx
                && qcx + r >= self.max_cx
                && qcy - r <= self.min_cy
                && qcy + r >= self.max_cy;

            for cy in (qcy - r)..=(qcy + r) {
                for cx in (qcx - r)..=(qcx + r) {
                    // Ring only — the interior was covered by earlier iterations.
                    if r > 0 && (cx - qcx).abs() != r && (cy - qcy).abs() != r {
                        continue;
                    }
                    for &si in self.bucket(cx, cy) {
                        let i = si as usize;
                        if Some(i) == skip {
                            continue;
                        }
                        let (sx, sy, _) = seeds[i];
                        let d = (px - sx).powi(2) + (py - sy).powi(2);
                        // Lowest index wins a tie, matching the linear scan.
                        let better = match best {
                            None => true,
                            Some((bi, bd)) => d < bd || (d == bd && i < bi),
                        };
                        if better {
                            best = Some((i, d));
                        }
                    }
                }
            }

            let reach = r as f32 * self.cell;
            if best.is_some_and(|(_, bd)| bd.sqrt() <= reach * BOUND_MARGIN) {
                break;
            }
            if reach * BOUND_MARGIN >= limit {
                break;
            }
            if exhausted {
                break;
            }
            r += 1;
        }
        best
    }
}

/// Simple deterministic hash for pseudo-random values from coordinates.
fn hash_pos(x: f32, y: f32, salt: u32) -> u32 {
    let ix = (x * 100.0) as i32;
    let iy = (y * 100.0) as i32;
    let mut h = (ix as u32).wrapping_mul(2654435761);
    h ^= (iy as u32).wrapping_mul(2246822519);
    h ^= salt.wrapping_mul(3266489917);
    h ^= h >> 16;
    h = h.wrapping_mul(2246822519);
    h ^= h >> 13;
    h
}

fn hash_f32(x: f32, y: f32, salt: u32) -> f32 {
    (hash_pos(x, y, salt) & 0xFFFF) as f32 / 65535.0
}

/// Dyson-style hatching: randomly scattered seeds in the exterior zone,
/// denser near walls, each with a random angle. Parallel lines fill each
/// Voronoi cell, extending in both directions from the seed.
fn draw_dyson_hatching(
    renderer: &mut dyn MapRenderer,
    floor: &CellSet,
    boundary_cells: &CellSet,
    radius_px: f32,
    density: f32,
    color: [u8; 4],
    walls: &WallDistance,
) {
    let base_spacing = (6.0 / density).max(2.0);

    let mut seeds: Vec<(f32, f32, f32)> = Vec::new(); // (x, y, angle)

    let search_r = (radius_px / GRID_PX).ceil() as i32 + 1;
    let mut exterior_cells: Vec<(i32, i32)> = Vec::new();
    for &(bx, by) in boundary_cells {
        for dy in -search_r..=search_r {
            for dx in -search_r..=search_r {
                let gx = bx + dx;
                let gy = by + dy;
                if !floor.contains(&(gx, gy)) {
                    exterior_cells.push((gx, gy));
                }
            }
        }
    }
    exterior_cells.sort();
    exterior_cells.dedup();

    for &(gx, gy) in &exterior_cells {
        let wx = gx as f32 * GRID_PX;
        let wy = gy as f32 * GRID_PX;

        let d = walls.dist(wx + GRID_PX / 2.0, wy + GRID_PX / 2.0);
        if d > radius_px {
            continue;
        }

        let dist_factor = (d / radius_px).max(0.1);
        let local_spacing = base_spacing * (0.5 + dist_factor * 1.5);

        let mut sy = wy + local_spacing * hash_f32(wx, wy, 1) * 0.5;
        while sy < wy + GRID_PX {
            let mut sx = wx + local_spacing * hash_f32(wx, sy, 2) * 0.5;
            while sx < wx + GRID_PX {
                let jx = sx + (hash_f32(sx, sy, 3) - 0.5) * local_spacing * 0.6;
                let jy = sy + (hash_f32(sx, sy, 4) - 0.5) * local_spacing * 0.6;

                let jgx = (jx / GRID_PX).floor() as i32;
                let jgy = (jy / GRID_PX).floor() as i32;
                if !floor.contains(&(jgx, jgy)) {
                    let jd = walls.dist(jx, jy);
                    if jd <= radius_px {
                        let angle = hash_f32(jx, jy, 7) * std::f32::consts::PI;
                        seeds.push((jx, jy, angle));
                    }
                }
                sx += local_spacing;
            }
            sy += local_spacing;
        }
    }

    if seeds.is_empty() {
        return;
    }

    // Both the Voronoi lookup below and the neighbour-spacing pass are nearest-seed
    // queries; without an index they scan all seeds and the whole routine goes
    // quadratic in the map's boundary length (billions of distance tests on a large
    // map). The grid answers them identically, in a couple of cells each.
    let grid = SeedGrid::build(&seeds, base_spacing);
    let floor_grid = CellGrid::new(floor);

    // Each seed's lines depend only on shared read-only data, so seeds are split into
    // contiguous chunks across threads; the chunks' lines are emitted in seed order,
    // exactly as a single pass would.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1)).clamp(1, 8);
    let chunk = seeds.len().div_ceil(threads);
    let lines: Vec<Vec<(f32, f32, f32, f32)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..seeds.len()).step_by(chunk.max(1))
            .map(|start| {
                let (seeds, grid, floor_grid) = (&seeds, &grid, &floor_grid);
                scope.spawn(move || {
                    let end = (start + chunk).min(seeds.len());
                    let mut out = Vec::new();
                    for seed_idx in start..end {
                        hatch_seed_lines(seeds, seed_idx, grid, floor_grid, walls, radius_px, base_spacing, density, &mut out);
                    }
                    out
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().expect("hatching thread")).collect()
    });
    let line_weight = 0.8;
    for (x1, y1, x2, y2) in lines.into_iter().flatten() {
        renderer.draw_line(x1, y1, x2, y2, line_weight, color);
    }
}

/// The hatch lines of one seed's Voronoi cell: parallel lines at the seed's angle,
/// each running out from the seed until it reaches floor, leaves the shading radius or
/// enters another seed's cell.
#[allow(clippy::too_many_arguments)]
fn hatch_seed_lines(
    seeds: &[(f32, f32, f32)],
    seed_idx: usize,
    grid: &SeedGrid,
    floor: &CellGrid,
    walls: &WallDistance,
    radius_px: f32,
    base_spacing: f32,
    density: f32,
    out: &mut Vec<(f32, f32, f32, f32)>,
) {
    let nearest_seed = |px: f32, py: f32| -> usize {
        grid.nearest(seeds, px, py, None, f32::INFINITY)
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let on_floor = |px: f32, py: f32| floor.contains((px / GRID_PX).floor() as i32, (py / GRID_PX).floor() as i32);

    let line_spacing = (2.5 / density).max(1.0);
    let step = 1.5;
    let (sx, sy, angle) = seeds[seed_idx];
    let line_dx = angle.cos();
    let line_dy = angle.sin();
    let perp_dx = -line_dy;
    let perp_dy = line_dx;

    // Nearest other seed, capped at radius_px * 2.0. Taking the square root of
    // the smallest squared distance gives the same f32 as the smallest of the
    // individual square roots, so the cap comparison is unchanged.
    let mut min_neighbor_dist = radius_px * 2.0;
    if let Some((_, d_sq)) = grid.nearest(seeds, sx, sy, Some(seed_idx), min_neighbor_dist) {
        let d = d_sq.sqrt();
        if d < min_neighbor_dist {
            min_neighbor_dist = d;
        }
    }
    let cell_half_width = (min_neighbor_dist / 2.0).min(base_spacing);
    let num_lines = (cell_half_width * 2.0 / line_spacing).ceil() as i32;

    for i in -num_lines / 2..=num_lines / 2 {
        let offset = i as f32 * line_spacing;
        let lx = sx + perp_dx * offset;
        let ly = sy + perp_dy * offset;

        if nearest_seed(lx, ly) != seed_idx {
            continue;
        }

        let mut neg_t = 0.0_f32;
        let mut pos_t = 0.0_f32;

        let mut t = step;
        loop {
            let px = lx + line_dx * t;
            let py = ly + line_dy * t;
            if on_floor(px, py) { break; }
            if walls.dist(px, py) > radius_px { break; }
            if nearest_seed(px, py) != seed_idx { break; }
            pos_t = t;
            t += step;
            if t > radius_px * 2.0 { break; }
        }

        t = -step;
        loop {
            let px = lx + line_dx * t;
            let py = ly + line_dy * t;
            if on_floor(px, py) { break; }
            if walls.dist(px, py) > radius_px { break; }
            if nearest_seed(px, py) != seed_idx { break; }
            neg_t = t;
            t -= step;
            if t < -radius_px * 2.0 { break; }
        }

        if pos_t - neg_t < step {
            continue;
        }

        out.push((lx + line_dx * neg_t, ly + line_dy * neg_t, lx + line_dx * pos_t, ly + line_dy * pos_t));
    }
}


/// Distance from a point to the nearest wall: the edges of boundary floor cells
/// within two cells, and the marching-squares contour segments (smooth cave walls).
///
/// Contour segments are bucketed by the grid cells their bounding boxes touch, so a
/// query only looks at those near it instead of every segment on the map (big caves
/// have tens of thousands, and the hatching makes millions of queries). The answer is
/// exact whenever it is within `reach_px`; beyond that it may come out larger than the
/// true distance, but still beyond `reach_px`, so a comparison against that radius
/// gives the same result either way.
pub(crate) struct WallDistance<'a> {
    boundary_cells: CellGrid,
    segments: &'a [(f32, f32, f32, f32)],
    /// Bucket grid over the segments' cells: `starts[i]..starts[i + 1]` indexes
    /// `members` for bucket `i` (row-major from `origin`, `width` buckets wide)
    origin: (i32, i32),
    width: i32,
    height: i32,
    starts: Vec<u32>,
    members: Vec<u32>,
    /// How many buckets out from the query's cell to look
    reach: i32,
}

impl<'a> WallDistance<'a> {
    pub(crate) fn new(boundary_cells: &'a CellSet, segments: &'a [(f32, f32, f32, f32)], reach_px: f32) -> Self {
        let cells_of = |&(x1, y1, x2, y2): &(f32, f32, f32, f32)| {
            let c = |v: f32| (v / GRID_PX).floor() as i32;
            (c(x1.min(x2)), c(y1.min(y2)), c(x1.max(x2)), c(y1.max(y2)))
        };
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for seg in segments {
            let (a, b, c, d) = cells_of(seg);
            (x0, y0, x1, y1) = (x0.min(a), y0.min(b), x1.max(c), y1.max(d));
        }
        let (width, height) = if segments.is_empty() { (0, 0) } else { (x1 - x0 + 1, y1 - y0 + 1) };
        let index = |x: i32, y: i32| ((y - y0) * width + (x - x0)) as usize;
        let mut counts = vec![0u32; (width * height) as usize + 1];
        for seg in segments {
            let (a, b, c, d) = cells_of(seg);
            for y in b..=d {
                for x in a..=c {
                    counts[index(x, y) + 1] += 1;
                }
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let starts = counts;
        let mut fill = starts.clone();
        let mut members = vec![0u32; *starts.last().unwrap_or(&0) as usize];
        for (si, seg) in segments.iter().enumerate() {
            let (a, b, c, d) = cells_of(seg);
            for y in b..=d {
                for x in a..=c {
                    let slot = &mut fill[index(x, y)];
                    members[*slot as usize] = si as u32;
                    *slot += 1;
                }
            }
        }
        let reach = (reach_px / GRID_PX).ceil() as i32 + 1;
        WallDistance { boundary_cells: CellGrid::new(boundary_cells), segments, origin: (x0, y0), width, height, starts, members, reach }
    }

    pub(crate) fn dist(&self, wx: f32, wy: f32) -> f32 {
        let gx = (wx / GRID_PX).floor() as i32;
        let gy = (wy / GRID_PX).floor() as i32;
        let mut min_dist_sq = f32::MAX;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let cx = gx + dx;
                let cy = gy + dy;
                if !self.boundary_cells.contains(cx, cy) { continue; }
                let cell_x1 = cx as f32 * GRID_PX;
                let cell_y1 = cy as f32 * GRID_PX;
                let nearest_x = wx.clamp(cell_x1, cell_x1 + GRID_PX);
                let nearest_y = wy.clamp(cell_y1, cell_y1 + GRID_PX);
                let d = (wx - nearest_x).powi(2) + (wy - nearest_y).powi(2);
                min_dist_sq = min_dist_sq.min(d);
            }
        }

        if self.segments.is_empty() {
            return min_dist_sq.sqrt();
        }
        // Contour segments in the buckets around the point
        let (ox, oy) = self.origin;
        let bx0 = (gx - self.reach - ox).max(0);
        let by0 = (gy - self.reach - oy).max(0);
        let bx1 = (gx + self.reach - ox).min(self.width - 1);
        let by1 = (gy + self.reach - oy).min(self.height - 1);
        for by in by0..=by1 {
            for bx in bx0..=bx1 {
                let i = (by * self.width + bx) as usize;
                for &si in &self.members[self.starts[i] as usize..self.starts[i + 1] as usize] {
                    let (x1, y1, x2, y2) = self.segments[si as usize];
                    min_dist_sq = min_dist_sq.min(point_to_segment_dist_sq(wx, wy, x1, y1, x2, y2));
                }
            }
        }

        min_dist_sq.sqrt()
    }
}

/// Squared distance from point (px, py) to line segment (x1,y1)-(x2,y2).
fn point_to_segment_dist_sq(px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 0.001 {
        return (px - x1).powi(2) + (py - y1).powi(2);
    }
    let t = ((px - x1) * dx + (py - y1) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let proj_x = x1 + t * dx;
    let proj_y = y1 + t * dy;
    (px - proj_x).powi(2) + (py - proj_y).powi(2)
}

fn draw_solid_shading(
    renderer: &mut dyn MapRenderer,
    floor: &CellSet,
    extents: (i32, i32, i32, i32),
    radius_px: f32,
    color: [u8; 4],
    walls: &WallDistance,
) {
    let mut shade_color = color;
    shade_color[3] = (color[3] as f32 * 0.3) as u8;

    let (min_x, min_y, max_x, max_y) = extents;
    for gy in min_y..max_y {
        for gx in min_x..max_x {
            if floor.contains(&(gx, gy)) { continue; }
            let wx = gx as f32 * GRID_PX + GRID_PX / 2.0;
            let wy = gy as f32 * GRID_PX + GRID_PX / 2.0;
            let d = walls.dist(wx, wy);
            if d < radius_px {
                let alpha = 1.0 - (d / radius_px);
                let mut c = shade_color;
                c[3] = (c[3] as f32 * alpha) as u8;
                renderer.fill_rect(gx as f32 * GRID_PX, gy as f32 * GRID_PX, GRID_PX, GRID_PX, c);
            }
        }
    }
}

fn draw_stippled_shading(
    renderer: &mut dyn MapRenderer,
    floor: &CellSet,
    extents: (i32, i32, i32, i32),
    radius_px: f32,
    density: f32,
    color: [u8; 4],
    walls: &WallDistance,
) {
    let dot_interval = (4.0 / density).max(1.5);

    let (min_x, min_y, max_x, max_y) = extents;
    for gy in min_y..max_y {
        for gx in min_x..max_x {
            if floor.contains(&(gx, gy)) { continue; }
            let wx = gx as f32 * GRID_PX;
            let wy = gy as f32 * GRID_PX;
            let d = walls.dist(wx + GRID_PX / 2.0, wy + GRID_PX / 2.0);
            if d >= radius_px { continue; }

            let mut dy = 1.0;
            while dy < GRID_PX {
                let row_offset = if ((dy / dot_interval) as i32) % 2 == 0 { 0.0 } else { dot_interval / 2.0 };
                let mut dx = 1.0 + row_offset;
                while dx < GRID_PX {
                    let px = wx + dx;
                    let py = wy + dy;
                    let pd = walls.dist(px, py);
                    if pd < radius_px {
                        let pgx = (px / GRID_PX).floor() as i32;
                        let pgy = (py / GRID_PX).floor() as i32;
                        if !floor.contains(&(pgx, pgy)) {
                            let alpha = 1.0 - (pd / radius_px);
                            let dot_size = 0.5 + alpha;
                            renderer.fill_rect(px - dot_size / 2.0, py - dot_size / 2.0, dot_size, dot_size, color);
                        }
                    }
                    dx += dot_interval;
                }
                dy += dot_interval;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random seed cloud, roughly matching the clustering of
    /// real hatch seeds (a band rather than a uniform fill).
    fn make_seeds(n: usize, salt: u32) -> Vec<(f32, f32, f32)> {
        (0..n)
            .map(|i| {
                let a = hash_f32(i as f32, 1.0, salt);
                let b = hash_f32(2.0, i as f32, salt);
                let c = hash_f32(i as f32, i as f32, salt);
                // Deliberately coarse so exact ties and near-ties actually occur.
                let x = (a * 400.0 * 4.0).round() / 4.0;
                let y = (b * 400.0 * 4.0).round() / 4.0;
                (x, y, c * std::f32::consts::PI)
            })
            .collect()
    }

    /// Wall distance by checking every contour segment: what `WallDistance` replaced.
    fn scan_wall_distance(wx: f32, wy: f32, boundary: &CellSet, segments: &[(f32, f32, f32, f32)]) -> f32 {
        let gx = (wx / GRID_PX).floor() as i32;
        let gy = (wy / GRID_PX).floor() as i32;
        let mut min_dist_sq = f32::MAX;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (cx, cy) = (gx + dx, gy + dy);
                if !boundary.contains(&(cx, cy)) { continue; }
                let (x1, y1) = (cx as f32 * GRID_PX, cy as f32 * GRID_PX);
                let (nx, ny) = (wx.clamp(x1, x1 + GRID_PX), wy.clamp(y1, y1 + GRID_PX));
                min_dist_sq = min_dist_sq.min((wx - nx).powi(2) + (wy - ny).powi(2));
            }
        }
        for &(x1, y1, x2, y2) in segments {
            min_dist_sq = min_dist_sq.min(point_to_segment_dist_sq(wx, wy, x1, y1, x2, y2));
        }
        min_dist_sq.sqrt()
    }

    #[test]
    fn wall_distance_matches_scanning_every_segment_within_reach() {
        // A ragged ring of short segments (like a cave contour) plus some boundary cells
        let g = GRID_PX;
        let segments: Vec<(f32, f32, f32, f32)> = (0..400).map(|i| {
            let a = i as f32 / 400.0 * std::f32::consts::TAU;
            let b = (i + 1) as f32 / 400.0 * std::f32::consts::TAU;
            let r = |t: f32| 30.0 * g + hash_f32(t, 0.0, 9) * g;
            (a.cos() * r(a), a.sin() * r(a), b.cos() * r(b), b.sin() * r(b))
        }).collect();
        let boundary: CellSet = (-5..5).map(|x| (x, 0)).collect();
        let reach = 2.5 * g;
        let walls = WallDistance::new(&boundary, &segments, reach);
        for i in 0..20_000 {
            let x = (hash_f32(i as f32, 1.0, 3) - 0.5) * 80.0 * g;
            let y = (hash_f32(1.0, i as f32, 4) - 0.5) * 80.0 * g;
            let (fast, slow) = (walls.dist(x, y), scan_wall_distance(x, y, &boundary, &segments));
            if slow <= reach {
                assert_eq!(fast, slow, "at {:?}", (x, y));
            } else {
                assert!(fast > reach, "at {:?}: {} vs {}", (x, y), fast, slow);
            }
        }
    }

    fn scan_nearest(seeds: &[(f32, f32, f32)], px: f32, py: f32) -> usize {
        // Byte-for-byte the loop SeedGrid replaced.
        let mut best = 0;
        let mut best_d = f32::MAX;
        for (i, &(sx, sy, _)) in seeds.iter().enumerate() {
            let d = (px - sx).powi(2) + (py - sy).powi(2);
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best
    }

    fn scan_min_neighbor(seeds: &[(f32, f32, f32)], idx: usize, cap: f32) -> f32 {
        let (sx, sy, _) = seeds[idx];
        let mut min_neighbor_dist = cap;
        for (j, &(ox, oy, _)) in seeds.iter().enumerate() {
            if j == idx {
                continue;
            }
            let d = ((ox - sx).powi(2) + (oy - sy).powi(2)).sqrt();
            if d < min_neighbor_dist {
                min_neighbor_dist = d;
            }
        }
        min_neighbor_dist
    }

    #[test]
    fn seed_grid_nearest_matches_linear_scan() {
        for &cell in &[2.0f32, 6.0, 12.0] {
            for &n in &[1usize, 2, 37, 500] {
                let seeds = make_seeds(n, 11);
                let grid = SeedGrid::build(&seeds, cell);
                for q in 0..600 {
                    // Query inside, on, and well outside the seed cloud.
                    let px = hash_f32(q as f32, 7.0, 3) * 520.0 - 60.0;
                    let py = hash_f32(9.0, q as f32, 3) * 520.0 - 60.0;
                    let got = grid
                        .nearest(&seeds, px, py, None, f32::INFINITY)
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    assert_eq!(
                        got,
                        scan_nearest(&seeds, px, py),
                        "cell={cell} n={n} q=({px},{py})"
                    );
                }
            }
        }
    }

    #[test]
    fn seed_grid_nearest_ties_pick_lowest_index() {
        // Two seeds exactly equidistant from the query: the scan keeps the first.
        let seeds = vec![(10.0, 0.0, 0.0), (-10.0, 0.0, 0.0)];
        let grid = SeedGrid::build(&seeds, 6.0);
        let got = grid.nearest(&seeds, 0.0, 0.0, None, f32::INFINITY).unwrap().0;
        assert_eq!(got, 0);
        assert_eq!(got, scan_nearest(&seeds, 0.0, 0.0));
    }

    #[test]
    fn seed_grid_min_neighbor_matches_linear_scan() {
        for &cap in &[20.0f32, 60.0, 4000.0] {
            let seeds = make_seeds(300, 23);
            let grid = SeedGrid::build(&seeds, 6.0);
            for idx in 0..seeds.len() {
                let (sx, sy, _) = seeds[idx];
                let mut got = cap;
                if let Some((_, d_sq)) = grid.nearest(&seeds, sx, sy, Some(idx), cap) {
                    let d = d_sq.sqrt();
                    if d < got {
                        got = d;
                    }
                }
                assert_eq!(got, scan_min_neighbor(&seeds, idx, cap), "cap={cap} idx={idx}");
            }
        }
    }

    #[test]
    fn seed_grid_single_seed_has_no_neighbor() {
        let seeds = vec![(1.0, 2.0, 0.0)];
        let grid = SeedGrid::build(&seeds, 6.0);
        assert!(grid.nearest(&seeds, 1.0, 2.0, Some(0), 50.0).is_none());
    }
}
