//! Drawing hex maps in the same ink-on-paper style as the dungeon maps: each hex's
//! terrain as scattered marks, rivers and roads through the hexes, a landmark in the
//! middle, and the grid over it all. Colour only for plants and water.

use std::collections::BTreeSet;

use crate::model::{HexCoord, HexMap, HexPathKind, HexTerrain, Theme, HEX_SIZE};
use crate::render::decor::{draw_decor, draw_hex_glyph, DecorPalette, GlyphSink, HexGlyph, MapRendererSink};
use crate::render::traits::MapRenderer;

/// How far into its hex a terrain mark may sit, as a fraction of the hex size.
const SLOT_RING: f32 = 0.5;
/// Terrain marks keep this far (fraction of the hex size) from rivers and roads.
const PATH_CLEARANCE: f32 = 0.24;

/// The hexes a viewer sees: every hex for the DM, only revealed ones for the players.
pub fn visible_hexes(map: &HexMap, players: bool) -> BTreeSet<HexCoord> {
    map.hexes.iter().map(|h| h.coord).filter(|c| !players || map.is_revealed(*c)).collect()
}

/// Draw a hex map. With `players` set, only the hexes the players have seen are drawn.
pub fn render_hex_map(r: &mut dyn MapRenderer, map: &HexMap, theme: &Theme, players: bool) {
    let visible = visible_hexes(map, players);
    let mut pal = DecorPalette::from_ink(theme.wall_color);
    pal.paper = theme.floor_color;
    let o = map.orientation;

    // Paper, with a wash for water
    for hex in map.hexes.iter().filter(|h| visible.contains(&h.coord)) {
        // A hair larger so neighbours meet without a seam
        let corners = o.corners(hex.coord, 1.01);
        r.fill_polygon(&corners, theme.floor_color);
        let wash = match hex.terrain {
            HexTerrain::Water => Some([pal.water[0], pal.water[1], pal.water[2], 90]),
            HexTerrain::Marsh => Some([pal.water[0], pal.water[1], pal.water[2], 28]),
            _ => None,
        };
        if let Some(wash) = wash {
            r.fill_polygon(&corners, wash);
        }
    }

    let paths = path_lines(map, &visible);

    // Terrain marks, kept off the rivers and roads
    {
        let mut sink = MapRendererSink { renderer: &mut *r };
        for hex in map.hexes.iter().filter(|h| visible.contains(&h.coord)) {
            let (cx, cy) = o.center(hex.coord);
            let (glyph, size, count) = terrain_marks(hex.terrain);
            let slots = slots_for(hex.coord, hex.feature.is_none(), count);
            for (i, (sx, sy)) in slots.into_iter().enumerate() {
                let (x, y) = (cx + sx * HEX_SIZE, cy + sy * HEX_SIZE);
                if near_any(&paths, x, y, PATH_CLEARANCE * HEX_SIZE) {
                    continue;
                }
                // Mountains: one big peak in an empty hex, smaller ones round a landmark
                let size = if hex.terrain == HexTerrain::Mountains && hex.feature.is_none() && i == 0 { size * 1.4 } else { size };
                match glyph {
                    Mark::Glyph(g) => draw_hex_glyph(&mut sink, g, x, y, size, &pal),
                    Mark::Tree => draw_decor(&mut sink, crate::model::DecorType::Tree, x, y, size, 1.0, 1.0, 0.0, &pal),
                }
            }
        }
    }

    // Rivers under roads, roads under tracks
    for kind in [HexPathKind::River, HexPathKind::Road, HexPathKind::Track] {
        for (k, line) in &paths {
            if *k == kind {
                stroke_path(r, kind, line, &pal);
            }
        }
    }

    // Landmarks
    {
        let mut sink = MapRendererSink { renderer: &mut *r };
        for hex in map.hexes.iter().filter(|h| visible.contains(&h.coord)) {
            if let Some(feature) = hex.feature {
                let (cx, cy) = o.center(hex.coord);
                draw_hex_glyph(&mut sink, HexGlyph::Feature(feature), cx, cy, HEX_SIZE * 0.32, &pal);
            }
        }
    }

    render_grid(r, map, &visible, theme.wall_color);
}

/// The party's token: a white disc with a banner on it.
pub fn draw_party_token(sink: &mut dyn GlyphSink, cx: f32, cy: f32, radius: f32) {
    sink.fill_circle((cx, cy), radius, [255, 255, 255, 240]);
    sink.stroke_circle((cx, cy), radius, radius * 0.16, [20, 20, 20, 255]);
    let u = radius * 0.5;
    // Pole and pennant
    sink.line((cx - u * 0.55, cy + u * 1.05), (cx - u * 0.55, cy - u * 1.05), radius * 0.1, [20, 20, 20, 255]);
    sink.fill_convex(&[(cx - u * 0.5, cy - u * 1.05), (cx + u * 1.0, cy - u * 0.6), (cx - u * 0.5, cy - u * 0.15)], [190, 50, 40, 255]);
}

enum Mark {
    Glyph(HexGlyph),
    Tree,
}

/// What's scattered over a hex of this terrain: the mark, its half-size in world
/// pixels, and how many.
fn terrain_marks(terrain: HexTerrain) -> (Mark, f32, usize) {
    use HexGlyph as G;
    match terrain {
        HexTerrain::Plains => (Mark::Glyph(G::GrassTuft), 4.0, 3),
        HexTerrain::Grassland => (Mark::Glyph(G::GrassTuft), 5.0, 6),
        HexTerrain::Farmland => (Mark::Glyph(G::Furrows), 8.0, 4),
        HexTerrain::Steppe => (Mark::Glyph(G::DryTuft), 5.0, 4),
        HexTerrain::Forest => (Mark::Tree, 7.5, 7),
        HexTerrain::Hills => (Mark::Glyph(G::Hill), 9.0, 3),
        HexTerrain::Mountains => (Mark::Glyph(G::Mountain), 10.0, 3),
        HexTerrain::Marsh => (Mark::Glyph(G::Reeds), 6.5, 4),
        HexTerrain::Desert => (Mark::Glyph(G::Dune), 7.0, 3),
        HexTerrain::Snow => (Mark::Glyph(G::Drift), 6.0, 2),
        HexTerrain::Water => (Mark::Glyph(G::Waves), 6.0, 3),
    }
}

/// A stable pseudo-random number for a hex.
fn hex_hash(c: HexCoord, salt: u32) -> u32 {
    let h = (c.q.wrapping_mul(73_856_093) ^ c.r.wrapping_mul(19_349_663) ^ (salt as i32).wrapping_mul(83_492_791)) as u32;
    h.wrapping_mul(0x9E37_79B1).rotate_left(13)
}

/// Where in a hex its terrain marks go (offsets in hex sizes): the centre first if
/// `centre_free`, then round a ring, starting at a different place in each hex so
/// neighbours don't repeat, each nudged a little.
fn slots_for(c: HexCoord, centre_free: bool, count: usize) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    if centre_free {
        out.push((0.0, 0.0));
    }
    let start = hex_hash(c, 1) % 6;
    // Spread the ring slots out when only a few are used
    let step = if count <= 3 { 2 } else { 1 };
    let mut k = 0;
    while out.len() < count && k < 6 {
        let i = (start + k * step) % 6;
        let a = (30.0 + 60.0 * i as f32).to_radians();
        let h = hex_hash(c, 10 + i);
        let jx = ((h % 100) as f32 / 100.0 - 0.5) * 0.14;
        let jy = (((h >> 8) % 100) as f32 / 100.0 - 0.5) * 0.14;
        out.push((SLOT_RING * a.cos() + jx, SLOT_RING * a.sin() + jy));
        k += 1;
    }
    out
}

/// Each path's line through the visible hexes, smoothed. A path the players have only
/// seen parts of breaks into the runs they've seen.
fn path_lines(map: &HexMap, visible: &BTreeSet<HexCoord>) -> Vec<(HexPathKind, Vec<(f32, f32)>)> {
    let mut out = Vec::new();
    for path in &map.paths {
        for run in path.hexes.split(|c| !visible.contains(c)).filter(|run| run.len() >= 2) {
            let pts: Vec<(f32, f32)> = run.iter().map(|&c| map.orientation.center(c)).collect();
            out.push((path.kind, chaikin(&pts, 3)));
        }
    }
    out
}

/// Round off a polyline's corners, keeping its ends.
fn chaikin(pts: &[(f32, f32)], iterations: usize) -> Vec<(f32, f32)> {
    let mut pts = pts.to_vec();
    for _ in 0..iterations {
        if pts.len() < 3 {
            break;
        }
        let mut next = vec![pts[0]];
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            next.push((a.0 * 0.75 + b.0 * 0.25, a.1 * 0.75 + b.1 * 0.25));
            next.push((a.0 * 0.25 + b.0 * 0.75, a.1 * 0.25 + b.1 * 0.75));
        }
        next.push(*pts.last().unwrap());
        pts = next;
    }
    pts
}

fn near_any(paths: &[(HexPathKind, Vec<(f32, f32)>)], x: f32, y: f32, d: f32) -> bool {
    let p = egui::pos2(x, y);
    paths.iter().any(|(_, line)| {
        line.windows(2).any(|w| crate::util::point_to_segment_dist(p, egui::pos2(w[0].0, w[0].1), egui::pos2(w[1].0, w[1].1)) < d)
    })
}

fn stroke_path(r: &mut dyn MapRenderer, kind: HexPathKind, line: &[(f32, f32)], pal: &DecorPalette) {
    match kind {
        HexPathKind::River => {
            for w in line.windows(2) {
                r.draw_line(w[0].0, w[0].1, w[1].0, w[1].1, 4.0, pal.water_edge);
            }
            // Round joints so the bends don't crack
            for &(x, y) in &line[1..line.len() - 1] {
                r.fill_circle(x, y, 2.0, pal.water_edge);
            }
        }
        HexPathKind::Road => {
            let ink = [pal.ink[0], pal.ink[1], pal.ink[2], 210];
            for w in line.windows(2) {
                r.draw_line(w[0].0, w[0].1, w[1].0, w[1].1, 1.6, ink);
            }
        }
        HexPathKind::Track => {
            // Dashes along the whole line, carried over its bends
            let ink = [pal.ink[0], pal.ink[1], pal.ink[2], 200];
            let (dash, gap) = (4.0, 3.0);
            let mut t = 0.0f32;
            for w in line.windows(2) {
                let (a, b) = (w[0], w[1]);
                let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                if len < 1e-3 {
                    continue;
                }
                let mut s = 0.0;
                while s < len {
                    let phase = t % (dash + gap);
                    let run = if phase < dash { (dash - phase).min(len - s) } else { (dash + gap - phase).min(len - s) };
                    if phase < dash {
                        let (f0, f1) = (s / len, (s + run) / len);
                        r.draw_line(a.0 + (b.0 - a.0) * f0, a.1 + (b.1 - a.1) * f0, a.0 + (b.0 - a.0) * f1, a.1 + (b.1 - a.1) * f1, 1.1, ink);
                    }
                    s += run;
                    t += run;
                }
            }
        }
    }
}

/// The grid: faint lines between hexes, a firm line round the edge of what's shown.
fn render_grid(r: &mut dyn MapRenderer, map: &HexMap, visible: &BTreeSet<HexCoord>, ink: [u8; 4]) {
    let o = map.orientation;
    let inner = [ink[0], ink[1], ink[2], 90];
    for &c in visible {
        let corners = o.corners(c, 1.0);
        let (cx, cy) = o.center(c);
        for n in c.neighbors() {
            let shown = visible.contains(&n);
            // Shared edges once, from the lower coordinate
            if shown && n < c {
                continue;
            }
            let (nx, ny) = o.center(n);
            let (mx, my) = ((cx + nx) / 2.0, (cy + ny) / 2.0);
            let mut by_dist: Vec<(f32, f32)> = corners.to_vec();
            by_dist.sort_by(|a, b| {
                let da = (a.0 - mx).powi(2) + (a.1 - my).powi(2);
                let db = (b.0 - mx).powi(2) + (b.1 - my).powi(2);
                da.total_cmp(&db)
            });
            let (a, b) = (by_dist[0], by_dist[1]);
            if shown {
                r.draw_line(a.0, a.1, b.0, b.1, 0.8, inner);
            } else {
                r.draw_line(a.0, a.1, b.0, b.1, 1.8, ink);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HexFeature, HexOrientation, HexPath};
    use crate::render::ImageRenderer;

    /// The floor 7 pasture as a sample map with every terrain, landmark and path kind.
    fn sample() -> HexMap {
        let mut map = HexMap::with_radius(3, HexTerrain::Grassland);
        let set = |map: &mut HexMap, q: i32, r: i32, t: HexTerrain| map.ensure_hex(HexCoord::new(q, r), t).terrain = t;
        for (q, r) in [(3, -3), (3, -2), (2, -3), (-3, 3)] {
            set(&mut map, q, r, HexTerrain::Hills);
        }
        for (q, r) in [(1, -3), (0, -3)] {
            set(&mut map, q, r, HexTerrain::Mountains);
        }
        for (q, r) in [(-3, 0), (-3, 1), (-2, 2), (-1, 3)] {
            set(&mut map, q, r, HexTerrain::Steppe);
        }
        for (q, r) in [(-1, -1), (0, -2), (-2, 0)] {
            set(&mut map, q, r, HexTerrain::Marsh);
        }
        for (i, t) in [HexTerrain::Plains, HexTerrain::Farmland, HexTerrain::Forest, HexTerrain::Desert, HexTerrain::Snow, HexTerrain::Water].into_iter().enumerate() {
            set(&mut map, i as i32 - 2, 4 - i as i32, t);
        }
        for (i, f) in HexFeature::ALL.iter().enumerate() {
            let c = HexCoord::new(-3 + (i as i32 % 7), if i < 7 { 5 } else { 6 } - (i as i32 % 7) / 2);
            map.ensure_hex(c, HexTerrain::Plains).feature = Some(*f);
        }
        map.hex_mut(HexCoord::new(0, 0)).unwrap().feature = Some(HexFeature::Fold);
        map.paths.push(HexPath::through(HexPathKind::River, &[HexCoord::new(-3, -0), HexCoord::new(-1, -1), HexCoord::new(1, -2), HexCoord::new(3, -3)]));
        map.paths.push(HexPath::through(HexPathKind::Road, &[HexCoord::new(-3, 3), HexCoord::new(0, 0)]));
        map.paths.push(HexPath::through(HexPathKind::Track, &[HexCoord::new(0, 0), HexCoord::new(3, 0)]));
        map
    }

    fn render(map: &HexMap, players: bool) -> ImageRenderer {
        let all: Vec<HexCoord> = map.hexes.iter().map(|h| h.coord).collect();
        let (x0, y0, x1, y1) = map.extents(&all).unwrap();
        let scale = 2.0;
        let mut r = ImageRenderer::new(((x1 - x0 + 20.0) * scale) as u32, ((y1 - y0 + 20.0) * scale) as u32, scale);
        r.offset_x = x0 - 10.0;
        r.offset_y = y0 - 10.0;
        r.fill_rect(x0 - 10.0, y0 - 10.0, x1 - x0 + 20.0, y1 - y0 + 20.0, [245, 240, 232, 255]);
        render_hex_map(&mut r, map, &Theme::default(), players);
        r
    }

    #[test]
    fn players_see_only_revealed_hexes() {
        let mut map = sample();
        map.move_party(HexCoord::new(0, 0));
        let dm = render(&map, false);
        let players = render(&map, true);
        let ink = |img: &ImageRenderer| img.image.pixels().filter(|p| p.0[0] < 128).count();
        assert!(ink(&players) > 0);
        assert!(ink(&players) * 3 < ink(&dm));
    }

    /// Renders the sample map to a PNG to look at. Run with
    /// `cargo test hex_contact_sheet -- --ignored`; `HEX_SHEET_OUT` sets the path.
    #[test]
    #[ignore]
    fn hex_contact_sheet() {
        let out = std::env::var("HEX_SHEET_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("hex_contact_sheet.png"));
        let mut map = sample();
        render(&map, false).image.save(&out).unwrap();
        map.orientation = HexOrientation::PointyTop;
        render(&map, false).image.save(out.with_extension("pointy.png")).unwrap();
    }
}
